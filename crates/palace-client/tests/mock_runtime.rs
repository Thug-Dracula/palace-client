//! Integration tests that drive the **real** runtime against a local mock
//! Palace server.
//!
//! The mock replays the raw bytes of the captured logon burst in
//! `fixtures/logon-run1` over loopback, so the whole stack runs for real: the
//! blocking connection FSM (`session.rs`), the session model (`state.rs`), the
//! asset intake (`assets.rs`) and the supervisor/compositor (`runtime.rs`).
//!
//! Everything is hermetic: the socket is `127.0.0.1`, the server is the fixture,
//! and the room's media names are pre-seeded into a temp directory so the media
//! worker never reaches for the network.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use palace_client::runtime::ClientRuntime;
use palace_client::trace::{self, Tracer};
use palace_client::{
    ChatKind, ClientConfig, ClientEvent, ClientHandle, ConnectionStatus, RoomInfo, ScreenState,
    ServerBanner, UserInfo,
};
use palace_wire::byteorder::{ByteOrder, Reader, Writer};
use palace_wire::fixture::{default_fixture_dir, Fixture};
use palace_wire::frame::{user_move_frame, Frame};
use palace_wire::messages::{
    aux_flags, client_logon_record_with_identity, reference_logon_record, AssetSpec,
    ClientIdentity, Message, Point, UserProp, UserRec,
};
use palace_wire::opcode;
use tokio::sync::mpsc::{error::TryRecvError, UnboundedReceiver};

// ---------------------------------------------------------------------------
// The recorded session
// ---------------------------------------------------------------------------

fn logon_fixture() -> Fixture {
    Fixture::load(&default_fixture_dir("logon-run1")).expect("the recorded fixture loads")
}

/// The raw bytes the server sent, in `seq` order. The fixture loader already
/// proves re-encoding reproduces each captured file byte for byte.
fn server_bytes(fixture: &Fixture) -> Vec<Vec<u8>> {
    fixture
        .server_frames()
        .map(|captured| {
            captured
                .frame
                .encode(fixture.byte_order)
                .expect("server frame re-encodes")
        })
        .collect()
}

/// The raw `ROOMDESC` payload from the fixture, for a server that serves one
/// room and nothing else.
fn room_desc_payload(fixture: &Fixture) -> Vec<u8> {
    fixture
        .server_frames()
        .find(|captured| captured.frame.opcode == opcode::ROOMDESC)
        .map(|captured| captured.frame.payload.clone())
        .expect("the fixture holds a room descriptor")
}

/// The room descriptor exactly as the server sent it.
fn room_desc(fixture: &Fixture) -> palace_room::RoomDesc {
    for captured in fixture.server_frames() {
        if captured.frame.opcode != opcode::ROOMDESC {
            continue;
        }
        if let Ok(room) = palace_room::decode_payload(&captured.frame.payload, fixture.byte_order) {
            return room;
        }
    }
    panic!("the fixture contains a decodable room description");
}

// ---------------------------------------------------------------------------
// Synthesised frames: the fixture is a replayer and carries no user record for
// the handshake's own id, so tests emit the users and messages they need.
// ---------------------------------------------------------------------------

/// The user id the fixture's `MSG_TIYID` handshake assigns to this connection.
const SELF_ID: i32 = 13;

/// Encode a `UserRec` body for room 901, not away and open to messages.
fn user_rec_body(
    order: ByteOrder,
    id: i32,
    name: &str,
    face: i16,
    color: i16,
    props: &[u32],
    pos: Point,
) -> Vec<u8> {
    let mut rec = UserRec {
        user_id: id,
        room_pos: pos,
        prop_spec: [AssetSpec::default(); AssetSpec::USER_PROP_SLOTS],
        room_id: 901,
        face_nbr: face,
        color_nbr: color,
        away_flag: 0,
        open_to_msgs: 1,
        nbr_props: props.len() as i16,
        name: name.to_string(),
    };
    for (slot, id) in rec.prop_spec.iter_mut().zip(props) {
        slot.id = *id as i32;
    }
    let mut w = Writer::new(order);
    rec.encode(&mut w);
    w.into_vec()
}

/// A `USERNEW` frame carrying a full `UserRec`. The body's id and the frame ref
/// agree, the shape the real `nprs` uses.
fn user_new_frame(
    order: ByteOrder,
    id: i32,
    name: &str,
    face: i16,
    color: i16,
    props: &[u32],
    pos: Point,
) -> Vec<u8> {
    let body = user_rec_body(order, id, name, face, color, props, pos);
    Frame::new(opcode::USERNEW, id, body)
        .encode(order)
        .expect("usernw encodes")
}

/// A `USERNEW` for the handshake's own user id, so a test can see the self user.
fn self_user_frame(order: ByteOrder) -> Vec<u8> {
    user_new_frame(order, SELF_ID, "RustProbe", 5, 4, &[], Point::new(306, 151))
}

fn user_face_frame(order: ByteOrder, id: i32, face: i16) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(face);
    Frame::new(opcode::USERFACE, id, w.into_vec())
        .encode(order)
        .expect("usrF encodes")
}

fn user_color_frame(order: ByteOrder, id: i32, color: i16) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(color);
    Frame::new(opcode::USERCOLOR, id, w.into_vec())
        .encode(order)
        .expect("usrC encodes")
}

/// A `usrN` body built independently of the crate encoder: a plain `PString`.
fn user_name_frame(order: ByteOrder, id: i32, name: &str) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_pstring(name);
    Frame::new(opcode::USERNAME, id, w.into_vec())
        .encode(order)
        .expect("usrN encodes")
}

fn user_prop_frame(order: ByteOrder, id: i32, props: &[(i32, u32)]) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i32(props.len() as i32);
    for (prop, crc) in props {
        w.write_i32(*prop);
        w.write_u32(*crc);
    }
    Frame::new(opcode::USERPROP, id, w.into_vec())
        .encode(order)
        .expect("usrP encodes")
}

fn user_desc_frame(
    order: ByteOrder,
    id: i32,
    face: i16,
    color: i16,
    props: &[(i32, u32)],
) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(face);
    w.write_i16(color);
    w.write_i32(props.len() as i32);
    for (prop, crc) in props {
        w.write_i32(*prop);
        w.write_u32(*crc);
    }
    Frame::new(opcode::USERDESC, id, w.into_vec())
        .encode(order)
        .expect("usrD encodes")
}

fn prop_new_frame(order: ByteOrder, id: i32, crc: u32, pos: Point) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i32(id);
    w.write_u32(crc);
    pos.encode(&mut w);
    Frame::new(opcode::PROPNEW, 0, w.into_vec())
        .encode(order)
        .expect("nPrp encodes")
}

fn prop_move_frame(order: ByteOrder, index: i32, pos: Point) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i32(index);
    pos.encode(&mut w);
    Frame::new(opcode::PROPMOVE, 0, w.into_vec())
        .encode(order)
        .expect("mPrp encodes")
}

fn prop_del_frame(order: ByteOrder, index: i32) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i32(index);
    Frame::new(opcode::PROPDEL, 0, w.into_vec())
        .encode(order)
        .expect("dPrp encodes")
}

fn door_lock_frame(order: ByteOrder, room_id: i16, door_id: i16) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(room_id);
    w.write_i16(door_id);
    Frame::new(opcode::DOORLOCK, 0, w.into_vec())
        .encode(order)
        .expect("lock encodes")
}

fn spot_state_frame(order: ByteOrder, room_id: i16, spot_id: i16, state: i16) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(room_id);
    w.write_i16(spot_id);
    w.write_i16(state);
    Frame::new(opcode::SPOTSTATE, 0, w.into_vec())
        .encode(order)
        .expect("sSta encodes")
}

fn spot_move_frame(order: ByteOrder, room_id: i16, spot_id: i16, pos: Point) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(room_id);
    w.write_i16(spot_id);
    pos.encode(&mut w);
    Frame::new(opcode::SPOTMOVE, 0, w.into_vec())
        .encode(order)
        .expect("coLs encodes")
}

/// A `down` frame. The reason code is the frame `refNum`; only `K_Verbose`
/// (16) carries a body, written here as a `CString` when `message` is given.
fn server_down_frame(order: ByteOrder, reason: i32, message: Option<&str>) -> Vec<u8> {
    let payload = match message {
        Some(text) => {
            let mut w = Writer::new(order);
            w.write_cstring(text);
            w.into_vec()
        }
        None => Vec::new(),
    };
    Frame::new(opcode::SERVERDOWN, reason, payload)
        .encode(order)
        .expect("down encodes")
}

fn talk_from_frame(order: ByteOrder, user_id: i32, text: &str) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_cstring(text);
    Frame::new(opcode::TALK, user_id, w.into_vec())
        .encode(order)
        .expect("talk encodes")
}

/// A server-side `talk` frame, used as an ordered marker: frames are processed
/// in arrival order, so seeing this chat line proves every earlier frame ran.
fn talk_marker_frame(order: ByteOrder, text: &str) -> Vec<u8> {
    talk_from_frame(order, SELF_ID, text)
}

/// Write a decodable solid prop blob named `<id>.bin` for each id, so the prop
/// store holds — and therefore draws — them.
fn seed_props_dir(ids: &[u32], rgb: [u8; 3]) -> PathBuf {
    let dir = unique_temp_dir("seed-props");
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for _ in 0..(8 * 8) {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    let image = palace_prop::PropImage::from_rgba(8, 8, rgba).expect("prop image");
    let blob = palace_prop::encode_s20_blob(&image, 0, 0, 0).expect("prop encodes");
    for id in ids {
        fs::write(dir.join(format!("{id}.bin")), &blob).expect("write seeded prop");
    }
    dir
}

// ---------------------------------------------------------------------------
// The mock server
// ---------------------------------------------------------------------------

/// A loopback TCP server that replays captured frames to every client that
/// connects and records whatever the client writes back.
struct MockServer {
    port: u16,
    stop: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<u8>>>,
    connections: Arc<AtomicU64>,
    accept: Option<JoinHandle<()>>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl MockServer {
    /// Start a server that replays `frames` and stays open until it is dropped.
    fn start(frames: Vec<Vec<u8>>) -> Self {
        Self::start_with(frames, false)
    }

    /// Like [`MockServer::start`], but half-closes the write side as soon as the
    /// client has sent something — a clean "server went away" without an RST.
    fn start_half_close(frames: Vec<Vec<u8>>) -> Self {
        Self::start_with(frames, true)
    }

    fn start_with(frames: Vec<Vec<u8>>, half_close: bool) -> Self {
        Self::start_inner(frames, half_close, None, Vec::new())
    }

    /// Like [`MockServer::start`], but holds back `tail` until the returned flag
    /// is set — a server-side "and then..." the test controls.
    fn start_gated(frames: Vec<Vec<u8>>, tail: Vec<Vec<u8>>) -> (Self, Arc<AtomicBool>) {
        let gate = Arc::new(AtomicBool::new(false));
        (
            Self::start_inner(frames, false, Some(gate.clone()), tail),
            gate,
        )
    }

    fn start_inner(
        frames: Vec<Vec<u8>>,
        half_close: bool,
        gate: Option<Arc<AtomicBool>>,
        tail: Vec<Vec<u8>>,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");

        let stop = Arc::new(AtomicBool::new(false));
        let received = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicU64::new(0));
        let workers = Arc::new(Mutex::new(Vec::new()));

        let accept = thread::spawn({
            let stop = stop.clone();
            let received = received.clone();
            let connections = connections.clone();
            let workers = workers.clone();
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::Relaxed);
                            let frames = frames.clone();
                            let stop = stop.clone();
                            let received = received.clone();
                            let gate = gate.clone();
                            let tail = tail.clone();
                            let handle = thread::spawn(move || {
                                serve(stream, frames, half_close, stop, received, gate, tail);
                            });
                            workers.lock().expect("workers").push(handle);
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        MockServer {
            port,
            stop,
            received,
            connections,
            accept: Some(accept),
            workers,
        }
    }

    fn received_bytes(&self) -> Vec<u8> {
        self.received.lock().expect("received").clone()
    }

    fn connections(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }

    /// Every frame the client sent, decoded in the session's byte order.
    fn received_frames(&self, order: ByteOrder) -> Vec<Frame> {
        Frame::decode_all(&self.received_bytes(), order).expect("client frames decode")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

fn serve(
    mut stream: TcpStream,
    frames: Vec<Vec<u8>>,
    half_close: bool,
    stop: Arc<AtomicBool>,
    received: Arc<Mutex<Vec<u8>>>,
    gate: Option<Arc<AtomicBool>>,
    tail: Vec<Vec<u8>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(25)));
    for bytes in &frames {
        if stream.write_all(bytes).is_err() {
            return;
        }
    }
    let _ = stream.flush();

    if let Some(gate) = gate {
        let mut buf = [0u8; 8192];
        while !gate.load(Ordering::Relaxed) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match stream.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => received
                    .lock()
                    .expect("received")
                    .extend_from_slice(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) => {}
                Err(_) => return,
            }
        }
        for bytes in &tail {
            if stream.write_all(bytes).is_err() {
                return;
            }
        }
        let _ = stream.flush();
    }

    let mut half_closed = false;
    let mut buf = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                received
                    .lock()
                    .expect("received")
                    .extend_from_slice(&buf[..n]);
                if half_close && !half_closed {
                    half_closed = true;
                    let _ = stream.shutdown(Shutdown::Write);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(_) => break,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

// ---------------------------------------------------------------------------
// Temp dirs and seeded media
// ---------------------------------------------------------------------------

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(tag: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "palace-client-mock-{}-{tag}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer
            .write_image_data(&[0x20u8; 8 * 8 * 4])
            .expect("png data");
    }
    out
}

/// A media root holding a placeholder for every image the room names, so
/// `missing_media` finds nothing and the HTTP worker is never asked to fetch.
fn seed_media_dir(room: &palace_room::RoomDesc) -> PathBuf {
    let dir = unique_temp_dir("seed");
    let png = tiny_png();
    let mut names = vec![room.picture.clone()];
    for picture in &room.pictures {
        if let Some(name) = &picture.name {
            names.push(name.clone());
        }
    }
    for name in names {
        let Some(base) = Path::new(&name).file_name() else {
            continue;
        };
        if base.is_empty() {
            continue;
        }
        fs::write(dir.join(base), &png).expect("write seeded media");
    }
    dir
}

fn config_for(port: u16, cache_root: PathBuf, seed_media: PathBuf) -> ClientConfig {
    ClientConfig {
        host: "127.0.0.1".to_string(),
        port,
        username: "RustProbe".to_string(),
        desired_room: 0,
        cache_root,
        seed_media: vec![seed_media],
        seed_props: Vec::new(),
        password: None,
        identity: ClientIdentity::default(),
    }
}

// ---------------------------------------------------------------------------
// Event stream helpers
// ---------------------------------------------------------------------------

fn collect_events(
    rx: &mut UnboundedReceiver<ClientEvent>,
    until: impl Fn(&[ClientEvent]) -> bool,
    timeout: Duration,
) -> Vec<ClientEvent> {
    let deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    loop {
        if until(&events) {
            return events;
        }
        if Instant::now() >= deadline {
            return events;
        }
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(TryRecvError::Disconnected) => return events,
        }
    }
}

fn statuses(events: &[ClientEvent], want: ConnectionStatus) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Status { status, message } if *status == want => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn banners(events: &[ClientEvent]) -> Vec<&ServerBanner> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Banner { banner } => Some(banner),
            _ => None,
        })
        .collect()
}

fn last_banner(events: &[ClientEvent]) -> Option<&ServerBanner> {
    banners(events).pop()
}

fn room_lists(events: &[ClientEvent]) -> Vec<&Vec<RoomInfo>> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Rooms { rooms } => Some(rooms),
            _ => None,
        })
        .collect()
}

fn entered_rooms(events: &[ClientEvent]) -> Vec<&RoomInfo> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::RoomEntered { room } => Some(room),
            _ => None,
        })
        .collect()
}

fn user_lists(events: &[ClientEvent]) -> Vec<&Vec<UserInfo>> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Users { users } => Some(users),
            _ => None,
        })
        .collect()
}

fn screens(events: &[ClientEvent]) -> Vec<&ScreenState> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Screen { screen } => Some(screen),
            _ => None,
        })
        .collect()
}

fn chats(events: &[ClientEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Chat { line } => Some(line.text.as_str()),
            _ => None,
        })
        .collect()
}

fn notes(events: &[ClientEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Note { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn sound_events(events: &[ClientEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Sound { name } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn midi_play_events(events: &[ClientEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::MidiPlay { name } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn midi_loop_events(events: &[ClientEvent]) -> Vec<(&str, i32)> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::MidiLoop { name, loops } => Some((name.as_str(), *loops)),
            _ => None,
        })
        .collect()
}

/// Every tooltip report, `Some(text)` to show and `None` to hide.
fn tooltips(events: &[ClientEvent]) -> Vec<Option<&str>> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Tooltip { text } => Some(text.as_deref()),
            _ => None,
        })
        .collect()
}

fn error_chats(events: &[ClientEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Chat { line } if line.kind == ChatKind::Error => Some(line.text.as_str()),
            _ => None,
        })
        .collect()
}

#[derive(Debug)]
struct ScriptRun<'a> {
    event: &'a str,
    fired: usize,
    effects: &'a [String],
}

fn script_runs(events: &[ClientEvent]) -> Vec<ScriptRun<'_>> {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Script {
                event,
                fired,
                effects,
                ..
            } => Some(ScriptRun {
                event: event.as_str(),
                fired: *fired,
                effects: effects.as_slice(),
            }),
            _ => None,
        })
        .collect()
}

/// The frame version once it has held still for a moment, so a later comparison
/// is not confused by media still arriving.
fn settled_frame_version(handle: &ClientHandle, timeout: Duration) -> u64 {
    let deadline = Instant::now() + timeout;
    let mut version = handle.frames().version();
    let mut changed_at = Instant::now();
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
        let now = handle.frames().version();
        if now != version {
            version = now;
            changed_at = Instant::now();
        } else if changed_at.elapsed() > Duration::from_millis(250) {
            break;
        }
    }
    version
}

fn wait_for(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Happy path: the whole captured logon burst
// ---------------------------------------------------------------------------

#[test]
fn the_runtime_replays_the_recorded_logon_burst() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);

    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("happy-cache");
    let seed = seed_media_dir(&room);

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    // Wait until the room is entered, all 81 rooms are known and a frame has
    // been composited.
    let events = collect_events(
        &mut rx,
        |collected| {
            room_lists(collected).iter().any(|rooms| rooms.len() == 81)
                && screens(collected)
                    .iter()
                    .any(|screen| screen.room_id == 901)
                && !entered_rooms(collected).is_empty()
        },
        Duration::from_secs(20),
    );

    // Status reached Connected on the real socket.
    assert!(
        !statuses(&events, ConnectionStatus::Connected).is_empty(),
        "the runtime reported Connected; events: {events:?}"
    );

    // Banner carries the real server name and version.
    let banner = last_banner(&events).expect("a banner was emitted");
    assert_eq!(banner.name.as_deref(), Some("Balamb Garden"));
    assert_eq!(banner.version.as_deref(), Some("1.22"));
    assert_eq!(
        banner.media_base.as_deref(),
        Some("https://media.palace.example.info/palace/media")
    );
    assert_eq!(banner.total_users, Some(2));
    assert_eq!(banner.user_id, 13, "the handshake assigned user id 13");
    assert_eq!(banner.byte_order, "little");

    // The room list is the server's full 81.
    let rooms = room_lists(&events);
    assert_eq!(rooms.last().map(|rooms| rooms.len()), Some(81));

    // The room the server put us in.
    let entered = entered_rooms(&events);
    let entered = entered.last().expect("a room was entered");
    assert_eq!(entered.id, 901);
    assert_eq!(entered.name, "Balamb Garden");

    // The room's users.
    let users = user_lists(&events);
    let users = users.last().expect("a user list was emitted");
    assert!(
        users
            .iter()
            .any(|user| user.id == 13 && user.name == "RustProbe"),
        "our own user is in the list: {users:?}"
    );

    // A screen came out of the compositor, and so did its PNG.
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed for room 901");
    assert_eq!(screen.room_name, "Balamb Garden");
    assert!(screen.version >= 1);
    let png = handle.frames().png().expect("the frame store holds a PNG");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "the frame is a real PNG");
    assert!(handle.frames().version() >= 1);

    // The runtime's own sign-on line.
    assert!(chats(&events)
        .iter()
        .any(|text| text.contains("signed on as \"RustProbe\"")));

    // The room descriptor's `ON ENTER` script actually dispatched, with the
    // effects the recorded room asks for.
    let runs = script_runs(&events);
    let enter = runs
        .iter()
        .find(|run| run.event == "ENTER")
        .expect("the room's ON ENTER handler fired");
    assert!(enter.fired >= 1, "at least one handler ran");
    assert!(
        enter.effects.iter().any(|effect| effect.contains("garden")),
        "the recorded room's ON ENTER effects reference 'garden': {:?}",
        enter.effects
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::Sound { name } if name == "garden")),
        "the effect was applied as a structured sound event: {:?}",
        events
            .iter()
            .filter(|event| matches!(event, ClientEvent::Sound { .. }))
            .collect::<Vec<_>>()
    );

    // The real bytes of the client's reply went out over the socket: the runtime
    // built its logon from the honest client profile carrying this install's
    // generated identity, which differs from the reference record in the cleared
    // `AUTHENTICATE` bit, the registration pair and the PUID fields.
    assert!(
        wait_for(
            || !server.received_bytes().is_empty(),
            Duration::from_secs(5)
        ),
        "the client sent data"
    );
    let sent = server.received_frames(order);
    let logon = sent
        .iter()
        .find(|frame| frame.opcode == opcode::LOGON)
        .expect("the runtime sent a logon");
    assert_eq!(logon.ref_num, 0);
    let expected = client_logon_record_with_identity("RustProbe", 0, ClientIdentity::default())
        .logon_frame(order);
    assert_eq!(
        logon.payload, expected.payload,
        "the configured user's logon carries this install's identity"
    );

    // The capture still holds the reference client's logon, unchanged.
    let captured_logon = fixture
        .client_frames()
        .find(|captured| captured.frame.opcode == opcode::LOGON)
        .expect("the capture holds the real client's logon");
    let reference = reference_logon_record("RustProbe", 0).logon_frame(order);
    assert_eq!(
        captured_logon.frame.payload, reference.payload,
        "the capture still reproduces the reference logon"
    );

    // The sent logon must differ from that capture in the registration pair at
    // offsets 0..8, the `auxFlags` word at offset 72 and the PUID words at
    // offsets 76..84, and nowhere else.
    let captured = &captured_logon.frame.payload;
    assert_eq!(captured.len(), logon.payload.len());
    let differing: Vec<usize> = (0..captured.len())
        .filter(|&i| captured[i] != logon.payload[i])
        .collect();
    assert!(
        !differing.is_empty(),
        "the runtime logon must differ from the capture"
    );
    assert!(
        differing
            .iter()
            .all(|&i| (0..8).contains(&i) || (72..76).contains(&i) || (76..84).contains(&i)),
        "the runtime logon differs from the capture only in the registration pair, \
         auxFlags and the PUID, got {differing:?}"
    );
    assert_ne!(
        &logon.payload[0..8],
        &captured[0..8],
        "the registration pair must not be the captured identity"
    );
    assert_ne!(
        &logon.payload[76..84],
        &captured[76..84],
        "the PUID must not be the captured identity"
    );
    let sent_flags = u32::from_le_bytes(logon.payload[72..76].try_into().expect("four bytes"));
    let captured_flags = u32::from_le_bytes(captured[72..76].try_into().expect("four bytes"));
    assert_eq!(
        captured_flags ^ sent_flags,
        aux_flags::AUTHENTICATE,
        "the only auxFlags difference is the cleared AUTHENTICATE bit"
    );
    assert_eq!(sent_flags & aux_flags::AUTHENTICATE, 0);
    assert!(
        sent.iter()
            .any(|frame| frame.opcode == opcode::LISTOFALLROOMS),
        "the runtime asked for the room list"
    );
    assert!(
        sent.iter()
            .any(|frame| frame.opcode == opcode::LISTOFALLUSERS),
        "the runtime asked for the user list"
    );

    // Clean shutdown.
    handle.disconnect();
    let shut_down = collect_events(
        &mut rx,
        |collected| !statuses(collected, ConnectionStatus::Disconnected).is_empty(),
        Duration::from_secs(5),
    );
    assert!(
        !statuses(&shut_down, ConnectionStatus::Disconnected).is_empty(),
        "disconnect reached Disconnected"
    );
    assert!(!handle.is_running(), "the supervisor stopped");
    assert_eq!(handle.host(), "127.0.0.1");
    assert_eq!(handle.port(), server.port);

    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn an_empty_capture_is_sent_by_the_runtime_as_empty_list_requests() {
    // The captured client frames 0011/0013 (`rLst`/`uLst` with no body) are what
    // the runtime builds for its own list requests; assert the runtime emits
    // exactly those frames, not something it invented.
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("lists-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| !entered_rooms(collected).is_empty(),
        Duration::from_secs(20),
    );
    assert!(!entered_rooms(&events).is_empty());
    assert!(wait_for(
        || server.received_bytes().len() >= 12 * 3,
        Duration::from_secs(5)
    ));

    let sent = server.received_frames(order);
    for opcode_wanted in [opcode::LISTOFALLROOMS, opcode::LISTOFALLUSERS] {
        let captured = fixture
            .client_frames()
            .find(|captured| captured.frame.opcode == opcode_wanted)
            .expect("the capture holds this client frame");
        let ours = sent
            .iter()
            .find(|frame| frame.opcode == opcode_wanted)
            .expect("the runtime sent this frame");
        assert_eq!(
            ours,
            &captured.frame,
            "{} matches the recorded client frame",
            opcode_wanted.describe()
        );
    }

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_floor_click_shows_the_move_before_the_server_replies() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("predict-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    // The click needs the compositor's transform, so wait for the first frame.
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed");

    let settled = settled_frame_version(&handle, Duration::from_secs(5));

    // Click the floor far past the room edge: it clamps to the avatar margin and
    // cannot be inside a hotspot (the reference room's hotspots end well short).
    let floor = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(floor.x, floor.y);

    // The predicted move is drawn locally without waiting for the server.
    assert!(
        wait_for(
            || handle.frames().version() > settled,
            Duration::from_secs(5)
        ),
        "the click's predicted position was composited before any server reply"
    );

    // The server is still told where we went: a `uLoc` went out alongside.
    assert!(
        wait_for(
            || server
                .received_frames(order)
                .iter()
                .any(|frame| frame.opcode == opcode::USERMOVE),
            Duration::from_secs(5)
        ),
        "the runtime still notified the server with a USERMOVE frame"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

/// True once the server has named our own user in the room.
fn self_is_known(events: &[ClientEvent]) -> bool {
    user_lists(events)
        .iter()
        .any(|users| users.iter().any(|user| user.is_self))
}

#[test]
fn a_click_applies_the_local_move_before_the_frame_is_sent() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("order-cache");
    let seed = seed_media_dir(&room);

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    // The click needs the compositor's transform and the self record.
    let events = collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty() && self_is_known(collected),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed");
    let before = handle.frames().version();

    // A floor click far outside the room clamps to the avatar margin.
    let floor = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(floor.x, floor.y);

    let expected = (
        screen.geometry.room_w as i32 - 22,
        screen.geometry.room_h as i32 - 22,
    );
    // Each session owns its event stream, so this order stands even while other
    // tests share the process-wide tracer.
    let after = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.starts_with("walk: sent"))
        },
        Duration::from_secs(5),
    );
    let walk_notes: Vec<&str> = notes(&after)
        .into_iter()
        .filter(|text| text.starts_with("walk: local apply") || text.starts_with("walk: sent"))
        .collect();
    assert_eq!(
        walk_notes.len(),
        2,
        "the walk emitted a local-apply and a sent note: {walk_notes:?}"
    );
    assert!(
        walk_notes[0].starts_with("walk: local apply") && walk_notes[1].starts_with("walk: sent"),
        "the local apply is reported before the send: {walk_notes:?}"
    );
    let target = format!("to=({},{})", expected.0, expected.1);
    assert!(
        walk_notes[0].contains(&target) && walk_notes[1].contains(&target),
        "both diagnostics name the same clamped target: {walk_notes:?}"
    );
    assert!(
        wait_for(
            || sent_moves(&server, order).contains(&expected),
            Duration::from_secs(5)
        ),
        "the server was told where the local avatar went"
    );
    assert!(
        wait_for(
            || handle.frames().version() > before,
            Duration::from_secs(5)
        ),
        "the local move was composited without waiting for the server"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_walk_click_is_applied_locally_then_sent_with_the_same_target() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, screen, cache, seed) =
        start_room_with_self(&fixture, "walk-order");

    let floor = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(floor.x, floor.y);

    let expected = (
        screen.geometry.room_w as i32 - 22,
        screen.geometry.room_h as i32 - 22,
    );
    let after = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.starts_with("walk: sent"))
        },
        Duration::from_secs(5),
    );
    let walk_notes: Vec<&str> = notes(&after)
        .into_iter()
        .filter(|text| text.starts_with("walk: local apply") || text.starts_with("walk: sent"))
        .collect();
    assert_eq!(
        walk_notes.len(),
        2,
        "one local-apply and one sent note: {walk_notes:?}"
    );
    assert!(
        walk_notes[0].starts_with("walk: local apply") && walk_notes[1].starts_with("walk: sent"),
        "the notes are local-first: {walk_notes:?}"
    );
    let target = format!("to=({},{})", expected.0, expected.1);
    assert!(
        walk_notes[0].contains(&target) && walk_notes[1].contains(&target),
        "both notes name the same target: {walk_notes:?}"
    );
    assert_eq!(
        sent_moves(&server, order),
        vec![expected],
        "the server was told exactly where the local avatar was placed"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

/// A local move must ask for its own redraw in the click path, before the wire
/// is touched and without waiting for any server frame. The trigger is the
/// position change, so a no-op click must not ask for a redraw.
#[test]
fn a_walk_click_requests_a_redraw_before_the_server_is_told() {
    let fixture = logon_fixture();
    let (server, handle, mut rx, screen, cache, seed) =
        start_room_with_self(&fixture, "walk-redraw");

    let floor = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    let expected = (
        screen.geometry.room_w as i32 - 22,
        screen.geometry.room_h as i32 - 22,
    );

    // Stop at the server send: every event before it came from the local path.
    handle.click(floor.x, floor.y);
    let after = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.starts_with("walk: sent"))
        },
        Duration::from_secs(5),
    );
    let walk_notes: Vec<&str> = notes(&after)
        .into_iter()
        .filter(|text| {
            text.starts_with("walk: local apply")
                || text.starts_with("walk: redraw requested")
                || text.starts_with("walk: sent")
        })
        .collect();
    assert_eq!(
        walk_notes.len(),
        3,
        "the move applies, requests a redraw, then tells the server: {walk_notes:?}"
    );
    assert!(
        walk_notes[0].starts_with("walk: local apply")
            && walk_notes[1].starts_with("walk: redraw requested")
            && walk_notes[2].starts_with("walk: sent"),
        "the redraw request is part of the local apply, before the send: {walk_notes:?}"
    );
    let target = format!("to=({},{})", expected.0, expected.1);
    assert!(
        walk_notes[0].contains(&target)
            && walk_notes[1].contains(&target)
            && walk_notes[2].contains(&target),
        "every walk diagnostic names the clicked target: {walk_notes:?}"
    );

    // The trigger is the change: clicking the same spot again moves nothing and
    // so must not request another redraw.
    handle.click(floor.x, floor.y);
    let repeat = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.starts_with("walk: sent"))
        },
        Duration::from_secs(5),
    );
    let repeat_notes: Vec<&str> = notes(&repeat)
        .into_iter()
        .filter(|text| text.starts_with("walk:"))
        .collect();
    assert!(
        repeat_notes
            .iter()
            .any(|text| text.starts_with("walk: local apply") && text.contains("changed=false")),
        "the repeat click changed nothing: {repeat_notes:?}"
    );
    assert!(
        !repeat_notes
            .iter()
            .any(|text| text.starts_with("walk: redraw requested")),
        "a move that changes nothing must not request a redraw: {repeat_notes:?}"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn an_agreeing_server_echo_does_not_recompose_the_frame() {
    let _guard = trace_guard();
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let probe = unique_temp_dir("echo-trace");
    let trace_path = probe.join("session.log");
    trace::install(Some(Arc::new(
        Tracer::to_path(&trace_path).expect("the trace file opens"),
    )));
    // The click lands at room (30_000, 30_000), which clamps to (490, 362); the
    // server then echoes exactly that position back.
    let echo = user_move_frame(Point::new(362, 490), SELF_ID, order)
        .encode(order)
        .expect("the echo encodes");
    let (server, gate) = MockServer::start_gated(server_bytes(&fixture), vec![echo]);
    let cache = unique_temp_dir("echo-cache");
    let seed = seed_media_dir(&room);

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty() && self_is_known(collected),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed");
    let floor = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(floor.x, floor.y);

    // Wait for the click's own predicted frame and its local application.
    assert!(
        wait_for(
            || handle.frames().version() > screen.version,
            Duration::from_secs(5)
        ),
        "the click composited a local frame"
    );

    // Now the server echoes the same position: reconciliation must be silent.
    gate.store(true, Ordering::Relaxed);
    assert!(
        wait_for(
            || trace_text(&trace_path).contains("self_move echo"),
            Duration::from_secs(3)
        ),
        "the server echo was processed"
    );
    let text = trace_text(&trace_path);
    assert!(
        text.contains("self_move local id=13 to=(490,362) changed=true"),
        "the click applied the local move: {text}"
    );
    assert!(
        text.contains("self_move echo id=13 at=(490,362) redrew=false"),
        "the agreeing server echo was reconciled without a redraw"
    );
    assert!(
        server
            .received_frames(order)
            .iter()
            .any(|frame| frame.opcode == opcode::USERMOVE),
        "the client still told the server where it moved"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_at_a_hotspot_resolves_at_dpr_two() {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("dpr2-map-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty(),
        Duration::from_secs(20),
    );

    // A non-integer fit scale at DPR 2, the shape the live report showed.
    handle.set_viewport(960.0, 540.0, 2.0, 1.0, false);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| (screen.geometry.dpr - 2.0).abs() < f64::EPSILON)
        },
        Duration::from_secs(10),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| (screen.geometry.dpr - 2.0).abs() < f64::EPSILON)
        .expect("the geometry reached DPR 2");
    assert!(
        (screen.geometry.scale.fract()).abs() > f64::EPSILON,
        "the scale is deliberately not a whole number: {}",
        screen.geometry.scale
    );

    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a scripted hotspot");
    let point = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(
            f64::from(spot.loc.h),
            f64::from(spot.loc.v),
        ));
    handle.click(point.x, point.y);

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: click at room"))
                && script_runs(collected)
                    .iter()
                    .any(|run| run.event == "SELECT")
        },
        Duration::from_secs(10),
    );
    let click_note = notes(&events)
        .into_iter()
        .find(|text| text.starts_with("script: click at room"))
        .expect("the click was reported");
    assert!(
        click_note.contains("hit hotspot"),
        "a click on a hotspot's rendered centre must hit it at DPR 2: {click_note}"
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "the hotspot's ON SELECT handler fired"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

// ---------------------------------------------------------------------------
// The command surface: goto, say, click, script, viewport, refresh, reconnect
// ---------------------------------------------------------------------------

#[test]
fn commands_drive_the_live_session() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("cmd-cache");
    let seed = seed_media_dir(&room);
    let props_seed = unique_temp_dir("cmd-props");
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props_seed.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    // Wait for the first composed frame; the click needs its transform.
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed");

    handle.set_mouse(300, 200);

    // Click a scripted hotspot's anchor point, mapped through the same
    // transform the compositor reported.
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a scripted hotspot");
    let room_x = f64::from(spot.loc.h);
    let room_y = f64::from(spot.loc.v);
    let viewport = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(room_x, room_y));
    handle.click(viewport.x, viewport.y);

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: click at room"))
                && script_runs(collected)
                    .iter()
                    .any(|run| run.event == "SELECT")
        },
        Duration::from_secs(10),
    );
    let click_note = notes(&events)
        .into_iter()
        .find(|text| text.starts_with("script: click at room"))
        .expect("the click was reported");
    assert!(
        click_note.contains("hit hotspot"),
        "the hotspot was hit: {click_note}"
    );
    let select = script_runs(&events);
    assert!(
        select
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "the room's ON SELECT handler fired: {select:?}"
    );

    // A click far outside every hotspot is reported as a miss, not as a hit.
    let empty = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(empty.x, empty.y);
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("hit no hotspot"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("hit no hotspot")),
        "the empty click was reported: {:?}",
        notes(&events)
    );

    // Say: the runtime encodes and sends a real TALK frame.
    handle.say("hello garden");
    assert!(
        wait_for(
            || {
                server
                    .received_frames(order)
                    .iter()
                    .any(|frame| frame.opcode == opcode::TALK)
            },
            Duration::from_secs(5)
        ),
        "the runtime sent a TALK frame"
    );
    let sent = server.received_frames(order);
    let talk = sent
        .iter()
        .find(|frame| frame.opcode == opcode::TALK)
        .expect("a talk frame");
    match Message::decode(talk.opcode, talk.ref_num, &talk.payload, order).expect("talk decodes") {
        Message::Talk(talk) => {
            assert_eq!(talk.text, "hello garden");
            assert_eq!(talk.user_id, 13);
        }
        other => panic!("expected Talk, got {other:?}"),
    }

    // Run a bare IPTSCRAE instruction: the effect is applied locally.
    handle.run_script("\"from the box\" SAY");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("from the input box"))
                && chats(collected).contains(&"from the box")
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("from the input box")),
        "the script box ran: {:?}",
        notes(&events)
    );
    assert!(
        chats(&events).contains(&"from the box"),
        "the SAY effect produced a chat line: {:?}",
        chats(&events)
    );

    // An effect that returns a note rather than a chat line.
    handle.run_script("\"status from a test\" STATUSMSG");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: status from a test"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: status from a test")),
        "the STATUSMSG effect was applied: {:?}",
        notes(&events)
    );

    // Every sound effect now emits a structured event rather than a note string.
    // One source, because the runtime keeps only the last queued script.
    handle.run_script("\"garden\" SOUND \"garden\" MIDIPLAY 99 \"garden\" MIDILOOP MIDISTOP BEEP");
    let events = collect_events(
        &mut rx,
        |collected| {
            !sound_events(collected).is_empty()
                && !midi_play_events(collected).is_empty()
                && !midi_loop_events(collected).is_empty()
                && collected
                    .iter()
                    .any(|event| matches!(event, ClientEvent::MidiStop))
                && collected
                    .iter()
                    .any(|event| matches!(event, ClientEvent::Beep))
        },
        Duration::from_secs(5),
    );
    assert_eq!(
        sound_events(&events).first().copied(),
        Some("garden"),
        "SOUND produced a sound event: {:?}",
        sound_events(&events)
    );
    assert_eq!(
        midi_play_events(&events).first().copied(),
        Some("garden"),
        "MIDIPLAY produced a midi_play event: {:?}",
        midi_play_events(&events)
    );
    assert_eq!(
        midi_loop_events(&events).first().copied(),
        Some(("garden", 99)),
        "MIDILOOP carried its name and loop count: {:?}",
        midi_loop_events(&events)
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::MidiStop)),
        "MIDISTOP produced a midi_stop event"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::Beep)),
        "BEEP produced a beep event"
    );

    // A script that does not parse is reported as a transcript error.
    handle.run_script("\"unterminated");
    let events = collect_events(
        &mut rx,
        |collected| {
            error_chats(collected)
                .iter()
                .any(|text| text.contains("script error"))
        },
        Duration::from_secs(5),
    );
    assert!(
        error_chats(&events)
            .iter()
            .any(|text| text.contains("script error")),
        "the bad script produced an error line: {:?}",
        error_chats(&events)
    );

    // A viewport change recomputes and re-emits the frame geometry.
    handle.set_viewport(1280.0, 720.0, 1.0, 1.0, false);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| (screen.geometry.viewport_w - 1280.0).abs() < 1e-9)
        },
        Duration::from_secs(5),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| (screen.geometry.viewport_w - 1280.0).abs() < 1e-9),
        "a frame with the new viewport was emitted"
    );

    // A device-pixel-ratio change invalidates the composited frame itself.
    handle.set_viewport(1280.0, 720.0, 2.0, 1.0, false);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| (screen.geometry.dpr - 2.0).abs() < 1e-9)
        },
        Duration::from_secs(5),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| (screen.geometry.dpr - 2.0).abs() < 1e-9),
        "the new DPR was composited into the frame: {:?}",
        screens(&events)
            .iter()
            .map(|screen| screen.geometry.dpr)
            .collect::<Vec<_>>()
    );

    // Refresh replays the whole current state without a round trip.
    handle.refresh();
    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Connected).is_empty()
                && !room_lists(collected).is_empty()
                && !entered_rooms(collected).is_empty()
                && !screens(collected).is_empty()
        },
        Duration::from_secs(5),
    );
    assert!(
        !statuses(&events, ConnectionStatus::Connected).is_empty(),
        "refresh re-emitted the current status"
    );
    assert!(
        !room_lists(&events).is_empty() && !entered_rooms(&events).is_empty(),
        "refresh replayed the room list and the entered room"
    );
    assert!(
        !screens(&events).is_empty(),
        "refresh replayed the last composited frame"
    );

    // Reconnect tears the session down and dials the mock again.
    handle.reconnect();
    let events = collect_events(
        &mut rx,
        |collected| !statuses(collected, ConnectionStatus::Connected).is_empty(),
        Duration::from_secs(20),
    );
    assert!(
        !statuses(&events, ConnectionStatus::Connected).is_empty(),
        "the runtime reconnected and reached Connected again"
    );
    assert!(
        server.connections() >= 2,
        "the mock saw a second connection ({} so far)",
        server.connections()
    );

    // Go to a new room: the runtime sends the reference `navR` encoding.
    handle.goto_room(903);
    assert!(
        wait_for(
            || {
                server.received_frames(order).iter().any(|frame| {
                    frame.opcode == opcode::ROOMGOTO && frame.payload == 903u16.to_le_bytes()
                })
            },
            Duration::from_secs(5)
        ),
        "a navR frame for room 903 was sent"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props_seed);
}

#[test]
fn a_ping_from_the_server_is_answered_with_a_matching_pong() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let mut frames = server_bytes(&fixture);
    frames.push(
        Frame::empty(opcode::PING, 42)
            .encode(order)
            .expect("ping encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("ping-cache");
    let seed = seed_media_dir(&room_desc(&fixture));
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| !entered_rooms(collected).is_empty(),
        Duration::from_secs(20),
    );
    assert!(!entered_rooms(&events).is_empty());
    assert!(
        wait_for(
            || {
                server
                    .received_frames(order)
                    .iter()
                    .any(|frame| frame.opcode == opcode::PONG && frame.ref_num == 42)
            },
            Duration::from_secs(5)
        ),
        "the runtime answered the ping with a pong carrying the same ref"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_at_a_viewport_pixel_lands_on_the_room_cell_the_frame_drew_there() {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("click-mapping-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let screen = *screens(&events)
        .iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed for the room");
    assert_eq!(
        (screen.geometry.room_w, screen.geometry.room_h),
        (512.0, 384.0),
        "the round trip is read against the room the background defines"
    );

    // Landmarks inside hotspot 2's box (an ON SELECT region around 465,26) and
    // away from it, so a shifted click both moves where the cursor lands and can
    // flip a hit into a miss.
    let mut clicked = 0usize;
    for zoom in [1.0, 1.5, 2.0] {
        for dpr in [1.0, 1.25, 2.0] {
            handle.set_viewport(1000.0, 700.0, dpr, zoom, false);
            // Wait for the frame the *requested* viewport produced, not simply a
            // 1000-wide one: a leftover frame from the previous zoom would carry
            // the old transform and make the click map through it.
            let matches_request = |screen: &&ScreenState| {
                (screen.geometry.viewport_w - 1000.0).abs() < 1e-9
                    && (screen.geometry.zoom - zoom).abs() < 1e-9
                    && (screen.geometry.dpr - dpr).abs() < 1e-9
            };
            let events = collect_events(
                &mut rx,
                |collected| {
                    screens(collected)
                        .iter()
                        .any(|screen| screen.room_id == 901 && matches_request(screen))
                },
                Duration::from_secs(10),
            );
            let screen = *screens(&events)
                .iter()
                .find(|screen| screen.room_id == 901 && matches_request(screen))
                .expect("the requested viewport was composited");
            let transform = screen.geometry.transform();
            for (room_x, room_y) in [
                (500, 50),
                (480, 100),
                (490, 300),
                (300, 200),
                (250, 250),
                (505, 60),
                (465, 40),
                (10, 10),
            ] {
                let point = transform.room_to_viewport(palace_render::PointF::new(
                    f64::from(room_x),
                    f64::from(room_y),
                ));
                handle.click(point.x, point.y);
                let events = collect_events(
                    &mut rx,
                    |collected| {
                        notes(collected)
                            .iter()
                            .any(|text| text.starts_with("script: click at room"))
                    },
                    Duration::from_secs(10),
                );
                let note = *notes(&events)
                    .iter()
                    .find(|text| text.starts_with("script: click at room"))
                    .expect("the click was reported");
                assert_eq!(
                    clicked_room_note(note),
                    (room_x, room_y),
                    "a click drawn at room ({room_x},{room_y}) resolves back to that cell \
                     (zoom {zoom}, dpr {dpr}); note: {note}"
                );
                clicked += 1;
            }
        }
    }
    assert_eq!(
        clicked, 72,
        "every landmark in every combination was clicked"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_in_the_letterbox_is_not_clamped_onto_the_room_edge() {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("letterbox-click-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    assert!(!screens(&events).is_empty(), "a frame was composed");

    for (zoom, dpr) in [(1.0, 1.0), (1.0, 2.0)] {
        handle.set_viewport(1000.0, 700.0, dpr, zoom, false);
        let events = collect_events(
            &mut rx,
            |collected| {
                notes(collected)
                    .iter()
                    .any(|text| text.starts_with("script: click"))
            },
            Duration::from_secs(10),
        );
        let screen = *screens(&events)
            .iter()
            .find(|screen| (screen.geometry.viewport_w - 1000.0).abs() < 1e-9)
            .expect("the requested viewport was composited");
        let bars = screen.geometry.transform().content_rect();
        assert!(
            bars.x > 5.0,
            "the pillarbox is wide enough for the click to land inside it"
        );
        for (x, y) in [(5.0, 5.0), (5.0, 350.0), (995.0, 350.0)] {
            handle.click(x, y);
            let events = collect_events(
                &mut rx,
                |collected| {
                    notes(collected)
                        .iter()
                        .any(|text| text.starts_with("script: click at room"))
                },
                Duration::from_secs(5),
            );
            let note = *notes(&events)
                .iter()
                .find(|text| text.starts_with("script: click at room"))
                .expect("the click was reported");
            let (rx, ry) = clicked_room_note(note);
            let outside = rx < 0 || ry < 0 || rx >= 512 || ry >= 384;
            assert!(
                outside,
                "a click in the letterbox at ({x},{y}) must stay outside the room \
                 and be reported as a miss, not clamped onto an edge (zoom {zoom}, dpr {dpr}); note: {note}"
            );
            assert!(
                note.contains("hit no hotspot"),
                "an out-of-room click cannot be inside a hotspot; note: {note}"
            );
        }
    }

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_before_the_room_loads_is_ignored() {
    let fixture = logon_fixture();
    let mut frames = server_bytes(&fixture);
    frames.truncate(1);
    let server = MockServer::start_half_close(frames);
    let cache = unique_temp_dir("early-click-cache");
    let seed = seed_media_dir(&room_desc(&fixture));
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    handle.click(10.0, 10.0);
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("the room view is not ready"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("the room view is not ready")),
        "the early click was ignored with a note: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn leaving_a_room_dispatches_the_leave_handler() {
    let fixture = logon_fixture();
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("leave-cache");
    let seed = seed_media_dir(&room_desc(&fixture));
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| !entered_rooms(collected).is_empty(),
        Duration::from_secs(20),
    );
    assert!(!entered_rooms(&events).is_empty());

    // Navigating away while the room is occupied fires the room's ON LEAVE.
    handle.goto_room(777);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "LEAVE")
        },
        Duration::from_secs(5),
    );
    let leave = script_runs(&events);
    assert!(
        leave
            .iter()
            .any(|run| run.event == "LEAVE" && run.fired >= 1),
        "the room's ON LEAVE handler fired: {leave:?}"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_refused_room_change_restores_the_room_and_its_scripts() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let payload = room_desc_payload(&fixture);

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload.clone())
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![
        Frame::new(opcode::NAVERROR, 1, Vec::new())
            .encode(order)
            .expect("the nav error encodes"),
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    ];
    let (server, gate) = MockServer::start_gated(initial, tail);
    let cache = unique_temp_dir("refused-nav-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let before = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("the room composited before the refused change")
        .version;

    handle.goto_room(750);
    assert!(
        wait_for(
            || sent_room_goto(&server, order) == Some(750),
            Duration::from_secs(5),
        ),
        "the client asked for room 750"
    );
    gate.store(true, Ordering::Relaxed);

    assert!(
        wait_for(
            || {
                server
                    .received_frames(order)
                    .iter()
                    .filter(|frame| frame.opcode == opcode::ROOMGOTO)
                    .count()
                    >= 2
            },
            Duration::from_secs(10),
        ),
        "a refused change must be answered by re-requesting the room we are still in"
    );
    let gotos: Vec<u16> = server
        .received_frames(order)
        .iter()
        .filter(|frame| frame.opcode == opcode::ROOMGOTO)
        .filter_map(|frame| Reader::new(&frame.payload, order).read_u16().ok())
        .collect();
    assert_eq!(
        gotos,
        vec![750, room.header.room_id as u16],
        "the client asked for room 750, was refused, then asked for its own room again"
    );

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.version > before)
        },
        Duration::from_secs(10),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901 && screen.version > before)
        .expect("the restored room recomposited");

    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a scripted hotspot");
    let (x, y) = hotspot_click(spot, screen);
    handle.click(x, y);

    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "the restored room still dispatches the hotspot's ON SELECT: {:?}",
        script_runs(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

// ---------------------------------------------------------------------------
// Failure paths
// ---------------------------------------------------------------------------

#[test]
fn a_server_that_closes_mid_session_is_reported_and_retried() {
    let fixture = logon_fixture();
    let mut frames = server_bytes(&fixture);
    // Only the handshake and the version frame: then the server goes away.
    frames.truncate(3);
    let server = MockServer::start_half_close(frames);
    let cache = unique_temp_dir("close-cache");
    let seed = seed_media_dir(&room_desc(&fixture));
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            chats(collected)
                .iter()
                .any(|text| text.contains("server closed the connection"))
                && !statuses(collected, ConnectionStatus::Connecting).is_empty()
        },
        Duration::from_secs(15),
    );

    assert!(
        !statuses(&events, ConnectionStatus::Connected).is_empty(),
        "the session came up before the server left"
    );
    assert!(
        chats(&events)
            .iter()
            .any(|text| text.contains("server closed the connection")),
        "the close was reported: {:?}",
        chats(&events)
    );
    let reconnecting = statuses(&events, ConnectionStatus::Connecting);
    assert!(
        reconnecting
            .iter()
            .any(|message| message.as_deref() == Some("reconnecting")),
        "the supervisor moved into reconnect backoff: {reconnecting:?}"
    );
    assert!(handle.is_running(), "it is retrying, not stopped");

    handle.disconnect();
    let shut_down = collect_events(
        &mut rx,
        |collected| !statuses(collected, ConnectionStatus::Disconnected).is_empty(),
        Duration::from_secs(5),
    );
    assert!(!statuses(&shut_down, ConnectionStatus::Disconnected).is_empty());
    assert!(!handle.is_running());

    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_server_down_reason_reaches_the_client_without_reconnecting() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(server_down_frame(order, 12, None)); // K_Banished
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("server-down-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Disconnected).is_empty()
                && chats(collected)
                    .iter()
                    .any(|text| text.contains("banished"))
        },
        Duration::from_secs(15),
    );

    let disconnected = statuses(&events, ConnectionStatus::Disconnected);
    assert!(
        disconnected.iter().any(|message| message
            .as_deref()
            .is_some_and(|text| text.contains("banished"))),
        "the server's reason reached the status stream: {disconnected:?}"
    );
    assert!(
        chats(&events).iter().any(|text| text.contains("banished")),
        "the transcript names the reason: {:?}",
        chats(&events)
    );
    assert!(
        !chats(&events)
            .iter()
            .any(|text| text.contains("ignored unimplemented opcode")),
        "SERVERDOWN must not fall through to the unimplemented-opcode note: {:?}",
        chats(&events)
    );
    assert!(
        statuses(&events, ConnectionStatus::Connecting).is_empty(),
        "a server that ended the session must not be retried: {:?}",
        statuses(&events, ConnectionStatus::Connecting)
    );
    assert!(wait_for(|| !handle.is_running(), Duration::from_secs(2)));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        server.connections(),
        1,
        "the mock saw exactly one connection, so no reconnect was attempted"
    );

    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_server_down_as_the_first_packet_is_reported_without_reconnecting() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    // A server-full refusal instead of the `tiyr` banner.
    let frames = vec![server_down_frame(order, 8, None)];
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("server-down-handshake-cache");
    let seed = unique_temp_dir("server-down-handshake-seed");
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Disconnected).is_empty()
                && chats(collected).iter().any(|text| text.contains("full"))
        },
        Duration::from_secs(10),
    );

    assert!(
        statuses(&events, ConnectionStatus::Disconnected)
            .iter()
            .any(|message| message.as_deref().is_some_and(|text| text.contains("full"))),
        "the refusal's reason reached the status stream: {:?}",
        statuses(&events, ConnectionStatus::Disconnected)
    );
    assert!(
        statuses(&events, ConnectionStatus::Error).is_empty(),
        "a refused handshake is a disconnect with a reason, not a retryable wire error: {:?}",
        statuses(&events, ConnectionStatus::Error)
    );
    assert!(statuses(&events, ConnectionStatus::Connecting).is_empty());
    assert!(wait_for(|| !handle.is_running(), Duration::from_secs(2)));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(server.connections(), 1, "the refusal must not be retried");

    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_verbose_server_down_shows_the_servers_own_message() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(server_down_frame(order, 16, Some("scheduled maintenance")));
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("verbose-down-cache");
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Disconnected).is_empty()
                && chats(collected)
                    .iter()
                    .any(|text| text.contains("scheduled maintenance"))
        },
        Duration::from_secs(15),
    );

    assert!(
        statuses(&events, ConnectionStatus::Disconnected)
            .iter()
            .any(|message| message.as_deref() == Some("scheduled maintenance")),
        "the server's own message is the status: {:?}",
        statuses(&events, ConnectionStatus::Disconnected)
    );
    assert!(
        chats(&events)
            .iter()
            .any(|text| text.contains("scheduled maintenance")),
        "the transcript carries the server's own message: {:?}",
        chats(&events)
    );
    assert!(statuses(&events, ConnectionStatus::Connecting).is_empty());
    assert!(wait_for(|| !handle.is_running(), Duration::from_secs(2)));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(server.connections(), 1);

    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_malformed_frame_header_is_reported_as_a_wire_error() {
    let fixture = logon_fixture();
    let mut frames = server_bytes(&fixture);
    frames.truncate(1); // the TIYID handshake only
                        // A header that claims a 2 GiB payload: the transport must refuse it.
    let mut bad = vec![0u8; 12];
    bad[..4].copy_from_slice(b"junk");
    bad[4..8].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    frames.push(bad);

    let server = MockServer::start(frames);
    let cache = unique_temp_dir("bad-cache");
    let seed = seed_media_dir(&room_desc(&fixture));
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Error).is_empty()
                && !error_chats(collected).is_empty()
        },
        Duration::from_secs(10),
    );

    let errors = statuses(&events, ConnectionStatus::Error);
    assert!(
        !errors.is_empty(),
        "the malformed header produced an error status; events: {events:?}"
    );
    let Some(Some(message)) = errors.first() else {
        panic!("the error status carried a message: {errors:?}");
    };
    assert!(
        message.contains("wire"),
        "the error names the wire failure: {message}"
    );
    assert!(
        error_chats(&events)
            .iter()
            .any(|text| text.contains("wire")),
        "the transcript carries the error: {:?}",
        error_chats(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_refused_connection_is_reported_as_an_io_error() {
    // Grab a port, then release it so nothing is listening.
    let port = {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let cache = unique_temp_dir("refused-cache");
    let seed = unique_temp_dir("refused-seed");
    let (handle, stream) = ClientRuntime::spawn(config_for(port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            !statuses(collected, ConnectionStatus::Error).is_empty()
                && !error_chats(collected).is_empty()
        },
        Duration::from_secs(10),
    );

    let errors = statuses(&events, ConnectionStatus::Error);
    let Some(Some(message)) = errors.first() else {
        panic!("a refused connection produced an error status with a message: {errors:?}");
    };
    assert!(
        message.contains("io"),
        "the error names the io failure: {message}"
    );
    assert!(
        error_chats(&events).iter().any(|text| text.contains("io")),
        "the transcript carries the error: {:?}",
        error_chats(&events)
    );

    handle.disconnect();
    cleanup(&cache);
    cleanup(&seed);
}

// ---------------------------------------------------------------------------
// DIMROOM: a script dims the frame it renders
// ---------------------------------------------------------------------------

/// A plainly visible fill (200/255 grey) so a dim is a measurable change.
const BRIGHT: [u8; 3] = [0xC8, 0xC8, 0xC8];

fn solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        writer.write_image_data(&data).expect("png data");
    }
    out
}

fn room_payload(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/rooms")
        .join(format!("{name}.bin"));
    fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Seed every room's background, and only that, with one solid `rgb` fill.
///
/// Overlay pictures are left out on purpose: `build::build_with` draws them in a
/// band that is *not* dimmed, so a bright overlay would hide a `DIMROOM`. The
/// background is drawn under the dim, so it is the honest probe.
fn seed_solid_media_dir(room: &palace_room::RoomDesc, rgb: [u8; 3]) -> PathBuf {
    let dir = unique_temp_dir("seed-solid");
    let png = solid_png(512, 384, rgb);
    if let Some(base) = Path::new(&room.picture).file_name() {
        if !base.is_empty() {
            fs::write(dir.join(base), &png).expect("write seeded media");
        }
    }
    dir
}

fn frame_rgba(png: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png buffer size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    buf
}

fn mean_luma(rgba: &[u8]) -> f64 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for px in rgba.chunks_exact(4) {
        if px[3] == 0 {
            continue;
        }
        sum += 0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2]);
        count += 1;
    }
    assert!(count > 0, "frame has no opaque pixels");
    sum / count as f64
}

fn frame_luma(handle: &ClientHandle) -> f64 {
    let png = handle.frames().png().expect("the frame store holds a PNG");
    mean_luma(&frame_rgba(&png))
}

/// The integer room cell out of the runtime's own report, e.g.
/// `script: click at room (465,26) hit hotspot 2`.
fn clicked_room_note(note: &str) -> (i32, i32) {
    let (_, rest) = note
        .split_once("click at room (")
        .expect("the note reports a room coordinate");
    let (pair, _) = rest
        .split_once(')')
        .expect("the coordinate is parenthesised");
    let (x, y) = pair.split_once(',').expect("the note is (x,y)");
    (
        x.parse().expect("an integer room x"),
        y.parse().expect("an integer room y"),
    )
}

/// The viewport point that hits the first hotspot whose script calls `DIMROOM`.
fn dim_hotspot_click(room: &palace_room::RoomDesc, screen: &ScreenState) -> (f64, f64) {
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("DIMROOM"))
        })
        .expect("the room has a hotspot whose script calls DIMROOM");
    let point = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(
            f64::from(spot.loc.h),
            f64::from(spot.loc.v),
        ));
    (point.x, point.y)
}

/// The seeded fill makes every opaque pixel the same brightness, so a change in
/// the composited frame is a change in `DIMROOM` and nothing else.
#[test]
fn a_script_dimroom_darkens_the_composited_frame() {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, room_desc_payload(&fixture))
            .encode(fixture.byte_order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("dim-script-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    assert!(
        !screens(&events).is_empty(),
        "a frame was composited for the entered room"
    );
    let undimmed = frame_luma(&handle);
    let baseline_version = handle.frames().version();
    assert!(
        undimmed > 100.0,
        "the seeded room renders a bright frame (luma {undimmed})"
    );

    handle.run_script("40 DIMROOM");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: DIMROOM 40%"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: DIMROOM 40%")),
        "the effect was applied and reported: {:?}",
        notes(&events)
    );
    assert!(
        wait_for(
            || handle.frames().version() > baseline_version,
            Duration::from_secs(5)
        ),
        "the dim was composited into a new frame"
    );

    let dimmed = frame_luma(&handle);
    let ratio = dimmed / undimmed;
    assert!(
        (ratio - 0.4).abs() < 0.05,
        "DIMROOM 40 multiplied the frame by ~0.4 (ratio {ratio}, {dimmed} vs {undimmed})"
    );
    assert!(
        dimmed < undimmed * 0.6,
        "the dimmed frame is measurably darker ({dimmed} vs {undimmed})"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn dimroom_percent_clamps_to_the_valid_range() {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, room_desc_payload(&fixture))
            .encode(fixture.byte_order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("dim-clamp-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    assert!(!screens(&events).is_empty());
    let baseline = frame_luma(&handle);
    let mut version = handle.frames().version();

    handle.run_script("150 DIMROOM");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: DIMROOM 150%"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: DIMROOM 150%")),
        "the out-of-range value reached the effect: {:?}",
        notes(&events)
    );
    assert!(wait_for(
        || handle.frames().version() > version,
        Duration::from_secs(5)
    ));
    let over = frame_luma(&handle);
    assert!(
        (over - baseline).abs() < baseline * 0.02,
        "DIMROOM 150 saturates to undimmed ({over} vs {baseline})"
    );
    version = handle.frames().version();

    handle.run_script("0 50 - DIMROOM");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: DIMROOM -50%"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: DIMROOM -50%")),
        "the negative value reached the effect: {:?}",
        notes(&events)
    );
    assert!(wait_for(
        || handle.frames().version() > version,
        Duration::from_secs(5)
    ));
    let under = frame_luma(&handle);
    assert!(
        under < baseline * 0.02,
        "DIMROOM -50 saturates to fully dark ({under} vs {baseline})"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_real_room_hotspot_script_dimroom_darkens_the_frame() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("86");
    let room = palace_room::decode_payload(&payload, order).expect("room 86 decodes");

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );

    let server = MockServer::start(frames);
    let cache = unique_temp_dir("dim-room-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(|screen| screen.room_id == 86),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 86)
        .expect("a frame was composited for room 86");
    let undimmed = frame_luma(&handle);
    let baseline_version = handle.frames().version();
    assert!(
        undimmed > 100.0,
        "the seeded room renders a bright frame (luma {undimmed})"
    );

    let (x, y) = dim_hotspot_click(&room, screen);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: DIMROOM 25%"))
                && script_runs(collected).iter().any(|run| {
                    run.event == "SELECT" && run.effects.iter().any(|effect| effect == "DIMROOM 25")
                })
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events).iter().any(|run| {
            run.event == "SELECT" && run.effects.iter().any(|effect| effect == "DIMROOM 25")
        }),
        "the room's hotspot script ran DIMROOM 25: {:?}",
        script_runs(&events)
    );
    assert!(
        wait_for(
            || handle.frames().version() > baseline_version,
            Duration::from_secs(5)
        ),
        "the dim was composited into a new frame"
    );

    let dimmed = frame_luma(&handle);
    let ratio = dimmed / undimmed;
    assert!(
        (ratio - 0.25).abs() < 0.05,
        "the real hotspot's DIMROOM 25 dimmed the frame to ~0.25 (ratio {ratio}, {dimmed} vs {undimmed})"
    );
    assert!(
        dimmed < undimmed * 0.5,
        "the dimmed frame is measurably darker ({dimmed} vs {undimmed})"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn entering_a_new_room_resets_the_dim() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload_86 = room_payload("86");
    let room_86 = palace_room::decode_payload(&payload_86, order).expect("room 86 decodes");
    let payload_887 = room_payload("887");
    let room_887 = palace_room::decode_payload(&payload_887, order).expect("room 887 decodes");

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload_86)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![Frame::new(opcode::ROOMDESC, 0, payload_887)
        .encode(order)
        .expect("the room descriptor encodes")];
    let (server, gate) = MockServer::start_gated(initial, tail);

    let cache = unique_temp_dir("dim-reset-cache");
    let seed_86 = seed_solid_media_dir(&room_86, BRIGHT);
    let seed_887 = seed_solid_media_dir(&room_887, BRIGHT);
    let mut cfg = config_for(server.port, cache.clone(), seed_86.clone());
    cfg.seed_media.push(seed_887.clone());
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(|screen| screen.room_id == 86),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 86)
        .expect("a frame was composited for room 86");
    let undimmed = frame_luma(&handle);
    assert!(
        undimmed > 100.0,
        "the seeded room renders a bright frame (luma {undimmed})"
    );

    let (x, y) = dim_hotspot_click(&room_86, screen);
    handle.click(x, y);
    let dim_version = handle.frames().version();
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: DIMROOM 25%"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: DIMROOM 25%")),
        "the room 86 hotspot dimmed the frame: {:?}",
        notes(&events)
    );
    assert!(wait_for(
        || handle.frames().version() > dim_version,
        Duration::from_secs(5)
    ));
    let dimmed = frame_luma(&handle);
    assert!(
        dimmed < undimmed * 0.5,
        "room 86 is dim before the room change ({dimmed} vs {undimmed})"
    );
    let dimmed_version = handle.frames().version();

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 887)
        },
        Duration::from_secs(20),
    );
    assert!(
        screens(&events).iter().any(|screen| screen.room_id == 887),
        "the second room descriptor was entered: {:?}",
        screens(&events)
            .iter()
            .map(|screen| screen.room_id)
            .collect::<Vec<_>>()
    );
    assert!(wait_for(
        || handle.frames().version() > dimmed_version,
        Duration::from_secs(5)
    ));
    let after = frame_luma(&handle);
    assert!(
        after > undimmed * 0.9,
        "entering a new room reset the dim to undimmed ({after} vs {undimmed})"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed_86);
    cleanup(&seed_887);
}

// ---------------------------------------------------------------------------
// Appearance and loose-prop messages: another user's face/colour and the room's
// props must change both the model and the composited frame
// ---------------------------------------------------------------------------

/// The handshake plus the fixture's room descriptor, with nothing else.
fn room_only_frames(fixture: &Fixture) -> Vec<Vec<u8>> {
    let order = fixture.byte_order;
    vec![
        server_bytes(fixture)[0].clone(),
        Frame::new(opcode::ROOMDESC, 0, room_desc_payload(fixture))
            .encode(order)
            .expect("room descriptor encodes"),
    ]
}

/// The two users that make the determinism test meaningful: same y and
/// overlapping x, so any order-dependent draw would be visible.
const ORDER_USERS: [(i32, &str, i16, i16); 2] = [(101, "Alpha", 3, 5), (102, "Beta", 7, 2)];

fn frames_with_users_in_order(fixture: &Fixture, forward: bool) -> Vec<Vec<u8>> {
    let order = fixture.byte_order;
    let mut frames = room_only_frames(fixture);
    let indices: [usize; 2] = if forward { [0, 1] } else { [1, 0] };
    for index in indices {
        let (id, name, face, color) = ORDER_USERS[index];
        frames.push(user_new_frame(
            order,
            id,
            name,
            face,
            color,
            &[],
            Point::new(200 + 20 * index as i16, 200),
        ));
    }
    frames
}

/// One run: serve `frames`, wait until every user is drawn, then capture the
/// composited PNG. The frame is read after a viewport change forces a fresh
/// composition, so the captured bytes are the same pipeline step in both runs.
fn compose_and_capture(frames: Vec<Vec<u8>>, tag: &str) -> Vec<u8> {
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(frames);
    let cache = unique_temp_dir(&format!("determinism-{tag}-cache"));
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
                && user_lists(collected).iter().any(|users| {
                    users.iter().any(|user| user.id == ORDER_USERS[0].0)
                        && users.iter().any(|user| user.id == ORDER_USERS[1].0)
                })
        },
        Duration::from_secs(20),
    );
    let baseline = screens(&events)
        .iter()
        .find(|screen| screen.room_id == 901)
        .map(|screen| screen.version)
        .expect("a frame was composed");
    assert!(
        user_lists(&events).iter().any(|users| {
            users.iter().any(|user| user.id == ORDER_USERS[0].0)
                && users.iter().any(|user| user.id == ORDER_USERS[1].0)
        }),
        "both users arrived before the frame was captured"
    );
    // The user list is emitted before the next composition, so wait for the
    // frame that actually has both of them before capturing anything.
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.version >= baseline + 2)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.version >= baseline + 2),
        "the frame with both users was composed"
    );

    handle.set_viewport(1000.0, 700.0, 1.0, 1.0, false);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.version > baseline)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.version > baseline),
        "the settled frame was recomposed"
    );
    let png = handle.frames().png().expect("a frame was composited");

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    png
}

/// Users are stored in a map and can arrive in any order; the composited frame
/// must not depend on that order. This is the guard against a future change
/// leaking iteration order into the frame.
#[test]
fn the_same_users_inserted_in_a_different_order_composite_byte_identical_frames() {
    let fixture = logon_fixture();
    let forward = compose_and_capture(frames_with_users_in_order(&fixture, true), "forward");
    let reversed = compose_and_capture(frames_with_users_in_order(&fixture, false), "reversed");
    assert_eq!(
        forward, reversed,
        "the same room and users in a different insertion order must produce byte-identical frames"
    );
    // 512x384 at dpr 1: the captured PNG is the room, not the viewport.
    assert_eq!(
        frame_rgba(&forward).len(),
        512 * 384 * 4,
        "the captured frame is the room at dpr 1"
    );
}

#[test]
fn a_remote_face_and_colour_change_update_the_user_and_recompose() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(user_new_frame(
        order,
        21,
        "Other",
        1,
        2,
        &[],
        Point::new(100, 120),
    ));
    let tail = vec![
        user_face_frame(order, 21, 7),
        user_color_frame(order, 21, 9),
    ];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("appearance-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
                && user_lists(collected)
                    .iter()
                    .any(|users| users.iter().any(|user| user.id == 21))
        },
        Duration::from_secs(20),
    );
    assert!(
        user_lists(&events)
            .iter()
            .any(|users| users.iter().any(|user| user.id == 21 && user.face == 1)),
        "another user entered with face 1"
    );
    let baseline_png = handle.frames().png().expect("a frame was composited");
    let baseline_version = handle.frames().version();

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            let face = user_lists(collected)
                .iter()
                .any(|users| users.iter().any(|user| user.id == 21 && user.face == 7));
            let color = user_lists(collected)
                .iter()
                .any(|users| users.iter().any(|user| user.id == 21 && user.color == 9));
            face && color
        },
        Duration::from_secs(10),
    );
    assert!(
        user_lists(&events)
            .iter()
            .any(|users| users.iter().any(|user| user.id == 21 && user.face == 7)),
        "USERFACE updated the other user's face"
    );
    assert!(
        user_lists(&events)
            .iter()
            .any(|users| users.iter().any(|user| user.id == 21 && user.color == 9)),
        "USERCOLOR updated the other user's colour"
    );
    assert!(
        wait_for(
            || handle.frames().version() > baseline_version,
            Duration::from_secs(5)
        ),
        "the appearance change was composited into a new frame"
    );
    assert_ne!(
        handle.frames().png().as_deref(),
        Some(baseline_png.as_slice()),
        "the composited frame changed when the other user's face and colour did"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_self_user_appearance_message_reaches_the_self_record() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(self_user_frame(order));
    let tail = vec![
        user_prop_frame(order, SELF_ID, &[(10, 0), (20, 0), (30, 0)]),
        user_desc_frame(order, SELF_ID, 9, 3, &[(111, 7), (222, 0)]),
    ];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("self-appearance-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
                && user_lists(collected)
                    .iter()
                    .any(|users| users.iter().any(|user| user.id == SELF_ID))
        },
        Duration::from_secs(20),
    );
    assert!(
        user_lists(&events)
            .iter()
            .any(|users| users.iter().any(|user| user.id == SELF_ID && user.is_self)),
        "the handshake's own user is present and flagged self"
    );

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            user_lists(collected).iter().any(|users| {
                users
                    .iter()
                    .any(|user| user.id == SELF_ID && user.props == vec![111, 222])
            })
        },
        Duration::from_secs(10),
    );
    assert!(
        user_lists(&events).iter().any(|users| {
            users
                .iter()
                .any(|user| user.id == SELF_ID && user.props == vec![10, 20, 30])
        }),
        "USERPROP replaced the self user's worn list wholesale"
    );
    let last_self = user_lists(&events)
        .iter()
        .filter_map(|users| users.iter().find(|user| user.id == SELF_ID))
        .next_back()
        .expect("the self user was re-emitted");
    assert_eq!(
        (last_self.face, last_self.color),
        (9, 3),
        "USERDESC carried the new face and colour"
    );
    assert_eq!(
        last_self.props,
        vec![111, 222],
        "USERDESC replaced the worn props in order"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_script_rename_sends_a_usern_frame_with_our_id_and_name() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, screen, cache, seed) =
        start_room_with_self(&fixture, "rename-send");
    let initial_version = screen.version;

    handle.run_script("\"Bob\" SETUSERNAME");
    assert!(
        wait_for(
            || server
                .received_frames(order)
                .iter()
                .any(|frame| frame.opcode == opcode::USERNAME),
            Duration::from_secs(5)
        ),
        "SETUSERNAME reached the server as a usrN frame"
    );
    match sent_message(&server, order, opcode::USERNAME) {
        Message::UserName(name) => {
            assert_eq!(name.user_id, SELF_ID, "the rename names our own user id");
            assert_eq!(name.name, "Bob");
        }
        other => panic!("expected UserName, got {other:?}"),
    }

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("script: SETUSERNAME \"Bob\""))
                && screens(collected)
                    .iter()
                    .any(|screen| screen.room_id == 901 && screen.version > initial_version)
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("script: SETUSERNAME \"Bob\"")),
        "the rename was reported: {:?}",
        notes(&events)
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("local only")),
        "the stale '(local only)' note is gone: {:?}",
        notes(&events)
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.version > initial_version),
        "the rename was applied locally at once and forced a re-render: {:?}",
        screens(&events)
            .iter()
            .map(|screen| screen.version)
            .collect::<Vec<_>>()
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_usern_for_another_user_renames_that_user() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(user_new_frame(
        order,
        21,
        "Other",
        1,
        2,
        &[],
        Point::new(100, 120),
    ));
    let tail = vec![user_name_frame(order, 21, "Renamed")];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("remote-rename-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
                && user_lists(collected).iter().any(|users| {
                    users
                        .iter()
                        .any(|user| user.id == 21 && user.name == "Other")
                })
        },
        Duration::from_secs(20),
    );
    assert!(
        user_lists(&events).iter().any(|users| users
            .iter()
            .any(|user| user.id == 21 && user.name == "Other")),
        "another user entered before the rename: {:?}",
        user_lists(&events)
    );

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            user_lists(collected).iter().any(|users| {
                users
                    .iter()
                    .any(|user| user.id == 21 && user.name == "Renamed")
            })
        },
        Duration::from_secs(10),
    );
    assert!(
        user_lists(&events).iter().any(|users| users
            .iter()
            .any(|user| user.id == 21 && user.name == "Renamed")),
        "usrN renamed the other user and re-emitted the room list: {:?}",
        user_lists(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_usern_for_the_local_user_with_the_previous_name_reverts_it() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(self_user_frame(order));
    let tail = vec![user_name_frame(order, SELF_ID, "RustProbe")];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("self-revert-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
                && user_lists(collected).iter().any(|users| {
                    users
                        .iter()
                        .any(|user| user.id == SELF_ID && user.name == "RustProbe")
                })
        },
        Duration::from_secs(20),
    );
    assert!(
        user_lists(&events).iter().any(|users| users
            .iter()
            .any(|user| user.id == SELF_ID && user.name == "RustProbe")),
        "our user entered as RustProbe: {:?}",
        user_lists(&events)
    );

    // Rename ourselves locally. The mock server is gated, so it has not
    // answered yet; the model must already read "Bob", which shows as a fresh
    // render of the name tag.
    let entry_version = screens(&events)
        .iter()
        .find(|screen| screen.room_id == 901)
        .map(|screen| screen.version)
        .expect("the entered room was composited");
    handle.run_script("\"Bob\" SETUSERNAME");
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.version > entry_version)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.version > entry_version),
        "the local rename was applied at once and forced a re-render: {:?}",
        screens(&events)
            .iter()
            .map(|screen| screen.version)
            .collect::<Vec<_>>()
    );

    // The documented failure path: the server reports the *previous* name back.
    // Applying it verbatim must put RustProbe in place, so a Users event for the
    // self user can only be emitted if "Bob" had been applied locally first.
    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            user_lists(collected).iter().any(|users| {
                users
                    .iter()
                    .any(|user| user.id == SELF_ID && user.name == "RustProbe")
            })
        },
        Duration::from_secs(10),
    );
    assert!(
        user_lists(&events).iter().any(|users| users
            .iter()
            .any(|user| user.id == SELF_ID && user.name == "RustProbe")),
        "the server's previous-name reply reverted the local rename: {:?}",
        user_lists(&events)
    );

    match sent_message(&server, order, opcode::USERNAME) {
        Message::UserName(name) => {
            assert_eq!(name.user_id, SELF_ID);
            assert_eq!(name.name, "Bob", "the rename we asked for was sent");
        }
        other => panic!("expected UserName, got {other:?}"),
    }

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn propnew_appends_a_loose_prop_that_reaches_the_composited_frame() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let frames = room_only_frames(&fixture);
    let tail = vec![prop_new_frame(
        order,
        4242,
        0xdead_beef,
        Point::new(90, 150),
    )];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("propnew-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let props = seed_props_dir(&[4242], [0x10, 0x20, 0xE0]);
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    assert!(
        screens(&events)
            .iter()
            .all(|screen| screen.loose_props == 0),
        "the fixture room starts with no loose props"
    );
    let baseline_png = handle.frames().png().expect("a frame was composited");
    let baseline_version = handle.frames().version();

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 1),
        "PROPNEW grew the room's loose-prop list"
    );
    assert!(
        wait_for(
            || handle.frames().version() > baseline_version,
            Duration::from_secs(5)
        ),
        "the appended prop was composited into a new frame"
    );
    assert_ne!(
        handle.frames().png().as_deref(),
        Some(baseline_png.as_slice()),
        "the composited frame changed when the prop was added"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props);
}

#[test]
fn propmove_repositions_a_loose_prop_and_recomposes() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(prop_new_frame(order, 4242, 0, Point::new(50, 60)));
    let tail = vec![prop_move_frame(order, 0, Point::new(360, 300))];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("propmove-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let props = seed_props_dir(&[4242], [0xE0, 0x30, 0x10]);
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 1)
        },
        Duration::from_secs(20),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 1),
        "the prop is present before the move"
    );
    let at_rest_png = handle.frames().png().expect("a frame was composited");
    let before_move_version = handle.frames().version();

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 1 && screen.version > before_move_version)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 1),
        "PROPMOVE left the prop in the room"
    );
    assert!(
        wait_for(
            || handle.frames().version() > before_move_version,
            Duration::from_secs(5)
        ),
        "the move was composited into a new frame"
    );
    assert_ne!(
        handle.frames().png().as_deref(),
        Some(at_rest_png.as_slice()),
        "the composited frame changed when the prop moved"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props);
}

#[test]
fn propdel_removes_the_loose_prop_and_recomposes() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(prop_new_frame(order, 4242, 0, Point::new(50, 60)));
    let tail = vec![prop_del_frame(order, 0)];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("propdel-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let props = seed_props_dir(&[4242], [0x20, 0xE0, 0x20]);
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 1)
        },
        Duration::from_secs(20),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 1),
        "the prop is present before the delete"
    );
    let with_prop_png = handle.frames().png().expect("a frame was composited");
    let before_delete_version = handle.frames().version();

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 0)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 0),
        "PROPDEL shrank the room's loose-prop list"
    );
    assert!(
        wait_for(
            || handle.frames().version() > before_delete_version,
            Duration::from_secs(5)
        ),
        "the delete was composited into a new frame"
    );
    assert_ne!(
        handle.frames().png().as_deref(),
        Some(with_prop_png.as_slice()),
        "the composited frame changed when the prop was deleted"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props);
}

#[test]
fn a_stale_prop_index_does_not_clear_the_room() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(prop_new_frame(order, 4242, 0, Point::new(50, 60)));
    let tail = vec![
        prop_del_frame(order, 999),
        prop_new_frame(order, 5252, 0, Point::new(200, 220)),
    ];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("stale-index-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let props = seed_props_dir(&[4242, 5252], [0x80, 0x80, 0x80]);
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 1)
        },
        Duration::from_secs(20),
    );
    assert!(screens(&events)
        .iter()
        .any(|screen| screen.loose_props == 1));

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 2)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 2),
        "a stale PROPDEL index is ignored, so both props remain"
    );
    assert!(
        chats(&events)
            .iter()
            .any(|text| text.contains("ignored PROPDEL")),
        "the stale index was reported"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props);
}

#[test]
fn propdel_minus_one_clears_every_loose_prop() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(prop_new_frame(order, 4242, 0, Point::new(50, 60)));
    frames.push(prop_new_frame(order, 5252, 0, Point::new(200, 220)));
    let tail = vec![prop_del_frame(order, -1)];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("propdel-all-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let props = seed_props_dir(&[4242, 5252], [0x80, 0x80, 0x80]);
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 2)
        },
        Duration::from_secs(20),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 2),
        "both props are present before the clear"
    );

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.loose_props == 0)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.loose_props == 0),
        "PROPDEL -1 cleared every loose prop"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props);
}

// ---------------------------------------------------------------------------
// Door locks: only a lockable door (`HS_LockableDoor`, type 3) whose hotspot
// state is 1 is refused locally instead of dispatching SELECT. A plain door's
// state is a picture selector, so state 1 must still dispatch.
// ---------------------------------------------------------------------------

/// Find the first plain door (`HS_Door`, type 1) in a room that also has an
/// `ON SELECT` handler, so a click on it is observably dispatched when it is
/// not refused.
fn clickable_door(room: &palace_room::RoomDesc) -> &palace_room::Hotspot {
    room.hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 1
                && hotspot
                    .script
                    .as_deref()
                    .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a door with an ON SELECT script")
}

/// Find the first lockable door (`HS_LockableDoor`, type 3) with an
/// `ON SELECT` handler: the only door whose state-1 click may be refused.
fn lockable_door(room: &palace_room::RoomDesc) -> &palace_room::Hotspot {
    room.hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 3
                && hotspot
                    .script
                    .as_deref()
                    .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a lockable door with an ON SELECT script")
}

/// The viewport point that hits a hotspot's anchor, through the transform the
/// compositor reported.
fn hotspot_click(spot: &palace_room::Hotspot, screen: &ScreenState) -> (f64, f64) {
    let point = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(
            f64::from(spot.loc.h),
            f64::from(spot.loc.v),
        ));
    (point.x, point.y)
}

#[test]
fn a_click_puts_the_pointer_where_the_click_landed() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("25567");
    let room = palace_room::decode_payload(&payload, order).expect("room 25567 decodes");
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| hotspot.hotspot_type != 1 || hotspot.dest == 0)
        .expect("room 25567 has a hotspot that does not change rooms");

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("mousepos-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 25567)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 25567)
        .expect("a frame was composed for room 25567");

    let (x, y) = hotspot_click(spot, screen);
    handle.click(x, y);
    handle.run_script("MOUSEPOS ITOA STATUSMSG ITOA STATUSMSG");

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .filter_map(|text| text.strip_prefix("script: "))
                .filter(|text| text.trim().parse::<i32>().is_ok())
                .count()
                >= 2
        },
        Duration::from_secs(10),
    );
    let reported: Vec<i32> = notes(&events)
        .iter()
        .filter_map(|text| text.strip_prefix("script: "))
        .filter_map(|text| text.trim().parse().ok())
        .collect();

    assert!(
        reported.contains(&i32::from(spot.loc.h)) && reported.contains(&i32::from(spot.loc.v)),
        "MOUSEPOS should report the click's room position ({}, {}), got {reported:?} from {:?}",
        spot.loc.h,
        spot.loc.v,
        notes(&events)
    );
}

#[test]
fn hovering_a_hotspot_dispatches_rollover_and_leaving_dispatches_rollout() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("86");
    let room = palace_room::decode_payload(&payload, order).expect("room 86 decodes");
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("ON ROLLOVER"))
        })
        .expect("room 86 has a hotspot with ON ROLLOVER");
    let spot_id = i32::from(spot.id);

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("hover-cache");
    let seed = seed_media_dir(&room);
    let props_seed = unique_temp_dir("hover-props");
    let mut cfg = config_for(server.port, cache.clone(), seed.clone());
    cfg.seed_props = vec![props_seed.clone()];
    let (handle, stream) = ClientRuntime::spawn(cfg);
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(|screen| screen.room_id == 86),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 86)
        .expect("a frame was composed for room 86");
    let (vx, vy) = hotspot_click(spot, screen);
    let empty = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(-500.0, -500.0));

    // Attach the handlers this test observes: a tooltip on entry, an empty
    // MOUSEMOVE so the movement event is visible, and a clear on leaving.
    handle.run_script(format!(
        "{{ \"hover tooltip\" SETTOOLTIP }} \"ROLLOVER\" {spot_id} SETSPOTSCRIPT \
         {{ \"mv\" STATUSMSG }} \"MOUSEMOVE\" {spot_id} SETSPOTSCRIPT \
         {{ CLEARTOOLTIP }} \"ROLLOUT\" {spot_id} SETSPOTSCRIPT"
    ));
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .filter(|text| text.contains("SETSPOTSCRIPT"))
                .count()
                >= 3
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .filter(|text| text.contains("SETSPOTSCRIPT"))
            .count()
            >= 3,
        "the three handlers attached: {:?}",
        notes(&events)
    );

    handle.move_mouse(vx, vy);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "ROLLOVER")
                && script_runs(collected)
                    .iter()
                    .any(|run| run.event == "MOUSEMOVE")
                && tooltips(collected).contains(&Some("hover tooltip"))
        },
        Duration::from_secs(10),
    );
    let runs = script_runs(&events);
    assert!(
        runs.iter()
            .any(|run| run.event == "ROLLOVER" && run.fired >= 1),
        "entering the hotspot runs its ON ROLLOVER: {runs:?}"
    );
    assert!(
        runs.iter()
            .any(|run| run.event == "MOUSEMOVE" && run.fired >= 1),
        "the movement itself runs the hotspot's ON MOUSEMOVE: {runs:?}"
    );
    assert!(
        tooltips(&events).contains(&Some("hover tooltip")),
        "SETTOOLTIP reached the client: {:?}",
        tooltips(&events)
    );

    handle.move_mouse(empty.x, empty.y);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "ROLLOUT")
        },
        Duration::from_secs(10),
    );
    let runs = script_runs(&events);
    assert!(
        runs.iter()
            .any(|run| run.event == "ROLLOUT" && run.fired >= 1),
        "leaving the hotspot runs its ON ROLLOUT: {runs:?}"
    );
    assert!(
        tooltips(&events).iter().any(Option::is_none),
        "CLEARTOOLTIP reached the client: {:?}",
        tooltips(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&props_seed);
}

/// The room id of the first `ROOMGOTO` (`navR`) frame the client sent, if any.
/// The client sends it when a script's `GOTOROOM` becomes an effect, and the
/// server answers it with the destination room's descriptor.
fn sent_room_goto(server: &MockServer, order: ByteOrder) -> Option<u16> {
    let frames = server.received_frames(order);
    let frame = frames
        .iter()
        .find(|frame| frame.opcode == opcode::ROOMGOTO)?;
    Reader::new(&frame.payload, order).read_u16().ok()
}

/// Room 887's `ON ENTER` arms a one-shot `1 ALARMEXEC` that rewrites door 1 and
/// door 2's states on a later tick; a lock sent before it fires would be undone.
/// Wait for that alarm's recompose — the only one after the first frame here —
/// before a lock test acts.
fn wait_for_entry_alarm(handle: &ClientHandle, screen: &ScreenState) {
    let version = screen.version;
    assert!(
        wait_for(
            || handle.frames().version() > version,
            Duration::from_secs(5)
        ),
        "the room's entry alarm recomposed the frame"
    );
}

#[test]
fn a_click_on_a_plain_door_with_state_one_still_dispatches_select() {
    const LOCK_MARKER: &str = "plain-door-lock-processed";
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("887");
    let room = palace_room::decode_payload(&payload, order).expect("room 887 decodes");
    let door = clickable_door(&room);
    assert_eq!(door.hotspot_type, 1, "HS_Door, which cannot be locked");
    assert_eq!(door.state, 0, "the fixture door starts at picture 0");

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![
        door_lock_frame(order, 887, door.id),
        talk_marker_frame(order, LOCK_MARKER),
    ];
    let (server, gate) = MockServer::start_gated(initial, tail);
    let cache = unique_temp_dir("plain-door-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 887)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 887)
        .expect("a frame was composed for room 887");
    let (x, y) = hotspot_click(door, screen);
    wait_for_entry_alarm(&handle, screen);

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            chats(collected)
                .iter()
                .any(|text| text.contains(LOCK_MARKER))
        },
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(LOCK_MARKER)),
        "the DOORLOCK frame was processed before the click: {:?}",
        chats(&events)
    );

    handle.set_mouse(300, 200);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
                || notes(collected)
                    .iter()
                    .any(|text| text.contains("locked door"))
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "a plain door at state 1 must dispatch SELECT: {:?}",
        notes(&events)
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("locked door")),
        "a plain door is never refused, whatever its picture state: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_lock_for_another_room_does_not_refuse_this_rooms_door() {
    const MARKER: &str = "foreign-lock-processed";
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("86");
    let room = palace_room::decode_payload(&payload, order).expect("room 86 decodes");
    let door = lockable_door(&room);

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![
        door_lock_frame(order, 999, door.id),
        spot_state_frame(order, 86, 1, 1),
        talk_marker_frame(order, MARKER),
    ];
    let (server, gate) = MockServer::start_gated(initial, tail);
    let cache = unique_temp_dir("foreign-lock-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(|screen| screen.room_id == 86),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 86)
        .expect("a frame was composed for room 86");
    let (x, y) = hotspot_click(door, screen);

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).iter().any(|text| text.contains(MARKER)),
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(MARKER)),
        "both gate frames were processed before the click: {:?}",
        chats(&events)
    );

    handle.set_mouse(300, 200);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
                || notes(collected)
                    .iter()
                    .any(|text| text.contains("locked door"))
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "a lock aimed at room 999 must not lock this room's door: {:?}",
        notes(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains(&format!("hit hotspot {}", door.id))),
        "the click landed on the door: {:?}",
        notes(&events)
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("locked door")),
        "the foreign lock produced no refusal here: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_on_a_lockable_door_that_is_locked_is_still_refused() {
    const LOCK_MARKER: &str = "lockable-door-lock-processed";
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("86");
    let room = palace_room::decode_payload(&payload, order).expect("room 86 decodes");
    let door = lockable_door(&room);
    assert_eq!(door.hotspot_type, 3, "HS_LockableDoor");
    assert_eq!(door.state, 0, "the lockable fixture door starts unlocked");

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![
        door_lock_frame(order, 86, door.id),
        talk_marker_frame(order, LOCK_MARKER),
    ];
    let (server, gate) = MockServer::start_gated(initial, tail);
    let cache = unique_temp_dir("lockable-door-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(|screen| screen.room_id == 86),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 86)
        .expect("a frame was composed for room 86");
    let (x, y) = hotspot_click(door, screen);

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| {
            chats(collected)
                .iter()
                .any(|text| text.contains(LOCK_MARKER))
        },
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(LOCK_MARKER)),
        "the DOORLOCK frame was processed before the click: {:?}",
        chats(&events)
    );

    handle.set_mouse(300, 200);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("locked door"))
                || script_runs(collected)
                    .iter()
                    .any(|run| run.event == "SELECT" && run.fired >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("locked door")),
        "a lockable door at state 1 is still refused: {:?}",
        notes(&events)
    );
    assert!(
        !script_runs(&events).iter().any(|run| run.event == "SELECT"),
        "the refused click did not dispatch SELECT: {:?}",
        script_runs(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains(&format!("hit hotspot {}", door.id))),
        "the click landed on the door: {:?}",
        notes(&events)
    );
    assert!(
        sent_room_goto(&server, order).is_none(),
        "a refused locked-door click must not request a room change"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_on_a_door_sends_the_room_change_to_its_destination() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("25567");
    let room = palace_room::decode_payload(&payload, order).expect("room 25567 decodes");
    let door = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 1
                && hotspot.dest != 0
                && hotspot.script.as_deref().is_some_and(|script| {
                    script.contains("ON SELECT") && script.contains("DEST GOTOROOM")
                })
        })
        .expect("room 25567 has a door whose SELECT walks its DEST");
    assert_eq!(door.hotspot_type, 1, "HS_Door");
    assert_eq!(door.dest, 889, "the door's destination room");

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("door-gotoroom-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 25567)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 25567)
        .expect("a frame was composed for room 25567");
    assert!(
        sent_room_goto(&server, order).is_none(),
        "the client asked to change rooms before the door was clicked"
    );

    let (x, y) = hotspot_click(door, screen);
    handle.set_mouse(300, 200);
    handle.click(x, y);

    assert!(
        wait_for(
            || sent_room_goto(&server, order).is_some(),
            Duration::from_secs(10)
        ),
        "the clicked door's DEST GOTOROOM sent a room-change request"
    );
    assert_eq!(
        sent_room_goto(&server, order),
        Some(door.dest as u16),
        "the room-change request names the clicked door's destination"
    );

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains(&format!("script: GOTOROOM {}", door.dest)))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains(&format!("hit hotspot {}", door.id))),
        "the click landed on the door, not a neighbour: {:?}",
        notes(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains(&format!("script: GOTOROOM {}", door.dest))),
        "the door's script reported GOTOROOM to its destination: {:?}",
        notes(&events)
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("locked door")),
        "a plain door is never refused: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_click_on_a_door_with_no_handler_navigates_to_its_destination() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let door = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 1
                && hotspot.dest != 0
                && !hotspot
                    .script
                    .as_deref()
                    .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("room 901 has a door no script claims");
    assert_eq!(door.hotspot_type, 1, "HS_Door");
    assert_eq!(door.dest, 1100, "the door's destination room");
    assert!(
        door.script
            .as_deref()
            .is_some_and(|script| script.contains("ON LEAVE")),
        "the fixture door has only an ON LEAVE handler, so its click runs no select"
    );

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, room_desc_payload(&fixture))
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("door-auto-nav-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed for room 901");
    assert!(
        sent_room_goto(&server, order).is_none(),
        "the client asked to change rooms before the door was clicked"
    );

    let (x, y) = hotspot_click(door, screen);
    handle.set_mouse(300, 200);
    handle.click(x, y);

    assert!(
        wait_for(
            || sent_room_goto(&server, order) == Some(door.dest as u16),
            Duration::from_secs(10)
        ),
        "a door with no SELECT handler must send its destination: {:?}",
        server.received_frames(order)
    );

    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "LEAVE" && run.fired >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "LEAVE" && run.fired >= 1),
        "navigating away ran the room's ON LEAVE handlers: {:?}",
        script_runs(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_scripted_gotoroom_runs_on_leave_through_the_session_loop() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("25567");
    let room = palace_room::decode_payload(&payload, order).expect("room 25567 decodes");
    let door = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 1
                && hotspot.dest != 0
                && hotspot.script.as_deref().is_some_and(|script| {
                    script.contains("ON SELECT") && script.contains("DEST GOTOROOM")
                })
        })
        .expect("room 25567 has a door whose SELECT walks its DEST");

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("door-gotoroom-leave-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 25567)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 25567)
        .expect("a frame was composed for room 25567");

    let (x, y) = hotspot_click(door, screen);
    handle.set_mouse(300, 200);
    handle.click(x, y);

    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "LEAVE" && run.fired >= 1)
        },
        Duration::from_secs(15),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "LEAVE" && run.fired >= 1),
        "a scripted GOTOROOM ran the room's ON LEAVE handlers: {:?}",
        script_runs(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_door_with_a_select_handler_does_not_auto_navigate() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("25567");
    let room = palace_room::decode_payload(&payload, order).expect("room 25567 decodes");
    let door = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 1
                && hotspot.dest != 0
                && hotspot.script.as_deref().is_some_and(|script| {
                    script.contains("ON SELECT") && !script.contains("GOTOROOM")
                })
        })
        .expect("room 25567 has a door whose ON SELECT does not change rooms");
    assert_eq!(door.hotspot_type, 1, "HS_Door");
    assert_ne!(door.dest, 0, "the door has a destination to ignore");

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("door-select-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 25567)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 25567)
        .expect("a frame was composed for room 25567");

    let (x, y) = hotspot_click(door, screen);
    handle.set_mouse(300, 200);
    handle.click(x, y);

    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "the door's own ON SELECT ran: {:?}",
        notes(&events)
    );
    assert!(
        sent_room_goto(&server, order).is_none(),
        "a handler ran for the click, so the client must not also navigate"
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("is a door to room")),
        "no auto-navigation was reported: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_non_door_hotspot_with_a_nonzero_state_still_dispatches_select() {
    const MARKER: &str = "spot-state-processed";
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot.hotspot_type == 0
                && hotspot
                    .script
                    .as_deref()
                    .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("room 901 has an ordinary hotspot with an ON SELECT script");

    let frames = room_only_frames(&fixture);
    let tail = vec![
        spot_state_frame(order, 901, spot.id, 1),
        talk_marker_frame(order, MARKER),
    ];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("ordinary-state-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901)
        .expect("a frame was composed for room 901");
    let (x, y) = hotspot_click(spot, screen);

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).iter().any(|text| text.contains(MARKER)),
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(MARKER)),
        "the SPOTSTATE frame was processed before the click: {:?}",
        chats(&events)
    );

    handle.set_mouse(300, 200);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
                || notes(collected)
                    .iter()
                    .any(|text| text.contains("locked door"))
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "a HS_Normal hotspot with state 1 is script state, not a lock: {:?}",
        notes(&events)
    );
    assert!(
        !notes(&events)
            .iter()
            .any(|text| text.contains("locked door")),
        "an ordinary hotspot is never refused: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

// ---------------------------------------------------------------------------
// Walk, visibility and appearance commands
// ---------------------------------------------------------------------------

/// Serve the handshake, the room descriptor and the self user, then wait for the
/// composited room frame every one of these tests starts from.
fn start_room_with_self(
    fixture: &Fixture,
    tag: &str,
) -> (
    MockServer,
    ClientHandle,
    UnboundedReceiver<ClientEvent>,
    ScreenState,
    PathBuf,
    PathBuf,
) {
    let order = fixture.byte_order;
    let room = room_desc(fixture);
    let mut frames = room_only_frames(fixture);
    frames.push(self_user_frame(order));
    let server = MockServer::start(frames);
    let cache = unique_temp_dir(&format!("{tag}-cache"));
    let seed = seed_media_dir(&room);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.avatars >= 1)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901 && screen.avatars >= 1)
        .expect("a frame with the self user was composed for room 901")
        .clone();
    (server, handle, rx, screen, cache, seed)
}

/// The first frame the client sent with `wanted`'s opcode, decoded.
fn sent_message(
    server: &MockServer,
    order: ByteOrder,
    wanted: palace_wire::opcode::Opcode,
) -> Message {
    let frames = server.received_frames(order);
    let frame = frames
        .iter()
        .find(|frame| frame.opcode == wanted)
        .unwrap_or_else(|| panic!("the client sent {}", wanted.describe()));
    Message::decode(frame.opcode, frame.ref_num, &frame.payload, order)
        .unwrap_or_else(|error| panic!("{} decodes: {error}", wanted.describe()))
}

/// Every `USERMOVE` the client sent, decoded to `(x, y)` in send order.
fn sent_moves(server: &MockServer, order: ByteOrder) -> Vec<(i32, i32)> {
    server
        .received_frames(order)
        .iter()
        .filter(|frame| frame.opcode == opcode::USERMOVE)
        .map(|frame| {
            match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order)
                .expect("uLoc decodes")
            {
                Message::UserMove(mv) => (i32::from(mv.position.h), i32::from(mv.position.v)),
                other => panic!("expected UserMove, got {other:?}"),
            }
        })
        .collect()
}

#[test]
fn a_floor_click_walks_while_a_hotspot_click_still_selects_and_does_not() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let (server, handle, mut rx, screen, cache, seed) = start_room_with_self(&fixture, "walk");

    // A room point far outside every hotspot is the floor: the click walks.
    let empty = screen
        .geometry
        .transform()
        .room_to_viewport(palace_render::PointF::new(30_000.0, 30_000.0));
    handle.click(empty.x, empty.y);

    assert!(
        wait_for(
            || server
                .received_frames(order)
                .iter()
                .any(|frame| frame.opcode == opcode::USERMOVE),
            Duration::from_secs(5)
        ),
        "a bare floor click sent MSG_USERMOVE"
    );
    let expected = (
        screen.geometry.room_w as i32 - 22,
        screen.geometry.room_h as i32 - 22,
    );
    let frames = server.received_frames(order);
    let moved = frames
        .iter()
        .find(|frame| frame.opcode == opcode::USERMOVE)
        .expect("a usermove frame");
    assert_eq!(moved.ref_num, SELF_ID, "refNum names the moving user");
    match Message::decode(moved.opcode, moved.ref_num, &moved.payload, order).expect("uLoc decodes")
    {
        Message::UserMove(mv) => assert_eq!(
            (i32::from(mv.position.h), i32::from(mv.position.v)),
            expected,
            "the target is clamped to the avatar margin, like the renderer"
        ),
        other => panic!("expected UserMove, got {other:?}"),
    }

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("walking to room"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("hit no hotspot")),
        "the walk still reports the click hit no hotspot: {:?}",
        notes(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("walking to room")),
        "and reports the move: {:?}",
        notes(&events)
    );

    // A hotspot click keeps its old path and must not also walk.
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| {
            hotspot
                .script
                .as_deref()
                .is_some_and(|script| script.contains("ON SELECT"))
        })
        .expect("the room has a scripted hotspot");
    let before = server
        .received_frames(order)
        .iter()
        .filter(|frame| frame.opcode == opcode::USERMOVE)
        .count();
    let (x, y) = hotspot_click(spot, &screen);
    handle.click(x, y);
    let events = collect_events(
        &mut rx,
        |collected| {
            script_runs(collected)
                .iter()
                .any(|run| run.event == "SELECT" && run.fired >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        script_runs(&events)
            .iter()
            .any(|run| run.event == "SELECT" && run.fired >= 1),
        "the hotspot still dispatches SELECT: {:?}",
        notes(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("hit hotspot")),
        "the hotspot click is still reported as a hit: {:?}",
        notes(&events)
    );
    let after = server
        .received_frames(order)
        .iter()
        .filter(|frame| frame.opcode == opcode::USERMOVE)
        .count();
    assert_eq!(before, after, "a hotspot click must not also walk");

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_setpos_script_moves_the_local_user_to_the_clamped_position() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, screen, cache, seed) = start_room_with_self(&fixture, "setpos");
    let expected = (screen.geometry.room_w as i32 - 22, 22);
    let initial_version = screen.version;

    handle.run_script("9000 -50 SETPOS");
    assert!(
        wait_for(
            || !sent_moves(&server, order).is_empty(),
            Duration::from_secs(5)
        ),
        "a SETPOS script sent MSG_USERMOVE"
    );
    assert_eq!(
        sent_moves(&server, order),
        vec![expected],
        "SETPOS is clamped to the room, exactly as the frame is"
    );

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.version > initial_version)
        },
        Duration::from_secs(5),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.version > initial_version),
        "the move forced a re-render: {:?}",
        screens(&events)
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains(&format!("SETPOS to ({},{})", expected.0, expected.1))),
        "the note states the position come to rest at: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_move_script_moves_the_local_user_relative_to_where_they_stand() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, _screen, cache, seed) = start_room_with_self(&fixture, "move-rel");

    handle.run_script("100 100 SETPOS");
    assert!(
        wait_for(
            || sent_moves(&server, order).len() == 1,
            Duration::from_secs(5)
        ),
        "the SETPOS that fixes the origin landed"
    );
    assert_eq!(sent_moves(&server, order), vec![(100, 100)]);

    handle.run_script("10 20 MOVE");
    assert!(
        wait_for(
            || sent_moves(&server, order).len() == 2,
            Duration::from_secs(5)
        ),
        "the MOVE landed"
    );
    assert_eq!(
        sent_moves(&server, order),
        vec![(100, 100), (110, 120)],
        "MOVE is relative to the position we stand at, not the room origin"
    );

    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("MOVE (10,20) to (110,120)"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("MOVE (10,20) to (110,120)")),
        "the note names the delta and the rest position: {:?}",
        notes(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn the_local_position_after_a_setpos_is_the_position_in_the_sent_frame() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, _rx, _screen, cache, seed) = start_room_with_self(&fixture, "move-local");

    handle.run_script("100 200 SETPOS");
    assert!(
        wait_for(
            || sent_moves(&server, order).len() == 1,
            Duration::from_secs(5)
        ),
        "the SETPOS landed"
    );
    let setpos = sent_moves(&server, order)[0];
    assert_eq!(setpos, (100, 200));

    // A relative move is computed from the local model. If SETPOS had not been
    // applied locally, this frame would be relative to the old position.
    handle.run_script("10 20 MOVE");
    assert!(
        wait_for(
            || sent_moves(&server, order).len() == 2,
            Duration::from_secs(5)
        ),
        "the MOVE landed"
    );
    let moves = sent_moves(&server, order);
    assert_eq!(
        moves[1],
        (setpos.0 + 10, setpos.1 + 20),
        "the local model took the SETPOS position, so the next relative move starts there"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn hiding_avatars_empties_the_scene_and_showing_restores_it() {
    let fixture = logon_fixture();
    let (server, handle, mut rx, screen, cache, seed) =
        start_room_with_self(&fixture, "visibility");
    assert!(
        screen.avatars >= 1,
        "the self user is drawn before anything is hidden"
    );

    handle.set_visibility(true, false);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.avatars == 0)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.avatars == 0),
        "hiding avatars leaves none in the composed scene: {:?}",
        screens(&events)
    );

    handle.set_visibility(true, true);
    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.avatars >= 1)
        },
        Duration::from_secs(10),
    );
    assert!(
        screens(&events)
            .iter()
            .any(|screen| screen.room_id == 901 && screen.avatars >= 1),
        "showing avatars restores them: {:?}",
        screens(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn set_avatar_sends_the_face_and_colour_frames_normalised() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, _rx, screen, cache, seed) = start_room_with_self(&fixture, "set-avatar");

    handle.set_avatar(2, 3);
    assert!(
        wait_for(
            || {
                let frames = server.received_frames(order);
                frames.iter().any(|frame| frame.opcode == opcode::USERFACE)
                    && frames.iter().any(|frame| frame.opcode == opcode::USERCOLOR)
            },
            Duration::from_secs(5)
        ),
        "set_avatar sent both MSG_USERFACE and MSG_USERCOLOR"
    );
    match sent_message(&server, order, opcode::USERFACE) {
        Message::UserFace(face) => {
            assert_eq!(face.face_nbr, 2);
            assert_eq!(face.user_id, SELF_ID);
        }
        other => panic!("expected UserFace, got {other:?}"),
    }
    match sent_message(&server, order, opcode::USERCOLOR) {
        Message::UserColor(color) => {
            assert_eq!(color.color_nbr, 3);
            assert_eq!(color.user_id, SELF_ID);
        }
        other => panic!("expected UserColor, got {other:?}"),
    }

    // Values past the sheet are normalised before they reach the wire: face 99
    // wraps to 0, colour 99 clamps to the last colour.
    handle.set_avatar(99, 99);
    assert!(
        wait_for(
            || {
                let frames = server.received_frames(order);
                let face = frames
                    .iter()
                    .filter(|frame| frame.opcode == opcode::USERFACE)
                    .count();
                let color = frames
                    .iter()
                    .filter(|frame| frame.opcode == opcode::USERCOLOR)
                    .count();
                face >= 2 && color >= 2
            },
            Duration::from_secs(5)
        ),
        "the clamped request also sent both frames"
    );
    let frames = server.received_frames(order);
    let last_face = frames
        .iter()
        .rfind(|frame| frame.opcode == opcode::USERFACE)
        .expect("a face frame");
    match Message::decode(
        last_face.opcode,
        last_face.ref_num,
        &last_face.payload,
        order,
    )
    .expect("usrF decodes")
    {
        Message::UserFace(face) => assert_eq!(face.face_nbr, 0, "face 99 wraps to face 0"),
        other => panic!("expected UserFace, got {other:?}"),
    }
    let last_color = frames
        .iter()
        .rfind(|frame| frame.opcode == opcode::USERCOLOR)
        .expect("a colour frame");
    match Message::decode(
        last_color.opcode,
        last_color.ref_num,
        &last_color.payload,
        order,
    )
    .expect("usrC decodes")
    {
        Message::UserColor(color) => {
            assert_eq!(color.color_nbr, 15, "colour 99 clamps to the last colour")
        }
        other => panic!("expected UserColor, got {other:?}"),
    }

    assert!(
        handle.frames().version() > screen.version,
        "the change re-rendered the frame without waiting for a server echo"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

/// Every `USERPROP` the client sent, decoded in send order.
fn sent_user_props(server: &MockServer, order: ByteOrder) -> Vec<UserProp> {
    server
        .received_frames(order)
        .iter()
        .filter(|frame| frame.opcode == opcode::USERPROP)
        .map(|frame| {
            match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order)
                .expect("usrP decodes")
            {
                Message::UserProp(prop) => prop,
                other => panic!("expected UserProp, got {other:?}"),
            }
        })
        .collect()
}

/// The asset ids of a decoded `USERPROP`, in order.
fn ids_of(prop: &UserProp) -> Vec<i32> {
    prop.props.iter().map(|spec| spec.id).collect()
}

#[test]
fn set_props_sends_one_userprop_naming_us_and_the_worn_ids() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, _rx, initial, cache, seed) =
        start_room_with_self(&fixture, "set-props-send");

    handle.set_props(vec![10, 20, 30]);
    assert!(
        wait_for(
            || !sent_user_props(&server, order).is_empty(),
            Duration::from_secs(5)
        ),
        "set_props reached the server as a usrP frame"
    );

    let sent = sent_user_props(&server, order);
    assert_eq!(sent.len(), 1, "exactly one USERPROP frame, not two");
    assert_eq!(sent[0].user_id, SELF_ID, "refNum is our own user id");
    let ids = ids_of(&sent[0]);
    assert_eq!(
        ids,
        vec![10, 20, 30],
        "the body carries the worn ids in order"
    );
    assert!(
        sent[0].props.iter().all(|spec| spec.crc == 0),
        "the reference sends crc 0 for the art it has not uploaded"
    );

    assert!(
        wait_for(
            || handle.frames().version() > initial.version,
            Duration::from_secs(5)
        ),
        "the change re-rendered the frame without waiting for a server echo"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_script_hasprop_reads_the_locally_worn_list() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, _initial, cache, seed) =
        start_room_with_self(&fixture, "hasprop-script");

    handle.set_props(vec![10, 20, 30]);
    assert!(
        wait_for(
            || !sent_user_props(&server, order).is_empty(),
            Duration::from_secs(5)
        ),
        "the worn list reached the server first"
    );

    // The branch body only runs when HASPROP reads the worn id as worn.
    handle.run_script("{ \"worn-10\" SAY } 10 HASPROP IF");
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).contains(&"worn-10"),
        Duration::from_secs(5),
    );
    assert!(
        chats(&events).contains(&"worn-10"),
        "HASPROP saw the worn prop through the script engine: {:?}",
        chats(&events)
    );

    handle.run_script("{ \"worn-99\" SAY } 99 HASPROP IF");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("from the input box"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("from the input box")),
        "the negative script ran: {:?}",
        notes(&events)
    );
    assert!(
        !chats(&events).contains(&"worn-99"),
        "HASPROP is false for an id we do not wear: {:?}",
        chats(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn an_alarm_hasprop_reads_the_worn_list_the_last_event_left() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("25567");
    let room = palace_room::decode_payload(&payload, order).expect("room 25567 decodes");
    let spot = room
        .hotspots
        .iter()
        .find(|hotspot| hotspot.hotspot_type != 1 || hotspot.dest == 0)
        .expect("room 25567 has a hotspot that does not change rooms");
    let spot_id = i32::from(spot.id);

    let mut frames = vec![server_bytes(&fixture)[0].clone()];
    frames.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("hasprop-alarm-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 25567)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 25567)
        .expect("a frame was composed for room 25567")
        .clone();

    handle.run_script(format!(
        "{{ [ 10 ] SETPROPS {{ {{ \"alarm-worn-10\" SAY }} 10 HASPROP IF }} 3 ALARMEXEC }} \
         \"SELECT\" {spot_id} SETSPOTSCRIPT"
    ));
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("SETSPOTSCRIPT"))
        },
        Duration::from_secs(10),
    );
    assert!(
        notes(&events)
            .iter()
            .any(|text| text.contains("SETSPOTSCRIPT")),
        "the SELECT handler attached: {:?}",
        notes(&events)
    );

    let (vx, vy) = hotspot_click(spot, &screen);
    handle.click(vx, vy);
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).contains(&"alarm-worn-10"),
        Duration::from_secs(6),
    );
    assert!(
        chats(&events).contains(&"alarm-worn-10"),
        "the alarm's HASPROP read the worn list the click event left: {:?}",
        chats(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn setting_the_same_worn_list_twice_sends_one_userprop() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, _rx, _initial, cache, seed) =
        start_room_with_self(&fixture, "set-props-idempotent");

    handle.set_props(vec![10, 20]);
    assert!(
        wait_for(
            || sent_user_props(&server, order).len() == 1,
            Duration::from_secs(5)
        ),
        "the first set sent a frame"
    );

    // The commands are processed in order, so the third set's frame can only
    // land after the middle, unchanged set has been handled. Waiting for that
    // last frame (not a bare count) makes the check free of sleeps.
    handle.set_props(vec![10, 20]);
    handle.set_props(vec![10, 20, 30]);
    assert!(
        wait_for(
            || {
                sent_user_props(&server, order)
                    .last()
                    .is_some_and(|prop| ids_of(prop) == vec![10, 20, 30])
            },
            Duration::from_secs(5)
        ),
        "the changed set sent the second frame"
    );
    assert_eq!(
        sent_user_props(&server, order).len(),
        2,
        "an unchanged set is not re-sent: the frame would carry no new information"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_worn_list_longer_than_nine_is_clamped_and_reported() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let (server, handle, mut rx, _initial, cache, seed) =
        start_room_with_self(&fixture, "set-props-cap");

    handle.set_props((1..=10).collect());
    assert!(
        wait_for(
            || !sent_user_props(&server, order).is_empty(),
            Duration::from_secs(5)
        ),
        "the capped list reached the server"
    );
    let sent = sent_user_props(&server, order);
    assert_eq!(sent.len(), 1);
    let ids = ids_of(&sent[0]);
    assert_eq!(
        ids,
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
        "only the first nine ids are worn; the tenth never reaches the wire"
    );

    // The clamp is not silent: the runtime says what it ignored.
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("ignored 1"))
        },
        Duration::from_secs(5),
    );
    assert!(
        notes(&events).iter().any(|text| text.contains("ignored 1")),
        "the clamped id was reported: {:?}",
        notes(&events)
    );

    // The model is capped too: the ninth is worn, the tenth is not.
    handle.run_script("{ \"nine\" SAY } 9 HASPROP IF");
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).contains(&"nine"),
        Duration::from_secs(5),
    );
    assert!(
        chats(&events).contains(&"nine"),
        "the ninth prop is worn: {:?}",
        chats(&events)
    );
    handle.run_script("{ \"ten\" SAY } 10 HASPROP IF");
    let events = collect_events(
        &mut rx,
        |collected| {
            notes(collected)
                .iter()
                .any(|text| text.contains("from the input box"))
        },
        Duration::from_secs(5),
    );
    assert!(
        !chats(&events).contains(&"ten"),
        "the tenth prop is not worn: {:?}",
        chats(&events)
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

/// The name a frame draws above an avatar is the name the wire carried in the
/// `UserRec`, and it is rasterized for the *specific* string on the spec — not
/// just "some tag". The same test checks that the prop filter still wins: a user
/// whose prop art never arrives is skipped, so they must get no tag at all.
///
/// This is the guard that failed while `avatar_specs` left `AvatarSpec::name`
/// `None`: every tag was silently suppressed and the whole suite stayed green.
#[test]
fn a_named_user_reaches_the_frame_as_a_name_tag() {
    const NAME: &str = "Alpha";
    const GHOST: &str = "Ghost";
    const OTHER_ID: i32 = 77;
    const GHOST_ID: i32 = 78;
    const MISSING_PROP: u32 = 4_242;

    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(user_new_frame(
        order,
        OTHER_ID,
        NAME,
        3,
        5,
        &[],
        Point::new(200, 240),
    ));
    frames.push(user_new_frame(
        order,
        GHOST_ID,
        GHOST,
        3,
        5,
        &[MISSING_PROP],
        Point::new(100, 100),
    ));
    let server = MockServer::start(frames);
    let cache = unique_temp_dir("name-tag-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    // The named user is drawn; the one wearing an unseeded prop is skipped and
    // counted. Wait for the frame that reports exactly that.
    fn drawn_and_skipped(screen: &&ScreenState) -> bool {
        screen.room_id == 901
            && screen.avatars == 1
            && screen
                .notes
                .iter()
                .any(|note| note.contains("prop art not received"))
    }
    let events = collect_events(
        &mut rx,
        |collected| screens(collected).iter().any(drawn_and_skipped),
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(drawn_and_skipped)
        .expect("the named user was drawn and the prop-less one skipped")
        .clone();
    assert_eq!(
        screen.avatars, 1,
        "the prop filter still skips the user whose art has not arrived"
    );

    // A settled frame with names visible, then the same frame with names off.
    // Avatars stay on, so the only difference is the tags.
    let visible_version = settled_frame_version(&handle, Duration::from_secs(5));
    let with_names = frame_rgba(&handle.frames().png().expect("a composed frame"));
    handle.set_visibility(false, true);
    assert!(
        wait_for(
            || handle.frames().version() > visible_version,
            Duration::from_secs(5)
        ),
        "toggling names re-rendered the frame"
    );
    settled_frame_version(&handle, Duration::from_secs(5));
    let without_names = frame_rgba(&handle.frames().png().expect("a composed frame"));

    let width = screen.geometry.bitmap_w as usize;
    let height = screen.geometry.bitmap_h as i64;
    let room_w = screen.geometry.room_w as i32;
    let room_h = screen.geometry.room_h as i32;
    // The tag blit origin, exactly as `draw_name_tag` computes it.
    let tag_origin = |name: &str, anchor: (i32, i32)| {
        let tag = palace_render::name_tag(name).expect("the name rasterizes");
        let (text_x, text_y) = palace_render::name_tag_position(anchor.0, anchor.1, tag.text_width);
        let (origin_x, origin_y) = (tag.origin_x, tag.origin_y);
        (
            tag,
            (text_x - f64::from(origin_x)).floor() as i64,
            (text_y - f64::from(origin_y)).floor() as i64,
        )
    };
    // The wire name's fully opaque glyph pixels must be pure white exactly at the
    // reference placement. A `None` name draws nothing, so this assertion is what
    // catches the regression.
    let alpha = palace_render::clamp_avatar_position(240, 200, room_w, room_h);
    let (tag, blit_x, blit_y) = tag_origin(NAME, alpha);
    let mut opaque = 0usize;
    let mut changed = 0usize;
    for sy in 0..tag.image.height() {
        for sx in 0..tag.image.width() {
            let Some(pixel) = tag.image.pixel(sx, sy) else {
                continue;
            };
            if pixel[3] != 255 {
                continue;
            }
            let dx = blit_x + i64::from(sx);
            let dy = blit_y + i64::from(sy);
            if dx < 0 || dy < 0 || dx >= width as i64 || dy >= height {
                continue;
            }
            let at = (dy as usize * width + dx as usize) * 4;
            opaque += 1;
            assert_eq!(
                with_names[at..at + 4],
                pixel,
                "the tag for {NAME} must be drawn at ({dx},{dy})"
            );
            if with_names[at..at + 4] != without_names[at..at + 4] {
                changed += 1;
            }
        }
    }
    assert!(
        opaque > 0,
        "the name {NAME} has fully opaque glyph pixels to find"
    );
    assert!(changed > 0, "the tag is drawn only while names are visible");

    // The prop filter wins over the name: the skipped user's glyph pixels must
    // stay un-drawn, proving the two rules do not contradict each other.
    let ghost_anchor = palace_render::clamp_avatar_position(100, 100, room_w, room_h);
    let (ghost_tag, ghost_blit_x, ghost_blit_y) = tag_origin(GHOST, ghost_anchor);
    let mut ghost_opaque = 0usize;
    let mut ghost_matched = 0usize;
    for sy in 0..ghost_tag.image.height() {
        for sx in 0..ghost_tag.image.width() {
            let Some(pixel) = ghost_tag.image.pixel(sx, sy) else {
                continue;
            };
            if pixel[3] != 255 {
                continue;
            }
            let dx = ghost_blit_x + i64::from(sx);
            let dy = ghost_blit_y + i64::from(sy);
            if dx < 0 || dy < 0 || dx >= width as i64 || dy >= height {
                continue;
            }
            ghost_opaque += 1;
            let at = (dy as usize * width + dx as usize) * 4;
            if with_names[at..at + 4] == pixel {
                ghost_matched += 1;
            }
        }
    }
    assert!(
        ghost_opaque > 0,
        "the skipped name has opaque pixels to look for"
    );
    assert_eq!(
        ghost_matched, 0,
        "a user whose prop art never arrived must not be tagged"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_chat_message_reaches_the_frame_as_chat_text() {
    const SPEAKER: &str = "Alpha";
    const SPEAKER_ID: i32 = 77;
    const MESSAGE: &str = "hello there";

    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let mut frames = room_only_frames(&fixture);
    frames.push(user_new_frame(
        order,
        SPEAKER_ID,
        SPEAKER,
        3,
        5,
        &[],
        Point::new(200, 240),
    ));
    // The chat line is held back until a settled, chat-free frame exists.
    let tail = vec![talk_from_frame(order, SPEAKER_ID, MESSAGE)];
    let (server, gate) = MockServer::start_gated(frames, tail);
    let cache = unique_temp_dir("chat-text-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 901 && screen.avatars == 1)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 901 && screen.avatars == 1)
        .expect("the speaker was drawn")
        .clone();

    let before_version = settled_frame_version(&handle, Duration::from_secs(5));
    let before = frame_rgba(&handle.frames().png().expect("a composed frame"));

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).contains(&MESSAGE),
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).contains(&MESSAGE),
        "the wire chat line reached the transcript: {:?}",
        chats(&events)
    );
    assert!(
        wait_for(
            || handle.frames().version() > before_version,
            Duration::from_secs(5)
        ),
        "the chat line recomposed the frame"
    );
    settled_frame_version(&handle, Duration::from_secs(5));
    let after = frame_rgba(&handle.frames().png().expect("a composed frame"));
    assert_ne!(before, after, "the chat text changed the composed frame");

    // The chat text must appear at exactly the reference placement: the
    // speaker's clamped avatar anchor, the 20 px gap, and the rasterized image.
    let width = screen.geometry.bitmap_w as usize;
    let height = screen.geometry.bitmap_h as i64;
    let anchor = palace_render::clamp_avatar_position(
        240,
        200,
        screen.geometry.room_w as i32,
        screen.geometry.room_h as i32,
    );
    let item = palace_render::ChatText {
        text: MESSAGE.to_string(),
        x: anchor.0,
        y: anchor.1,
        style: palace_render::ChatStyle::Talk,
    };
    let render = palace_render::chat_text(&item).expect("the message rasterizes");
    let (text_x, text_y) = palace_render::chat_position(
        item.x,
        item.y,
        render.text_width,
        render.text_height,
        screen.geometry.room_w,
        screen.geometry.room_h,
    );
    let blit_x = (text_x - f64::from(render.origin_x)).floor() as i64;
    let blit_y = (text_y - f64::from(render.origin_y)).floor() as i64;

    let mut opaque = 0usize;
    for sy in 0..render.image.height() {
        for sx in 0..render.image.width() {
            let Some(pixel) = render.image.pixel(sx, sy) else {
                continue;
            };
            if pixel[3] != 255 {
                continue;
            }
            let dx = blit_x + i64::from(sx);
            let dy = blit_y + i64::from(sy);
            if dx < 0 || dy < 0 || dx >= width as i64 || dy >= height {
                continue;
            }
            let at = (dy as usize * width + dx as usize) * 4;
            opaque += 1;
            assert_eq!(
                after[at..at + 4],
                pixel,
                "the chat text for {MESSAGE} must be drawn at ({dx},{dy})"
            );
        }
    }
    assert!(
        opaque > 0,
        "the message has fully opaque glyph pixels to find"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

#[test]
fn a_spot_move_for_this_room_recomposes_and_a_foreign_one_does_not() {
    const FOREIGN: &str = "foreign-spot-move-processed";
    const LOCAL: &str = "local-spot-move-processed";
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let payload = room_payload("887");
    let room = palace_room::decode_payload(&payload, order).expect("room 887 decodes");
    let spot_id = room.hotspots[0].id;

    let mut initial = vec![server_bytes(&fixture)[0].clone()];
    initial.push(
        Frame::new(opcode::ROOMDESC, 0, payload)
            .encode(order)
            .expect("the room descriptor encodes"),
    );
    let tail = vec![
        spot_move_frame(order, 999, spot_id, Point::new(1, 1)),
        talk_marker_frame(order, FOREIGN),
        spot_move_frame(order, 887, spot_id, Point::new(240, 120)),
        talk_marker_frame(order, LOCAL),
    ];
    let (server, gate) = MockServer::start_gated(initial, tail);
    let cache = unique_temp_dir("spot-move-cache");
    let seed = seed_solid_media_dir(&room, BRIGHT);
    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    let events = collect_events(
        &mut rx,
        |collected| {
            screens(collected)
                .iter()
                .any(|screen| screen.room_id == 887)
        },
        Duration::from_secs(20),
    );
    let screen = screens(&events)
        .into_iter()
        .find(|screen| screen.room_id == 887)
        .expect("a frame was composed for room 887");
    wait_for_entry_alarm(&handle, screen);
    let settled = settled_frame_version(&handle, Duration::from_secs(5));

    gate.store(true, Ordering::Relaxed);
    let events = collect_events(
        &mut rx,
        |collected| chats(collected).iter().any(|text| text.contains(FOREIGN)),
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(FOREIGN)),
        "the foreign spot move was processed before its marker: {:?}",
        chats(&events)
    );
    assert_eq!(
        handle.frames().version(),
        settled,
        "a move aimed at room 999 must not recompose this room"
    );

    let events = collect_events(
        &mut rx,
        |collected| chats(collected).iter().any(|text| text.contains(LOCAL)),
        Duration::from_secs(10),
    );
    assert!(
        chats(&events).iter().any(|text| text.contains(LOCAL)),
        "the in-room spot move was processed before its marker: {:?}",
        chats(&events)
    );
    assert!(
        handle.frames().version() > settled,
        "the move aimed at room 887 recomposed the frame"
    );

    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);
}

// ---------------------------------------------------------------------------
// The opt-in trace
// ---------------------------------------------------------------------------

/// The tracer is process-wide, so the tests that install one must not overlap.
static TRACE_LOCK: Mutex<()> = Mutex::new(());

fn trace_guard() -> std::sync::MutexGuard<'static, ()> {
    TRACE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn trace_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn a_live_session_records_frames_scripts_effects_and_events_in_the_trace() {
    let _guard = trace_guard();
    let fixture = logon_fixture();
    let order = fixture.byte_order;
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("trace-cache");
    let seed = seed_media_dir(&room);
    let probe = unique_temp_dir("trace-out");
    let trace_path = probe.join("session.log");

    trace::install(Some(Arc::new(
        Tracer::to_path(&trace_path).expect("the trace file opens"),
    )));

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();

    // A room must arrive first so the lifecycle scripts have fired.
    let events = collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty(),
        Duration::from_secs(20),
    );
    assert!(
        !screens(&events).is_empty(),
        "the session composed a frame before the trace was read"
    );
    assert!(
        wait_for(
            || trace_text(&trace_path).contains("room_arrived room=901"),
            Duration::from_secs(5)
        ),
        "the room arrival reached the trace"
    );

    // A navigation request, a worn-prop change and a chat line give the trace
    // one of each sent frame the arena diagnosis needs.
    handle.goto_room(903);
    assert!(
        wait_for(
            || {
                server
                    .received_frames(order)
                    .iter()
                    .any(|frame| frame.opcode == opcode::ROOMGOTO)
                    && trace_text(&trace_path).contains("room=903")
            },
            Duration::from_secs(5)
        ),
        "the navR request and its destination were traced"
    );
    handle.set_props(vec![99]);
    handle.say("trace probe");
    assert!(
        wait_for(
            || {
                let sent = server.received_frames(order);
                sent.iter().any(|frame| frame.opcode == opcode::USERPROP)
                    && sent.iter().any(|frame| frame.opcode == opcode::TALK)
                    && trace_text(&trace_path).contains("worn_props user=13 ids=[99]")
            },
            Duration::from_secs(5)
        ),
        "the prop change and chat were traced"
    );
    let _ = collect_events(&mut rx, |_| false, Duration::from_millis(300));

    handle.disconnect();
    drop(server);
    let text = trace_text(&trace_path);
    trace::install(None);
    cleanup(&cache);
    cleanup(&seed);
    cleanup(&probe);

    assert!(
        text.contains("opcode=tiyr(TIYID)") && text.contains("recv "),
        "every received frame is traced with its opcode: {text}"
    );
    assert!(
        text.contains("opcode=regi(LOGON)") && text.contains("send "),
        "every sent frame is traced with its opcode: {text}"
    );
    assert!(
        text.contains("opcode=navR(ROOMGOTO)") && text.contains("room=903"),
        "the room-change request names its destination: {text}"
    );
    assert!(
        text.contains("worn_props user=13 ids=[99]"),
        "the worn-prop change is recorded: {text}"
    );
    assert!(
        text.contains("script event=") && text.contains("fired="),
        "script dispatches with their handler counts are recorded: {text}"
    );
    assert!(
        text.contains("[room] #901"),
        "the emitted RoomEntered event is recorded as the harness renders it: {text}"
    );
    assert!(
        text.contains("event  [note]") || text.contains("event [note]"),
        "the emitted notes are recorded: {text}"
    );
    assert!(
        text.contains("room_arrived room=901") && text.contains("nav_request room=903"),
        "the state transitions are recorded: {text}"
    );
}

#[test]
fn a_trace_write_failure_does_not_break_the_session() {
    let _guard = trace_guard();
    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("trace-fail-cache");
    let seed = seed_media_dir(&room);

    // `/dev/full` accepts the open and fails every write — the "sink went away
    // mid-session" case. On a platform without it, a path that cannot be opened
    // proves the same degradation.
    let full = Path::new("/dev/full");
    let failing = full
        .exists()
        .then(|| Tracer::to_path(full).expect("opening /dev/full succeeds"));
    if let Some(failing) = failing {
        trace::install(Some(Arc::new(failing)));
    } else {
        let missing = unique_temp_dir("trace-missing").join("no/such/dir/log");
        assert!(
            Tracer::to_path(&missing).is_err(),
            "an unopenable target is an error, not a panic"
        );
        trace::install(None);
    }

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();
    let events = collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty(),
        Duration::from_secs(20),
    );
    handle.disconnect();
    drop(server);
    trace::install(None);
    cleanup(&cache);
    cleanup(&seed);

    assert!(
        !screens(&events).is_empty(),
        "the session keeps producing frames while the trace sink fails"
    );
    assert!(
        !trace::enabled(),
        "the failing tracer did not latch the gate on"
    );
}

#[test]
fn with_tracing_unset_the_session_runs_and_creates_no_trace_file() {
    let _guard = trace_guard();
    trace::install(None);
    assert!(!trace::enabled(), "tracing starts off");
    assert!(trace::tracer().is_none(), "no tracer is installed");

    let fixture = logon_fixture();
    let room = room_desc(&fixture);
    let server = MockServer::start(server_bytes(&fixture));
    let cache = unique_temp_dir("trace-unset-cache");
    let seed = seed_media_dir(&room);

    let (handle, stream) =
        ClientRuntime::spawn(config_for(server.port, cache.clone(), seed.clone()));
    let mut rx = stream.into_receiver();
    let events = collect_events(
        &mut rx,
        |collected| !screens(collected).is_empty(),
        Duration::from_secs(20),
    );
    handle.disconnect();
    drop(server);
    cleanup(&cache);
    cleanup(&seed);

    assert!(
        !screens(&events).is_empty(),
        "the session behaves exactly as before with tracing unset"
    );
    assert!(
        !trace::enabled() && trace::tracer().is_none(),
        "nothing turned tracing on during the session"
    );
}

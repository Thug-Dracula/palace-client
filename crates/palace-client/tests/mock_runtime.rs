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
use palace_client::{
    ChatKind, ClientConfig, ClientEvent, ClientHandle, ConnectionStatus, RoomInfo, ScreenState,
    ServerBanner, UserInfo,
};
use palace_wire::byteorder::{ByteOrder, Writer};
use palace_wire::fixture::{default_fixture_dir, Fixture};
use palace_wire::frame::Frame;
use palace_wire::messages::{reference_logon_record, AssetSpec, Message, Point, UserRec};
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
        while !gate.load(Ordering::Relaxed) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
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
        notes(&events)
            .iter()
            .any(|text| text.contains("SOUND garden")),
        "the effect was applied and reported: {:?}",
        notes(&events)
    );

    // The real bytes of the client's reply went out over the socket: the logon
    // the runtime built for the configured user matches the reference record
    // byte for byte, and matches the captured client frame.
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
    let expected = reference_logon_record("RustProbe", 0).logon_frame(order);
    assert_eq!(
        logon.payload, expected.payload,
        "the configured user's logon"
    );
    let captured_logon = fixture
        .client_frames()
        .find(|captured| captured.frame.opcode == opcode::LOGON)
        .expect("the capture holds the real client's logon");
    assert_eq!(
        logon.payload, captured_logon.frame.payload,
        "the runtime sent the same logon the real client did"
    );
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

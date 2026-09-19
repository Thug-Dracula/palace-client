//! Opt-in, file-based trace of one live session.
//!
//! The runtime is otherwise silent: the only visibility into live behaviour is
//! the UI notes area and the separate `live-smoke` harness, neither of which can
//! reproduce a user's actual interactions. This module records what the runtime
//! actually does — the frames that arrive, the frames we send, the script events
//! that fire (with the effects they produced), every [`ClientEvent`] emitted, and
//! the state transitions that matter — so a live bug can be read back instead of
//! guessed at.
//!
//! # Turning it on
//!
//! Set `PALACE_TRACE` in the launcher's environment before starting the client:
//!
//! ```text
//! PALACE_TRACE=/tmp/palace-trace.log   palace-client     # append to a file
//! PALACE_TRACE=1                       palace-client     # write to stderr
//! PALACE_TRACE=stderr                  palace-client     # write to stderr
//! ```
//!
//! With the variable unset the whole facility is inert: no file is created, no
//! output is produced, and every hook costs one atomic load. The instrumented
//! programs (`live-smoke` included) honour the variable because
//! [`crate::ClientRuntime::spawn`] calls [`start_from_env`].
//!
//! # Tags
//!
//! Every line starts with an RFC 3339 UTC timestamp, the milliseconds since the
//! trace opened, and one of five tags:
//!
//! | Tag | Meaning |
//! |-----|---------|
//! | `recv` | A frame received: opcode, `ref`, payload length, and the decoded message where one exists. |
//! | `send` | A frame sent: same shape, so the request/reply pair is visible. |
//! | `script` | A script event dispatched: handler name, spot scope, how many handlers fired, the effects, and any faults. |
//! | `event` | A [`ClientEvent`] emitted, rendered exactly as the `live-smoke` harness prints it. |
//! | `state` | A transition that matters: room arrival/leave, a navigation request, a worn-prop change. |
//!
//! # Guarantees
//!
//! * **Never panics, never propagates an I/O error.** If the file cannot be
//!   opened the facility degrades to a no-op; if a write fails it latches off and
//!   notes the failure once on stderr. Tracing can never break a session.
//! * **Thread-safe.** One [`Mutex`] guards the sink; the runtime may log from the
//!   network thread and the media thread.
//! * **Bounded.** A file sink is capped at [`MAX_BYTES`]; when it fills it is
//!   rotated to `<path>.1`, older archives are shifted down, and only
//!   [`MAX_ARCHIVES`] are kept. stderr has no cap because it is not ours to
//!   rotate.
//! * **Readable while running.** Every line is flushed as it is written.
//! * **Behaviour-neutral.** No control flow reads the tracing state.
//!
//! Only the standard library is used.
//!
//! [`ClientEvent`]: crate::runtime::ClientEvent

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use palace_host::{DispatchReport, Effect};
use palace_room::RoomDesc;
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;
use palace_wire::messages::{AssetSpec, Message};
use palace_wire::opcode;

use crate::runtime::ClientEvent;

/// The environment variable that turns tracing on.
pub const ENV_VAR: &str = "PALACE_TRACE";

/// The environment variable that turns the hotspot-script dump on.
pub const DUMP_SCRIPTS_ENV_VAR: &str = "PALACE_DUMP_SCRIPTS";

/// The environment variable that turns the script variable-store dump on.
pub const DUMP_VARS_ENV_VAR: &str = "PALACE_DUMP_VARS";

/// Longest single line the tracer will write before truncating. A script can
/// echo arbitrarily long chat, and a trace must not grow an unbounded line.
const MAX_LINE: usize = 4096;

/// Largest active trace file before it is rotated.
///
/// Traces are far more verbose than the app log (`logging.rs` caps that one at
/// 5 MiB), so this cap is deliberately larger; a single archive preserves the
/// most recent context across a rotation while the total on disk stays bounded.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// How many rotated trace files are kept (`<path>.1` through `<path>.N`).
///
/// One archive is enough for a trace: it keeps the run-up to the rotation
/// alongside the fresh active file without letting disk use grow without bound.
pub const MAX_ARCHIVES: u32 = 1;

/// The process-wide tracer, or `None` while tracing is off.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// The installed tracer. Replaced only by [`install`].
static SLOT: OnceLock<Mutex<Option<Arc<Tracer>>>> = OnceLock::new();
/// Guards [`start_from_env`] so the environment is consulted at most once.
static ENV_INIT: Once = Once::new();
/// Guards the one-off stderr note about a failed tracer.
static NOTE_ONCE: AtomicBool = AtomicBool::new(false);
/// Caches the `PALACE_DUMP_SCRIPTS` decision so it is read at most once.
static DUMP_SCRIPTS: OnceLock<bool> = OnceLock::new();
/// Caches the `PALACE_DUMP_VARS` decision so it is read at most once.
static DUMP_VARS: OnceLock<bool> = OnceLock::new();

/// One open trace target.
///
/// Held behind an [`Arc`] in the process-wide slot so the hook functions can
/// clone a handle without keeping the slot lock held while they write.
pub struct Tracer {
    sink: Mutex<Sink>,
    start: Instant,
    start_wall: SystemTime,
    failed: AtomicBool,
}

enum Sink {
    File(FileSink),
    Stderr,
}

/// One open trace file plus its rotation policy.
///
/// The file handle and the written-byte counter live together so that a single
/// [`Mutex`] — the one on [`Tracer::sink`] — guards both. The write path only
/// has `&self`, so the counter is interior-mutable rather than reset by the
/// caller.
struct FileSink {
    file: File,
    path: PathBuf,
    written: u64,
    max_bytes: u64,
    max_archives: u32,
}

impl FileSink {
    /// Open `path` for appending, honouring any bytes already there.
    fn open(path: &Path, max_bytes: u64, max_archives: u32) -> io::Result<FileSink> {
        let file = open_append(path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(FileSink {
            file,
            path: path.to_path_buf(),
            written,
            max_bytes,
            max_archives,
        })
    }

    /// Append one framed line, rotating first when it would cross the cap.
    ///
    /// Rotation is best effort: if the rename or reopen fails the bytes are
    /// still appended to the file that is open, so tracing degrades to plain
    /// appending instead of panicking or dropping the tracer.
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.written.saturating_add(bytes.len() as u64) > self.max_bytes {
            let _ = self.rotate();
        }
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.written = self.written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    /// Shift the archives down, move the active file to `.1`, and reopen empty.
    fn rotate(&mut self) -> io::Result<()> {
        if self.max_archives > 0 {
            let _ = fs::remove_file(archive_path(&self.path, self.max_archives));
            for index in (1..self.max_archives).rev() {
                let from = archive_path(&self.path, index);
                if from.exists() {
                    let _ = fs::rename(&from, archive_path(&self.path, index + 1));
                }
            }
            let _ = fs::rename(&self.path, archive_path(&self.path, 1));
        } else {
            let _ = fs::remove_file(&self.path);
        }
        self.file = open_append(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Tracer {
    /// Open (creating or appending) a trace file.
    ///
    /// Returns the I/O error rather than panicking; [`start_from_env`] turns
    /// that into an inert no-op.
    pub fn to_path(path: impl AsRef<Path>) -> io::Result<Tracer> {
        Tracer::to_path_with_limits(path.as_ref(), MAX_BYTES, MAX_ARCHIVES)
    }

    /// Open a trace file with an explicit rotation policy.
    ///
    /// [`to_path`] supplies the published [`MAX_BYTES`] and [`MAX_ARCHIVES`];
    /// tests supply a small cap so the boundary is crossed in a few writes.
    fn to_path_with_limits(path: &Path, max_bytes: u64, max_archives: u32) -> io::Result<Tracer> {
        let sink = FileSink::open(path, max_bytes, max_archives)?;
        Ok(Tracer::new(Sink::File(sink)))
    }

    /// Trace to stderr, for `PALACE_TRACE=1` / `PALACE_TRACE=stderr`.
    #[must_use]
    pub fn to_stderr() -> Tracer {
        Tracer::new(Sink::Stderr)
    }

    fn new(sink: Sink) -> Tracer {
        Tracer {
            sink: Mutex::new(sink),
            start: Instant::now(),
            start_wall: SystemTime::now(),
            failed: AtomicBool::new(false),
        }
    }

    /// Whether a write has failed and the tracer has latched off.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Write one framed line, or latch off if the sink refuses it.
    fn line(&self, tag: &str, message: &str) {
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        let text = sanitize(message);
        let line = format!("{} {tag:<6} {text}\n", self.timestamp());
        let result = match self.sink.lock() {
            Ok(mut sink) => write_sink(&mut sink, line.as_bytes()),
            Err(poisoned) => write_sink(&mut poisoned.into_inner(), line.as_bytes()),
        };
        if let Err(error) = result {
            self.failed.store(true, Ordering::Relaxed);
            note_failure("a trace write failed", &error);
        }
    }

    /// `2026-09-17T12:34:56.789Z +1234ms` — wall clock and time since open.
    fn timestamp(&self) -> String {
        let elapsed = self.start.elapsed();
        let wall = self
            .start_wall
            .checked_add(elapsed)
            .unwrap_or(self.start_wall);
        let since = wall.duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = since.as_secs() as i64;
        let millis = since.subsec_millis();
        let (year, month, day, hour, minute, second) = civil_from_secs(secs);
        format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z +{}ms",
            elapsed.as_millis()
        )
    }

    /// A frame received.
    pub fn frame_in(&self, frame: &Frame, order: ByteOrder) {
        self.line("recv", &describe_frame(frame, order));
    }

    /// A frame sent.
    pub fn frame_out(&self, frame: &Frame, order: ByteOrder) {
        self.line("send", &describe_frame(frame, order));
    }

    /// One dispatched script event and everything it did.
    pub fn dispatch(&self, report: &DispatchReport, spot: Option<i32>) {
        let problems: Vec<String> = report
            .runs
            .iter()
            .filter_map(|run| run.error.clone())
            .collect();
        self.line(
            "script",
            &format!(
                "event={} spot={} fired={} effects=[{}] problems=[{}]",
                report.handler,
                spot.map_or_else(|| "-".to_string(), |spot| spot.to_string()),
                report.runs.len(),
                summarize_effects(&report.effects),
                problems.join("; ")
            ),
        );
    }

    /// Effects applied outside a [`DispatchReport`]: the input-box script and
    /// expired alarms.
    pub fn script_effects(&self, event: &str, effects: &[Effect]) {
        self.line(
            "script",
            &format!(
                "event={event} spot=- fired=0 effects=[{}] problems=[]",
                summarize_effects(effects)
            ),
        );
    }

    /// A [`ClientEvent`] emitted, rendered as the `live-smoke` harness renders it.
    pub fn client_event(&self, event: &ClientEvent) {
        self.line("event", &describe_client_event(event));
    }

    /// A free-form state transition line.
    pub fn state(&self, text: &str) {
        self.line("state", text);
    }

    /// A room the client has arrived in.
    pub fn room_arrived(&self, room_id: i32, name: &str) {
        self.state(&format!("room_arrived room={room_id} name={name:?}"));
    }

    /// The room left behind by a navigation request, if one was loaded.
    pub fn room_leave(&self, room_id: Option<i32>) {
        self.state(&format!(
            "room_leave room={}",
            room_id.map_or_else(|| "-".to_string(), |id| id.to_string())
        ));
    }

    /// A request to navigate to `room_id`.
    pub fn nav_request(&self, room_id: i32) {
        self.state(&format!("nav_request room={room_id}"));
    }

    /// The signed-in user's worn list after a change.
    pub fn worn_props(&self, user_id: i32, props: &[AssetSpec]) {
        let ids: Vec<String> = props.iter().map(|spec| spec.id.to_string()).collect();
        self.state(&format!(
            "worn_props user={user_id} ids=[{}]",
            ids.join(",")
        ));
    }
}

fn write_sink(sink: &mut Sink, bytes: &[u8]) -> io::Result<()> {
    match sink {
        Sink::File(file) => file.append(bytes),
        Sink::Stderr => {
            let mut stderr = io::stderr().lock();
            stderr.write_all(bytes).and_then(|()| stderr.flush())
        }
    }
}

/// Open `path` for appending, creating it if it does not exist.
fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// `<path>.1` for `index` 1, and so on — the rotation naming convention shared
/// with the desktop shell's log.
fn archive_path(path: &Path, index: u32) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// The process-wide tracer, or `None` while tracing is off.
///
/// This is the hot-path gate: one atomic load when tracing is disabled.
#[must_use]
pub fn tracer() -> Option<Arc<Tracer>> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let slot = SLOT.get()?;
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clone()
}

/// Whether tracing is on.
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Replace the process-wide tracer (or turn it off with `None`).
///
/// Production code goes through [`start_from_env`]; tests and embedders use this
/// to point the runtime at a specific file.
pub fn install(tracer: Option<Arc<Tracer>>) {
    let enabled = tracer.is_some();
    let slot = SLOT.get_or_init(|| Mutex::new(None));
    {
        let mut guard = match slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = tracer;
    }
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Read `PALACE_TRACE` once and install the tracer it names.
///
/// Called by the runtime entry point; idempotent, and never overrides a tracer
/// installed explicitly with [`install`].
pub fn start_from_env() {
    ENV_INIT.call_once(|| {
        if let Some(tracer) = tracer_from_value(std::env::var_os(ENV_VAR)) {
            install(Some(Arc::new(tracer)));
        }
    });
}

/// Build a tracer from a raw `PALACE_TRACE` value: `None`/empty is off, `1` and
/// `stderr` mean stderr, anything else is a file path.
fn tracer_from_value(value: Option<OsString>) -> Option<Tracer> {
    let value = value?;
    let text = value.to_string_lossy();
    if text.is_empty() {
        return None;
    }
    if text == "1" || text.eq_ignore_ascii_case("stderr") {
        return Some(Tracer::to_stderr());
    }
    match Tracer::to_path(Path::new(&value)) {
        Ok(tracer) => Some(tracer),
        Err(error) => {
            note_failure("the trace file could not be opened", &error);
            None
        }
    }
}

fn note_failure(context: &str, error: &dyn fmt::Display) {
    if !NOTE_ONCE.swap(true, Ordering::Relaxed) {
        eprintln!("palace-client: tracing is off, {context}: {error}");
    }
}

/// `opcode=navR(ROOMGOTO) ref=7 len=2 room=86` — the shape every frame line uses.
fn describe_frame(frame: &Frame, order: ByteOrder) -> String {
    let mut text = format!(
        "opcode={} ref={} len={}",
        frame.opcode.describe(),
        frame.ref_num,
        frame.payload.len()
    );
    if frame.opcode == opcode::ROOMGOTO {
        if let Some(room) = read_u16(&frame.payload, order) {
            text.push_str(&format!(" room={room}"));
            return text;
        }
    }
    if frame.opcode == opcode::XTALK || frame.opcode == opcode::XWHISPER {
        let kind = if frame.opcode == opcode::XTALK {
            "xtalk"
        } else {
            "xwhisper"
        };
        let decoded = crate::xtlk::decode_payload(&frame.payload, order)
            .unwrap_or_else(|| "<undecodable>".to_string());
        text.push_str(&format!(" msg={kind} {decoded:?}"));
        return text;
    }
    let summary = match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
        Ok(Message::Unknown { .. }) => {
            if frame.opcode.is_known() {
                "message body not decoded".to_string()
            } else {
                "unknown opcode".to_string()
            }
        }
        Ok(message) => message.describe(),
        Err(error) => format!("decode error: {error}"),
    };
    text.push_str(&format!(" msg={summary}"));
    text
}

fn read_u16(bytes: &[u8], order: ByteOrder) -> Option<u16> {
    let pair: [u8; 2] = bytes.get(..2)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u16::from_le_bytes(pair),
        ByteOrder::Big => u16::from_be_bytes(pair),
    })
}

fn summarize_effects(effects: &[Effect]) -> String {
    effects
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One line per [`ClientEvent`], matching the `live-smoke` harness's first line
/// for the same event so the log and the harness agree.
#[must_use]
pub fn describe_client_event(event: &ClientEvent) -> String {
    match event {
        ClientEvent::Status { status, message } => format!("[status] {status:?} {message:?}"),
        ClientEvent::Banner { banner } => format!(
            "[banner] {} v{:?} media={:?} users={:?}",
            banner.name.as_deref().unwrap_or("?"),
            banner.version,
            banner.media_base,
            banner.total_users
        ),
        ClientEvent::Rooms { rooms } => format!("[rooms] {}", rooms.len()),
        ClientEvent::Users { users } => format!("[users] {}", users.len()),
        ClientEvent::RoomEntered { room } => {
            format!("[room] #{} {:?} users={}", room.id, room.name, room.users)
        }
        ClientEvent::Chat { line } => {
            format!("[chat:{:?}] {}: {}", line.kind, line.name, line.text)
        }
        ClientEvent::Screen { screen } => format!(
            "[screen] v{} room {} {}x{} buffer {}x{} scale {:.3} dpr {} avatars={} loose={} pending={}",
            screen.version,
            screen.room_id,
            screen.geometry.room_w,
            screen.geometry.room_h,
            screen.geometry.bitmap_w,
            screen.geometry.bitmap_h,
            screen.geometry.scale,
            screen.geometry.dpr,
            screen.avatars,
            screen.loose_props,
            screen.props_pending
        ),
        ClientEvent::Avatars { roster } => format!(
            "[avatars] v{} room {} count={}",
            roster.version,
            roster.room_id,
            roster.avatars.len()
        ),
        ClientEvent::Script {
            event,
            fired,
            effects,
            problems,
        } => format!(
            "[script] ON {event}: {fired} handler(s) fired effects=[{}] problems=[{}]",
            effects.join(", "),
            problems.join("; ")
        ),
        ClientEvent::Note { text } => format!("[note] {text}"),
        ClientEvent::Sound { name } => format!("[sound] {name:?}"),
        ClientEvent::MidiPlay { name } => format!("[midi] play {name:?}"),
        ClientEvent::MidiLoop { name, loops } => format!("[midi] loop {name:?} x{loops}"),
        ClientEvent::MidiStop => "[midi] stop".to_string(),
        ClientEvent::Beep => "[beep]".to_string(),
        ClientEvent::Tooltip { text } => match text {
            Some(text) => format!("[tooltip] {text:?}"),
            None => "[tooltip] cleared".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Free-function hooks: the shape the runtime calls.
// ---------------------------------------------------------------------------

/// A frame received.
pub fn frame_in(frame: &Frame, order: ByteOrder) {
    if let Some(tracer) = tracer() {
        tracer.frame_in(frame, order);
    }
}

/// A frame sent.
pub fn frame_out(frame: &Frame, order: ByteOrder) {
    if let Some(tracer) = tracer() {
        tracer.frame_out(frame, order);
    }
}

/// A dispatched script event.
pub fn dispatch(report: &DispatchReport, spot: Option<i32>) {
    if let Some(tracer) = tracer() {
        tracer.dispatch(report, spot);
    }
}

/// Effects applied outside a [`DispatchReport`].
pub fn script_effects(event: &str, effects: &[Effect]) {
    if let Some(tracer) = tracer() {
        tracer.script_effects(event, effects);
    }
}

/// A [`ClientEvent`] emitted.
pub fn client_event(event: &ClientEvent) {
    if let Some(tracer) = tracer() {
        tracer.client_event(event);
    }
}

/// A state transition.
pub fn state(text: &str) {
    if let Some(tracer) = tracer() {
        tracer.state(text);
    }
}

/// A room arrived in.
pub fn room_arrived(room_id: i32, name: &str) {
    if let Some(tracer) = tracer() {
        tracer.room_arrived(room_id, name);
    }
}

/// The room left behind by a navigation request.
pub fn room_leave(room_id: Option<i32>) {
    if let Some(tracer) = tracer() {
        tracer.room_leave(room_id);
    }
}

/// A navigation request.
pub fn nav_request(room_id: i32) {
    if let Some(tracer) = tracer() {
        tracer.nav_request(room_id);
    }
}

/// A worn-prop change.
pub fn worn_props(user_id: i32, props: &[AssetSpec]) {
    if let Some(tracer) = tracer() {
        tracer.worn_props(user_id, props);
    }
}

/// An avatar roster was published: how many avatars, and how many JSON bytes.
pub fn avatar_roster(count: usize, bytes: usize) {
    let Some(tracer) = tracer() else {
        return;
    };
    tracer.state(&format!("avatar_roster count={count} bytes={bytes}"));
}

/// The signed-in user's own move was applied to the model before the wire.
pub fn self_move_applied(user_id: i32, x: i32, y: i32, changed: bool) {
    let Some(tracer) = tracer() else {
        return;
    };
    tracer.state(&format!(
        "self_move local id={user_id} to=({x},{y}) changed={changed}"
    ));
}

/// The predicted move was written to the server.
pub fn move_sent(user_id: i32, x: i32, y: i32) {
    let Some(tracer) = tracer() else {
        return;
    };
    tracer.state(&format!("move_sent id={user_id} to=({x},{y})"));
}

/// The predicted move asked for its own frame, before the loop waits on the
/// socket for a server reply.
pub fn redraw_requested(user_id: i32, x: i32, y: i32) {
    let Some(tracer) = tracer() else {
        return;
    };
    tracer.state(&format!("redraw_requested id={user_id} to=({x},{y})"));
}

// ---------------------------------------------------------------------------
// PALACE_DUMP_SCRIPTS: the live hotspot-script dump
// ---------------------------------------------------------------------------

/// Whether the hotspot-script dump is on.
///
/// Read once and cached; with `PALACE_DUMP_SCRIPTS` unset every call after the
/// first is a single atomic-free `OnceLock` load.
fn dump_scripts_enabled() -> bool {
    *DUMP_SCRIPTS.get_or_init(|| std::env::var_os(DUMP_SCRIPTS_ENV_VAR).is_some())
}

/// Print every hotspot's script source in `room` to stderr, verbatim.
///
/// Enabled by `PALACE_DUMP_SCRIPTS`; with it unset this returns without
/// touching stderr, so a normal run stays byte-for-byte unchanged. Each hotspot
/// gets a header line naming its index, id and name, then its raw source
/// between `source-begin`/`source-end` markers, so the text can be read back
/// exactly as the live server sent it.
pub fn dump_scripts(room: &RoomDesc) {
    if !dump_scripts_enabled() {
        return;
    }
    let room_id = room.header.room_id;
    for (index, hotspot) in room.hotspots.iter().enumerate() {
        let name = hotspot.name.as_deref().unwrap_or("");
        eprintln!(
            "palace-dump-scripts: room={room_id} name={:?} hotspot index={index} id={} name={name:?}",
            room.name, hotspot.id
        );
        eprintln!(
            "palace-dump-scripts: source-begin room={room_id} hotspot={}",
            hotspot.id
        );
        if let Some(source) = hotspot.script.as_deref() {
            eprint!("{source}");
            if !source.ends_with('\n') {
                eprintln!();
            }
        } else {
            eprintln!("palace-dump-scripts: (no script)");
        }
        eprintln!(
            "palace-dump-scripts: source-end room={room_id} hotspot={}",
            hotspot.id
        );
    }
}

// ---------------------------------------------------------------------------
// PALACE_DUMP_VARS: the script variable store at each dispatch
// ---------------------------------------------------------------------------

/// Whether the script variable-store dump is on.
///
/// Read once and cached, like [`dump_scripts_enabled`], so the hot path never
/// touches the environment.
#[must_use]
pub fn dump_vars_enabled() -> bool {
    *DUMP_VARS.get_or_init(|| std::env::var_os(DUMP_VARS_ENV_VAR).is_some())
}

/// Print the script variable store around one dispatch to stderr.
///
/// Enabled by `PALACE_DUMP_VARS`. `before` is the global store as the handlers
/// saw it and `after` as they left it, both already rendered as text, so the
/// line shows both the gate's inputs and any assignment the handler made.
/// `username` is the host's `USERNAME`, which is a command rather than a
/// variable and so never appears in the global store itself.
pub fn dump_vars(
    handler: &str,
    spot: Option<i32>,
    username: &str,
    before: &[(String, String)],
    after: &[(String, String)],
) {
    if !dump_vars_enabled() {
        return;
    }
    eprintln!(
        "palace-dump-vars: event={handler} spot={} USERNAME={username:?} before=[{}] after=[{}]",
        spot.map_or_else(|| "-".to_string(), |spot| spot.to_string()),
        render_vars(before),
        render_vars(after)
    );
}

fn render_vars(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Collapse a message to one line and cap its length, so no field can inject a
/// newline or grow a trace line without bound.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_LINE));
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
        if out.len() >= MAX_LINE {
            out.push_str("...<truncated>");
            break;
        }
    }
    out
}

/// Days-to-civil conversion (Howard Hinnant's `civil_from_days`), with the
/// time-of-day split off first. Overflow-free for every representable `i64`.
fn civil_from_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    (
        year,
        month as u32,
        day as u32,
        hour as u32,
        minute as u32,
        second as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_host::HandlerRun;
    use palace_wire::frame::navr_frame;
    use palace_wire::messages::UserProp;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::AtomicU64;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "palace-client-trace-{}-{tag}-{seq}.log",
            std::process::id()
        ))
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("the trace file is readable")
    }

    fn report(handler: &str, effects: Vec<Effect>) -> DispatchReport {
        DispatchReport {
            handler: handler.to_string(),
            runs: vec![HandlerRun {
                spot: 0,
                steps: 3,
                error: None,
                effects: effects.clone(),
            }],
            effects,
            chat_string: None,
        }
    }

    #[test]
    fn frame_lines_name_the_opcode_direction_and_decoded_kind() {
        let path = temp_path("frames");
        let _ = std::fs::remove_file(&path);
        let tracer = Tracer::to_path(&path).expect("the trace file opens");
        let order = ByteOrder::Little;

        tracer.frame_out(&navr_frame(86, 7, order), order);
        let props = UserProp {
            user_id: 7,
            props: vec![AssetSpec { id: 99, crc: 0 }, AssetSpec { id: 101, crc: 0 }],
        };
        tracer.frame_in(&props.frame(order).expect("prop frame encodes"), order);

        let mut cipher = crate::xtlk::encrypt(b"secret hello");
        let declared = (cipher.len() + 3) as i16;
        let mut body = declared.to_le_bytes().to_vec();
        body.append(&mut cipher);
        body.push(0);
        tracer.frame_in(&Frame::new(opcode::XTALK, 13, body), order);

        let text = read(&path);
        assert!(
            text.contains(" send ") && text.contains("opcode=navR(ROOMGOTO)"),
            "the sent navR is reported with its opcode: {text}"
        );
        assert!(
            text.contains("room=86"),
            "the destination room is named: {text}"
        );
        assert!(
            text.contains(" recv ") && text.contains("opcode=usrP(USERPROP)"),
            "the received prop frame is reported: {text}"
        );
        assert!(
            text.contains("id=7") && text.contains("99") && text.contains("101"),
            "the user id and every worn prop id are decoded: {text}"
        );
        assert!(
            text.contains("xtalk") && text.contains("secret hello"),
            "encrypted chat is decrypted in the trace: {text}"
        );
        assert!(
            text.lines()
                .all(|line| line.contains("Z +") && line.contains("ms ")),
            "every line is timestamped: {text}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_dispatch_line_names_the_event_spot_effects_and_faults() {
        let path = temp_path("dispatch");
        let _ = std::fs::remove_file(&path);
        let tracer = Tracer::to_path(&path).expect("the trace file opens");
        let mut report = report("ROOMREADY", vec![Effect::GotoRoom { room: 4 }]);
        report.runs[0].error = Some("stack underflow".to_string());
        tracer.dispatch(&report, Some(12));
        let text = read(&path);
        assert!(text.contains("event=ROOMREADY"), "{text}");
        assert!(text.contains("spot=12"), "{text}");
        assert!(text.contains("fired=1"), "{text}");
        assert!(
            text.contains("GOTOROOM 4"),
            "the effect that sends the user back is recorded: {text}"
        );
        assert!(
            text.contains("stack underflow"),
            "the fault is recorded: {text}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_client_event_line_matches_the_harness_presentation() {
        let path = temp_path("event");
        let _ = std::fs::remove_file(&path);
        let tracer = Tracer::to_path(&path).expect("the trace file opens");
        tracer.client_event(&ClientEvent::Note {
            text: "script: GOTOROOM 4".to_string(),
        });
        let text = read(&path);
        assert!(text.contains("event  [note] script: GOTOROOM 4"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_state_transition_line_carries_its_fields() {
        let path = temp_path("state");
        let _ = std::fs::remove_file(&path);
        let tracer = Tracer::to_path(&path).expect("the trace file opens");
        tracer.room_arrived(86, "Arena");
        tracer.nav_request(4);
        tracer.worn_props(7, &[AssetSpec { id: 99, crc: 0 }]);
        let text = read(&path);
        assert!(
            text.contains("room_arrived room=86 name=\"Arena\""),
            "{text}"
        );
        assert!(text.contains("nav_request room=4"), "{text}");
        assert!(text.contains("worn_props user=7 ids=[99]"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_write_past_the_cap_rotates_the_file_and_keeps_one_archive() {
        let path = temp_path("rotate");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(archive_path(&path, 1));
        let tracer = Tracer::to_path_with_limits(&path, 200, MAX_ARCHIVES).expect("opens");

        for index in 0..10 {
            tracer.state(&format!("line {index} {}", "x".repeat(40)));
        }

        let archive = archive_path(&path, 1);
        assert!(archive.is_file(), "crossing the cap creates a .1 archive");
        let active = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        assert!(
            active <= 200,
            "the active file stays under the cap: {active}"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rotation_never_keeps_more_than_max_archives() {
        let path = temp_path("archive-cap");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(archive_path(&path, 1));
        let tracer = Tracer::to_path_with_limits(&path, 200, MAX_ARCHIVES).expect("opens");

        for index in 0..100 {
            tracer.state(&format!("line {index} {}", "y".repeat(60)));
        }

        assert!(
            archive_path(&path, 1).is_file(),
            "at least one rotation happened"
        );
        assert!(
            !archive_path(&path, 2).exists(),
            "no archive past MAX_ARCHIVES is kept"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(archive_path(&path, 1));
    }

    #[test]
    fn the_trace_text_still_reaches_the_active_file_after_rotation() {
        let path = temp_path("rotate-text");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(archive_path(&path, 1));
        let tracer = Tracer::to_path_with_limits(&path, 200, MAX_ARCHIVES).expect("opens");

        for index in 0..6 {
            tracer.state(&format!("filler {index} {}", "z".repeat(60)));
        }
        tracer.state("the final marker survives");

        let text = read(&path);
        assert!(
            text.contains("the final marker survives"),
            "the newest text lands in the active file: {text}"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(archive_path(&path, 1));
    }

    #[test]
    fn an_unset_target_installs_nothing_and_creates_no_file() {
        assert!(
            tracer_from_value(None).is_none(),
            "an unset PALACE_TRACE is off"
        );
        assert!(
            tracer_from_value(Some(OsString::from(""))).is_none(),
            "an empty PALACE_TRACE is off"
        );
        assert!(
            !enabled(),
            "nothing installed a tracer, so the hot gate stays closed"
        );
    }

    #[test]
    fn an_unwritable_target_degrades_instead_of_panicking() {
        // A directory that does not exist cannot be opened for writing.
        let missing = std::env::temp_dir().join(format!(
            "palace-client-trace-missing-{}/trace.log",
            std::process::id()
        ));
        assert!(
            tracer_from_value(Some(missing.clone().into_os_string())).is_none(),
            "a target that cannot be opened degrades to no tracer"
        );
        assert!(!missing.exists(), "no partial file is left behind");

        // `/dev/full` accepts the open and fails every write, which is the real
        // "the disk went away mid-session" case. The tracer must latch off.
        let full = Path::new("/dev/full");
        if full.exists() {
            let tracer = Tracer::to_path(full).expect("opening /dev/full succeeds");
            tracer.client_event(&ClientEvent::Note {
                text: "will not fit".to_string(),
            });
            assert!(
                tracer.is_failed(),
                "a failed write latches the tracer off rather than panicking"
            );
        }
    }

    #[test]
    fn civil_time_converts_a_known_epoch() {
        assert_eq!(
            civil_from_secs(1_789_603_200),
            (2026, 9, 17, 0, 0, 0),
            "the timestamp formatter agrees with a known instant"
        );
        assert_eq!(
            civil_from_secs(0),
            (1970, 1, 1, 0, 0, 0),
            "the epoch itself is midnight UTC"
        );
    }
}

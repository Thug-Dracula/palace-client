//! The composed runtime: connection, session state, asset intake, compositing.
//!
//! One blocking supervisor thread owns the socket, the `palace-asset` pipeline
//! and the `SceneBuilder`; a second thread does blocking media HTTP. Commands
//! arrive over a channel, results leave as [`ClientEvent`]s, and the current
//! frame lands in a shared [`FrameStore`] that the presentation layer reads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use palace_asset::{
    run_script_worker, AssetPipeline, AssetType, MediaConfig, PipelineEvent, ScriptFetch,
    ScriptOutcome, UreqTransport,
};
use palace_host::{
    effect_frame, move_target, Effect, HostView, PenState, ScriptEngine, ScriptEvent, UserView,
    WireContext,
};
use palace_render::{
    clamp_avatar_position, clamp_dpr, render, AnimationClock, AvatarSpec, MediaStore, PointF,
    PropStore, RenderOptions, SceneBuilder, SizeF, ViewTransform, COLOR_VARIANTS, FACE_VARIANTS,
};
use palace_wire::byteorder::Writer;
use palace_wire::frame::{user_color_frame, user_face_frame, user_move_frame, Frame};
use palace_wire::messages::{reference_logon_record, AssetSpec, Point, Talk, UserProp};
use palace_wire::opcode;
use serde::Serialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::assets::{
    missing_media, missing_props, run_media_worker, AssetWorkspace, MediaJob, MediaResult,
};
use crate::error::{ClientError, Result};
use crate::frame::{FrameStore, ScreenState, ViewGeometry};
use crate::session::{Connection, POLL_SLICE};
use crate::state::{
    ChatKind, ChatLine, ConnectionStatus, RoomInfo, ScriptStimulus, ServerBanner, SessionState,
    UserInfo, HS_LOCK,
};

const MEDIA_REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const PROP_REQUEST_BUDGET: usize = 80;

/// `HS_Door` (1): "a door" (protocol reference :1665). It cannot be locked —
/// the reference names only `HS_LockableDoor` (3) as "a door that can be
/// locked" (:1667) — so a plain door's `state` is a picture selector and
/// nothing else. Named here so the exclusion stays explicit in
/// [`state_means_locked`].
const HS_DOOR: i16 = 1;
/// `HS_LockableDoor` (3): "a door that can be locked" (:1667). The only door
/// whose `state` may mean `HS_Lock`.
const HS_LOCKABLE_DOOR: i16 = 3;

/// How to reach a server and what to draw with.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub desired_room: i16,
    pub cache_root: PathBuf,
    pub seed_media: Vec<PathBuf>,
    pub seed_props: Vec<PathBuf>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            host: "localhost".to_string(),
            port: 9998,
            username: "Guest".to_string(),
            desired_room: 0,
            cache_root: default_cache_root(),
            seed_media: Vec::new(),
            seed_props: Vec::new(),
        }
    }
}

/// Where fetched media and props are cached when nothing overrides it.
///
/// The candidates are passed in rather than read, so the Windows rung is
/// testable from Linux — a Windows-only `cfg` would leave the precedence
/// unverified on the machine this is developed on.
fn cache_root_from(
    xdg_cache_home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    home: Option<PathBuf>,
    temp: PathBuf,
) -> PathBuf {
    xdg_cache_home
        .or(local_app_data)
        .or_else(|| home.map(|h| h.join(".cache")))
        .unwrap_or(temp)
        .join("palace-client")
}

/// `XDG_CACHE_HOME`, then `LOCALAPPDATA`, then `HOME/.cache`, then the temp dir.
///
/// `XDG_CACHE_HOME` and `HOME` are Unix conventions that Windows does not set, so
/// without the `LOCALAPPDATA` rung this fell through to `/tmp` — which on Windows
/// resolves to `C:\tmp`, a directory at the root of the current drive.
fn default_cache_root() -> PathBuf {
    cache_root_from(
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::temp_dir(),
    )
}

/// The per-server directory name under the cache root.
///
/// `host:port` reads well but cannot name a Windows directory, so it is escaped:
/// `localhost:9998` becomes `localhost%3A9998`. Escaping instead of replacing
/// keeps distinct hosts distinct, and an IPv6 host carries colons of its own, so
/// this is not only about separating the port.
fn session_dir_name(host: &str, port: u16) -> String {
    palace_asset::escape_name_component(&format!("{host}:{port}"))
}

fn session_cache_dir(cfg: &ClientConfig) -> PathBuf {
    cfg.cache_root.join(session_dir_name(&cfg.host, cfg.port))
}

/// What the UI can ask the runtime to do.
#[derive(Debug, Clone)]
pub enum ClientCommand {
    GotoRoom(i32),
    Say(String),
    Click {
        x: f64,
        y: f64,
    },
    SetVisibility {
        names: bool,
        avatars: bool,
    },
    SetAvatar {
        face: i16,
        color: i16,
    },
    /// Replace the signed-in user's worn props with the complete list `props`.
    ///
    /// `USERPROP` replaces the worn list rather than extending it, and id `0` is
    /// the empty slot, not a prop. Ids outside the 9-prop cap are ignored; see
    /// [`set_self_props`].
    SetProps {
        props: Vec<u32>,
    },
    RunScript(String),
    SetViewport {
        width: f64,
        height: f64,
        dpr: f64,
        zoom: f64,
        native: bool,
    },
    Refresh,
    Disconnect,
    Reconnect,
}

/// What the runtime reports back.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Status {
        status: ConnectionStatus,
        message: Option<String>,
    },
    Banner {
        banner: ServerBanner,
    },
    Rooms {
        rooms: Vec<RoomInfo>,
    },
    Users {
        users: Vec<UserInfo>,
    },
    RoomEntered {
        room: RoomInfo,
    },
    Chat {
        line: ChatLine,
    },
    Screen {
        screen: ScreenState,
    },
    Script {
        event: String,
        fired: usize,
        effects: Vec<String>,
        problems: Vec<String>,
    },
    Note {
        text: String,
    },
}

#[derive(Debug, Clone, Copy)]
struct ViewportSpec {
    width: f64,
    height: f64,
    dpr: f64,
    zoom: f64,
    native: bool,
}

impl Default for ViewportSpec {
    fn default() -> Self {
        ViewportSpec {
            width: 960.0,
            height: 540.0,
            dpr: 1.0,
            zoom: 1.0,
            native: false,
        }
    }
}

struct Shared {
    cfg: ClientConfig,
    frames: Arc<FrameStore>,
    events: UnboundedSender<ClientEvent>,
    viewport: Mutex<ViewportSpec>,
    running: AtomicBool,
    chat_seq: AtomicU64,
    debug_frames: bool,
    last_room: Mutex<Option<i32>>,
    transform: Mutex<Option<ViewTransform>>,
    mouse: Mutex<(i32, i32)>,
    room_size: Mutex<(f64, f64)>,
}

impl Shared {
    fn emit(&self, event: ClientEvent) {
        crate::trace::client_event(&event);
        let _ = self.events.send(event);
    }

    fn note(&self, text: impl Into<String>) {
        self.emit(ClientEvent::Note { text: text.into() });
    }

    fn chat(&self, kind: ChatKind, text: impl Into<String>) {
        let seq = self.chat_seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.emit(ClientEvent::Chat {
            line: ChatLine {
                seq,
                user_id: -1,
                name: "client".to_string(),
                text: text.into(),
                kind,
            },
        });
    }

    fn viewport(&self) -> ViewportSpec {
        match self.viewport.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn set_transform(&self, transform: ViewTransform) {
        match self.transform.lock() {
            Ok(mut guard) => *guard = Some(transform),
            Err(poisoned) => *poisoned.into_inner() = Some(transform),
        }
    }

    fn transform(&self) -> Option<ViewTransform> {
        match self.transform.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn set_mouse(&self, x: i32, y: i32) {
        match self.mouse.lock() {
            Ok(mut guard) => *guard = (x, y),
            Err(poisoned) => *poisoned.into_inner() = (x, y),
        }
    }

    fn mouse(&self) -> (i32, i32) {
        match self.mouse.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    fn set_room_size(&self, width: f64, height: f64) {
        match self.room_size.lock() {
            Ok(mut guard) => *guard = (width, height),
            Err(poisoned) => *poisoned.into_inner() = (width, height),
        }
    }

    fn room_size(&self) -> (i32, i32) {
        let (width, height) = match self.room_size.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        };
        (width.round() as i32, height.round() as i32)
    }
}

/// A cloneable sender into the running runtime.
#[derive(Clone)]
pub struct ClientHandle {
    tx: UnboundedSender<ClientCommand>,
    frames: Arc<FrameStore>,
    shared: Arc<Shared>,
}

impl ClientHandle {
    /// The shared frame store, for a URI-scheme handler.
    #[must_use]
    pub fn frames(&self) -> Arc<FrameStore> {
        self.frames.clone()
    }

    /// Send a command, ignoring a closed runtime.
    pub fn send(&self, command: ClientCommand) {
        let _ = self.tx.send(command);
    }

    /// Navigate to a room.
    pub fn goto_room(&self, room_id: i32) {
        self.send(ClientCommand::GotoRoom(room_id));
    }

    /// Send a chat line.
    pub fn say(&self, text: impl Into<String>) {
        self.send(ClientCommand::Say(text.into()));
    }

    /// Click the room at a viewport pixel; hotspots are hit-tested in room
    /// coordinates through the same transform the compositor reports.
    pub fn click(&self, x: f64, y: f64) {
        self.send(ClientCommand::Click { x, y });
    }

    /// Run a bare IPTSCRAE instruction sequence against the live host.
    pub fn run_script(&self, source: impl Into<String>) {
        self.send(ClientCommand::RunScript(source.into()));
    }

    /// Show or hide name tags and avatars in the composed frame.
    pub fn set_visibility(&self, names: bool, avatars: bool) {
        self.send(ClientCommand::SetVisibility { names, avatars });
    }

    /// Change the signed-in user's face and colour, locally and on the server.
    pub fn set_avatar(&self, face: i16, color: i16) {
        self.send(ClientCommand::SetAvatar { face, color });
    }

    /// Replace the signed-in user's worn props, locally and on the server.
    pub fn set_props(&self, props: Vec<u32>) {
        self.send(ClientCommand::SetProps { props });
    }

    /// Report the pointer position, in viewport pixels.
    pub fn set_mouse(&self, x: i32, y: i32) {
        self.shared.set_mouse(x, y);
    }

    /// Tell the runtime the viewport changed.
    pub fn set_viewport(&self, width: f64, height: f64, dpr: f64, zoom: f64, native: bool) {
        self.send(ClientCommand::SetViewport {
            width,
            height,
            dpr,
            zoom,
            native,
        });
    }

    /// Ask for a fresh composition.
    pub fn refresh(&self) {
        self.send(ClientCommand::Refresh);
    }

    /// Sign off and stop the supervisor.
    pub fn disconnect(&self) {
        self.send(ClientCommand::Disconnect);
    }

    /// Reconnect after a disconnect.
    pub fn reconnect(&self) {
        self.send(ClientCommand::Reconnect);
    }

    /// Whether the supervisor is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    /// The configured host.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.shared.cfg.host
    }

    /// The configured port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.shared.cfg.port
    }
}

/// The receiving end of the runtime's event stream.
pub struct ClientEventStream {
    rx: UnboundedReceiver<ClientEvent>,
}

impl ClientEventStream {
    /// Await the next event, or `None` when the runtime has stopped.
    pub async fn recv(&mut self) -> Option<ClientEvent> {
        self.rx.recv().await
    }

    /// Take the raw receiver.
    #[must_use]
    pub fn into_receiver(self) -> UnboundedReceiver<ClientEvent> {
        self.rx
    }
}

/// Entry point: start a runtime.
pub struct ClientRuntime;

impl ClientRuntime {
    /// Start the runtime and return its handle and event stream.
    pub fn spawn(cfg: ClientConfig) -> (ClientHandle, ClientEventStream) {
        crate::trace::start_from_env();
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (ev_tx, ev_rx) = unbounded_channel();
        let frames = Arc::new(FrameStore::new());
        let shared = Arc::new(Shared {
            cfg,
            frames: frames.clone(),
            events: ev_tx,
            viewport: Mutex::new(ViewportSpec::default()),
            running: AtomicBool::new(true),
            chat_seq: AtomicU64::new(0),
            debug_frames: std::env::var_os("PALACE_DEBUG_FRAMES").is_some(),
            last_room: Mutex::new(None),
            transform: Mutex::new(None),
            mouse: Mutex::new((0, 0)),
            room_size: Mutex::new((512.0, 384.0)),
        });

        let handle = ClientHandle {
            tx: cmd_tx,
            frames,
            shared: shared.clone(),
        };
        let supervisor = shared.clone();
        let spawned = thread::Builder::new()
            .name("palace-net".to_string())
            .spawn(move || supervisor_loop(supervisor, cmd_rx));
        if let Err(e) = spawned {
            shared.running.store(false, Ordering::Relaxed);
            shared.emit(ClientEvent::Status {
                status: ConnectionStatus::Error,
                message: Some(format!("could not start network thread: {e}")),
            });
        }

        (handle, ClientEventStream { rx: ev_rx })
    }
}

fn supervisor_loop(shared: Arc<Shared>, mut cmd_rx: UnboundedReceiver<ClientCommand>) {
    let mut backoff_ms = 1_500u64;
    loop {
        if !shared.running.load(Ordering::Relaxed) {
            shared.emit(ClientEvent::Status {
                status: ConnectionStatus::Disconnected,
                message: None,
            });
            return;
        }
        match run_session(&shared, &mut cmd_rx) {
            Ok(true) => backoff_ms = 1_500,
            Ok(false) => {
                if !shared.running.load(Ordering::Relaxed) {
                    shared.emit(ClientEvent::Status {
                        status: ConnectionStatus::Disconnected,
                        message: None,
                    });
                    return;
                }
                shared.emit(ClientEvent::Status {
                    status: ConnectionStatus::Connecting,
                    message: Some("reconnecting".to_string()),
                });
                if !sleep_interruptible(&shared, &mut cmd_rx, backoff_ms) {
                    return;
                }
                backoff_ms = (backoff_ms * 2).min(20_000);
            }
            Err(e) => {
                shared.emit(ClientEvent::Status {
                    status: ConnectionStatus::Error,
                    message: Some(e.to_string()),
                });
                shared.chat(ChatKind::Error, e.to_string());
                if !sleep_interruptible(&shared, &mut cmd_rx, backoff_ms) {
                    return;
                }
                backoff_ms = (backoff_ms * 2).min(20_000);
            }
        }
    }
}

fn sleep_interruptible(
    shared: &Arc<Shared>,
    cmd_rx: &mut UnboundedReceiver<ClientCommand>,
    total_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(total_ms);
    while Instant::now() < deadline {
        if !shared.running.load(Ordering::Relaxed) {
            shared.emit(ClientEvent::Status {
                status: ConnectionStatus::Disconnected,
                message: None,
            });
            return false;
        }
        while let Ok(command) = cmd_rx.try_recv() {
            match command {
                ClientCommand::Disconnect => {
                    shared.running.store(false, Ordering::Relaxed);
                    shared.emit(ClientEvent::Status {
                        status: ConnectionStatus::Disconnected,
                        message: None,
                    });
                    return false;
                }
                ClientCommand::Reconnect => return true,
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(120));
    }
    true
}

fn run_session(
    shared: &Arc<Shared>,
    cmd_rx: &mut UnboundedReceiver<ClientCommand>,
) -> Result<bool> {
    let cfg = &shared.cfg;
    let session_root = session_cache_dir(cfg);
    let mut workspace = AssetWorkspace::new(&session_root)?;

    let (media_tx, media_rx) = mpsc::channel::<MediaJob>();
    let (media_done_tx, media_done_rx) = mpsc::channel::<MediaResult>();
    let media_thread = thread::Builder::new()
        .name("palace-media".to_string())
        .spawn({
            let dir = workspace.media_dir().to_path_buf();
            move || run_media_worker(dir, media_rx, media_done_tx)
        })
        .ok();

    let (script_fetch_tx, script_fetch_rx) = mpsc::channel::<ScriptFetch>();
    let (script_base_tx, script_base_rx) = mpsc::channel::<String>();
    let (script_done_tx, script_done_rx) = mpsc::channel::<ScriptOutcome>();
    let script_thread = thread::Builder::new()
        .name("palace-script-http".to_string())
        .spawn({
            let config = MediaConfig {
                user_agent: format!("palace-client/{}", env!("CARGO_PKG_VERSION")),
                ..MediaConfig::default()
            };
            move || {
                run_script_worker(
                    UreqTransport::from_config(&config),
                    script_base_rx,
                    script_fetch_rx,
                    script_done_tx,
                )
            }
        })
        .ok();

    let mut conn = Connection::connect(&cfg.host, cfg.port, Duration::from_secs(8))?;
    let handshake = conn.handshake(Duration::from_secs(12))?;
    let order = handshake.byte_order;
    let user_id = handshake.user_id();
    crate::trace::frame_in(&handshake.frame, order);
    shared.note(format!(
        "connected to {}:{} ({}-endian, user id {user_id})",
        cfg.host,
        cfg.port,
        order.label()
    ));

    let mut builder = build_scene_builder(&workspace, cfg);
    let mut pipeline = AssetPipeline::new();
    pipeline.set_byte_order(order);

    let mut scripts = ScriptEngine::with_palace_limits();
    let mut signon_pending = true;
    let mut click_pending: Option<(f64, f64)> = None;
    let mut run_source_pending: Option<String> = None;
    let mut occupied_room = false;

    let mut state = SessionState::new(&cfg.host, cfg.port);
    state.banner.byte_order = order.label().to_string();
    state.banner.user_id = user_id;
    shared.emit(ClientEvent::Banner {
        banner: state.banner.clone(),
    });

    let record = reference_logon_record(&cfg.username, cfg.desired_room);
    conn.send(&record.logon_frame(order))?;
    let _ = conn.send(&Frame::empty(opcode::LISTOFALLROOMS, 0));
    let _ = conn.send(&Frame::empty(opcode::LISTOFALLUSERS, 0));

    state.set_status(ConnectionStatus::Connected);
    shared.emit(ClientEvent::Status {
        status: ConnectionStatus::Connected,
        message: None,
    });
    shared.chat(
        ChatKind::System,
        format!("signed on as \"{}\"", cfg.username),
    );

    let start = Instant::now();
    let mut dirty_render = true;
    let mut dirty_geom = true;
    let mut media_base: Option<String> = None;
    let mut last_room_size: Option<(f64, f64)> = None;
    let mut last_screen: Option<ScreenState> = None;
    let mut last_dpr = shared.viewport().dpr;
    let mut last_asset_request = Instant::now() - MEDIA_REQUEST_INTERVAL;
    let mut props_received = 0usize;
    let mut restored_room = false;

    loop {
        let mut goto: Option<i32> = None;
        let mut say: Option<String> = None;
        let mut end: Option<bool> = None;
        let mut resync = false;
        while let Ok(command) = cmd_rx.try_recv() {
            match command {
                ClientCommand::GotoRoom(id) => {
                    if let Ok(mut guard) = shared.last_room.lock() {
                        *guard = Some(id);
                    }
                    goto = Some(id);
                }
                ClientCommand::Say(text) => say = Some(text),
                ClientCommand::Click { x, y } => click_pending = Some((x, y)),
                ClientCommand::SetVisibility { names, avatars } => {
                    if apply_visibility(&mut state, &mut builder, names, avatars) {
                        dirty_render = true;
                    }
                }
                ClientCommand::SetAvatar { face, color } => {
                    let face = normalize_face(i32::from(face));
                    let color = normalize_color(i32::from(color));
                    let order = state.byte_order();
                    conn.send(&user_face_frame(face, state.banner.user_id, order))?;
                    conn.send(&user_color_frame(color, state.banner.user_id, order))?;
                    let face_changed = set_self_face(&mut state, i32::from(face));
                    let color_changed = set_self_color(&mut state, i32::from(color));
                    if face_changed || color_changed {
                        dirty_render = true;
                    }
                }
                ClientCommand::SetProps { props } => {
                    let outcome = set_self_props(&mut state, &mut conn, &props)?;
                    if outcome.changed {
                        dirty_render = true;
                    }
                    if outcome.dropped > 0 {
                        shared.note(format!(
                            "props: kept the first {MAX_WORN_PROPS} worn props, ignored {}",
                            outcome.dropped
                        ));
                    }
                }
                ClientCommand::RunScript(source) => run_source_pending = Some(source),
                ClientCommand::SetViewport {
                    width,
                    height,
                    dpr,
                    zoom,
                    native,
                } => {
                    if let Ok(mut guard) = shared.viewport.lock() {
                        *guard = ViewportSpec {
                            width,
                            height,
                            dpr,
                            zoom,
                            native,
                        };
                    }
                    if (dpr - last_dpr).abs() > f64::EPSILON {
                        last_dpr = dpr;
                        dirty_render = true;
                    }
                    dirty_geom = true;
                }
                ClientCommand::Refresh => resync = true,
                ClientCommand::Disconnect => {
                    end = Some(false);
                    break;
                }
                ClientCommand::Reconnect => {
                    end = Some(true);
                    break;
                }
            }
        }

        if let Some(clean) = end {
            if !clean {
                shared.running.store(false, Ordering::Relaxed);
                shared.emit(ClientEvent::Status {
                    status: ConnectionStatus::Disconnected,
                    message: None,
                });
                shared.chat(ChatKind::System, "signed off");
            }
            let _ = conn.send(&Frame::empty(opcode::LOGOFF, 0));
            drop(media_tx);
            drop(script_fetch_tx);
            if let Some(handle) = media_thread {
                let _ = handle.join();
            }
            if let Some(handle) = script_thread {
                let _ = handle.join();
            }
            return Ok(clean);
        }

        if resync {
            shared.emit(ClientEvent::Status {
                status: state.status,
                message: state.last_error().map(str::to_string),
            });
            shared.emit(ClientEvent::Banner {
                banner: state.banner.clone(),
            });
            shared.emit(ClientEvent::Rooms {
                rooms: state.rooms.clone(),
            });
            shared.emit(ClientEvent::Users {
                users: state.users_in_room(),
            });
            if let Some(room) = state.current_room.clone() {
                shared.emit(ClientEvent::RoomEntered { room });
            }
            let replay_from = state.chat.len().saturating_sub(120);
            for line in state.chat[replay_from..].iter().cloned() {
                shared.emit(ClientEvent::Chat { line });
            }
            if let Some(screen) = &last_screen {
                shared.emit(ClientEvent::Screen {
                    screen: screen.clone(),
                });
            }
            dirty_render = true;
        }

        if let Some(room_id) = goto {
            if occupied_room {
                crate::trace::room_leave(state.current_room.as_ref().map(|room| room.id));
                for event in run_dispatch(
                    &mut scripts,
                    ScriptEvent::Leave,
                    &mut state,
                    shared,
                    &mut conn,
                    &mut dirty_render,
                    None,
                ) {
                    shared.emit(event);
                }
                occupied_room = false;
            }
            state.begin_room_change();
            crate::trace::nav_request(room_id);
            conn.send(&state.navigate_frame(room_id))?;
            dirty_render = true;
        }
        if let Some(text) = say {
            let (events, rewritten) = run_chat_dispatch(
                &mut scripts,
                ScriptEvent::OutChat,
                &mut state,
                shared,
                &mut conn,
                &mut dirty_render,
                &text,
            );
            for event in events {
                shared.emit(event);
            }
            if !rewritten.is_empty() {
                let mut writer = Writer::new(order);
                Talk {
                    user_id,
                    text: rewritten,
                }
                .encode(&mut writer);
                conn.send(&Frame::new(opcode::TALK, user_id, writer.into_vec()))?;
            } else {
                shared.note("an ON OUTCHAT script suppressed the outgoing line");
            }
        }
        if let Some((x, y)) = click_pending.take() {
            match click_room_point(shared, x, y) {
                Some((rx, ry)) => {
                    // Rooms hit-test themselves with MOUSEPOS inside their ON
                    // SELECT handlers, so the pointer must be at the click before
                    // the handler runs - Colosseum 7774 ejects when x is below 78.
                    shared.set_mouse(rx, ry);
                    let view = host_view(&state, shared);
                    match view.spot_at(rx, ry).map(|spot| spot.id) {
                        Some(id) => {
                            shared.note(format!(
                                "script: click at room ({rx},{ry}) hit hotspot {id}"
                            ));
                            if is_locked_door(&mut state, id) {
                                shared.note(format!(
                                    "script: hotspot {id} is a locked door, click refused"
                                ));
                            } else {
                                for event in run_dispatch(
                                    &mut scripts,
                                    ScriptEvent::Select,
                                    &mut state,
                                    shared,
                                    &mut conn,
                                    &mut dirty_render,
                                    Some(id),
                                ) {
                                    shared.emit(event);
                                }
                            }
                        }
                        None => {
                            shared
                                .note(format!("script: click at room ({rx},{ry}) hit no hotspot"));
                            let (room_w, room_h) = shared.room_size();
                            match walk_target(&state, room_w, room_h, rx, ry) {
                                Some((mx, my)) => {
                                    conn.send(&user_move_frame(
                                        Point::new(my as i16, mx as i16),
                                        state.banner.user_id,
                                        state.byte_order(),
                                    ))?;
                                    if apply_self_move(&mut state, mx, my) {
                                        dirty_render = true;
                                    }
                                    shared.note(format!("script: walking to room ({mx},{my})"));
                                }
                                None => shared.note(
                                    "script: click ignored, not connected or no room is loaded",
                                ),
                            }
                        }
                    }
                }
                None => shared.note("script: click ignored, the room view is not ready"),
            }
        }
        if let Some(source) = run_source_pending.take() {
            // The input box runs against the *current* session: without this a
            // box script reads whatever view the last event dispatch left, so a
            // `HASPROP` would answer for the wrong worn list.
            scripts.set_view(host_view(&state, shared));
            match scripts.run_source(&source) {
                Ok(run) => {
                    shared.note(format!(
                        "script: ran {} instruction(s) from the input box",
                        run.steps
                    ));
                    crate::trace::script_effects("RUNSOURCE", &run.effects);
                    let context = wire_context(&state, shared, scripts.host().pen);
                    let mut follow = Vec::new();
                    for effect in &run.effects {
                        for event in apply_effect(
                            effect,
                            &context,
                            &mut state,
                            shared,
                            &mut conn,
                            &mut dirty_render,
                            &mut follow,
                        ) {
                            shared.emit(event);
                        }
                    }
                }
                Err(error) => shared.chat(ChatKind::Error, format!("script error: {error}")),
            }
        }

        let now = start.elapsed().as_millis() as u64;
        let mut pipeline_events: Vec<PipelineEvent> = Vec::new();

        match conn.poll_frame(POLL_SLICE) {
            Ok(Some(frame)) => {
                crate::trace::frame_in(&frame, order);
                if shared.debug_frames {
                    shared.note(format!(
                        "<- {} ref={} len={} body={}",
                        frame.opcode.describe(),
                        frame.ref_num,
                        frame.payload.len(),
                        frame
                            .printable_payload()
                            .chars()
                            .take(60)
                            .collect::<String>()
                    ));
                }
                if palace_asset::owns(frame.opcode) {
                    pipeline_events.extend(pipeline.on_frame(&frame, order, now));
                }
                let applied = state.apply(&frame, order);
                for outbound in &applied.outbound {
                    let _ = conn.send(outbound);
                }
                for event in drain_pipeline(
                    &mut pipeline,
                    &mut builder,
                    &mut conn,
                    &mut workspace,
                    &mut props_received,
                    &mut dirty_render,
                    pipeline_events.drain(..),
                ) {
                    shared.emit(event);
                }
                if applied.banner {
                    shared.emit(ClientEvent::Banner {
                        banner: state.banner.clone(),
                    });
                }
                if let Some(url) = state.banner.media_base.clone() {
                    if media_base.as_deref() != Some(url.as_str()) {
                        media_base = Some(url.clone());
                        let _ = script_base_tx.send(url);
                        dirty_render = true;
                    }
                }
                if applied.rooms {
                    shared.emit(ClientEvent::Rooms {
                        rooms: state.rooms.clone(),
                    });
                }
                if applied.users {
                    shared.emit(ClientEvent::Users {
                        users: state.users_in_room(),
                    });
                }
                if applied.room_entered {
                    if let Some(room) = state.current_room.clone() {
                        shared.emit(ClientEvent::RoomEntered { room: room.clone() });
                        crate::trace::room_arrived(room.id, &room.name);
                        if let Some(desc) = state.room_desc.clone() {
                            scripts.load_room(&desc);
                            for problem in &scripts.problems {
                                shared.note(format!(
                                    "script: hotspot {} did not parse: {}",
                                    problem.spot, problem.error
                                ));
                            }
                            occupied_room = true;
                            if signon_pending {
                                signon_pending = false;
                                for event in run_dispatch(
                                    &mut scripts,
                                    ScriptEvent::SignOn,
                                    &mut state,
                                    shared,
                                    &mut conn,
                                    &mut dirty_render,
                                    None,
                                ) {
                                    shared.emit(event);
                                }
                            }
                        }
                        let wanted = shared.last_room.lock().ok().and_then(|guard| *guard);
                        if !restored_room {
                            restored_room = true;
                            if let Some(target) = wanted {
                                if target != room.id {
                                    state.begin_room_change();
                                    crate::trace::nav_request(target);
                                    conn.send(&state.navigate_frame(target))?;
                                    dirty_render = true;
                                }
                            }
                        }
                    }
                }
                for event in dispatch_scripts(
                    &mut scripts,
                    &applied.scripts,
                    &mut state,
                    shared,
                    &mut conn,
                    &mut dirty_render,
                ) {
                    shared.emit(event);
                }
                for mut line in applied.chat {
                    if matches!(line.kind, ChatKind::Talk | ChatKind::Whisper)
                        && scripts.has_handler(ScriptEvent::InChat)
                    {
                        let (events, rewritten) = run_chat_dispatch(
                            &mut scripts,
                            ScriptEvent::InChat,
                            &mut state,
                            shared,
                            &mut conn,
                            &mut dirty_render,
                            &line.text,
                        );
                        for event in events {
                            shared.emit(event);
                        }
                        if rewritten != line.text {
                            state.rewrite_chat_line(line.seq, rewritten.clone());
                            line.text = rewritten;
                        }
                    }
                    shared.emit(ClientEvent::Chat { line });
                }
                if applied.render {
                    dirty_render = true;
                }
            }
            Ok(None) => {}
            Err(ClientError::Disconnected) => {
                shared.chat(ChatKind::System, "the server closed the connection");
                drop(media_tx);
                drop(script_fetch_tx);
                if let Some(handle) = media_thread {
                    let _ = handle.join();
                }
                if let Some(handle) = script_thread {
                    let _ = handle.join();
                }
                return Ok(false);
            }
            Err(e) => {
                drop(media_tx);
                drop(script_fetch_tx);
                if let Some(handle) = media_thread {
                    let _ = handle.join();
                }
                if let Some(handle) = script_thread {
                    let _ = handle.join();
                }
                return Err(e);
            }
        }

        pipeline_events.extend(pipeline.poll(now));
        for event in drain_pipeline(
            &mut pipeline,
            &mut builder,
            &mut conn,
            &mut workspace,
            &mut props_received,
            &mut dirty_render,
            pipeline_events.into_iter(),
        ) {
            shared.emit(event);
        }

        while let Ok(result) = media_done_rx.try_recv() {
            match result {
                MediaResult::Fetched { name, path } => {
                    builder.media_mut().insert_path(&name, path);
                    shared.note(format!("fetched media {name}"));
                    dirty_render = true;
                }
                MediaResult::Failed { name, error } => {
                    shared.note(format!("media {name} failed: {error}"));
                }
            }
        }

        let ticks = (now * 60 / 1000) as i64;
        let alarm_effects = scripts.advance(ticks);
        if !alarm_effects.is_empty() {
            crate::trace::script_effects("ALARM", &alarm_effects);
            let context = wire_context(&state, shared, scripts.host().pen);
            let mut follow = Vec::new();
            for effect in &alarm_effects {
                for event in apply_effect(
                    effect,
                    &context,
                    &mut state,
                    shared,
                    &mut conn,
                    &mut dirty_render,
                    &mut follow,
                ) {
                    shared.emit(event);
                }
            }
            if !follow.is_empty() {
                shared.note("script: an alarm asked to re-dispatch; it will not compound");
            }
        }

        if !state.pending_fetches.is_empty() {
            for fetch in std::mem::take(&mut state.pending_fetches) {
                if script_fetch_tx.send(fetch).is_err() {
                    shared.note("script: fetch worker is gone");
                }
            }
        }

        while let Ok(outcome) = script_done_rx.try_recv() {
            for event in deliver_script_outcome(
                &mut scripts,
                &mut state,
                outcome,
                shared,
                &mut conn,
                &mut dirty_render,
            ) {
                shared.emit(event);
            }
        }

        if last_asset_request.elapsed() >= MEDIA_REQUEST_INTERVAL {
            last_asset_request = Instant::now();
            request_assets(
                &state,
                &builder,
                &mut pipeline,
                &mut workspace,
                media_base.as_deref(),
                &media_tx,
                now,
            );
        }

        if dirty_render {
            if let Some(screen) = compose(&state, &mut builder, shared, start) {
                last_room_size = Some((screen.geometry.room_w, screen.geometry.room_h));
                shared.emit(ClientEvent::Screen {
                    screen: screen.clone(),
                });
                last_screen = Some(screen);
            }
            dirty_render = false;
            dirty_geom = false;
        } else if dirty_geom {
            if let (Some(previous), Some((room_w, room_h))) = (last_screen.clone(), last_room_size)
            {
                let viewport = shared.viewport();
                let mut updated = previous;
                updated.geometry = ViewGeometry::compute(
                    SizeF::new(room_w, room_h),
                    SizeF::new(viewport.width, viewport.height),
                    viewport.zoom,
                    viewport.native,
                    clamp_dpr(viewport.dpr),
                );
                shared.set_transform(updated.geometry.transform());
                shared.emit(ClientEvent::Screen {
                    screen: updated.clone(),
                });
                last_screen = Some(updated);
            }
            dirty_geom = false;
        }
    }
}

fn build_scene_builder(workspace: &AssetWorkspace, cfg: &ClientConfig) -> SceneBuilder {
    let mut media_roots = vec![workspace.media_dir().to_path_buf()];
    media_roots.extend(cfg.seed_media.iter().cloned());
    let media = MediaStore::new(&media_roots);

    let mut props = PropStore::new();
    props.add_directory(workspace.props_dir());
    for seed in &cfg.seed_props {
        props.add_directory(seed);
    }
    SceneBuilder::new(media, props)
}

fn drain_pipeline(
    pipeline: &mut AssetPipeline,
    builder: &mut SceneBuilder,
    conn: &mut Connection,
    workspace: &mut AssetWorkspace,
    props_received: &mut usize,
    dirty_render: &mut bool,
    events: impl Iterator<Item = PipelineEvent>,
) -> Vec<ClientEvent> {
    let mut out = Vec::new();
    for event in events {
        match event {
            PipelineEvent::Send { frames } | PipelineEvent::Serve { frames } => {
                for frame in frames {
                    let _ = conn.send(&frame);
                }
            }
            PipelineEvent::AssetReady { key, .. } if key.asset_type == AssetType::PROP => {
                if let Some(asset) = pipeline.cache().peek(&key) {
                    let blob = asset.data.clone();
                    let dest = workspace.props_dir().join(format!("{}.bin", key.id));
                    let _ = std::fs::write(&dest, &blob);
                    builder.props_mut().insert_blob(key.id as u32, blob);
                    *props_received += 1;
                    *dirty_render = true;
                    out.push(ClientEvent::Note {
                        text: format!("received prop #{} ({} total)", key.id, *props_received),
                    });
                }
            }
            PipelineEvent::AssetRejected { key, error } => out.push(ClientEvent::Note {
                text: format!("asset {key} rejected: {error}"),
            }),
            PipelineEvent::Discarded { key, reason } => out.push(ClientEvent::Note {
                text: format!("asset {key} discarded: {reason}"),
            }),
            PipelineEvent::RequestFailed { key, attempts } => out.push(ClientEvent::Note {
                text: format!("asset {key} failed after {attempts} attempts"),
            }),
            PipelineEvent::RequestDropped { key, reason } => out.push(ClientEvent::Note {
                text: format!("asset {key} dropped: {reason}"),
            }),
            PipelineEvent::Partial {
                key,
                received,
                expected,
            } => out.push(ClientEvent::Note {
                text: format!("asset {key} partial {received}/{expected}"),
            }),
            _ => {}
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn request_assets(
    state: &SessionState,
    builder: &SceneBuilder,
    pipeline: &mut AssetPipeline,
    workspace: &mut AssetWorkspace,
    media_base: Option<&str>,
    media_tx: &Sender<MediaJob>,
    now: u64,
) {
    let Some(room) = &state.room_desc else {
        return;
    };
    let mut prop_ids: Vec<u32> = room.loose_props.iter().map(|p| p.spec.id).collect();
    for user in state.users_in_room() {
        prop_ids.extend(user.props.iter().copied());
    }
    for id in missing_props(builder.props(), &prop_ids)
        .into_iter()
        .take(PROP_REQUEST_BUDGET)
    {
        let _ = pipeline.request_prop(id as i32, now);
    }

    let Some(base) = media_base else {
        return;
    };
    let mut names: Vec<String> = Vec::new();
    if !room.picture.is_empty() {
        names.push(room.picture.clone());
    }
    for picture in &room.pictures {
        if let Some(name) = &picture.name {
            names.push(name.clone());
        }
    }
    for name in missing_media(builder.media(), workspace, &names) {
        workspace.mark_requested(&name);
        let _ = media_tx.send(MediaJob {
            base_url: base.to_string(),
            name,
        });
    }
}

fn compose(
    state: &SessionState,
    builder: &mut SceneBuilder,
    shared: &Arc<Shared>,
    start: Instant,
) -> Option<ScreenState> {
    let live = state.room_desc.as_ref()?;
    let mut room = live.clone();

    let loose_ids: Vec<u32> = room.loose_props.iter().map(|p| p.spec.id).collect();
    let props_pending = missing_props(builder.props(), &loose_ids).len();
    room.loose_props
        .retain(|prop| builder.props().contains(prop.spec.id));

    let (avatars, hidden_avatars) = visible_avatars(state, builder.props());

    builder.clear_pic_opacity();
    for ((spot, index), alpha) in &state.pic_opacity {
        builder.set_pic_opacity(*spot, *index, *alpha);
    }

    let mut scene = builder.build_with(&room, &avatars, &[]);
    scene.dim_level = state.room_dim;
    // The builder seeds `scene.draw` from the room descriptor alone; the session
    // list is the room's own commands plus every `DRAW` received since, so it is
    // authoritative. `compose` is the only path that turns state into pixels.
    scene.draw = state.draw.clone();
    let (logical_w, logical_h) = scene.logical_size();
    let viewport = shared.viewport();
    let dpr = clamp_dpr(viewport.dpr);
    let options = RenderOptions {
        dpr,
        clock: AnimationClock::at(start.elapsed().as_millis() as u64),
    };
    let canvas = render(&scene, options);
    let png = canvas.to_png_bytes().ok()?;
    let version = shared.frames.put(png);

    let mut notes: Vec<String> = scene.notes.iter().map(|n| n.to_string()).collect();
    if props_pending > 0 {
        notes.push(format!("{props_pending} loose prop(s) still pending"));
    }
    if hidden_avatars > 0 {
        notes.push(format!(
            "{hidden_avatars} avatar(s) not drawn: prop art not received yet"
        ));
    }

    let geometry = ViewGeometry::compute(
        SizeF::new(logical_w, logical_h),
        SizeF::new(viewport.width, viewport.height),
        viewport.zoom,
        viewport.native,
        dpr,
    );
    shared.set_transform(geometry.transform());
    shared.set_room_size(logical_w, logical_h);

    Some(ScreenState {
        version,
        room_id: i32::from(room.header.room_id),
        room_name: room.name.clone(),
        avatars: avatars.len(),
        loose_props: room.loose_props.len(),
        props_pending,
        notes,
        geometry,
    })
}

/// The avatar specs to draw for the users in a room, plus how many were hidden.
///
/// A user with no worn props still becomes an avatar — their built-in face is
/// always available — which is how the signed-in user finally becomes visible.
/// Only a user who wears props none of which have arrived yet is skipped, and
/// counted so the caller can report why.
fn avatar_specs(users: &[UserInfo], props: &PropStore) -> (Vec<AvatarSpec>, usize) {
    let mut hidden = 0usize;
    let avatars = users
        .iter()
        .filter_map(|user| {
            if !user.props.is_empty() && !user.props.iter().any(|id| props.contains(*id)) {
                hidden += 1;
                return None;
            }
            let mut spec =
                AvatarSpec::new(i32::from(user.x), i32::from(user.y), user.props.clone())
                    .with_face_color(user.face, user.color);
            spec.name = Some(user.name.clone());
            Some(spec)
        })
        .collect();
    (avatars, hidden)
}

fn visible_avatars(state: &SessionState, props: &PropStore) -> (Vec<AvatarSpec>, usize) {
    if state.avatars_hidden {
        (Vec::new(), 0)
    } else {
        avatar_specs(&state.users_in_room(), props)
    }
}

/// Apply a visibility request to the render inputs.
///
/// Name tags live on the [`SceneBuilder`] and survive a room change; avatars are
/// suppressed by [`SessionState::avatars_hidden`], the same switch the
/// `HIDEAVATARS`/`SHOWAVATARS` effects throw. Returns whether either input moved,
/// so the caller re-renders only for a real toggle.
fn apply_visibility(
    state: &mut SessionState,
    builder: &mut SceneBuilder,
    names: bool,
    avatars: bool,
) -> bool {
    let names_changed = builder.name_tags_visible() != names;
    builder.set_name_tags_visible(names);
    let avatars_changed = state.avatars_hidden == avatars;
    state.avatars_hidden = !avatars;
    names_changed || avatars_changed
}

/// Build the snapshot a running script observes.
fn host_view(state: &SessionState, shared: &Arc<Shared>) -> HostView {
    let (mouse_x, mouse_y) = shared.mouse();
    let (room_w, room_h) = shared.room_size();
    let self_user = state.users.get(&state.banner.user_id);
    let mut view = HostView {
        self_id: state.banner.user_id,
        self_name: self_user.map(|u| u.name.clone()).unwrap_or_default(),
        self_x: self_user.map_or(0, |u| i32::from(u.x)),
        self_y: self_user.map_or(0, |u| i32::from(u.y)),
        self_props: self_user
            .map(|u| u.props.iter().map(|p| i64::from(*p)).collect())
            .unwrap_or_default(),
        room_width: room_w,
        room_height: room_h,
        server_name: state.banner.name.clone().unwrap_or_default(),
        mouse: (mouse_x, mouse_y),
        ..HostView::default()
    };
    if let Some(room) = &state.room_desc {
        view.apply_room(room);
    }
    view.users = state
        .users_in_room()
        .iter()
        .map(|user| UserView {
            id: user.id,
            name: user.name.clone(),
            x: i32::from(user.x),
            y: i32::from(user.y),
            props: user.props.iter().map(|p| i64::from(*p)).collect(),
        })
        .collect();
    view
}

/// The session facts an effect needs to become a wire frame.
fn wire_context(state: &SessionState, shared: &Arc<Shared>, pen: PenState) -> WireContext {
    let (room_w, room_h) = shared.room_size();
    let self_user = state.users.get(&state.banner.user_id);
    WireContext {
        byte_order: state.byte_order(),
        user_id: state.banner.user_id,
        room_id: state.current_room.as_ref().map_or(0, |room| room.id),
        room_width: room_w,
        room_height: room_h,
        self_pos: (
            self_user.map_or(0, |u| i32::from(u.x)),
            self_user.map_or(0, |u| i32::from(u.y)),
        ),
        pen,
    }
}

/// Turn a viewport click into room coordinates, through the compositor's own
/// transform. `None` until a frame has been composited.
fn click_room_point(shared: &Arc<Shared>, x: f64, y: f64) -> Option<(i32, i32)> {
    let transform = shared.transform()?;
    let point = transform.viewport_to_room(PointF::new(x, y));
    Some((point.x.round() as i32, point.y.round() as i32))
}

/// The room position a bare floor click asks the signed-in user to walk to.
///
/// `None` when there is nowhere to walk: the session is not connected, or no
/// room is loaded. A target is clamped exactly as the renderer clamps an avatar
/// anchor, so the client and the frame agree on where the avatar comes to rest.
fn walk_target(
    state: &SessionState,
    room_width: i32,
    room_height: i32,
    rx: i32,
    ry: i32,
) -> Option<(i32, i32)> {
    if state.status != ConnectionStatus::Connected || state.room_desc.is_none() {
        return None;
    }
    Some(clamp_avatar_position(rx, ry, room_width, room_height))
}

/// Apply our own move to the model at once.
///
/// The reference server relays a user's own `uLoc` only to the *other* users in
/// the room (protocol reference :2126), so it never echoes it back; without this
/// the avatar would not move until the client reconnected.
fn apply_self_move(state: &mut SessionState, x: i32, y: i32) -> bool {
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let changed = user.x != x as i16 || user.y != y as i16;
    user.x = x as i16;
    user.y = y as i16;
    changed
}

fn report_event(report: &palace_host::DispatchReport) -> ClientEvent {
    ClientEvent::Script {
        event: report.handler.clone(),
        fired: report.runs.len(),
        effects: report
            .effects
            .iter()
            .map(|effect| effect.to_string())
            .collect(),
        problems: report
            .runs
            .iter()
            .filter_map(|run| run.error.clone())
            .collect(),
    }
}

/// Dispatch every script event a decoded frame recorded.
///
/// Each entry is fired exactly once, in decode order, after `state.apply` has
/// taken the frame's change. Nothing here synthesises a follow-up stimulus from
/// a handler's effects, so a handler that sets spot state cannot re-enter this
/// loop; the alarm path's "will not compound" note is its equivalent.
fn dispatch_scripts(
    scripts: &mut ScriptEngine,
    stimuli: &[ScriptStimulus],
    state: &mut SessionState,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
) -> Vec<ClientEvent> {
    let mut out = Vec::new();
    for stimulus in stimuli {
        out.extend(run_dispatch(
            scripts,
            stimulus.event,
            state,
            shared,
            conn,
            dirty_render,
            stimulus.spot,
        ));
    }
    out
}

/// Dispatch one event and apply everything it asked for.
fn run_dispatch(
    scripts: &mut ScriptEngine,
    event: ScriptEvent,
    state: &mut SessionState,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
    only: Option<i32>,
) -> Vec<ClientEvent> {
    run_event(scripts, event, state, shared, conn, dirty_render, only, 0)
}

#[allow(clippy::too_many_arguments)]
fn run_event(
    scripts: &mut ScriptEngine,
    event: ScriptEvent,
    state: &mut SessionState,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
    only: Option<i32>,
    depth: u32,
) -> Vec<ClientEvent> {
    scripts.set_view(host_view(state, shared));
    let report = match only {
        Some(spot) => scripts.fire_spot(event, spot),
        None => scripts.fire(event),
    };
    crate::trace::dispatch(&report, only);
    let mut out = Vec::new();
    for run in &report.runs {
        if let Some(error) = &run.error {
            out.push(ClientEvent::Note {
                text: format!(
                    "script ON {} (hotspot {}) failed: {error}",
                    report.handler, run.spot
                ),
            });
        }
    }
    let context = wire_context(state, shared, scripts.host().pen);
    let mut follow = Vec::new();
    for effect in &report.effects {
        out.extend(apply_effect(
            effect,
            &context,
            state,
            shared,
            conn,
            dirty_render,
            &mut follow,
        ));
    }
    if !report.runs.is_empty() {
        out.push(report_event(&report));
    }
    if depth >= MAX_SCRIPT_FOLLOW_DEPTH {
        if !follow.is_empty() {
            out.push(ClientEvent::Note {
                text: "script: nested dispatch capped".to_string(),
            });
        }
        return out;
    }
    for (next, next_only) in follow {
        out.extend(run_event(
            scripts,
            next,
            state,
            shared,
            conn,
            dirty_render,
            next_only,
            depth + 1,
        ));
    }
    out
}

/// Run an `ON INCHAT` / `ON OUTCHAT` handler and return what `CHATSTR` became.
#[allow(clippy::too_many_arguments)]
fn run_chat_dispatch(
    scripts: &mut ScriptEngine,
    event: ScriptEvent,
    state: &mut SessionState,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
    text: &str,
) -> (Vec<ClientEvent>, String) {
    let mut view = host_view(state, shared);
    view.chat_string = text.to_string();
    scripts.set_view(view);
    let report = scripts.fire(event);
    crate::trace::dispatch(&report, None);
    let mut out = Vec::new();
    let context = wire_context(state, shared, scripts.host().pen);
    let mut follow = Vec::new();
    for effect in &report.effects {
        out.extend(apply_effect(
            effect,
            &context,
            state,
            shared,
            conn,
            dirty_render,
            &mut follow,
        ));
    }
    if !report.runs.is_empty() {
        out.push(report_event(&report));
    }
    (out, report.chat_string.unwrap_or_else(|| text.to_string()))
}

const MAX_SCRIPT_FOLLOW_DEPTH: u32 = 4;

/// Hand one completed script fetch back to the scripts.
///
/// A `text/iptscrae` or `text/ipt` body is executed in the sandboxed engine and
/// then dispatched as `HTTPRECEIVED` at the fetching hotspot (`0` = room-level);
/// anything else, and any failure, dispatches `HTTPERROR` with the URL and
/// reason. This mirrors sparky's `executeScriptSource` content-type branch and
/// its `xr` failure dispatch.
fn deliver_script_outcome(
    scripts: &mut ScriptEngine,
    state: &mut SessionState,
    outcome: ScriptOutcome,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
) -> Vec<ClientEvent> {
    let spot = outcome.spot();
    let only = (spot != 0).then_some(spot);
    let requested = outcome.requested().to_string();
    match outcome {
        ScriptOutcome::Response {
            url,
            body,
            content_type,
            ..
        } => {
            let probe = palace_asset::HttpResponse {
                status: 200,
                body,
                content_type,
            };
            if probe.is_script() {
                let source = String::from_utf8_lossy(&probe.body).into_owned();
                crate::trace::script_effects("HTTPRECEIVED-source", &[]);
                let run = scripts.execute_fetched_source(&source, spot);
                if let Some(error) = &run.error {
                    shared.note(format!(
                        "script: fetched source from {url} failed to run: {error}"
                    ));
                }
                if !run.effects.is_empty() {
                    crate::trace::script_effects("HTTPRECEIVED", &run.effects);
                    let context = wire_context(state, shared, scripts.host().pen);
                    let mut follow = Vec::new();
                    for effect in &run.effects {
                        for event in apply_effect(
                            effect,
                            &context,
                            state,
                            shared,
                            conn,
                            dirty_render,
                            &mut follow,
                        ) {
                            shared.emit(event);
                        }
                    }
                }
                let report = match only {
                    Some(spot) => scripts.fire_spot(ScriptEvent::HttpReceived, spot),
                    None => scripts.fire(ScriptEvent::HttpReceived),
                };
                crate::trace::dispatch(&report, only);
                vec![report_event(&report)]
            } else {
                vec![ClientEvent::Note {
                    text: format!(
                        "script: fetched {url} ({} bytes, content type {:?}) is not IPTSCRAE; not executed",
                        probe.body.len(),
                        probe.content_type.as_deref().unwrap_or("none")
                    ),
                }]
            }
        }
        ScriptOutcome::Failure { reason, .. } => {
            let report = match only {
                Some(spot) => scripts.fire_spot(ScriptEvent::HttpError, spot),
                None => scripts.fire(ScriptEvent::HttpError),
            };
            crate::trace::dispatch(&report, only);
            let mut out = Vec::new();
            if !report.runs.is_empty() {
                out.push(report_event(&report));
            }
            out.push(ClientEvent::Note {
                text: format!("script: HTTP fetch of {requested} failed: {reason}"),
            });
            out
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_effect(
    effect: &Effect,
    context: &WireContext,
    state: &mut SessionState,
    shared: &Arc<Shared>,
    conn: &mut Connection,
    dirty_render: &mut bool,
    follow: &mut Vec<(ScriptEvent, Option<i32>)>,
) -> Vec<ClientEvent> {
    // `effect_frame` also encodes `SETPROPS`, but the prop arms below send the
    // worn list through one shared path (`set_self_props`); skipping the generic
    // encoder for `SETPROPS` is what keeps it to a single `USERPROP`.
    //
    // The frame is kept, not just sent: a stroke is applied locally by decoding
    // the *same bytes* that go on the wire, so our own view cannot drift from
    // what the other clients are told.
    let outbound = if !matches!(effect, Effect::SetProps { .. }) {
        effect_frame(effect, context)
    } else {
        None
    };
    if let Some(frame) = &outbound {
        let _ = conn.send(frame);
    }
    match effect {
        Effect::Say { text } | Effect::GlobalMessage { text } => {
            shared.chat(ChatKind::Talk, text.clone());
            Vec::new()
        }
        Effect::SayAt { text, x, y } => {
            shared.chat(ChatKind::Talk, format!("@{x},{y} {text}"));
            Vec::new()
        }
        Effect::PrivateMessage { user, text } => {
            shared.chat(ChatKind::Whisper, format!("-> {user}: {text}"));
            Vec::new()
        }
        Effect::RoomMessage { text } | Effect::LocalMessage { text } => {
            shared.chat(ChatKind::System, text.clone());
            Vec::new()
        }
        Effect::SuperUserMessage { text } => {
            shared.chat(ChatKind::System, format!("susr: {text}"));
            Vec::new()
        }
        Effect::StatusMessage { text } | Effect::LogMessage { text } => vec![ClientEvent::Note {
            text: format!("script: {text}"),
        }],
        Effect::ErrorMessage { text } => {
            shared.chat(ChatKind::Error, text.clone());
            Vec::new()
        }
        Effect::GotoUrl { url } => vec![ClientEvent::Note {
            text: format!("script: GOTOURL {url} (reported, not opened)"),
        }],
        Effect::LaunchApp { app } => vec![ClientEvent::Note {
            text: format!("script: LAUNCHAPP {app} (reported, not launched)"),
        }],
        Effect::PlaySound { name } => vec![ClientEvent::Note {
            text: format!("script: SOUND {name}"),
        }],
        Effect::MidiPlay { name } => vec![ClientEvent::Note {
            text: format!("script: MIDIPLAY {name}"),
        }],
        Effect::MidiLoop { name, loops } => vec![ClientEvent::Note {
            text: format!("script: MIDILOOP {name} x{loops}"),
        }],
        Effect::MidiStop => vec![ClientEvent::Note {
            text: "script: MIDISTOP".to_string(),
        }],
        Effect::Beep => vec![ClientEvent::Note {
            text: "script: BEEP".to_string(),
        }],
        Effect::DimRoom { percent } => {
            state.room_dim = f64::from((*percent).clamp(0, 100)) / 100.0;
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!("script: DIMROOM {percent}%"),
            }]
        }
        Effect::SetSpotState { spot, state: value }
        | Effect::SetSpotStateLocal { spot, state: value } => {
            let room_id = state.room_desc.as_ref().map(|room| room.header.room_id);
            let applied =
                room_id.is_some_and(|room_id| set_local_spot_state(state, room_id, *spot, *value));
            if applied {
                *dirty_render = true;
            }
            vec![ClientEvent::Note {
                text: format!(
                    "script: spot {spot} -> state {value}{}",
                    if applied {
                        ""
                    } else {
                        " (spot not in this room)"
                    }
                ),
            }]
        }
        Effect::MoveSpot { spot, dx: x, dy: y } | Effect::MoveSpotLocal { spot, dx: x, dy: y } => {
            let changed = move_spot_to(state, *spot, *x, *y);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: spot {spot} moved to ({x},{y}){}",
                    no_change_suffix(changed)
                ),
            }]
        }
        Effect::SetPicOffset { spot, dx: x, dy: y } => {
            let changed = set_pic_offset(state, *spot, None, *x, *y);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: spot {spot} picture offset ({x},{y}){}",
                    no_change_suffix(changed)
                ),
            }]
        }
        Effect::SetPicOffsetLocal {
            spot,
            state: index,
            dx: x,
            dy: y,
        } => {
            let changed = set_pic_offset(state, *spot, Some(*index), *x, *y);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: spot {spot} state {index} picture offset ({x},{y}){}",
                    no_change_suffix(changed)
                ),
            }]
        }
        Effect::SetPicOpacity {
            spot,
            state: index,
            opacity,
        } => {
            let changed = set_pic_opacity(state, *spot, *index, *opacity);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: spot {spot} state {index} opacity {opacity:.2}{}",
                    no_change_suffix(changed)
                ),
            }]
        }
        // The frame was already sent above by `effect_frame`, which computes the
        // target with the same `move_target` used here, so the position applied
        // locally is the position on the wire. The server relays our own `uLoc`
        // only to the other users (protocol reference :2126), so without this the
        // avatar would not move on our own screen.
        Effect::MoveUserAbs { .. } | Effect::MoveUserRel { .. } => {
            let ready = state.status == ConnectionStatus::Connected && state.room_desc.is_some();
            match move_target(effect, context) {
                Some((x, y)) if ready => {
                    let applied = apply_self_move(state, x, y);
                    *dirty_render |= applied;
                    let suffix = if applied {
                        ""
                    } else {
                        " (you are not in the room)"
                    };
                    let text = match effect {
                        Effect::MoveUserRel { dx, dy } => {
                            format!("script: MOVE ({dx},{dy}) to ({x},{y}){suffix}")
                        }
                        _ => format!("script: SETPOS to ({x},{y}){suffix}"),
                    };
                    vec![ClientEvent::Note { text }]
                }
                _ => vec![ClientEvent::Note {
                    text: format!(
                        "script: {} not applied, not connected or no room is loaded",
                        effect.command()
                    ),
                }],
            }
        }
        Effect::GotoRoom { room } => {
            if let Ok(mut guard) = shared.last_room.lock() {
                *guard = Some(*room);
            }
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!("script: GOTOROOM {room}"),
            }]
        }
        Effect::SelectSpot { spot } => {
            follow.push((ScriptEvent::Select, Some(*spot)));
            Vec::new()
        }
        Effect::Macro { index } => {
            follow.push((ScriptEvent::Macro((*index).clamp(0, 9) as u8), None));
            Vec::new()
        }
        Effect::HideAvatars => {
            let changed = !state.avatars_hidden;
            state.avatars_hidden = true;
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: "script: HIDEAVATARS".to_string(),
            }]
        }
        Effect::ShowAvatars => {
            let changed = state.avatars_hidden;
            state.avatars_hidden = false;
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: "script: SHOWAVATARS".to_string(),
            }]
        }
        Effect::ClearLooseProps => {
            if let Some(room) = state.room_desc.as_mut() {
                room.loose_props.clear();
            }
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: "script: CLEARLOOSEPROPS".to_string(),
            }]
        }
        Effect::PaintClear => {
            state.draw.detonate();
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!("script: {}", effect.command()),
            }]
        }
        Effect::PaintUndo => {
            let undone = state.draw.undo();
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!(
                    "script: {}{}",
                    effect.command(),
                    if undone.is_some() {
                        ""
                    } else {
                        " (nothing to undo)"
                    }
                ),
            }]
        }
        // The stroke was already sent above. Decoding that exact frame is what
        // keeps the local list equal to the server's copy: the geometry is
        // parsed once, from the wire form, not recomputed from the effect.
        Effect::DrawLine { .. } | Effect::DrawLineRel { .. } => {
            if let Some(frame) = &outbound {
                let (cmd, _warnings) =
                    palace_room::decode_draw_record(&frame.payload, context.byte_order);
                state.draw.apply(cmd);
                *dirty_render = true;
            }
            Vec::new()
        }
        Effect::SetProps { props } => {
            let wanted: Vec<u32> = props
                .iter()
                .filter_map(|p| u32::try_from(*p).ok())
                .collect();
            let outcome = set_self_props(state, conn, &wanted).unwrap_or(PropOutcome::NONE);
            *dirty_render |= outcome.changed;
            if outcome.dropped > 0 {
                vec![ClientEvent::Note {
                    text: format!(
                        "script: SETPROPS kept the first {MAX_WORN_PROPS} worn props, ignored {}",
                        outcome.dropped
                    ),
                }]
            } else {
                Vec::new()
            }
        }
        Effect::Naked => {
            let outcome = set_self_props(state, conn, &[]).unwrap_or(PropOutcome::NONE);
            *dirty_render |= outcome.changed;
            Vec::new()
        }
        Effect::SetUserName { name } => {
            let changed = set_self_name(state, name);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: SETUSERNAME {name:?}"),
            }]
        }
        Effect::SetChatString { .. } => Vec::new(),
        Effect::Unsupported { command } => vec![ClientEvent::Note {
            text: format!("script: {command} is not implemented"),
        }],

        // An out-of-range face has no agreed meaning: OpenPalace clamps the *wire*
        // value to 0..15 (`PalaceClient.setFace`) but its user model then maps
        // anything past the last face to 0 (`PalaceUser.as`: `if (newValue > 12)
        // newValue = 0`), QPalace ignores a face of 13 or more outright
        // (`connection.hpp`), and ThePalacev0 wraps modulo 16 (`Index.js`). The
        // rule here follows OpenPalace's model, because this renderer draws
        // OpenPalace's sheet and face rules, so a script's face lands where an
        // OpenPalace user would see it.
        Effect::SetFace { face } => {
            let changed = set_self_face(state, *face);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: SETFACE {face}{}", no_change_suffix(changed)),
            }]
        }
        Effect::SetColor { color } => {
            let changed = set_self_color(state, *color);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: SETCOLOR {color}{}", no_change_suffix(changed)),
            }]
        }

        Effect::DonProp { prop } => {
            let changed = prop_id(*prop).is_some_and(|id| wear_prop(state, id));
            if changed {
                let _ = send_self_props(state, conn);
            }
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: DONPROP {prop}{}", no_change_suffix(changed)),
            }]
        }
        Effect::DoffProp => {
            let changed = doff_prop(state).is_some();
            if changed {
                let _ = send_self_props(state, conn);
            }
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: DOFFPROP{}", no_change_suffix(changed)),
            }]
        }
        Effect::RemoveProp { prop } => {
            let changed = prop_id(*prop).is_some_and(|id| remove_worn_prop(state, id));
            if changed {
                let _ = send_self_props(state, conn);
            }
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: REMOVEPROP {prop}{}", no_change_suffix(changed)),
            }]
        }

        // `REMOVELOOSEPROP` and `MOVELOOSEPROP` address a prop by its list index,
        // not by id. That index is the server's `propNum`, numbering props in the
        // order they were added, and a client's list is built by walking
        // `firstLProp` forward and appending — `PalaceClient.as` loads the room
        // that way and then applies the script's index to that same list before
        // forwarding it on — so it indexes `room.loose_props` directly.
        Effect::AddLooseProp { prop, x, y } => {
            let changed = prop_id(*prop).is_some_and(|id| add_loose_prop(state, id, *x, *y));
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: ADDLOOSEPROP {prop} at ({x},{y}){}",
                    no_change_suffix(changed)
                ),
            }]
        }
        Effect::RemoveLooseProp { index } => {
            let changed = remove_loose_prop(state, *index);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: REMOVELOOSEPROP {index}{}",
                    no_change_suffix(changed)
                ),
            }]
        }
        Effect::MoveLooseProp { index, x, y } => {
            let changed = move_loose_prop(state, *index, *x, *y);
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!(
                    "script: MOVELOOSEPROP {index} to ({x},{y}){}",
                    no_change_suffix(changed)
                ),
            }]
        }
        // `DROPPROP` takes off the most recently worn prop and leaves it on the
        // floor where you are (`PalaceController.dropProp`).
        Effect::DropProp { x, y } => {
            let dropped = drop_prop(state, *x, *y);
            if dropped.is_some() {
                let _ = send_self_props(state, conn);
            }
            *dirty_render |= dropped.is_some();
            vec![ClientEvent::Note {
                text: format!(
                    "script: DROPPROP at ({x},{y}){}",
                    no_change_suffix(dropped.is_some())
                ),
            }]
        }

        // The pen belongs to the host: `ScriptHost::pen` owns the position, colour,
        // width and layer, and `wire_context` reads it when a stroke is encoded,
        // so these have already taken effect by the time the effect arrives.
        // The stroke itself is not pen state: `LINE`/`LINETO` decode their own
        // outbound frame into the session's draw list, so nothing tracks a pen
        // position here.
        Effect::MovePen { x, y } => vec![ClientEvent::Note {
            text: format!("script: PENPOS ({x},{y})"),
        }],
        Effect::SetPenColor { r, g, b } => vec![ClientEvent::Note {
            text: format!("script: PENCOLOR ({r},{g},{b})"),
        }],
        Effect::SetPenSize { size } => vec![ClientEvent::Note {
            text: format!("script: PENSIZE {size}"),
        }],
        Effect::PaintLayer { front } => vec![ClientEvent::Note {
            text: format!("script: {}", if *front { "PENFRONT" } else { "PENBACK" }),
        }],

        // A lock rides the wire as `DOORLOCK` / `DOORUNLOCK`, so the room's other
        // occupants see it. The lock state this client consults is a
        // lockable door's `state`, which the server's echoed
        // `DOORLOCK`/`DOORUNLOCK` sets; a click on a locked *lockable* door is
        // refused before `SELECT` is dispatched (see `state_means_locked`).
        Effect::Lock { spot } => vec![ClientEvent::Note {
            text: format!("script: LOCK {spot}"),
        }],
        Effect::Unlock { spot } => vec![ClientEvent::Note {
            text: format!("script: UNLOCK {spot}"),
        }],
        // The alarm is already scheduled: `ScriptHost` records it and the engine
        // runs the handler when it falls due.
        Effect::SetSpotAlarm { spot, ticks } => vec![ClientEvent::Note {
            text: format!("script: SETALARM spot={spot} in {ticks} ticks"),
        }],
        Effect::FetchScript { url, spot } => {
            state.pending_fetches.push(ScriptFetch {
                requested: url.clone(),
                url: url.clone(),
                spot: *spot,
            });
            vec![ClientEvent::Note {
                text: format!("script: fetching {url} (hotspot {spot})"),
            }]
        }
    }
}

/// Change a hotspot's state in `room_id`, which must be the room the client is
/// currently showing: a message aimed at another room changes nothing. The
/// `state` field is what encodes a door's lock (:1677-1680), so the door and
/// spot messages share this setter with the local script effects.
pub(crate) fn set_local_spot_state(
    state: &mut SessionState,
    room_id: i16,
    spot: i32,
    value: i32,
) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    if room.header.room_id != room_id {
        return false;
    }
    let Ok(spot) = i16::try_from(spot) else {
        return false;
    };
    let Some(hotspot) = room.hotspots.iter_mut().find(|hotspot| hotspot.id == spot) else {
        return false;
    };
    hotspot.state = value.clamp(0, i32::from(i16::MAX)) as i16;
    true
}

fn no_change_suffix(applied: bool) -> &'static str {
    if applied {
        ""
    } else {
        " (no change)"
    }
}

fn prop_id(raw: i64) -> Option<u32> {
    u32::try_from(raw).ok()
}

/// How many props a user may wear at once (`PalaceUser.wearProp`).
const MAX_WORN_PROPS: usize = 9;

/// What applying a worn-list request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PropOutcome {
    /// Whether the model's worn list actually changed.
    changed: bool,
    /// Real ids the 9-prop cap ignored, for the caller to report.
    dropped: usize,
}

impl PropOutcome {
    /// Nothing changed and nothing was ignored.
    const NONE: PropOutcome = PropOutcome {
        changed: false,
        dropped: 0,
    };
}

/// The worn list the reference model would hold from `requested`.
///
/// `PalaceUser.setProps` clears and re-adds, skipping id 0 (the empty slot the
/// server pads a record with), skipping duplicates, and stopping at 9 — so a
/// request is filtered the same way here. Returns the first-seen order and the
/// number of real ids the cap ignored.
fn normalize_worn_props(requested: &[u32]) -> (Vec<u32>, usize) {
    let mut worn = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut dropped = 0usize;
    for id in requested {
        if *id == 0 || !seen.insert(*id) {
            continue;
        }
        if worn.len() >= MAX_WORN_PROPS {
            dropped += 1;
            continue;
        }
        worn.push(*id);
    }
    (worn, dropped)
}

/// A row for the signed-in user, used only until the server's own record
/// arrives (`nprs`/`rprs` replaces it wholesale).
fn self_row(user_id: i32, props: Vec<u32>) -> UserInfo {
    UserInfo {
        id: user_id,
        name: String::new(),
        face: 0,
        color: 0,
        room_id: 0,
        x: 0,
        y: 0,
        props,
        away: false,
        is_self: true,
    }
}

/// The signed-in user's row, inserting an empty one when the server's record
/// has not arrived yet so a worn-prop change is not lost. `None` before the
/// handshake has assigned an id.
fn self_user_mut(state: &mut SessionState) -> Option<&mut UserInfo> {
    let user_id = state.banner.user_id;
    if user_id <= 0 {
        return None;
    }
    Some(
        state
            .users
            .entry(user_id)
            .or_insert_with(|| self_row(user_id, Vec::new())),
    )
}

/// Replace the signed-in user's worn list in the model, without touching the
/// wire. Returns whether it changed and how many ids the cap ignored.
///
/// Our own record may not be in `state.users` yet when a command arrives: the
/// server's `UserRec` has not been seen, so `host_view`'s `self_props` — and
/// therefore `HASPROP` — reads an empty list. Rather than drop the props on the
/// floor, a self row is inserted; the server's later record replaces it.
fn replace_self_props(state: &mut SessionState, requested: &[u32]) -> PropOutcome {
    let (worn, dropped) = normalize_worn_props(requested);
    let unchanged = || PropOutcome {
        changed: false,
        dropped,
    };
    if state.banner.user_id <= 0 {
        return unchanged();
    }
    if !state.users.contains_key(&state.banner.user_id) && worn.is_empty() {
        return unchanged();
    }
    let Some(user) = self_user_mut(state) else {
        return unchanged();
    };
    if user.props == worn {
        return unchanged();
    }
    user.props = worn;
    PropOutcome {
        changed: true,
        dropped,
    }
}

/// Send the complete worn list as exactly one `USERPROP` frame.
///
/// `refNum` is our own user id and the body is `count` then `(asset id, 0)` per
/// prop (`PalaceClient.as::updateUserProps`). The list never exceeds the
/// encoder's bound because [`normalize_worn_props`] clamps it.
fn send_self_props(state: &SessionState, conn: &mut Connection) -> Result<()> {
    let user_id = state.banner.user_id;
    let props = state.users.get(&user_id).map_or_else(Vec::new, |user| {
        user.props
            .iter()
            .map(|id| AssetSpec {
                id: *id as i32,
                crc: 0,
            })
            .collect()
    });
    crate::trace::worn_props(user_id, &props);
    let frame = UserProp { user_id, props }.frame(state.byte_order())?;
    conn.send(&frame)
}

/// Apply a worn-list request to the model and, when it changed, tell the
/// server in one `USERPROP`.
///
/// This is the one path both a [`ClientCommand::SetProps`] and a script's
/// `SETPROPS`/`DONPROP`/`DOFFPROP`/`REMOVEPROP`/`NAKED` take, so the list
/// `HASPROP` reads and the list the server keeps cannot drift. An unchanged
/// list sends nothing: the frame would carry no new information and script
/// effects can repeat.
fn set_self_props(
    state: &mut SessionState,
    conn: &mut Connection,
    requested: &[u32],
) -> Result<PropOutcome> {
    let outcome = replace_self_props(state, requested);
    if outcome.changed {
        send_self_props(state, conn)?;
    }
    Ok(outcome)
}

/// `(x, y)` as a wire `Point`, which stores `(v, h)` = `(y, x)`.
fn point(x: i32, y: i32) -> Point {
    Point::new(y as i16, x as i16)
}

/// The face cell a requested face maps to, following OpenPalace's model rule:
/// floor at 0, and anything past the last defined face becomes face 0.
fn normalize_face(face: i32) -> i16 {
    if face > i32::from(FACE_VARIANTS - 1) {
        0
    } else {
        face.max(0) as i16
    }
}

/// The colour index a requested colour maps to, clamped into
/// `0..COLOR_VARIANTS`.
fn normalize_color(color: i32) -> i16 {
    color.clamp(0, i32::from(COLOR_VARIANTS - 1)) as i16
}

/// Change the signed-in user's face cell to `face` after normalisation.
fn set_self_face(state: &mut SessionState, face: i32) -> bool {
    let face = normalize_face(face);
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let changed = user.face != face;
    user.face = face;
    changed
}

/// Change the signed-in user's colour to `color` after normalisation.
fn set_self_color(state: &mut SessionState, color: i32) -> bool {
    let color = normalize_color(color);
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let changed = user.color != color;
    user.color = color;
    changed
}

/// Change the signed-in user's name to `name`.
///
/// Applied at once, before the matching `usrN` frame is sent, so a later script
/// dispatch in the same event cascade reads the new name from `USERNAME`.
fn set_self_name(state: &mut SessionState, name: &str) -> bool {
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    if user.name == name {
        return false;
    }
    user.name = name.to_string();
    true
}

/// Wear one more prop, appending it to the worn list. A prop already worn, or a
/// tenth prop, is refused rather than evicting an older one.
fn wear_prop(state: &mut SessionState, prop: u32) -> bool {
    let Some(user) = self_user_mut(state) else {
        return false;
    };
    if user.props.len() >= MAX_WORN_PROPS || user.props.contains(&prop) {
        return false;
    }
    user.props.push(prop);
    true
}

fn doff_prop(state: &mut SessionState) -> Option<u32> {
    self_user_mut(state).and_then(|user| user.props.pop())
}

fn remove_worn_prop(state: &mut SessionState, prop: u32) -> bool {
    let Some(user) = self_user_mut(state) else {
        return false;
    };
    let Some(at) = user.props.iter().position(|worn| *worn == prop) else {
        return false;
    };
    user.props.remove(at);
    true
}

/// Put a prop on the floor. The scripted path carries no CRC and no flags, so
/// only the id and the position reach the shared room state.
fn add_loose_prop(state: &mut SessionState, prop: u32, x: i32, y: i32) -> bool {
    state.add_loose_prop(
        AssetSpec {
            id: prop as i32,
            crc: 0,
        },
        point(x, y),
    )
}

/// Drop one loose prop by index, or every one when the index is `-1`.
fn remove_loose_prop(state: &mut SessionState, index: i32) -> bool {
    state.remove_loose_prop(index)
}

fn move_loose_prop(state: &mut SessionState, index: i32, x: i32, y: i32) -> bool {
    state.move_loose_prop(index, point(x, y))
}

fn drop_prop(state: &mut SessionState, x: i32, y: i32) -> Option<u32> {
    let prop = *state.users.get(&state.banner.user_id)?.props.last()?;
    if !add_loose_prop(state, prop, x, y) {
        return None;
    }
    let _ = remove_worn_prop(state, prop);
    Some(prop)
}

fn hotspot_mut(
    state: &mut SessionState,
    room_id: i16,
    spot: i32,
) -> Option<&mut palace_room::Hotspot> {
    let room = state.room_desc.as_mut()?;
    if room.header.room_id != room_id {
        return None;
    }
    room.hotspots
        .iter_mut()
        .find(|hotspot| i32::from(hotspot.id) == spot)
}

/// The room the client is currently showing, from the parsed room description.
fn current_room_id(state: &SessionState) -> Option<i16> {
    state.room_desc.as_ref().map(|room| room.header.room_id)
}

/// Whether a clicked hotspot is a door this client must refuse to open.
///
/// `state` is primarily a picture selector, not a lock flag: the protocol
/// reference says it "selects which of the pictures associated with the hotspot
/// should be displayed. Among other things, it encodes whether a door is locked
/// or unlocked: `HS_Unlock 0`, `HS_Lock 1`" (:1677-1680). Only a door that
/// *can* be locked therefore gets refusal on state 1; a plain `HS_Door` (1)
/// is just "a door" (:1665), so its state 1 is the second picture of an
/// ordinary two-frame door. The failure modes are asymmetric — refusing a
/// legitimate door means its script never runs and the door breaks permanently,
/// while failing to refuse a locked one merely lets the server deny the move —
/// so the rule stays as narrow as the spec allows.
fn state_means_locked(hotspot_type: i16, state: i16) -> bool {
    match hotspot_type {
        HS_LOCKABLE_DOOR => state == HS_LOCK,
        // `HS_Door` (1) cannot be locked; `HS_ShutableDoor` (2) uses its states
        // as the pictures of a door opened/closed by clicking; `HS_Bolt` (4)
        // locks the door named by its `dest`, not itself.
        HS_DOOR => false,
        _ => false,
    }
}

fn is_locked_door(state: &mut SessionState, spot: i32) -> bool {
    let Some(room_id) = current_room_id(state) else {
        return false;
    };
    hotspot_mut(state, room_id, spot)
        .is_some_and(|hotspot| state_means_locked(hotspot.hotspot_type, hotspot.state))
}

/// `SETLOC` / `SETLOCLOCAL`: `x y` are the spot's new absolute position, so this
/// replaces `loc`. `MSG_SPOTMOVE` carries a position rather than a delta, the
/// reference server assigns it, and OpenPalace changed its own implementation
/// from relative to absolute to match.
fn move_spot_to(state: &mut SessionState, spot: i32, x: i32, y: i32) -> bool {
    let Some(room_id) = current_room_id(state) else {
        return false;
    };
    move_spot_in_room(state, room_id, spot, x, y)
}

/// The same move for a room named by the wire: `MSG_SPOTMOVE` carries a
/// `RoomID`, and a move aimed at another room must not touch this one.
pub(crate) fn move_spot_in_room(
    state: &mut SessionState,
    room_id: i16,
    spot: i32,
    x: i32,
    y: i32,
) -> bool {
    let Some(hotspot) = hotspot_mut(state, room_id, spot) else {
        return false;
    };
    hotspot.loc = point(x, y);
    true
}

/// `SETPICLOC` / `SETPICLOCLOCAL`: the absolute picture offset of a state, or of
/// the hotspot's current state when `index` is `None`. The manual's worked
/// example proves these replace the offset rather than adding to it.
fn set_pic_offset(state: &mut SessionState, spot: i32, index: Option<i32>, x: i32, y: i32) -> bool {
    let Some(room_id) = current_room_id(state) else {
        return false;
    };
    set_pic_offset_in_room(state, room_id, spot, index, x, y)
}

/// The same offset assignment for a room named by the wire: `MSG_PICTMOVE`
/// carries a `RoomID` and moves the hotspot's *current* state picture, which is
/// exactly what `SETPICLOC` does.
pub(crate) fn set_pic_offset_in_room(
    state: &mut SessionState,
    room_id: i16,
    spot: i32,
    index: Option<i32>,
    x: i32,
    y: i32,
) -> bool {
    let Some(hotspot) = hotspot_mut(state, room_id, spot) else {
        return false;
    };
    let index = match index {
        Some(index) => usize::try_from(index).ok(),
        None => usize::try_from(hotspot.state).ok(),
    };
    let Some(target) = index.and_then(|index| hotspot.states.get_mut(index)) else {
        return false;
    };
    target.pic_loc = point(x, y);
    true
}

/// `MSG_SPOTDEL` removes a hotspot "from the current room" (:1900) and carries
/// no `RoomID`, so the room it names is the one already being shown.
pub(crate) fn remove_local_hotspot(state: &mut SessionState, spot: i32) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    let Ok(spot) = i16::try_from(spot) else {
        return false;
    };
    let before = room.hotspots.len();
    room.hotspots.retain(|hotspot| hotspot.id != spot);
    room.hotspots.len() != before
}

fn set_pic_opacity(state: &mut SessionState, spot: i32, index: i32, opacity: f64) -> bool {
    let (Ok(spot), Ok(index)) = (i16::try_from(spot), i16::try_from(index)) else {
        return false;
    };
    state
        .pic_opacity
        .insert((spot, index), opacity.clamp(0.0, 1.0));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_wire::byteorder::ByteOrder;

    #[test]
    fn only_a_lockable_door_reads_state_one_as_locked() {
        assert!(
            state_means_locked(HS_LOCKABLE_DOOR, 1),
            "HS_LockableDoor state 1 is locked"
        );
        assert!(
            !state_means_locked(HS_LOCKABLE_DOOR, 0),
            "HS_LockableDoor state 0 is unlocked"
        );
        for other in [HS_DOOR, 2, 4, 0, 5] {
            assert!(
                !state_means_locked(other, 1),
                "type {other} state 1 is a picture selector, not a lock, so its click must not be refused"
            );
        }
    }

    #[test]
    fn a_session_cache_directory_is_legal_on_windows() {
        let cfg = ClientConfig {
            host: "localhost".to_string(),
            port: 9998,
            cache_root: PathBuf::from("cache"),
            ..ClientConfig::default()
        };
        let dir = session_cache_dir(&cfg);
        for component in dir.components() {
            let text = component.as_os_str().to_string_lossy();
            for bad in [':', '*', '?', '"', '<', '>', '|'] {
                assert!(
                    !text.contains(bad),
                    "{bad:?} cannot appear in a Windows filename, but {component:?} has it"
                );
            }
        }
        assert!(
            dir.ends_with("localhost%3A9998"),
            "the port separator must be escaped, not literal: {dir:?}"
        );
        assert_eq!(
            session_dir_name("::1", 9998),
            "%3A%3A1%3A9998",
            "an IPv6 host carries colons of its own, not only the port separator"
        );
    }

    #[test]
    fn the_cache_root_prefers_the_directory_each_platform_sets() {
        let temp = PathBuf::from("temp-fallback");
        let local = PathBuf::from("C:/Users/x/AppData/Local");
        assert_eq!(
            cache_root_from(None, Some(local.clone()), None, temp.clone()),
            local.join("palace-client"),
            "Windows sets LOCALAPPDATA and neither of the Unix variables"
        );

        let xdg = PathBuf::from("/xdg");
        assert_eq!(
            cache_root_from(Some(xdg.clone()), Some(local), None, temp.clone()),
            xdg.join("palace-client"),
            "an explicit XDG_CACHE_HOME wins over the platform default"
        );

        let home = PathBuf::from("home");
        assert_eq!(
            cache_root_from(None, None, Some(home.clone()), temp.clone()),
            home.join(".cache").join("palace-client")
        );
        assert_eq!(
            cache_root_from(None, None, None, temp.clone()),
            temp.join("palace-client"),
            "the temp dir is the last resort, not a hardcoded /tmp"
        );
    }

    fn user(id: i32, props: Vec<u32>) -> UserInfo {
        UserInfo {
            id,
            name: format!("user-{id}"),
            face: 3,
            color: 7,
            room_id: 1,
            x: 10,
            y: 20,
            props,
            away: false,
            is_self: false,
        }
    }

    const SELF_ID: i32 = 7;

    fn session_with_self() -> SessionState {
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = SELF_ID;
        let mut me = user(SELF_ID, vec![]);
        me.is_self = true;
        state.users.insert(SELF_ID, me);
        state
    }

    fn session_in_room() -> SessionState {
        let mut state = session_with_self();
        let room = palace_room::decode_payload(
            include_bytes!("../../../fixtures/rooms/86.bin"),
            ByteOrder::Little,
        )
        .expect("the fixture room decodes");
        state.room_desc = Some(room);
        state
    }

    fn loose_ids(state: &SessionState) -> Vec<u32> {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .loose_props
            .iter()
            .map(|prop| prop.spec.id)
            .collect()
    }

    #[test]
    fn setface_past_the_last_face_falls_back_to_face_zero() {
        let mut state = session_with_self();
        assert!(set_self_face(&mut state, 4));
        assert_eq!(state.users[&SELF_ID].face, 4);
        assert!(set_self_face(&mut state, 13), "13 is outside the sheet");
        assert_eq!(
            state.users[&SELF_ID].face, 0,
            "OpenPalace maps past-the-end to 0"
        );
        assert!(set_self_face(&mut state, 6));
        assert!(set_self_face(&mut state, -1), "a negative face floors at 0");
        assert_eq!(state.users[&SELF_ID].face, 0);
        assert!(
            !set_self_face(&mut state, 0),
            "an unchanged face is no change"
        );
    }

    #[test]
    fn setcolor_clamps_to_the_palette() {
        let mut state = session_with_self();
        assert!(set_self_color(&mut state, 10));
        assert_eq!(state.users[&SELF_ID].color, 10);
        assert!(set_self_color(&mut state, 16), "16 clamps down to 15");
        assert_eq!(state.users[&SELF_ID].color, 15);
        assert!(set_self_color(&mut state, -3), "-3 clamps up to 0");
        assert_eq!(state.users[&SELF_ID].color, 0);
        assert!(
            !set_self_color(&mut state, 0),
            "an unchanged colour is no change"
        );
    }

    #[test]
    fn donprop_appends_and_refuses_a_duplicate_or_a_tenth() {
        let mut state = session_with_self();
        assert!(wear_prop(&mut state, 10));
        assert!(wear_prop(&mut state, 20));
        assert_eq!(
            state.users[&SELF_ID].props,
            vec![10, 20],
            "the newest goes last"
        );
        assert!(!wear_prop(&mut state, 10), "a prop already worn is refused");
        assert_eq!(state.users[&SELF_ID].props, vec![10, 20]);
        for id in 30..40 {
            wear_prop(&mut state, id);
        }
        assert_eq!(state.users[&SELF_ID].props.len(), MAX_WORN_PROPS);
        assert!(
            !wear_prop(&mut state, 99),
            "a tenth prop is refused, not swapped in"
        );
        assert_eq!(state.users[&SELF_ID].props.len(), MAX_WORN_PROPS);
    }

    #[test]
    fn doffprop_takes_the_most_recent_and_removeprop_the_named_one() {
        let mut state = session_with_self();
        for id in [10, 20, 30] {
            wear_prop(&mut state, id);
        }
        assert_eq!(doff_prop(&mut state), Some(30));
        assert_eq!(state.users[&SELF_ID].props, vec![10, 20]);
        assert!(remove_worn_prop(&mut state, 10));
        assert_eq!(state.users[&SELF_ID].props, vec![20]);
        assert!(
            !remove_worn_prop(&mut state, 10),
            "removing it twice does nothing"
        );
        assert_eq!(doff_prop(&mut state), Some(20));
        assert_eq!(doff_prop(&mut state), None, "nothing left to take off");
    }

    #[test]
    fn addlooseprop_appends_a_record_at_the_scripted_position() {
        let mut state = session_in_room();
        let before = loose_ids(&state).len();
        assert!(add_loose_prop(&mut state, 0xA26F_9DE3, 120, 240));
        assert_eq!(loose_ids(&state).len(), before + 1);
        let added = &state.room_desc.as_ref().unwrap().loose_props[before];
        assert_eq!(added.spec.id, 0xA26F_9DE3);
        assert_eq!(added.spec.crc, 0, "the wire carries no crc for a new prop");
        assert_eq!(
            (added.loc.h, added.loc.v),
            (120, 240),
            "loc stores x in h and y in v"
        );
        assert_eq!((added.flags, added.ref_con, added.next_ofst), (0, 0, 0));
    }

    #[test]
    fn removelooseprop_removes_by_index_and_clears_everything_for_minus_one() {
        let mut state = session_in_room();
        let base = loose_ids(&state).len();
        for (id, x, y) in [(1_u32, 10, 10), (2, 20, 20), (3, 30, 30)] {
            add_loose_prop(&mut state, id, x, y);
        }
        assert!(remove_loose_prop(&mut state, base as i32 + 1));
        assert_eq!(
            loose_ids(&state)[base..],
            [1, 3],
            "index 1 of the added props"
        );
        assert!(
            !remove_loose_prop(&mut state, 999),
            "out of range changes nothing"
        );
        assert_eq!(loose_ids(&state).len(), base + 2);
        assert!(remove_loose_prop(&mut state, -1));
        assert!(state.room_desc.as_ref().unwrap().loose_props.is_empty());
        assert!(
            !remove_loose_prop(&mut state, -1),
            "clearing an empty list is no change"
        );
    }

    #[test]
    fn movelooseprop_repositions_the_prop_at_that_index() {
        let mut state = session_in_room();
        add_loose_prop(&mut state, 5, 1, 2);
        let index = loose_ids(&state).len() - 1;
        assert!(move_loose_prop(&mut state, index as i32, 300, 400));
        let prop = &state.room_desc.as_ref().unwrap().loose_props[index];
        assert_eq!((prop.loc.h, prop.loc.v), (300, 400));
        assert!(!move_loose_prop(&mut state, 999, 0, 0));
    }

    #[test]
    fn dropprop_takes_the_last_worn_prop_and_leaves_it_on_the_floor() {
        let mut state = session_in_room();
        for id in [10, 20] {
            wear_prop(&mut state, id);
        }
        let before = loose_ids(&state).len();
        assert_eq!(drop_prop(&mut state, 55, 66), Some(20));
        assert_eq!(
            state.users[&SELF_ID].props,
            vec![10],
            "the dropped prop came off"
        );
        assert_eq!(loose_ids(&state)[before..], [20]);
        let dropped = &state.room_desc.as_ref().unwrap().loose_props[before];
        assert_eq!((dropped.loc.h, dropped.loc.v), (55, 66));
    }

    #[test]
    fn effects_on_a_missing_room_or_user_change_nothing() {
        let mut orphan = SessionState::new("test", 1);
        assert!(!add_loose_prop(&mut orphan, 1, 0, 0));
        assert!(!remove_loose_prop(&mut orphan, 0));
        assert!(!move_loose_prop(&mut orphan, 0, 0, 0));
        assert_eq!(drop_prop(&mut orphan, 0, 0), None);
        assert!(!set_self_face(&mut orphan, 3));
        assert!(!set_self_color(&mut orphan, 3));
        assert!(!wear_prop(&mut orphan, 1));
        assert_eq!(doff_prop(&mut orphan), None);
        assert!(!remove_worn_prop(&mut orphan, 1));
        assert!(!replace_self_props(&mut orphan, &[1]).changed);
    }

    #[test]
    fn normalize_worn_props_drops_the_empty_slot_duplicates_and_the_overflow() {
        assert_eq!(normalize_worn_props(&[]), (Vec::new(), 0));
        assert_eq!(
            normalize_worn_props(&[0, 7, 7, 0, 9]),
            (vec![7, 9], 0),
            "id 0 is the empty slot and a duplicate adds nothing"
        );
        let (worn, dropped) = normalize_worn_props(&(1..=11).collect::<Vec<u32>>());
        assert_eq!(worn, (1..=9).collect::<Vec<u32>>());
        assert_eq!(dropped, 2, "ids 10 and 11 are past the cap");
    }

    #[test]
    fn replace_self_props_keeps_the_list_when_our_record_has_not_arrived() {
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = SELF_ID;
        assert!(!state.users.contains_key(&SELF_ID));

        let outcome = replace_self_props(&mut state, &[10, 20]);
        assert!(outcome.changed, "the list is not dropped on the floor");
        assert_eq!(state.users[&SELF_ID].props, vec![10, 20]);
        assert!(state.users[&SELF_ID].is_self);

        assert!(!replace_self_props(&mut state, &[10, 20]).changed);
        assert!(replace_self_props(&mut state, &[]).changed);
        assert!(state.users[&SELF_ID].props.is_empty());

        let mut orphan = SessionState::new("test", 1);
        orphan.banner.user_id = SELF_ID;
        assert!(!replace_self_props(&mut orphan, &[]).changed);
        assert!(
            !orphan.users.contains_key(&SELF_ID),
            "an empty list creates no row"
        );
    }

    #[test]
    fn replace_self_props_reports_what_the_cap_ignored() {
        let mut state = session_with_self();
        let outcome = replace_self_props(&mut state, &(1..=10).collect::<Vec<u32>>());
        assert!(outcome.changed);
        assert_eq!(outcome.dropped, 1);
        assert_eq!(state.users[&SELF_ID].props, (1..=9).collect::<Vec<u32>>());
    }

    fn find_hotspot(state: &SessionState, id: i32) -> &palace_room::Hotspot {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .hotspots
            .iter()
            .find(|hotspot| i32::from(hotspot.id) == id)
            .expect("the hotspot is in the room")
    }

    fn a_hotspot_showing_a_state(state: &SessionState) -> (i32, usize) {
        let room = state.room_desc.as_ref().expect("a room");
        let hotspot = room
            .hotspots
            .iter()
            .find(|hotspot| {
                usize::try_from(hotspot.state).is_ok_and(|index| index < hotspot.states.len())
            })
            .expect("the fixture room has a hotspot showing one of its states");
        let index = usize::try_from(hotspot.state).expect("a state index");
        (i32::from(hotspot.id), index)
    }

    #[test]
    fn setloc_replaces_the_hotspot_position_with_absolute_coordinates() {
        let mut state = session_in_room();
        let (id, _) = a_hotspot_showing_a_state(&state);
        assert!(move_spot_to(&mut state, id, 137, 219));
        let hotspot = find_hotspot(&state, id);
        assert_eq!(
            (hotspot.loc.h, hotspot.loc.v),
            (137, 219),
            "SETLOC assigns the position rather than adding to it"
        );
        assert!(move_spot_to(&mut state, id, 142, 226));
        assert_eq!(
            (
                find_hotspot(&state, id).loc.h,
                find_hotspot(&state, id).loc.v
            ),
            (142, 226)
        );
        assert!(!move_spot_to(&mut state, 32_767, 1, 1), "no such spot");
    }

    #[test]
    fn setpicloc_replaces_the_picture_offset_of_the_current_state() {
        let mut state = session_in_room();
        let (id, index) = a_hotspot_showing_a_state(&state);
        assert!(set_pic_offset(&mut state, id, None, 54, -21));
        let shown = &find_hotspot(&state, id).states[index];
        assert_eq!(
            (shown.pic_loc.h, shown.pic_loc.v),
            (54, -21),
            "SETPICLOC assigns the offset rather than adding to it"
        );
        assert!(set_pic_offset(&mut state, id, None, 8, 9));
        let shown = &find_hotspot(&state, id).states[index];
        assert_eq!((shown.pic_loc.h, shown.pic_loc.v), (8, 9));
    }

    #[test]
    fn setpicloclocal_addresses_the_state_it_names() {
        let mut state = session_in_room();
        let (id, _) = a_hotspot_showing_a_state(&state);
        assert!(set_pic_offset(&mut state, id, Some(0), -50, -50));
        let first = &find_hotspot(&state, id).states[0];
        assert_eq!((first.pic_loc.h, first.pic_loc.v), (-50, -50));
        assert!(
            !set_pic_offset(&mut state, id, Some(99), 0, 0),
            "no such state"
        );
        assert!(
            !set_pic_offset(&mut state, id, Some(-1), 0, 0),
            "negative index"
        );
        assert!(
            !set_pic_offset(&mut state, 32_767, None, 0, 0),
            "no such spot"
        );
    }

    #[test]
    fn setpicopacity_records_a_clamped_alpha() {
        let mut state = session_in_room();
        assert!(set_pic_opacity(&mut state, 3, 1, 0.5));
        assert_eq!(state.pic_opacity.get(&(3, 1)).copied(), Some(0.5));
        assert!(set_pic_opacity(&mut state, 3, 2, 4.2));
        assert_eq!(state.pic_opacity.get(&(3, 2)).copied(), Some(1.0));
        assert!(set_pic_opacity(&mut state, 3, 3, -1.0));
        assert_eq!(state.pic_opacity.get(&(3, 3)).copied(), Some(0.0));
    }

    #[test]
    fn hideavatars_takes_the_avatars_out_of_the_scene() {
        let mut state = session_in_room();
        let props = PropStore::new();
        assert_eq!(
            visible_avatars(&state, &props).0.len(),
            1,
            "the user is drawn"
        );
        state.avatars_hidden = true;
        assert!(
            visible_avatars(&state, &props).0.is_empty(),
            "and now is not"
        );
        state.avatars_hidden = false;
        assert_eq!(visible_avatars(&state, &props).0.len(), 1, "and back again");
    }

    #[test]
    fn a_face_or_colour_change_reaches_the_composited_scene() {
        let mut state = session_in_room();
        let builder = SceneBuilder::new(MediaStore::default(), PropStore::new());
        let room = state.room_desc.clone().expect("a room");

        let face_cell = |state: &SessionState| {
            let (avatars, hidden) = avatar_specs(&state.users_in_room(), builder.props());
            assert_eq!(hidden, 0, "the only user wears nothing, so none is hidden");
            let scene = builder.build_with(&room, &avatars, &[]);
            let avatar = &scene.avatars[0];
            assert_eq!(avatar.parts.len(), 1, "a prop-less avatar is just its face");
            avatar.parts[0].image.clone()
        };

        assert!(set_self_face(&mut state, 5));
        let five = face_cell(&state);
        assert!(set_self_face(&mut state, 10));
        assert_ne!(
            five,
            face_cell(&state),
            "the drawn face must follow the user's face"
        );

        assert!(set_self_color(&mut state, 3));
        let three = face_cell(&state);
        assert!(set_self_color(&mut state, 11));
        assert_ne!(
            three,
            face_cell(&state),
            "the drawn face must follow the user's colour"
        );
    }

    #[test]
    fn a_prop_less_user_still_becomes_an_avatar_with_their_face() {
        let (avatars, hidden) = avatar_specs(&[user(1, vec![])], &PropStore::new());
        assert_eq!(hidden, 0, "a prop-less user is not hidden");
        assert_eq!(avatars.len(), 1, "the user is drawn");
        assert_eq!((avatars[0].face, avatars[0].color), (3, 7));
        assert_eq!((avatars[0].x, avatars[0].y), (10, 20));
        assert!(avatars[0].props.is_empty());
    }

    #[test]
    fn a_user_whose_prop_art_has_not_arrived_is_counted_not_drawn() {
        let (avatars, hidden) = avatar_specs(&[user(2, vec![123])], &PropStore::new());
        assert!(avatars.is_empty());
        assert_eq!(hidden, 1);
    }

    #[test]
    fn a_user_with_a_present_prop_is_drawn_with_their_face() {
        let mut props = PropStore::new();
        let _ = props.insert_blob(123, vec![0; 16]);
        let (avatars, hidden) = avatar_specs(&[user(3, vec![123])], &props);
        assert_eq!(hidden, 0);
        assert_eq!(avatars.len(), 1);
        assert_eq!((avatars[0].face, avatars[0].color), (3, 7));
    }

    #[test]
    fn a_floor_click_resolves_to_a_renderer_clamped_walk_target() {
        let mut state = session_in_room();
        state.status = ConnectionStatus::Connected;
        assert_eq!(
            walk_target(&state, 512, 384, 30_000, 30_000),
            Some((490, 362)),
            "a click past the room edge comes to rest on the avatar margin"
        );
        assert_eq!(
            walk_target(&state, 512, 384, -50, -50),
            Some((22, 22)),
            "a click before the origin is pushed onto the top-left margin"
        );
        assert_eq!(
            walk_target(&state, 512, 384, 200, 100),
            Some((200, 100)),
            "an interior point is not moved"
        );
    }

    #[test]
    fn a_walk_is_refused_without_a_connection_or_a_room() {
        let mut state = session_in_room();
        state.status = ConnectionStatus::Disconnected;
        assert_eq!(
            walk_target(&state, 512, 384, 100, 100),
            None,
            "a disconnected session must not ask to move"
        );
        state.status = ConnectionStatus::Connected;
        state.room_desc = None;
        assert_eq!(
            walk_target(&state, 512, 384, 100, 100),
            None,
            "without a room there is no place to walk to"
        );
    }

    #[test]
    fn a_self_move_updates_our_own_position_before_the_server_relays_it() {
        let mut state = session_in_room();
        state.status = ConnectionStatus::Connected;
        assert!(
            apply_self_move(&mut state, 123, 45),
            "the first move changes the model"
        );
        let me = state.users.get(&SELF_ID).expect("the self user is known");
        assert_eq!(
            (me.x, me.y),
            (123, 45),
            "the server relays our own uLoc only to others, so we apply it"
        );
        assert!(
            !apply_self_move(&mut state, 123, 45),
            "moving to the same spot changes nothing"
        );
    }

    #[test]
    fn a_user_name_reaches_the_avatar_spec() {
        let (avatars, hidden) = avatar_specs(&[user(1, vec![])], &PropStore::new());
        assert_eq!(hidden, 0);
        assert_eq!(
            avatars[0].name.as_deref(),
            Some("user-1"),
            "the tag the compositor draws needs the name on the spec"
        );
    }

    #[test]
    fn a_user_with_missing_prop_art_is_still_counted_even_with_a_name() {
        let (avatars, hidden) = avatar_specs(&[user(2, vec![123])], &PropStore::new());
        assert!(avatars.is_empty(), "the avatar is still skipped");
        assert_eq!(hidden, 1, "and still counted for the reason note");
    }

    #[test]
    fn a_visibility_request_reaches_the_render_inputs() {
        let mut state = session_in_room();
        let mut builder = SceneBuilder::new(MediaStore::default(), PropStore::new());
        assert!(builder.name_tags_visible(), "names start visible");
        assert!(!state.avatars_hidden, "avatars start visible");

        assert!(apply_visibility(&mut state, &mut builder, false, false));
        assert!(
            !builder.name_tags_visible(),
            "names off must reach the scene builder"
        );
        assert!(state.avatars_hidden, "avatars off must reach the model");
        assert!(
            visible_avatars(&state, builder.props()).0.is_empty(),
            "hidden avatars must leave the scene"
        );
        assert!(
            !apply_visibility(&mut state, &mut builder, false, false),
            "applying the same flags again changes nothing"
        );

        assert!(apply_visibility(&mut state, &mut builder, true, true));
        assert!(builder.name_tags_visible());
        assert!(!state.avatars_hidden);
        assert_eq!(
            visible_avatars(&state, builder.props()).0.len(),
            1,
            "showing again restores the avatar"
        );
    }

    #[test]
    fn face_and_colour_requests_are_normalised_to_the_sheet() {
        assert_eq!(normalize_face(-5), 0);
        assert_eq!(normalize_face(0), 0);
        assert_eq!(normalize_face(12), 12);
        assert_eq!(
            normalize_face(13),
            0,
            "past the last face falls back to face 0"
        );
        assert_eq!(normalize_face(9_999), 0);

        assert_eq!(normalize_color(-3), 0);
        assert_eq!(normalize_color(0), 0);
        assert_eq!(normalize_color(15), 15);
        assert_eq!(
            normalize_color(16),
            15,
            "past the last colour clamps to the last"
        );
        assert_eq!(normalize_color(9_999), 15);
    }

    use std::net::TcpListener;

    use palace_room::{Hotspot, RoomDesc};
    use palace_wire::messages::RoomRec;

    const HARNESS_SELF: i32 = 13;
    const HARNESS_ROOM: i16 = 901;

    fn scripted_room(scripts: &[(i16, &str)]) -> RoomDesc {
        let hotspots: Vec<Hotspot> = scripts
            .iter()
            .map(|(id, source)| Hotspot {
                id: *id,
                state: 0,
                script: Some((*source).to_string()),
                ..Hotspot::default()
            })
            .collect();
        RoomDesc {
            header: RoomRec {
                room_id: HARNESS_ROOM,
                nbr_hotspots: hotspots.len() as i16,
                ..RoomRec::default()
            },
            name: "Scripted".to_string(),
            picture: String::new(),
            artist: String::new(),
            password: String::new(),
            pictures: Vec::new(),
            hotspots,
            loose_props: Vec::new(),
            draw_cmds: Vec::new(),
            var_data: Vec::new(),
            trailing_len: 0,
            warnings: Vec::new(),
        }
    }

    fn state_in(room: &RoomDesc) -> SessionState {
        let mut state = SessionState::new("test", 1);
        state.status = ConnectionStatus::Connected;
        state.banner.user_id = HARNESS_SELF;
        state.current_room = Some(RoomInfo {
            id: i32::from(HARNESS_ROOM),
            name: room.name.clone(),
            users: 1,
            flags: 0,
        });
        state.room_desc = Some(room.clone());
        state.users.insert(
            HARNESS_SELF,
            UserInfo {
                id: HARNESS_SELF,
                name: "Self".to_string(),
                face: 0,
                color: 0,
                room_id: HARNESS_ROOM,
                x: 0,
                y: 0,
                props: Vec::new(),
                away: false,
                is_self: true,
            },
        );
        state
    }

    fn add_other_user(state: &mut SessionState, id: i32) {
        state.users.insert(
            id,
            UserInfo {
                id,
                name: format!("user-{id}"),
                face: 0,
                color: 0,
                room_id: HARNESS_ROOM,
                x: 0,
                y: 0,
                props: Vec::new(),
                away: false,
                is_self: false,
            },
        );
    }

    struct Harness {
        shared: Arc<Shared>,
        conn: Connection,
        _drain: thread::JoinHandle<()>,
    }

    fn harness() -> Harness {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener binds");
        let port = listener.local_addr().expect("listener address").port();
        let drain = thread::spawn(move || {
            use std::io::Read;
            if let Ok((mut stream, _)) = listener.accept() {
                let mut sink = [0u8; 256];
                while stream.read(&mut sink).is_ok_and(|read| read > 0) {}
            }
        });
        let conn = Connection::connect("127.0.0.1", port, Duration::from_secs(2))
            .expect("loopback connection");
        Harness {
            shared: test_shared(),
            conn,
            _drain: drain,
        }
    }

    fn test_shared() -> Arc<Shared> {
        let (events, _rx) = unbounded_channel();
        Arc::new(Shared {
            cfg: ClientConfig::default(),
            frames: Arc::new(FrameStore::new()),
            events,
            viewport: Mutex::new(ViewportSpec::default()),
            running: AtomicBool::new(true),
            chat_seq: AtomicU64::new(0),
            debug_frames: false,
            last_room: Mutex::new(None),
            transform: Mutex::new(None),
            mouse: Mutex::new((0, 0)),
            room_size: Mutex::new((512.0, 384.0)),
        })
    }

    fn dispatch_from_frame(
        scripts: &mut ScriptEngine,
        state: &mut SessionState,
        harness: &mut Harness,
        frame: &Frame,
    ) -> Vec<ClientEvent> {
        let applied = state.apply(frame, ByteOrder::Little);
        let mut dirty_render = false;
        dispatch_scripts(
            scripts,
            &applied.scripts,
            state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        )
    }

    fn note_texts(events: &[ClientEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Note { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn fired(events: &[ClientEvent], event: &str) -> usize {
        events
            .iter()
            .filter_map(|item| match item {
                ClientEvent::Script {
                    event: name, fired, ..
                } if name == event => Some(*fired),
                _ => None,
            })
            .sum()
    }

    fn door_body(room: i16, door: i16) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(room);
        w.write_i16(door);
        w.into_vec()
    }

    fn spot_state_body(room: i16, spot: i16, state: i16) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(room);
        w.write_i16(spot);
        w.write_i16(state);
        w.into_vec()
    }

    fn name_body(name: &str) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring(name);
        w.into_vec()
    }

    /// A loopback harness that keeps every byte the client sends.
    fn harness_capturing() -> (Harness, Arc<Mutex<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener binds");
        let port = listener.local_addr().expect("listener address").port();
        let sink = Arc::new(Mutex::new(Vec::new()));
        let capture = sink.clone();
        let drain = thread::spawn(move || {
            use std::io::Read as _;
            if let Ok((mut stream, _)) = listener.accept() {
                let mut chunk = [0u8; 1024];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if let Ok(mut guard) = capture.lock() {
                                guard.extend_from_slice(&chunk[..read]);
                            }
                        }
                    }
                }
            }
        });
        let conn = Connection::connect("127.0.0.1", port, Duration::from_secs(2))
            .expect("loopback connection");
        (
            Harness {
                shared: test_shared(),
                conn,
                _drain: drain,
            },
            sink,
        )
    }

    /// Wait until the captured byte stream contains `needle`.
    fn captured_contains(sink: &Arc<Mutex<Vec<u8>>>, needle: &[u8]) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(guard) = sink.lock() {
                if guard.windows(needle.len()).any(|window| window == needle) {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Compose the session's frame into raw RGBA plus its bitmap width.
    fn composed_rgba(state: &SessionState, shared: &Arc<Shared>) -> (Vec<u8>, u32) {
        let mut builder = SceneBuilder::new(MediaStore::new(&[]), PropStore::new());
        let screen =
            compose(state, &mut builder, shared, Instant::now()).expect("a frame composes");
        let png = shared.frames.png().expect("the frame store holds a PNG");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("png info");
        let mut buf = vec![0u8; reader.output_buffer_size().expect("png buffer size")];
        reader.next_frame(&mut buf).expect("png frame");
        (buf, screen.geometry.bitmap_w)
    }

    fn rgba_pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y as usize) * (width as usize) + x as usize) * 4;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    /// One raw `DRAW` record, the same layout the server sends: a 10-byte
    /// header then a `PATH` operand with a red PC5 tail. `points` are `(v, h)` =
    /// `(y, x)`, first absolute and the rest deltas.
    fn draw_body(command: u8, flags: u8, points: &[(i16, i16)]) -> Vec<u8> {
        let mut operand: Vec<u8> = Vec::new();
        operand.extend_from_slice(&1i16.to_le_bytes());
        operand.extend_from_slice(&(points.len().saturating_sub(1) as i16).to_le_bytes());
        operand.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        for (v, h) in points {
            operand.extend_from_slice(&v.to_le_bytes());
            operand.extend_from_slice(&h.to_le_bytes());
        }
        operand.extend_from_slice(&[255, 255, 0, 0]);
        operand.extend_from_slice(&[255, 255, 0, 0]);

        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&(u16::from(command) | (u16::from(flags) << 8)).to_le_bytes());
        bytes.extend_from_slice(&(operand.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&10i16.to_le_bytes());
        bytes.extend_from_slice(&operand);
        bytes
    }

    fn draw_frame(command: u8, flags: u8, points: &[(i16, i16)]) -> Frame {
        Frame::new(opcode::DRAW, 0, draw_body(command, flags, points))
    }

    /// A pen context with a red 1px pen on the back layer.
    fn pen_context() -> WireContext {
        WireContext {
            byte_order: ByteOrder::Little,
            user_id: HARNESS_SELF,
            room_id: i32::from(HARNESS_ROOM),
            room_width: 512,
            room_height: 384,
            self_pos: (0, 0),
            pen: PenState {
                pos: (0, 0),
                rgb: (255, 0, 0),
                size: 1,
                front: false,
            },
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];

    #[test]
    fn a_received_draw_message_paints_the_stroke_into_the_composed_frame() {
        let mut state = state_in(&scripted_room(&[]));
        // Move the self avatar out of the way: this stroke is a back-layer one,
        // so an avatar over it would (correctly) hide it.
        if let Some(me) = state.users.get_mut(&HARNESS_SELF) {
            me.x = 400;
            me.y = 300;
        }
        let harness = harness();
        let (before, width) = composed_rgba(&state, &harness.shared);

        let applied = state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert!(
            applied.render,
            "a received DRAW asks the runtime to recompose"
        );

        let (after, _) = composed_rgba(&state, &harness.shared);
        assert_eq!(
            rgba_pixel(&after, width, 10, 5),
            RED,
            "the stroke lies on y=5, x in 5..=15"
        );
        assert_ne!(
            rgba_pixel(&after, width, 10, 5),
            rgba_pixel(&before, width, 10, 5),
            "the composed frame changed where the stroke was painted"
        );
    }

    #[test]
    fn our_own_line_paints_our_frame_and_still_goes_on_the_wire() {
        let mut state = state_in(&scripted_room(&[]));
        if let Some(me) = state.users.get_mut(&HARNESS_SELF) {
            me.x = 400;
            me.y = 300;
        }
        let (mut harness, sink) = harness_capturing();
        let context = pen_context();

        let mut dirty = false;
        let mut follow = Vec::new();
        let events = apply_effect(
            &Effect::DrawLine {
                x1: 5,
                y1: 5,
                x2: 10,
                y2: 0,
            },
            &context,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty,
            &mut follow,
        );
        assert!(events.is_empty(), "a stroke is not a reported event");
        assert!(dirty, "our own LINE marks the frame dirty");
        assert_eq!(state.draw.back().len(), 1, "it appends to the back layer");

        assert!(
            captured_contains(&sink, &opcode::DRAW.value().to_le_bytes()),
            "the stroke must still reach the server as a DRAW frame"
        );

        let (rgba, width) = composed_rgba(&state, &harness.shared);
        assert_eq!(
            rgba_pixel(&rgba, width, 10, 5),
            RED,
            "the same stroke is on our own frame"
        );
    }

    #[test]
    fn a_front_layer_stroke_paints_over_an_avatar_while_a_back_layer_one_does_not() {
        // The self avatar clamps to (22,22), so its 44x44 body covers (44,44).
        let place_avatar = |state: &mut SessionState| {
            if let Some(me) = state.users.get_mut(&HARNESS_SELF) {
                me.x = 10;
                me.y = 5;
            }
        };

        let mut back = state_in(&scripted_room(&[]));
        place_avatar(&mut back);
        back.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(44, 44)]),
            ByteOrder::Little,
        );
        let harness = harness();
        let (back_rgba, width) = composed_rgba(&back, &harness.shared);
        assert_ne!(
            rgba_pixel(&back_rgba, width, 44, 44),
            RED,
            "a back-layer stroke is painted under the avatars"
        );

        let mut front = state_in(&scripted_room(&[]));
        place_avatar(&mut front);
        front.apply(
            &draw_frame(
                palace_room::draw_cmd::PATH,
                palace_room::draw_flags::LAYER_FRONT,
                &[(44, 44)],
            ),
            ByteOrder::Little,
        );
        let (front_rgba, _) = composed_rgba(&front, &harness.shared);
        assert_eq!(
            rgba_pixel(&front_rgba, width, 44, 44),
            RED,
            "a front-layer stroke is painted over the avatars"
        );
    }

    #[test]
    fn paintundo_removes_the_last_stroke_from_the_frame() {
        let mut state = state_in(&scripted_room(&[]));
        if let Some(me) = state.users.get_mut(&HARNESS_SELF) {
            me.x = 400;
            me.y = 300;
        }
        let mut harness = harness();
        let (blank, width) = composed_rgba(&state, &harness.shared);

        let context = pen_context();
        let mut dirty = false;
        let mut follow = Vec::new();
        apply_effect(
            &Effect::DrawLine {
                x1: 5,
                y1: 5,
                x2: 10,
                y2: 0,
            },
            &context,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty,
            &mut follow,
        );
        let (with_stroke, _) = composed_rgba(&state, &harness.shared);
        assert_eq!(rgba_pixel(&with_stroke, width, 10, 5), RED);

        dirty = false;
        let events = apply_effect(
            &Effect::PaintUndo,
            &context,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty,
            &mut follow,
        );
        assert!(dirty, "PAINTUNDO must mark the frame dirty, not no-op");
        assert_eq!(state.draw.len(), 0, "the stroke was undone");
        assert!(
            events.iter().any(
                |event| matches!(event, ClientEvent::Note { text } if text.contains("PAINTUNDO"))
            ),
            "the undo is still reported: {events:?}"
        );

        let (after, _) = composed_rgba(&state, &harness.shared);
        assert_eq!(
            rgba_pixel(&after, width, 10, 5),
            rgba_pixel(&blank, width, 10, 5),
            "the frame is back to the blank room"
        );
        assert_ne!(
            rgba_pixel(&after, width, 10, 5),
            RED,
            "the undone stroke is gone from the frame"
        );
    }

    #[test]
    fn a_door_lock_frame_runs_only_the_named_hotspots_lock_handler() {
        let room = scripted_room(&[
            (7, "ON LOCK { \"lock-7\" STATUSMSG }"),
            (8, "ON LOCK { \"lock-8\" STATUSMSG }"),
        ]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);
        let mut harness = harness();

        let events = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::DOORLOCK, 0, door_body(HARNESS_ROOM, 7)),
        );

        let notes = note_texts(&events);
        assert_eq!(fired(&events, "LOCK"), 1, "one ON LOCK ran: {notes:?}");
        assert!(
            notes.iter().any(|note| note.contains("lock-7")),
            "the handler for hotspot 7 ran: {notes:?}"
        );
        assert!(
            !notes.iter().any(|note| note.contains("lock-8")),
            "hotspot 8's handler is out of scope: {notes:?}"
        );
    }

    #[test]
    fn a_door_unlock_frame_runs_only_the_named_hotspots_unlock_handler() {
        let room = scripted_room(&[
            (7, "ON UNLOCK { \"unlock-7\" STATUSMSG }"),
            (8, "ON UNLOCK { \"unlock-8\" STATUSMSG }"),
        ]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);
        let mut harness = harness();

        let events = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::DOORUNLOCK, 0, door_body(HARNESS_ROOM, 7)),
        );

        let notes = note_texts(&events);
        assert_eq!(fired(&events, "UNLOCK"), 1, "one ON UNLOCK ran: {notes:?}");
        assert!(
            notes.iter().any(|note| note.contains("unlock-7")),
            "the handler for hotspot 7 ran: {notes:?}"
        );
        assert!(
            !notes.iter().any(|note| note.contains("unlock-8")),
            "hotspot 8's handler is out of scope: {notes:?}"
        );
    }

    #[test]
    fn a_spot_state_frame_runs_only_the_changed_hotspots_statechange_handler() {
        let room = scripted_room(&[
            (7, "ON STATECHANGE { \"state-7\" STATUSMSG }"),
            (8, "ON STATECHANGE { \"state-8\" STATUSMSG }"),
        ]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);
        let mut harness = harness();

        let events = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::SPOTSTATE, 0, spot_state_body(HARNESS_ROOM, 7, 1)),
        );

        let notes = note_texts(&events);
        assert_eq!(
            fired(&events, "STATECHANGE"),
            1,
            "one ON STATECHANGE ran: {notes:?}"
        );
        assert!(
            notes.iter().any(|note| note.contains("state-7")),
            "the handler for hotspot 7 ran: {notes:?}"
        );
        assert!(
            !notes.iter().any(|note| note.contains("state-8")),
            "hotspot 8's handler is out of scope: {notes:?}"
        );
        let hotspot = state
            .room_desc
            .as_ref()
            .expect("a room")
            .hotspots
            .iter()
            .find(|hotspot| hotspot.id == 7)
            .expect("hotspot 7");
        assert_eq!(
            hotspot.state, 1,
            "the handler ran after the model took the new state"
        );
    }

    #[test]
    fn a_name_change_runs_the_rooms_namechange_handler() {
        let room = scripted_room(&[(7, "ON NAMECHANGE { \"namechange-ran\" STATUSMSG }")]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);
        add_other_user(&mut state, 21);
        let mut harness = harness();

        let events = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::USERNAME, 21, name_body("Rico")),
        );

        let notes = note_texts(&events);
        assert_eq!(
            fired(&events, "NAMECHANGE"),
            1,
            "the room's ON NAMECHANGE ran: {notes:?}"
        );
        assert!(
            notes.iter().any(|note| note.contains("namechange-ran")),
            "the handler body ran: {notes:?}"
        );
    }

    #[test]
    fn a_user_exit_runs_the_rooms_userleave_handler_but_not_for_us() {
        let room = scripted_room(&[(7, "ON USERLEAVE { \"userleave-ran\" STATUSMSG }")]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);
        add_other_user(&mut state, 21);
        let mut harness = harness();

        let events = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::USEREXIT, 21, Vec::new()),
        );
        let notes = note_texts(&events);
        assert_eq!(
            fired(&events, "USERLEAVE"),
            1,
            "the room's ON USERLEAVE ran: {notes:?}"
        );
        assert!(
            notes.iter().any(|note| note.contains("userleave-ran")),
            "the handler body ran: {notes:?}"
        );

        let own_exit = dispatch_from_frame(
            &mut scripts,
            &mut state,
            &mut harness,
            &Frame::new(opcode::USEREXIT, HARNESS_SELF, Vec::new()),
        );
        assert_eq!(
            fired(&own_exit, "USERLEAVE"),
            0,
            "our own exit must not run ON USERLEAVE: {:?}",
            note_texts(&own_exit)
        );
    }

    fn room_frame(fixture: &str) -> Frame {
        let body = match fixture {
            "86" => include_bytes!("../../../fixtures/rooms/86.bin").to_vec(),
            "887" => include_bytes!("../../../fixtures/rooms/887.bin").to_vec(),
            other => panic!("no room fixture named {other}"),
        };
        Frame::new(opcode::ROOMDESC, 0, body)
    }

    fn script_events(events: &[ClientEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Script { event, fired, .. } if *fired > 0 => Some(event.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Mirror the session loop's arrival order: decode the frame (which records
    /// the lifecycle events), load the arrived room's scripts, then dispatch.
    fn arrive_from_frame(
        scripts: &mut ScriptEngine,
        state: &mut SessionState,
        harness: &mut Harness,
        frame: &Frame,
    ) -> Vec<ClientEvent> {
        let applied = state.apply(frame, ByteOrder::Little);
        if applied.room_entered {
            if let Some(desc) = state.room_desc.clone() {
                scripts.load_room(&desc);
            }
        }
        let mut dirty_render = false;
        dispatch_scripts(
            scripts,
            &applied.scripts,
            state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        )
    }

    #[test]
    fn a_room_arrival_fires_roomload_enter_and_roomready_in_order() {
        let room = scripted_room(&[(
            7,
            "ON ROOMLOAD { \"life-roomload\" STATUSMSG } \
             ON ENTER { \"life-enter\" STATUSMSG } \
             ON ROOMREADY { \"life-roomready\" STATUSMSG }",
        )]);
        let mut scripts = ScriptEngine::with_palace_limits();
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = HARNESS_SELF;
        let mut harness = harness();

        let applied = state.apply(&room_frame("86"), ByteOrder::Little);
        assert!(applied.room_entered);
        // The fixture carries no lifecycle handlers, so attach observable ones
        // to the room this arrival loads.
        state.room_desc = Some(room.clone());
        scripts.load_room(&room);
        let mut dirty_render = false;
        let events = dispatch_scripts(
            &mut scripts,
            &applied.scripts,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        );

        assert_eq!(
            script_events(&events),
            ["ROOMLOAD", "ENTER", "ROOMREADY"],
            "the lifecycle runs in reference order"
        );
        let notes = note_texts(&events);
        for expected in ["life-roomload", "life-enter", "life-roomready"] {
            assert!(
                notes.iter().any(|note| note.contains(expected)),
                "{expected} ran: {notes:?}"
            );
        }
    }

    #[test]
    fn a_roomready_handler_runs_with_the_rooms_hotspots_loaded() {
        let mut room = scripted_room(&[(7, "ON ROOMREADY { 7 SPOTNAME STATUSMSG }")]);
        room.hotspots[0].name = Some("ready-hotspot".to_string());

        let mut state = SessionState::new("test", 1);
        state.banner.user_id = HARNESS_SELF;
        let mut harness = harness();
        let applied = state.apply(&room_frame("86"), ByteOrder::Little);
        assert!(applied.room_entered);
        state.room_desc = Some(room.clone());

        let mut dirty_render = false;
        let unloaded = dispatch_scripts(
            &mut ScriptEngine::with_palace_limits(),
            &applied.scripts,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        );
        assert_eq!(
            fired(&unloaded, "ROOMREADY"),
            0,
            "a ROOMREADY handler cannot run before the room is loaded"
        );

        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let after = dispatch_scripts(
            &mut scripts,
            &applied.scripts,
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        );
        assert_eq!(
            fired(&after, "ROOMREADY"),
            1,
            "the loaded room's handler runs"
        );
        assert!(
            note_texts(&after)
                .iter()
                .any(|note| note.contains("ready-hotspot")),
            "the handler read the loaded hotspot: {:?}",
            note_texts(&after)
        );
    }

    #[test]
    fn a_second_arrival_runs_the_lifecycle_again_and_a_redescription_does_not() {
        let mut scripts = ScriptEngine::with_palace_limits();
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = HARNESS_SELF;
        let mut harness = harness();

        let first = arrive_from_frame(&mut scripts, &mut state, &mut harness, &room_frame("86"));
        assert!(
            fired(&first, "ENTER") > 0,
            "the first arrival runs ON ENTER: {:?}",
            note_texts(&first)
        );

        let again = arrive_from_frame(&mut scripts, &mut state, &mut harness, &room_frame("86"));
        assert_eq!(
            fired(&again, "ENTER"),
            0,
            "re-describing the same room does not re-run the lifecycle"
        );

        state.begin_room_change();
        let second = arrive_from_frame(&mut scripts, &mut state, &mut harness, &room_frame("887"));
        assert!(
            fired(&second, "ENTER") > 0,
            "a second room runs the lifecycle again: {:?}",
            note_texts(&second)
        );
    }

    #[test]
    fn the_corpus_roomready_handler_parses_and_runs() {
        let room = scripted_room(&[(7, "ON ROOMREADY {\"ludo/\"HTTPGET}")]);
        let mut scripts = ScriptEngine::with_palace_limits();
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = HARNESS_SELF;
        let mut harness = harness();

        scripts.load_room(&room);
        assert!(
            scripts.has_handler(ScriptEvent::RoomReady),
            "the corpus handler parses even though HTTPGET is not implemented"
        );

        let mut dirty_render = false;
        let events = dispatch_scripts(
            &mut scripts,
            &[ScriptStimulus {
                event: ScriptEvent::RoomReady,
                spot: None,
            }],
            &mut state,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        );
        assert_eq!(
            fired(&events, "ROOMREADY"),
            1,
            "the corpus handler runs on arrival"
        );
    }

    // ---------------------------------------------- LOADSCRIPT / HTTPGET dispatch

    fn fetch_outcome(
        scripts: &mut ScriptEngine,
        state: &mut SessionState,
        outcome: ScriptOutcome,
    ) -> Vec<ClientEvent> {
        let mut harness = harness();
        let mut dirty_render = false;
        deliver_script_outcome(
            scripts,
            state,
            outcome,
            &harness.shared,
            &mut harness.conn,
            &mut dirty_render,
        )
    }

    #[test]
    fn a_script_typed_response_is_executed_and_dispatched_at_its_spot() {
        let room = scripted_room(&[(9, "ON HTTPRECEIVED { \"ran\" SAY }")]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);

        let events = fetch_outcome(
            &mut scripts,
            &mut state,
            ScriptOutcome::Response {
                requested: "custo2.txt".to_string(),
                url: "http://media.example/custo2.txt".to_string(),
                spot: 9,
                body: b"\"body\" SAY".to_vec(),
                content_type: Some("text/iptscrae".to_string()),
            },
        );
        assert_eq!(fired(&events, "HTTPRECEIVED"), 1);
        assert_eq!(fired(&events, "HTTPERROR"), 0);
    }

    #[test]
    fn a_non_script_response_is_not_executed() {
        let room = scripted_room(&[(9, "ON HTTPRECEIVED { \"ran\" SAY }")]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);

        let events = fetch_outcome(
            &mut scripts,
            &mut state,
            ScriptOutcome::Response {
                requested: "custo2.txt".to_string(),
                url: "http://media.example/custo2.txt".to_string(),
                spot: 9,
                body: b"}{ hostile garbage {{".to_vec(),
                content_type: Some("text/plain".to_string()),
            },
        );
        assert_eq!(
            fired(&events, "HTTPRECEIVED"),
            0,
            "a text/plain body is data, not a script"
        );
    }

    #[test]
    fn a_failed_fetch_is_reported_and_fires_httperror() {
        let room = scripted_room(&[
            (9, "ON HTTPERROR { \"error-path\" SAY }"),
            (9, "ON HTTPRECEIVED { \"wrong-path\" SAY }"),
        ]);
        let mut scripts = ScriptEngine::with_palace_limits();
        scripts.load_room(&room);
        let mut state = state_in(&room);

        let events = fetch_outcome(
            &mut scripts,
            &mut state,
            ScriptOutcome::Failure {
                requested: "custo2.txt".to_string(),
                url: "http://media.example/custo2.txt".to_string(),
                spot: 9,
                reason: "HTTP 404".to_string(),
            },
        );
        assert_eq!(fired(&events, "HTTPERROR"), 1, "the error handler runs");
        assert_eq!(fired(&events, "HTTPRECEIVED"), 0, "no received dispatch");
        let notes = note_texts(&events);
        assert!(
            notes
                .iter()
                .any(|text| text.contains("custo2.txt") && text.contains("404")),
            "the failure is reported with the URL and reason: {notes:?}"
        );
    }
}

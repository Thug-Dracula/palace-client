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

use palace_asset::{AssetPipeline, AssetType, PipelineEvent};
use palace_host::{
    effect_frame, Effect, HostView, PenState, ScriptEngine, ScriptEvent, UserView, WireContext,
};
use palace_render::{
    clamp_dpr, render, AnimationClock, AvatarSpec, MediaStore, PointF, PropStore, RenderOptions,
    SceneBuilder, SizeF, ViewTransform, COLOR_VARIANTS, FACE_VARIANTS,
};
use palace_room::{LooseProp, LoosePropSpec};
use palace_wire::byteorder::Writer;
use palace_wire::frame::Frame;
use palace_wire::messages::{reference_logon_record, Point, Talk};
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
    ChatKind, ChatLine, ConnectionStatus, RoomInfo, ServerBanner, SessionState, UserInfo,
};

const MEDIA_REQUEST_INTERVAL: Duration = Duration::from_secs(2);
const PROP_REQUEST_BUDGET: usize = 80;

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
        let cache_root = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("palace-client");
        ClientConfig {
            host: "localhost".to_string(),
            port: 9998,
            username: "Guest".to_string(),
            desired_room: 0,
            cache_root,
            seed_media: Vec::new(),
            seed_props: Vec::new(),
        }
    }
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
    let session_root = cfg.cache_root.join(format!("{}:{}", cfg.host, cfg.port));
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

    let mut conn = Connection::connect(&cfg.host, cfg.port, Duration::from_secs(8))?;
    let handshake = conn.handshake(Duration::from_secs(12))?;
    let order = handshake.byte_order;
    let user_id = handshake.user_id();
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
            if let Some(handle) = media_thread {
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
                    let view = host_view(&state, shared);
                    match view.spot_at(rx, ry).map(|spot| spot.id) {
                        Some(id) => {
                            shared.note(format!(
                                "script: click at room ({rx},{ry}) hit hotspot {id}"
                            ));
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
                        None => {
                            shared.note(format!("script: click at room ({rx},{ry}) hit no hotspot"))
                        }
                    }
                }
                None => shared.note("script: click ignored, the room view is not ready"),
            }
        }
        if let Some(source) = run_source_pending.take() {
            match scripts.run_source(&source) {
                Ok(run) => {
                    shared.note(format!(
                        "script: ran {} instruction(s) from the input box",
                        run.steps
                    ));
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
                        media_base = Some(url);
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
                            for event in run_dispatch(
                                &mut scripts,
                                ScriptEvent::Enter,
                                &mut state,
                                shared,
                                &mut conn,
                                &mut dirty_render,
                                None,
                            ) {
                                shared.emit(event);
                            }
                        }
                        let wanted = shared.last_room.lock().ok().and_then(|guard| *guard);
                        if !restored_room {
                            restored_room = true;
                            if let Some(target) = wanted {
                                if target != room.id {
                                    state.begin_room_change();
                                    conn.send(&state.navigate_frame(target))?;
                                    dirty_render = true;
                                }
                            }
                        }
                    }
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
                if let Some(handle) = media_thread {
                    let _ = handle.join();
                }
                return Ok(false);
            }
            Err(e) => {
                drop(media_tx);
                if let Some(handle) = media_thread {
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
            Some(
                AvatarSpec::new(i32::from(user.x), i32::from(user.y), user.props.clone())
                    .with_face_color(user.face, user.color),
            )
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
    if let Some(frame) = effect_frame(effect, context) {
        let _ = conn.send(&frame);
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
            let applied = set_local_spot_state(state, *spot, *value);
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
        Effect::MoveUserAbs { x, y } | Effect::MoveUserRel { dx: x, dy: y } => {
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!("script: moved to ({x},{y})"),
            }]
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
        Effect::PaintClear | Effect::PaintUndo => {
            *dirty_render = true;
            vec![ClientEvent::Note {
                text: format!("script: {}", effect.command()),
            }]
        }
        Effect::DrawLine { .. } | Effect::DrawLineRel { .. } => Vec::new(),
        Effect::SetProps { props } => {
            if let Some(user) = state.users.get_mut(&state.banner.user_id) {
                user.props = props.iter().map(|p| *p as u32).collect();
            }
            *dirty_render = true;
            Vec::new()
        }
        Effect::Naked => {
            if let Some(user) = state.users.get_mut(&state.banner.user_id) {
                user.props.clear();
            }
            *dirty_render = true;
            Vec::new()
        }
        Effect::SetUserName { name } => {
            if let Some(user) = state.users.get_mut(&state.banner.user_id) {
                user.name = name.clone();
            }
            vec![ClientEvent::Note {
                text: format!("script: SETUSERNAME {name:?} (local only)"),
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
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: DONPROP {prop}{}", no_change_suffix(changed)),
            }]
        }
        Effect::DoffProp => {
            let changed = doff_prop(state).is_some();
            *dirty_render |= changed;
            vec![ClientEvent::Note {
                text: format!("script: DOFFPROP{}", no_change_suffix(changed)),
            }]
        }
        Effect::RemoveProp { prop } => {
            let changed = prop_id(*prop).is_some_and(|id| remove_worn_prop(state, id));
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
        // Nothing rasterizes a stroke yet (`LINE` and `LINETO` are dispatched and
        // dropped), so there is no local pen state to keep in step with.
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
        // occupants see it. This client keeps no lock state of its own — nothing
        // consults it, because a click does not yet refuse to open a locked door.
        Effect::Lock { spot } => vec![ClientEvent::Note {
            text: format!("script: LOCK {spot}"),
        }],
        Effect::Unlock { spot } => vec![ClientEvent::Note {
            text: format!("script: UNLOCK {spot}"),
        }],
        // The alarm is already scheduled: `ScriptHost` records it and the engine
        // runs the handler when it falls due.
        Effect::SetSpotAlarm { spot, ticks } => vec![ClientEvent::Note {
            text: format!("script: SETALARM spot {spot} in {ticks} ticks"),
        }],
    }
}

/// Change a hotspot's state in the locally rendered room.
fn set_local_spot_state(state: &mut SessionState, spot: i32, value: i32) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    let Some(hotspot) = room
        .hotspots
        .iter_mut()
        .find(|hotspot| i32::from(hotspot.id) == spot)
    else {
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

/// `(x, y)` as a wire `Point`, which stores `(v, h)` = `(y, x)`.
fn point(x: i32, y: i32) -> Point {
    Point::new(y as i16, x as i16)
}

/// Change the signed-in user's face cell, following OpenPalace's model rule:
/// floor at 0, and anything past the last defined face becomes face 0.
fn set_self_face(state: &mut SessionState, face: i32) -> bool {
    let face = if face > i32::from(FACE_VARIANTS - 1) {
        0
    } else {
        face.max(0)
    } as i16;
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let changed = user.face != face;
    user.face = face;
    changed
}

fn set_self_color(state: &mut SessionState, color: i32) -> bool {
    let color = color.clamp(0, i32::from(COLOR_VARIANTS - 1)) as i16;
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let changed = user.color != color;
    user.color = color;
    changed
}

/// Wear one more prop, appending it to the worn list. A prop already worn, or a
/// tenth prop, is refused rather than evicting an older one.
fn wear_prop(state: &mut SessionState, prop: u32) -> bool {
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    if user.props.len() >= MAX_WORN_PROPS || user.props.contains(&prop) {
        return false;
    }
    user.props.push(prop);
    true
}

fn doff_prop(state: &mut SessionState) -> Option<u32> {
    state
        .users
        .get_mut(&state.banner.user_id)
        .and_then(|user| user.props.pop())
}

fn remove_worn_prop(state: &mut SessionState, prop: u32) -> bool {
    let Some(user) = state.users.get_mut(&state.banner.user_id) else {
        return false;
    };
    let Some(at) = user.props.iter().position(|worn| *worn == prop) else {
        return false;
    };
    user.props.remove(at);
    true
}

/// Put a prop on the floor. Only the id and the position reach the wire, so every
/// other field of the record is zero.
fn add_loose_prop(state: &mut SessionState, prop: u32, x: i32, y: i32) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    room.loose_props.push(LooseProp {
        next_ofst: 0,
        reserved: 0,
        spec: LoosePropSpec { id: prop, crc: 0 },
        flags: 0,
        ref_con: 0,
        loc: point(x, y),
    });
    true
}

/// Drop one loose prop by index, or every one when the index is `-1`.
fn remove_loose_prop(state: &mut SessionState, index: i32) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    if index == -1 {
        let changed = !room.loose_props.is_empty();
        room.loose_props.clear();
        return changed;
    }
    let Ok(index) = usize::try_from(index) else {
        return false;
    };
    if index >= room.loose_props.len() {
        return false;
    }
    room.loose_props.remove(index);
    true
}

fn move_loose_prop(state: &mut SessionState, index: i32, x: i32, y: i32) -> bool {
    let Some(room) = state.room_desc.as_mut() else {
        return false;
    };
    let Ok(index) = usize::try_from(index) else {
        return false;
    };
    let Some(prop) = room.loose_props.get_mut(index) else {
        return false;
    };
    prop.loc = point(x, y);
    true
}

fn drop_prop(state: &mut SessionState, x: i32, y: i32) -> Option<u32> {
    let prop = *state.users.get(&state.banner.user_id)?.props.last()?;
    if !add_loose_prop(state, prop, x, y) {
        return None;
    }
    let _ = remove_worn_prop(state, prop);
    Some(prop)
}

fn hotspot_mut(state: &mut SessionState, spot: i32) -> Option<&mut palace_room::Hotspot> {
    state
        .room_desc
        .as_mut()?
        .hotspots
        .iter_mut()
        .find(|hotspot| i32::from(hotspot.id) == spot)
}

/// `SETLOC` / `SETLOCLOCAL`: `x y` are the spot's new absolute position, so this
/// replaces `loc`. `MSG_SPOTMOVE` carries a position rather than a delta, the
/// reference server assigns it, and OpenPalace changed its own implementation
/// from relative to absolute to match.
fn move_spot_to(state: &mut SessionState, spot: i32, x: i32, y: i32) -> bool {
    let Some(hotspot) = hotspot_mut(state, spot) else {
        return false;
    };
    hotspot.loc = point(x, y);
    true
}

/// `SETPICLOC` / `SETPICLOCLOCAL`: the absolute picture offset of a state, or of
/// the hotspot's current state when `index` is `None`. The manual's worked
/// example proves these replace the offset rather than adding to it.
fn set_pic_offset(state: &mut SessionState, spot: i32, index: Option<i32>, x: i32, y: i32) -> bool {
    let Some(hotspot) = hotspot_mut(state, spot) else {
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
}

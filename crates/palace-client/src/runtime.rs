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
use palace_render::{
    clamp_dpr, render, AnimationClock, AvatarSpec, MediaStore, PropStore, RenderOptions,
    SceneBuilder, SizeF,
};
use palace_wire::byteorder::Writer;
use palace_wire::frame::Frame;
use palace_wire::messages::{reference_logon_record, Talk};
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

fn run_session(shared: &Arc<Shared>, cmd_rx: &mut UnboundedReceiver<ClientCommand>) -> Result<bool> {
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
            state.begin_room_change();
            conn.send(&state.navigate_frame(room_id))?;
            dirty_render = true;
        }
        if let Some(text) = say {
            let mut writer = Writer::new(order);
            Talk { user_id, text }.encode(&mut writer);
            conn.send(&Frame::new(opcode::TALK, user_id, writer.into_vec()))?;
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
                        frame.printable_payload().chars().take(60).collect::<String>()
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
                for line in applied.chat {
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
            if let Some(screen) = compose(&state, &builder, shared, start) {
                last_room_size = Some((screen.geometry.room_w, screen.geometry.room_h));
                shared.emit(ClientEvent::Screen {
                    screen: screen.clone(),
                });
                last_screen = Some(screen);
            }
            dirty_render = false;
            dirty_geom = false;
        } else if dirty_geom {
            if let (Some(previous), Some((room_w, room_h))) = (last_screen.clone(), last_room_size) {
                let viewport = shared.viewport();
                let mut updated = previous;
                updated.geometry = ViewGeometry::compute(
                    SizeF::new(room_w, room_h),
                    SizeF::new(viewport.width, viewport.height),
                    viewport.zoom,
                    viewport.native,
                    clamp_dpr(viewport.dpr),
                );
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
    builder: &SceneBuilder,
    shared: &Arc<Shared>,
    start: Instant,
) -> Option<ScreenState> {
    let live = state.room_desc.as_ref()?;
    let mut room = live.clone();

    let loose_ids: Vec<u32> = room.loose_props.iter().map(|p| p.spec.id).collect();
    let props_pending = missing_props(builder.props(), &loose_ids).len();
    room.loose_props
        .retain(|prop| builder.props().contains(prop.spec.id));

    let mut hidden_avatars = 0usize;
    let avatars: Vec<AvatarSpec> = state
        .users_in_room()
        .iter()
        .filter_map(|user| {
            if user.props.is_empty() {
                return None;
            }
            if !user.props.iter().any(|id| builder.props().contains(*id)) {
                hidden_avatars += 1;
                return None;
            }
            Some(AvatarSpec::new(
                i32::from(user.x),
                i32::from(user.y),
                user.props.clone(),
            ))
        })
        .collect();

    let scene = builder.build_with(&room, &avatars, &[]);
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

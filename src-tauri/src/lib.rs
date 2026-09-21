//! Palace desktop client: the Tauri shell over the headless runtime.
//!
//! The window is deliberately thin — it displays one pre-composited PNG per
//! frame, draws the chrome around it, and forwards input. All protocol,
//! asset and compositing work lives in `palace-client` on worker threads.

pub mod bag;
pub mod chat_log;
pub mod commands;
pub mod editor;
pub mod geometry;
pub mod logging;
pub mod notify;
pub mod prefs;
pub mod protocol;
pub mod settings;
pub mod windows;

pub use settings::Settings;

use logging::Level;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use palace_audio::{AudioConfig, AudioEngine, AudioHandle};
use palace_client::{
    ClientConfig, ClientEvent, ClientEventStream, ClientHandle, ClientIdentity, ClientRuntime,
};
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};

/// Shared app state: the running client and the settings it was built from.
pub struct AppState {
    pub client: Mutex<Option<ClientHandle>>,
    pub settings: Mutex<Settings>,
    pub audio: Mutex<AudioEngine>,
    /// The vendored font `settings.soundfont` falls back to when it is `None`.
    pub bundled_soundfont: Option<PathBuf>,
    /// The prop bag folder and the operations the shell exposes over it.
    pub bag: bag::BagService,
    /// The epoch of the most recent `refresh` request.
    ///
    /// Bumped once per request and stamped on every replayed event, so the log
    /// can tell N windows seeding once each (N epochs) from one window
    /// re-seeding in a loop (many events under one epoch).
    pub refresh_epoch: AtomicU64,
}

/// Split a search path using the platform's own separator.
///
/// `std::env::split_paths` uses `;` on Windows and `:` on Unix. Splitting on `:`
/// ourselves broke Windows drive letters: `C:\props` came out as `C` and
/// `\props`, neither of which is a directory, so the seeds silently vanished.
fn split_paths(value: Option<String>) -> Vec<PathBuf> {
    value
        .map(|list| {
            std::env::split_paths(&list)
                .filter(|part| !part.as_os_str().is_empty())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

fn seed_media() -> Vec<PathBuf> {
    if let Some(explicit) = std::env::var_os("PALACE_SEED_MEDIA") {
        return split_paths(Some(explicit.to_string_lossy().into_owned()));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    [
        home.join("colosseum/http_harvest"),
        home.join("media/colosseum-bgs"),
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .collect()
}

fn seed_props() -> Vec<PathBuf> {
    split_paths(std::env::var_os("PALACE_SEED_PROPS").map(|v| v.to_string_lossy().into_owned()))
}

/// Build a config from settings plus the optional local read-only seeds.
#[must_use]
pub fn config_for(settings: &Settings) -> ClientConfig {
    ClientConfig {
        host: settings.host.clone(),
        port: settings.port,
        username: settings.username.clone(),
        password: settings.password.clone(),
        identity: settings
            .identity
            .map(ClientIdentity::from)
            .unwrap_or_default(),
        seed_media: seed_media(),
        seed_props: seed_props(),
        allow_avatar_upload: avatar_upload_allowed(),
        ..ClientConfig::default()
    }
}

/// Whether the user has explicitly opted into Type 1 avatar uploads.
///
/// Off unless `PALACE_ALLOW_AVATAR_UPLOAD=1`: writing an avatar to a server is
/// a deliberate act that needs the server owner's permission, so the default is
/// to validate and display locally without uploading.
#[must_use]
fn avatar_upload_allowed() -> bool {
    std::env::var_os("PALACE_ALLOW_AVATAR_UPLOAD").is_some_and(|value| value == "1")
}

/// The SoundFont vendored into the bundle, relative to the resource directory.
pub const BUNDLED_SOUNDFONT: &str = "resources/soundfonts/GeneralUser-GS.sf2";

/// Locate the vendored SoundFont inside the installed bundle.
///
/// Returns `None` outside a bundled layout (a bare `cargo test`, or a dev run
/// whose resources were not copied) and when the file is absent, which leaves
/// the engine on its fallback tone exactly as before the font was vendored.
#[must_use]
pub fn bundled_soundfont(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .resolve(BUNDLED_SOUNDFONT, BaseDirectory::Resource)
        .ok()
        .filter(|path| path.is_file())
}

/// The audio engine config the shell runs with.
///
/// The shell, unlike a test harness, wants sound, so it selects
/// [`AudioConfig::desktop`]; the library default stays silent. When the
/// settings name no font, `default_soundfont` — the vendored bundle — supplies
/// one, so MIDI is audible out of the box while a saved choice still wins.
#[must_use]
pub fn audio_config_for(settings: &Settings, default_soundfont: Option<&Path>) -> AudioConfig {
    AudioConfig {
        soundfont: settings
            .soundfont
            .clone()
            .or_else(|| default_soundfont.map(Path::to_path_buf)),
        enabled: settings.audio_enabled,
        volume: settings.audio_volume,
        ..AudioConfig::desktop()
    }
}

/// The audio config the desktop shell actually spawns.
///
/// Like [`audio_config_for`], except that a saved SoundFont which is no longer
/// on disk is replaced by the bundled bank at this point, with a warning, so
/// the engine never receives a dead path. Without this the engine would fail
/// to load the font and drop to its fallback tone, silently ignoring a bundled
/// bank that is sitting right there.
#[must_use]
pub fn shell_audio_config(settings: &Settings, bundled: Option<&Path>) -> AudioConfig {
    let (soundfont, substituted) =
        settings::resolve_soundfont(settings.soundfont.as_deref(), bundled);
    if substituted {
        if let Some(chosen) = settings.soundfont.as_deref() {
            settings::warn_missing_soundfont(chosen, soundfont.as_deref());
        }
    }
    AudioConfig {
        soundfont,
        enabled: settings.audio_enabled,
        volume: settings.audio_volume,
        ..AudioConfig::desktop()
    }
}

/// Mirror one runtime event into the diagnostic log.
///
/// Every diagnostic the log panel shows is a [`ClientEvent`], so logging at the
/// pump catches the script notes, connection changes, protocol errors and room
/// traffic in one place without touching the runtime. Frames and tooltips fire
/// far too often to keep at the default level; they are `debug`.
fn log_event(event: &ClientEvent) {
    let level = match event {
        ClientEvent::Screen { .. } | ClientEvent::Tooltip { .. } => Level::Debug,
        _ => Level::Info,
    };
    logging::log(level, palace_client::trace::describe_client_event(event));
}

/// Mirror one event into the chat transcript, when it is a chat line and file
/// logging is on.
///
/// Separate from the pump so the wiring can be tested without a live runtime:
/// the pump calls this for every event, and the service decides whether the
/// line is written. With file logging off this is a no-op, so the in-memory
/// transcript is unaffected.
pub fn record_chat_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: &ClientEvent) {
    if let ClientEvent::Chat { line } = event {
        if let Some(transcript) = app.try_state::<chat_log::ChatLogService>() {
            transcript.record(line);
        }
    }
}

/// Offer one chat line to the opt-in notification subsystem.
///
/// Like [`record_chat_event`], this runs in the pump rather than in a webview,
/// so one message produces at most one notification no matter how many windows
/// are open. The decision (opt-in, scope, ignore list) and the delivery both
/// live in [`notify`], which logs a failed platform call and never panics, so
/// a desktop with no notification service changes nothing for the app.
pub fn notify_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: &ClientEvent) {
    if let ClientEvent::Chat { line } = event {
        if let Some(service) = app.try_state::<notify::NotificationService>() {
            let notifier = notify::TauriNotifier::new(app.clone());
            let _ = service.deliver(&notifier, line);
        }
    }
}

/// The audio side effect one session event asks for, if any.
///
/// The sound engine is a single process-global owner (`AppState.audio`), and
/// this is the only mapping from a session event to a command on it. Because
/// the mapping runs once per event in the pump — never once per window — a
/// detached panel cannot double-fire audio no matter how many windows are open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundEffect {
    Sound(String),
    MidiPlay(String),
    MidiLoop(String, i32),
    MidiStop,
    Beep,
}

impl SoundEffect {
    /// Play this effect on the one engine.
    pub fn play(&self, audio: &AudioHandle) {
        match self {
            SoundEffect::Sound(name) => audio.play_sound(name.clone()),
            SoundEffect::MidiPlay(name) => audio.midi_play(name.clone()),
            SoundEffect::MidiLoop(name, loops) => audio.midi_loop(name.clone(), *loops),
            SoundEffect::MidiStop => audio.midi_stop(),
            SoundEffect::Beep => audio.beep(),
        }
    }
}

/// Map one session event to the audio effect it asks for, if it asks for one.
#[must_use]
pub fn sound_effect(event: &ClientEvent) -> Option<SoundEffect> {
    match event {
        ClientEvent::Sound { name } => Some(SoundEffect::Sound(name.clone())),
        ClientEvent::MidiPlay { name } => Some(SoundEffect::MidiPlay(name.clone())),
        ClientEvent::MidiLoop { name, loops } => Some(SoundEffect::MidiLoop(name.clone(), *loops)),
        ClientEvent::MidiStop => Some(SoundEffect::MidiStop),
        ClientEvent::Beep => Some(SoundEffect::Beep),
        _ => None,
    }
}

/// Handle exactly one session event: log it, mirror it, route its audio, then
/// broadcast it — except a screen, which is sent only to the window that owns
/// the room view.
///
/// Extracted from [`spawn_pump`] so the per-event behaviour can be tested
/// without a live runtime: in particular, that one sound event issues exactly
/// one audio command and a non-audio event issues none. `media_base` is the
/// pump's remembered base URL; the banner branch only forwards a change.
pub fn handle_pump_event<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    audio: &AudioHandle,
    media_base: &mut Option<String>,
    event: &ClientEvent,
) -> Result<(), tauri::Error> {
    log_event(event);
    record_chat_event(app, event);
    notify_event(app, event);
    logging::log(
        Level::Debug,
        format!("event_epoch epoch={}", refresh_epoch(app)),
    );
    if let ClientEvent::Banner { banner } = event {
        if let Some(base) = &banner.media_base {
            if media_base.as_deref() != Some(base.as_str()) {
                *media_base = Some(base.clone());
                audio.set_media_base(base.clone());
            }
        }
    } else if let Some(effect) = sound_effect(event) {
        effect.play(audio);
    }
    match event {
        ClientEvent::Screen { screen } => {
            forward_screen(app, screen);
            Ok(())
        }
        _ => app.emit(commands::EVENT_NAME, event),
    }
}

/// The epoch of the most recent `refresh` request, or 0 before any window seeds.
///
/// Read by the pump so a replayed event can be tagged with the request that
/// produced it. A missing `AppState` (a bare mock app in a unit test) reads as
/// 0, which is the same value a pre-seed event carries.
#[must_use]
pub fn refresh_epoch<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> u64 {
    app.try_state::<AppState>()
        .map_or(0, |state| state.refresh_epoch.load(Ordering::Relaxed))
}

/// Forward runtime events to the webview, routing sound effects to the engine.
pub fn spawn_pump(app: tauri::AppHandle, mut stream: ClientEventStream, audio: AudioHandle) {
    tauri::async_runtime::spawn(async move {
        let mut media_base: Option<String> = None;
        while let Some(event) = stream.recv().await {
            if handle_pump_event(&app, &audio, &mut media_base, &event).is_err() {
                break;
            }
        }
    });
}

/// Deliver a composed room screen to the one window that owns the room view.
///
/// The geometry in `screen` is computed for whichever viewport was reported
/// last, so broadcasting it would let a second window treat another window's
/// geometry as its own, re-report its own size, and fight over the one shared
/// frame. The reply is tagged with the owner label and the report epoch and is
/// sent with `emit_to`, so only that window receives it and a superseded epoch
/// can be dropped by the frontend.
///
/// Until a window has claimed the viewport there is no window the geometry
/// could belong to, so the event is dropped rather than broadcast: the owner's
/// first report makes the compositor emit a fresh screen for it.
pub fn forward_screen<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    screen: &palace_client::ScreenState,
) {
    let Some(claim) = app
        .try_state::<protocol::ViewportOwnerSlot>()
        .and_then(|slot| slot.current())
    else {
        return;
    };
    logging::log(
        Level::Debug,
        format!("geometry_emit owner={} epoch={}", claim.owner, claim.epoch),
    );
    let reply = protocol::GeometryReply {
        owner: claim.owner.clone(),
        epoch: claim.epoch,
        screen: screen.clone(),
    };
    if let Err(error) = app.emit_to(claim.owner.as_str(), commands::GEOMETRY_EVENT, reply) {
        logging::log(
            Level::Warn,
            format!(
                "geometry_emit_failed owner={} epoch={} {error}",
                claim.owner, claim.epoch
            ),
        );
    }
}

/// Start a runtime and wire it to the frame protocol and the event pump.
pub fn start_client(
    app: &tauri::AppHandle,
    settings: &Settings,
    audio: AudioHandle,
) -> Result<ClientHandle, String> {
    let (handle, stream) = ClientRuntime::spawn(config_for(settings));
    if let Some(slot) = app.try_state::<protocol::FrameSlot>() {
        slot.set(handle.frames());
    }
    if let Some(slot) = app.try_state::<protocol::AvatarImageSlot>() {
        slot.set(handle.avatar_images());
    }
    spawn_pump(app.clone(), stream, audio);
    Ok(handle)
}

/// The app-wide window-event handler.
///
/// A panel window's `CloseRequested` must never tear down the shared session:
/// it only asks `main` to re-dock the panel, and the panel's own window is
/// destroyed afterwards. Only the `main` window disconnects — the pre-existing
/// single-window behaviour, now guarded by label because other windows exist.
pub fn handle_window_event<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    event: &tauri::WindowEvent,
) {
    let label = window.label();
    match event {
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
            // Drags and resizes fire continuously; the store only marks itself
            // dirty here and the autosave thread writes at its own pace.
            geometry::note_geometry_changed(window);
        }
        tauri::WindowEvent::CloseRequested { .. } => {
            // Capture before the window goes away. This saves geometry but
            // never changes the detached flag: a panel closing because the app
            // is quitting must stay marked detached for the next launch.
            geometry::note_window_closing(window);
            match windows::close_action(label) {
                windows::CloseAction::ReattachPanel => {
                    if windows::is_quitting() {
                        // The app is on its way out, so this close is part of
                        // the quit rather than a re-attach: signal nothing and
                        // leave the detached flag alone.
                        logging::log(
                            Level::Info,
                            format!("panel closing during quit label={label}"),
                        );
                    } else {
                        logging::log(Level::Info, format!("panel close requested label={label}"));
                        windows::panel_close_requested(window.app_handle(), label);
                    }
                }
                windows::CloseAction::DisconnectMain => {
                    logging::log(Level::Info, "window close requested; disconnecting");
                    if let Some(state) = window.try_state::<AppState>() {
                        if let Ok(guard) = state.client.lock() {
                            if let Some(client) = guard.as_ref() {
                                client.disconnect();
                                std::thread::sleep(Duration::from_millis(150));
                            }
                        }
                    }
                    // Closing `main` quits: it takes the open panels with it
                    // rather than leaving them over a dead session.
                    windows::begin_quit(window.app_handle());
                }
                windows::CloseAction::Ignore => {}
            }
        }
        tauri::WindowEvent::Destroyed => {
            logging::log_window(
                label,
                logging::WindowMilestone::Destroyed,
                "os window destroyed",
            );
            let action = windows::destroyed_action(
                label,
                windows::is_quitting(),
                windows::take_close_requested(label),
            );
            if action == windows::DestroyAction::ReattachPanel {
                // A panel whose webview died on its own: re-dock it so `main`
                // never keeps a ghost placeholder. Reuses PANEL_CLOSED_EVENT and
                // the `reattached` milestone; `via=destroyed` distinguishes it.
                windows::panel_destroyed(window.app_handle(), label);
            }
            // A `main` that dies without a close request (a crashed webview, a
            // window-manager kill) would otherwise orphan open panels over a
            // dead session.
            if label == windows::MAIN_LABEL
                && !windows::is_quitting()
                && (windows::any_panel_open(window.app_handle())
                    || windows::any_tool_window_open(window.app_handle()))
            {
                windows::begin_quit(window.app_handle());
            }
        }
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(error) = logging::init() {
        eprintln!("palace: could not open the diagnostic log: {error}");
    }
    let slot = protocol::FrameSlot::default();
    let handler_slot = slot.clone();
    let catalog_slot = protocol::CatalogSlot::default();
    let image_slot = protocol::AvatarImageSlot::default();
    let handler_images = image_slot.clone();
    let viewport_owner = protocol::ViewportOwnerSlot::default();
    match palace_prop::PropCatalog::open_default() {
        Some(catalog) => {
            logging::log(
                Level::Info,
                format!("prop catalog loaded: {} entries", catalog.len()),
            );
            catalog_slot.set(Arc::new(catalog));
        }
        None => logging::log(
            Level::Info,
            "no readable prop bag found; prop catalog empty",
        ),
    }
    let handler_catalog = catalog_slot.clone();
    let bag_slot = protocol::BagSlot::default();
    let handler_bag = bag_slot.clone();
    let editor_slot = editor::EditorSlot::default();
    let bag_service = bag::BagService::discover();
    let initial_bag = bag_service.snapshot();
    logging::log(
        Level::Info,
        format!(
            "bag catalog loaded: {} entries, {} shelves",
            initial_bag.catalog.len(),
            initial_bag.shelves.len()
        ),
    );
    bag_slot.set(Arc::new(initial_bag));
    let defaults = Settings::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(slot)
        .manage(catalog_slot)
        .manage(image_slot)
        .manage(bag_slot)
        .manage(editor_slot)
        .manage(viewport_owner)
        .manage(chat_log::ChatLogService::new())
        .manage(notify::NotificationService::new())
        .register_asynchronous_uri_scheme_protocol("palace", move |_ctx, request, responder| {
            protocol::handle(
                &handler_slot,
                &handler_catalog,
                &handler_images,
                &handler_bag,
                &request,
                responder,
            );
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::connect,
            commands::disconnect,
            commands::goto_room,
            commands::say,
            commands::click,
            commands::mousemove,
            commands::mouse_leave,
            commands::set_visibility,
            commands::set_avatar,
            commands::set_props,
            commands::set_type1_avatar,
            commands::clear_type1_avatar,
            commands::type1_avatar_limits,
            commands::set_viewport,
            commands::set_ui_scale,
            commands::set_fullscreen,
            commands::refresh,
            commands::get_audio_state,
            commands::set_soundfont,
            commands::set_audio_enabled,
            commands::set_volume,
            commands::bag_collections,
            commands::bag_shelves,
            commands::bag_catalog,
            commands::bag_rebuild_thumbnail,
            commands::bag_rebuild_thumbnails,
            commands::bag_add,
            commands::bag_remove,
            commands::bag_move,
            commands::bag_duplicate,
            commands::bag_rename,
            commands::bag_create_collection,
            commands::bag_delete_collection,
            commands::bag_favourite,
            commands::bag_trash,
            commands::bag_trash_list,
            commands::bag_trash_restore,
            commands::bag_trash_purge,
            commands::gather_prop,
            commands::outfits_list,
            commands::outfits_save,
            commands::outfits_apply,
            commands::outfits_rename,
            commands::outfits_delete,
            commands::outfits_duplicate,
            editor::editor_open_blank,
            editor::editor_open_bag_prop,
            editor::editor_open_image,
            editor::editor_state,
            editor::editor_frame_png,
            editor::editor_undo,
            editor::editor_redo,
            editor::editor_add_frame,
            editor::editor_duplicate_frame,
            editor::editor_delete_frame,
            editor::editor_move_frame,
            editor::editor_select_frame,
            editor::editor_copy_frame,
            editor::editor_paste_frame,
            editor::editor_paint_stroke,
            editor::editor_erase_stroke,
            editor::editor_begin_stroke,
            editor::editor_extend_stroke,
            editor::editor_flood_fill,
            editor::editor_pick_color,
            editor::editor_clear_frame,
            editor::editor_flip,
            editor::editor_rotate,
            editor::editor_crop,
            editor::editor_crop_square,
            editor::editor_shape_crop,
            editor::editor_resize,
            editor::editor_adjust_brightness_contrast,
            editor::editor_adjust_colour,
            editor::editor_remove_background,
            editor::editor_sample_key_color,
            editor::editor_save_prop,
            editor::editor_export_png,
            editor::editor_export_webp,
            editor::editor_add_text,
            editor::editor_import_overlay,
            editor::editor_update_layer,
            editor::editor_move_layer,
            editor::editor_delete_layer,
            editor::editor_guide_state,
            editor::editor_set_guide_toggles,
            editor::editor_set_onion_radius,
            editor::editor_guide_geometry,
            editor::editor_onion_neighbours,
            editor::editor_snap_point,
            windows::open_panel,
            windows::close_panel,
            windows::open_preferences,
            windows::close_preferences,
            geometry::get_layout_memory,
            geometry::set_layout_remember,
            geometry::reset_layout,
            prefs::get_prefs,
            prefs::set_prefs,
            prefs::reset_prefs,
            prefs::set_connection_settings,
            chat_log::chat_log_status,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            logging::log(Level::Info, "palace-app starting");
            logging::log_window(
                windows::MAIN_LABEL,
                logging::WindowMilestone::Created,
                "config window",
            );
            let saved = settings::config_path(&handle).and_then(|path| settings::load(&path));
            let mut settings = Settings::resolve(defaults, saved, args.into_iter());
            if settings.ensure_identity() {
                if let Some(path) = settings::config_path(&handle) {
                    if let Err(error) = settings::save(&path, &settings) {
                        logging::log(
                            Level::Warn,
                            format!("could not persist the generated identity: {error}"),
                        );
                        eprintln!("palace: could not persist the generated identity: {error}");
                    }
                }
            }
            logging::log(
                Level::Info,
                format!(
                    "connecting to {}:{} as {}",
                    settings.host, settings.port, settings.username
                ),
            );
            let bundled = bundled_soundfont(&handle);
            let audio = AudioEngine::spawn(shell_audio_config(&settings, bundled.as_deref()));
            let audio_handle = audio.handle();
            app.manage(AppState {
                client: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                audio: Mutex::new(audio),
                bundled_soundfont: bundled,
                bag: bag_service.clone(),
                refresh_epoch: AtomicU64::new(0),
            });
            // Applied before the pump starts, so the session's first line is written.
            if let Some(path) = settings::config_path(&handle) {
                let prefs = settings::read_prefs(&path);
                if let Some(transcript) = handle.try_state::<chat_log::ChatLogService>() {
                    transcript.configure_from_prefs(&prefs);
                }
                if let Some(notifications) = handle.try_state::<notify::NotificationService>() {
                    notifications.configure_from_prefs(&prefs, &settings.username);
                }
            }
            // Layout memory loads before any window moves and is restored on
            // the main thread once `setup` returns; until the restore has run
            // the store ignores move events, so a platform-placed window can
            // never overwrite the saved rectangle on disk.
            let layout = geometry::LayoutStore::discover(&handle);
            let _ = app.manage(layout.clone());
            if let Err(error) = geometry::spawn_autosave(&layout) {
                logging::log(
                    Level::Warn,
                    format!("could not start the window-layout autosave thread: {error}"),
                );
            }
            geometry::schedule_restore(&handle);
            match start_client(&handle, &settings, audio_handle) {
                Ok(client) => {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if let Ok(mut guard) = state.client.lock() {
                            *guard = Some(client);
                        }
                    }
                    logging::log(Level::Info, "client runtime started");
                }
                Err(error) => {
                    logging::log(Level::Error, format!("could not start client: {error}"));
                    eprintln!("palace: could not start client: {error}");
                }
            }
            Ok(())
        })
        .on_window_event(handle_window_event)
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            logging::log(Level::Error, format!("fatal: {error}"));
            eprintln!("palace: fatal: {error}");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            host: "h.test".to_string(),
            port: 4444,
            username: "Someone".to_string(),
            soundfont: None,
            audio_enabled: true,
            audio_volume: 1.0,
            identity: None,
            puid: None,
            password: None,
        }
    }

    #[test]
    fn the_audio_config_carries_the_soundfont_and_audio_preferences() {
        let settings = Settings {
            soundfont: Some(PathBuf::from("/tmp/font.sf2")),
            audio_enabled: false,
            audio_volume: 0.25,
            ..sample()
        };
        let audio = audio_config_for(&settings, None);
        assert_eq!(audio.soundfont, Some(PathBuf::from("/tmp/font.sf2")));
        assert!(!audio.enabled);
        assert_eq!(audio.volume, 0.25);
    }

    #[test]
    fn the_desktop_shell_still_opens_a_device() {
        let audio = audio_config_for(&sample(), None);
        assert_eq!(audio.device, palace_audio::DeviceMode::Open);
    }

    #[test]
    fn the_bundled_font_is_the_default_but_a_saved_choice_wins() {
        let bundled = PathBuf::from("/opt/palace/resources/soundfonts/GeneralUser-GS.sf2");
        assert_eq!(
            audio_config_for(&sample(), Some(&bundled)).soundfont,
            Some(bundled.clone()),
            "an unset soundfont falls back to the vendored font"
        );
        let chosen = Settings {
            soundfont: Some(PathBuf::from("/tmp/mine.sf2")),
            ..sample()
        };
        assert_eq!(
            audio_config_for(&chosen, Some(&bundled)).soundfont,
            Some(PathBuf::from("/tmp/mine.sf2")),
            "a saved soundfont beats the vendored fallback"
        );
    }

    #[test]
    fn the_shell_uses_the_bundled_bank_when_a_saved_soundfont_is_gone() {
        let directory =
            std::env::temp_dir().join(format!("palace-app-soundfont-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
        let bundled = directory.join("GeneralUser-GS.sf2");
        std::fs::write(&bundled, b"bundled").expect("the bundled font is writable");
        let gone = directory.join("deleted.sf2");

        let settings = Settings {
            soundfont: Some(gone.clone()),
            ..sample()
        };
        assert_eq!(
            shell_audio_config(&settings, Some(&bundled)).soundfont,
            Some(bundled.clone()),
            "a dead path is replaced by the bundled bank, not handed to the engine"
        );
        assert_eq!(
            shell_audio_config(&settings, None).soundfont,
            None,
            "without a bundle the engine starts on its fallback tone"
        );
        assert_eq!(
            shell_audio_config(&settings, Some(&bundled)).volume,
            1.0,
            "the rest of the audio preferences are carried through"
        );

        let live = Settings {
            soundfont: Some(bundled.clone()),
            ..sample()
        };
        assert_eq!(
            shell_audio_config(&live, Some(&bundled)).soundfont,
            Some(bundled.clone()),
            "an on-disk choice is used as chosen"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn config_carries_the_settings() {
        let cfg = config_for(&sample());
        assert_eq!(cfg.host, "h.test");
        assert_eq!(cfg.port, 4444);
        assert_eq!(cfg.username, "Someone");
        assert_eq!(cfg.password, None);
    }

    #[test]
    fn config_carries_the_persisted_identity() {
        let settings = Settings {
            identity: Some(settings::StoredIdentity {
                registration: settings::StoredRegistration {
                    crc: 0x0506_0708,
                    counter: 0x090a_0b0c,
                },
                puid: settings::StoredPuid {
                    ctr: 0x0a0b_0c0d,
                    crc: 0x0102_0304,
                },
            }),
            ..sample()
        };
        let identity = config_for(&settings).identity;
        assert_eq!(identity.registration.crc, 0x0506_0708);
        assert_eq!(identity.registration.counter, 0x090a_0b0c);
        assert_eq!(identity.puid.ctr, 0x0a0b_0c0d);
        assert_eq!(identity.puid.crc, 0x0102_0304);
    }

    #[test]
    fn config_carries_the_credential_without_exposing_it() {
        let settings = Settings {
            password: Some(palace_client::Secret::new("hunter2")),
            ..sample()
        };
        let cfg = config_for(&settings);
        assert_eq!(cfg.password, Some(palace_client::Secret::new("hunter2")));
        assert!(
            !format!("{cfg:?}").contains("hunter2"),
            "the credential must not render in Debug"
        );
    }
}

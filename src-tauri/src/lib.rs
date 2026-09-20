//! Palace desktop client: the Tauri shell over the headless runtime.
//!
//! The window is deliberately thin — it displays one pre-composited PNG per
//! frame, draws the chrome around it, and forwards input. All protocol,
//! asset and compositing work lives in `palace-client` on worker threads.

pub mod bag;
pub mod commands;
pub mod editor;
pub mod logging;
pub mod protocol;
pub mod settings;

pub use settings::Settings;

use logging::Level;
use std::path::{Path, PathBuf};
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

/// Forward runtime events to the webview, routing sound effects to the engine.
pub fn spawn_pump(app: tauri::AppHandle, mut stream: ClientEventStream, audio: AudioHandle) {
    tauri::async_runtime::spawn(async move {
        let mut media_base: Option<String> = None;
        while let Some(event) = stream.recv().await {
            log_event(&event);
            match &event {
                ClientEvent::Banner { banner } => {
                    if let Some(base) = &banner.media_base {
                        if media_base.as_deref() != Some(base.as_str()) {
                            media_base = Some(base.clone());
                            audio.set_media_base(base.clone());
                        }
                    }
                }
                ClientEvent::Sound { name } => audio.play_sound(name.clone()),
                ClientEvent::MidiPlay { name } => audio.midi_play(name.clone()),
                ClientEvent::MidiLoop { name, loops } => audio.midi_loop(name.clone(), *loops),
                ClientEvent::MidiStop => audio.midi_stop(),
                ClientEvent::Beep => audio.beep(),
                _ => {}
            }
            if app.emit(commands::EVENT_NAME, &event).is_err() {
                break;
            }
        }
    });
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
        .manage(slot)
        .manage(catalog_slot)
        .manage(image_slot)
        .manage(bag_slot)
        .manage(editor_slot)
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
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            logging::log(Level::Info, "palace-app starting");
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
            let audio = AudioEngine::spawn(audio_config_for(&settings, bundled.as_deref()));
            let audio_handle = audio.handle();
            app.manage(AppState {
                client: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                audio: Mutex::new(audio),
                bundled_soundfont: bundled,
                bag: bag_service.clone(),
            });
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
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                logging::log(Level::Info, "window close requested; disconnecting");
                if let Some(state) = window.try_state::<AppState>() {
                    if let Ok(guard) = state.client.lock() {
                        if let Some(client) = guard.as_ref() {
                            client.disconnect();
                            std::thread::sleep(Duration::from_millis(150));
                        }
                    }
                }
            }
        })
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

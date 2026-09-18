//! Palace desktop client: the Tauri shell over the headless runtime.
//!
//! The window is deliberately thin — it displays one pre-composited PNG per
//! frame, draws the chrome around it, and forwards input. All protocol,
//! asset and compositing work lives in `palace-client` on worker threads.

pub mod commands;
pub mod protocol;
pub mod settings;

pub use settings::Settings;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use palace_audio::{AudioConfig, AudioEngine, AudioHandle};
use palace_client::{ClientConfig, ClientEvent, ClientEventStream, ClientHandle, ClientRuntime};
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};

/// Shared app state: the running client and the settings it was built from.
pub struct AppState {
    pub client: Mutex<Option<ClientHandle>>,
    pub settings: Mutex<Settings>,
    pub audio: Mutex<AudioEngine>,
    /// The vendored font `settings.soundfont` falls back to when it is `None`.
    pub bundled_soundfont: Option<PathBuf>,
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
        seed_media: seed_media(),
        seed_props: seed_props(),
        ..ClientConfig::default()
    }
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

/// Forward runtime events to the webview, routing sound effects to the engine.
pub fn spawn_pump(app: tauri::AppHandle, mut stream: ClientEventStream, audio: AudioHandle) {
    tauri::async_runtime::spawn(async move {
        let mut media_base: Option<String> = None;
        while let Some(event) = stream.recv().await {
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
    spawn_pump(app.clone(), stream, audio);
    Ok(handle)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let slot = protocol::FrameSlot::default();
    let handler_slot = slot.clone();
    let defaults = Settings::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(slot)
        .register_asynchronous_uri_scheme_protocol("palace", move |_ctx, request, responder| {
            protocol::handle(&handler_slot, &request, responder);
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
            commands::set_viewport,
            commands::refresh,
            commands::get_audio_state,
            commands::set_soundfont,
            commands::set_audio_enabled,
            commands::set_volume,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let saved = settings::config_path(&handle).and_then(|path| settings::load(&path));
            let settings = Settings::resolve(defaults, saved, args.into_iter());
            let bundled = bundled_soundfont(&handle);
            let audio = AudioEngine::spawn(audio_config_for(&settings, bundled.as_deref()));
            let audio_handle = audio.handle();
            app.manage(AppState {
                client: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                audio: Mutex::new(audio),
                bundled_soundfont: bundled,
            });
            match start_client(&handle, &settings, audio_handle) {
                Ok(client) => {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if let Ok(mut guard) = state.client.lock() {
                            *guard = Some(client);
                        }
                    }
                }
                Err(error) => eprintln!("palace: could not start client: {error}"),
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
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
        .unwrap_or_else(|error| eprintln!("palace: fatal: {error}"));
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

//! Palace desktop client: the Tauri shell over the headless runtime.
//!
//! The window is deliberately thin — it displays one pre-composited PNG per
//! frame, draws the chrome around it, and forwards input. All protocol,
//! asset and compositing work lives in `palace-client` on worker threads.

pub mod commands;
pub mod protocol;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use palace_client::{ClientConfig, ClientEventStream, ClientHandle, ClientRuntime};
use tauri::{Emitter, Manager};

/// Shared app state: the running client and the settings it was built from.
pub struct AppState {
    pub client: Mutex<Option<ClientHandle>>,
    pub settings: Mutex<Settings>,
}

/// Where to connect and who to be.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub username: String,
}

impl Settings {
    /// Defaults from the environment, matching a launchable-out-of-the-box config.
    #[must_use]
    pub fn from_env() -> Self {
        Settings {
            host: std::env::var("PALACE_HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: std::env::var("PALACE_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(9998),
            username: std::env::var("PALACE_USER").unwrap_or_else(|_| "Guest".to_string()),
        }
    }

    /// Apply `--host` / `--port` / `--user` from the command line.
    #[must_use]
    pub fn with_args(mut self, args: impl Iterator<Item = String>) -> Self {
        let mut pending: Option<String> = None;
        for arg in args {
            if let Some(key) = pending.take() {
                match key.as_str() {
                    "host" => self.host = arg,
                    "port" => {
                        if let Ok(port) = arg.parse() {
                            self.port = port;
                        }
                    }
                    "user" => self.username = arg,
                    _ => {}
                }
                continue;
            }
            match arg.as_str() {
                "--host" => pending = Some("host".to_string()),
                "--port" => pending = Some("port".to_string()),
                "--user" => pending = Some("user".to_string()),
                other => {
                    if let Some(value) = other.strip_prefix("--host=") {
                        self.host = value.to_string();
                    } else if let Some(value) = other.strip_prefix("--port=") {
                        if let Ok(port) = value.parse() {
                            self.port = port;
                        }
                    } else if let Some(value) = other.strip_prefix("--user=") {
                        self.username = value.to_string();
                    }
                }
            }
        }
        self
    }
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
        seed_media: seed_media(),
        seed_props: seed_props(),
        ..ClientConfig::default()
    }
}

/// Forward runtime events to the webview.
pub fn spawn_pump(app: tauri::AppHandle, mut stream: ClientEventStream) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = stream.recv().await {
            if app.emit(commands::EVENT_NAME, &event).is_err() {
                break;
            }
        }
    });
}

/// Start a runtime and wire it to the frame protocol and the event pump.
pub fn start_client(app: &tauri::AppHandle, settings: &Settings) -> Result<ClientHandle, String> {
    let (handle, stream) = ClientRuntime::spawn(config_for(settings));
    if let Some(slot) = app.try_state::<protocol::FrameSlot>() {
        slot.set(handle.frames());
    }
    spawn_pump(app.clone(), stream);
    Ok(handle)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let slot = protocol::FrameSlot::default();
    let handler_slot = slot.clone();
    let settings = Settings::from_env().with_args(std::env::args().skip(1));

    tauri::Builder::default()
        .manage(AppState {
            client: Mutex::new(None),
            settings: Mutex::new(settings.clone()),
        })
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
            commands::set_viewport,
            commands::refresh,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            match start_client(&handle, &settings) {
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

    fn args(list: &[&str]) -> std::vec::IntoIter<String> {
        list.iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn cli_flags_override_settings() {
        let base = Settings {
            host: "localhost".to_string(),
            port: 9998,
            username: "Guest".to_string(),
        };
        let parsed = base.with_args(args(&[
            "--host",
            "example.org",
            "--port",
            "1234",
            "--user",
            "Tester",
        ]));
        assert_eq!(parsed.host, "example.org");
        assert_eq!(parsed.port, 1234);
        assert_eq!(parsed.username, "Tester");
    }

    #[test]
    fn cli_flags_accept_equals_form_and_ignore_junk() {
        let base = Settings {
            host: "localhost".to_string(),
            port: 9998,
            username: "Guest".to_string(),
        };
        let parsed = base.with_args(args(&["--port=bogus", "--host=x.test", "--nonsense", "-v"]));
        assert_eq!(parsed.host, "x.test");
        assert_eq!(parsed.port, 9998, "an unparsable port leaves the default");
    }

    #[test]
    fn config_carries_the_settings() {
        let settings = Settings {
            host: "h.test".to_string(),
            port: 4444,
            username: "Someone".to_string(),
        };
        let cfg = config_for(&settings);
        assert_eq!(cfg.host, "h.test");
        assert_eq!(cfg.port, 4444);
        assert_eq!(cfg.username, "Someone");
    }
}

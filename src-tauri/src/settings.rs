//! The launch settings and the file they persist to.
//!
//! Settings arrive from three sources: the environment, the saved config file
//! and the command line. They are merged once at startup (see
//! [`Settings::resolve`]) and written back whenever the interface changes them.
//! The file is user-editable, so every read failure is non-fatal: the app falls
//! back to defaults and says so once. No path here is ever split or normalised;
//! a Windows drive letter has to survive a round trip untouched.

use std::path::{Path, PathBuf};

use tauri::Manager;

/// The settings file's name inside the platform app-config directory.
pub const CONFIG_FILE: &str = "settings.json";

/// Where to connect, who to be, and how the audio engine should start.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub soundfont: Option<PathBuf>,
    #[serde(default = "audio_enabled_default")]
    pub audio_enabled: bool,
    #[serde(default = "audio_volume_default")]
    pub audio_volume: f32,
}

fn audio_enabled_default() -> bool {
    true
}

fn audio_volume_default() -> f32 {
    1.0
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
            soundfont: std::env::var_os("PALACE_SOUNDFONT").map(PathBuf::from),
            audio_enabled: audio_enabled_default(),
            audio_volume: audio_volume_default(),
        }
    }

    /// Apply `--host` / `--port` / `--user` / `--soundfont` from the command line.
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
                    "soundfont" => self.soundfont = Some(PathBuf::from(arg)),
                    _ => {}
                }
                continue;
            }
            match arg.as_str() {
                "--host" => pending = Some("host".to_string()),
                "--port" => pending = Some("port".to_string()),
                "--user" => pending = Some("user".to_string()),
                "--soundfont" => pending = Some("soundfont".to_string()),
                other => {
                    if let Some(value) = other.strip_prefix("--host=") {
                        self.host = value.to_string();
                    } else if let Some(value) = other.strip_prefix("--port=") {
                        if let Ok(port) = value.parse() {
                            self.port = port;
                        }
                    } else if let Some(value) = other.strip_prefix("--user=") {
                        self.username = value.to_string();
                    } else if let Some(value) = other.strip_prefix("--soundfont=") {
                        self.soundfont = Some(PathBuf::from(value));
                    }
                }
            }
        }
        self
    }

    /// Resolve the settings a launch runs with.
    ///
    /// Precedence, weakest first: the environment-derived defaults, then the
    /// saved config file, then the command line. The file is the user's
    /// persisted intent across restarts; the flags are the most specific
    /// instruction for this particular launch and win.
    #[must_use]
    pub fn resolve(
        defaults: Settings,
        saved: Option<Settings>,
        args: impl Iterator<Item = String>,
    ) -> Settings {
        saved.unwrap_or(defaults).with_args(args)
    }
}

/// The settings file inside the platform app-config directory.
///
/// `None` means the platform cannot name a config directory; the app then runs
/// on defaults and simply does not persist. The path is assembled with
/// `PathBuf::join`, never string concatenation.
#[must_use]
pub fn config_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|directory| directory.join(CONFIG_FILE))
}

/// Read the saved settings.
///
/// `None` covers a missing, unreadable or malformed file. A user-editable file
/// is untrusted input: a bad one is logged once and the caller falls back to
/// defaults rather than failing to start.
#[must_use]
pub fn load(path: &Path) -> Option<Settings> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "palace: could not read settings at {}: {error}",
                    path.display()
                );
            }
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(settings) => Some(settings),
        Err(error) => {
            eprintln!(
                "palace: ignoring malformed settings at {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// Write the settings to `path` atomically.
///
/// The JSON goes to a sibling temp file first and is renamed over the target, so
/// a crash mid-write cannot leave a half-written config behind. `PathBuf`
/// handles the separators, so this is safe with a Windows-style target path.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
    }
    let json = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)
        .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("could not replace {}: {error}", path.display())
    })
}

/// Check a chosen SoundFont path, returning the [`PathBuf`] to hand the engine.
///
/// `None` and a blank string both mean "clear it". A path is accepted only when
/// it ends in `.sf2`, case-insensitively (`rustysynth` reads SF2 and rejects the
/// compressed SF3), and names an existing file. The string is opaque: it is
/// never split on a separator or joined, so a Windows drive letter survives.
pub fn validate_soundfont(path: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(raw) = path else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let candidate = PathBuf::from(raw);
    let is_sf2 = candidate
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sf2"));
    if !is_sf2 {
        return Err(format!("{} is not an .sf2 SoundFont", candidate.display()));
    }
    if !candidate.is_file() {
        return Err(format!("{} does not exist", candidate.display()));
    }
    Ok(Some(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(list: &[&str]) -> std::vec::IntoIter<String> {
        list.iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn sample() -> Settings {
        Settings {
            host: "localhost".to_string(),
            port: 9998,
            username: "Guest".to_string(),
            soundfont: None,
            audio_enabled: true,
            audio_volume: 1.0,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("palace-settings-{tag}-{unique}"));
        std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
        directory
    }

    #[test]
    fn cli_flags_override_settings() {
        let parsed = sample().with_args(args(&[
            "--host",
            "example.org",
            "--port",
            "1234",
            "--user",
            "Tester",
            "--soundfont",
            "/tmp/font.sf2",
        ]));
        assert_eq!(parsed.host, "example.org");
        assert_eq!(parsed.port, 1234);
        assert_eq!(parsed.username, "Tester");
        assert_eq!(parsed.soundfont, Some(PathBuf::from("/tmp/font.sf2")));
    }

    #[test]
    fn cli_flags_accept_equals_form_and_ignore_junk() {
        let parsed =
            sample().with_args(args(&["--port=bogus", "--host=x.test", "--nonsense", "-v"]));
        assert_eq!(parsed.host, "x.test");
        assert_eq!(parsed.port, 9998, "an unparsable port leaves the default");
    }

    #[test]
    fn settings_round_trip_through_the_config_file() {
        let directory = scratch("round-trip");
        let path = directory.join(CONFIG_FILE);
        let original = Settings {
            host: "example.org".to_string(),
            port: 1234,
            username: "Tester".to_string(),
            soundfont: Some(PathBuf::from("/fonts/tim.sf2")),
            audio_enabled: false,
            audio_volume: 0.4,
        };
        save(&path, &original).expect("the settings are writable");
        let loaded = load(&path).expect("the settings are readable again");
        assert_eq!(loaded, original);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_config_file_is_not_an_error() {
        let directory = scratch("missing");
        let path = directory.join(CONFIG_FILE);
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_malformed_config_file_falls_back_instead_of_failing() {
        let directory = scratch("malformed");
        let path = directory.join(CONFIG_FILE);
        std::fs::write(&path, "{ this is not json").expect("the bad file is writable");
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_partial_config_file_keeps_the_audio_defaults() {
        let directory = scratch("partial");
        let path = directory.join(CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"host":"h.test","port":4,"username":"N","soundfont":null}"#,
        )
        .expect("the file is writable");
        let loaded = load(&path).expect("a file with the core fields loads");
        assert!(loaded.audio_enabled, "the absent field takes its default");
        assert_eq!(loaded.audio_volume, 1.0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_windows_style_soundfont_path_survives_the_config_round_trip() {
        let directory = scratch("windows");
        let path = directory.join(CONFIG_FILE);
        let windows = PathBuf::from(r"C:\some\dir\font.sf2");
        let original = Settings {
            soundfont: Some(windows.clone()),
            ..sample()
        };
        save(&path, &original).expect("the settings are writable");
        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        assert!(
            raw.contains(r"C:\\some\\dir\\font.sf2"),
            "the JSON escapes the backslashes but the path is whole: {raw}"
        );
        let loaded = load(&path).expect("the settings are readable again");
        assert_eq!(
            loaded.soundfont,
            Some(windows),
            "the drive letter is not split off"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn saved_settings_override_env_defaults_and_cli_args_win_over_both() {
        let defaults = Settings {
            host: "env.host".to_string(),
            port: 1,
            username: "Env".to_string(),
            ..sample()
        };
        let saved = Settings {
            host: "file.host".to_string(),
            port: 2,
            username: "File".to_string(),
            ..sample()
        };
        let resolved = Settings::resolve(defaults.clone(), Some(saved), args(&["--port", "3"]));
        assert_eq!(resolved.host, "file.host", "the file beats the environment");
        assert_eq!(resolved.username, "File");
        assert_eq!(resolved.port, 3, "the command line beats the file");

        let no_file = Settings::resolve(defaults, None, args(&[]));
        assert_eq!(
            no_file.host, "env.host",
            "without a file the environment stands"
        );
    }

    #[test]
    fn a_non_sf2_path_is_rejected() {
        assert!(validate_soundfont(Some("/fonts/font.sf3")).is_err());
        assert!(validate_soundfont(Some("/fonts/font.mid")).is_err());
        assert!(validate_soundfont(Some("/fonts/font")).is_err());
    }

    #[test]
    fn a_missing_sf2_path_is_rejected() {
        let error = validate_soundfont(Some("/no/such/font.sf2")).expect_err("it is missing");
        assert!(error.contains("/no/such/font.sf2"), "{error}");
    }

    #[test]
    fn an_existing_sf2_path_is_accepted_case_insensitively() {
        let directory = scratch("accept");
        let font = directory.join("MyFont.SF2");
        std::fs::write(&font, b"not a real font").expect("the file is writable");
        let accepted = validate_soundfont(font.to_str()).expect("an existing .sf2 is usable");
        assert_eq!(accepted, Some(font));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_null_or_blank_selection_clears_the_soundfont() {
        assert_eq!(validate_soundfont(None), Ok(None));
        assert_eq!(validate_soundfont(Some("   ")), Ok(None));
    }

    #[test]
    fn a_windows_style_path_is_never_split() {
        // Were the string split on `:` the error would name only `C`, and the
        // drive letter would be gone. The whole path in the message is the pin.
        let error =
            validate_soundfont(Some(r"C:\some\dir\font.sf2")).expect_err("it is not on disk");
        assert!(error.contains(r"C:\some\dir\font.sf2"), "{error}");
    }
}

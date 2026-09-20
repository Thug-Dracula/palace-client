//! The launch settings and the file they persist to.
//!
//! Settings arrive from three sources: the environment, the saved config file
//! and the command line. They are merged once at startup (see
//! [`Settings::resolve`]) and written back whenever the interface changes them.
//! The file is user-editable, so every read failure is non-fatal: the app falls
//! back to defaults and says so once. No path here is ever split or normalised;
//! a Windows drive letter has to survive a round trip untouched.
//!
//! # The file is shared
//!
//! The same `settings.json` is also read and written by the original
//! PalaceChat client, so a write is an additive merge into whatever is already
//! there rather than a rebuild from this struct: keys this build does not know
//! keep their name, value and position, and the original file is copied to a
//! `.bak` beside it before the first modified write. This client's own
//! preferences live under one `prefs` object (see `PREFERENCES.md`); the
//! legacy top level `puid` is migrated into the identity and never written
//! back. [`read_document`], [`update_prefs`] and [`write_document`] are the
//! merge layer; [`save`] writes the typed fields through it.

use std::path::{Path, PathBuf};

use palace_client::{ClientIdentity, Puid, RegistrationCode, Secret};
use tauri::Manager;

use crate::logging::{self, Level};

/// The settings file's name inside the platform app-config directory.
pub const CONFIG_FILE: &str = "settings.json";

/// The block in the shared file that holds this client's own preferences.
pub const PREFS_KEY: &str = "prefs";

/// The key inside [`PREFS_KEY`] that records which schema wrote the block.
pub const PREFS_SCHEMA_VERSION_KEY: &str = "schema_version";

/// The schema version this build writes into the [`PREFS_KEY`] block.
pub const PREFS_SCHEMA_VERSION: u64 = 1;

/// Appended to the settings file's name for the one-time backup.
pub const BACKUP_SUFFIX: &str = ".bak";

/// The backup path for `path`: the full name with [`BACKUP_SUFFIX`] appended.
#[must_use]
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Report a recoverable settings problem on stderr and in the diagnostic log.
fn warn(message: &str) {
    eprintln!("palace: {message}");
    logging::log(Level::Warn, message);
}

/// The reference client's identity seed: the wall clock in milliseconds since
/// the Unix epoch, truncated to 32 bits.
fn clock_seed() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u32)
        .unwrap_or(0)
}

/// The per-install PUID as it is stored in `settings.json`.
///
/// The wire type lives in `palace-wire`, which has no serde dependency; this is
/// the one-line persistence mirror of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPuid {
    pub ctr: u32,
    pub crc: u32,
}

impl From<StoredPuid> for Puid {
    fn from(stored: StoredPuid) -> Self {
        Puid {
            ctr: stored.ctr,
            crc: stored.crc,
        }
    }
}

impl From<Puid> for StoredPuid {
    fn from(puid: Puid) -> Self {
        StoredPuid {
            ctr: puid.ctr,
            crc: puid.crc,
        }
    }
}

/// The registration-code pair as it is stored in `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredRegistration {
    pub crc: u32,
    pub counter: u32,
}

impl From<RegistrationCode> for StoredRegistration {
    fn from(code: RegistrationCode) -> Self {
        StoredRegistration {
            crc: code.crc,
            counter: code.counter,
        }
    }
}

impl From<StoredRegistration> for RegistrationCode {
    fn from(stored: StoredRegistration) -> Self {
        RegistrationCode {
            crc: stored.crc,
            counter: stored.counter,
        }
    }
}

/// The per-install identity as it is stored in `settings.json`: both the
/// registration pair and the PUID, so every run sends the same two pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredIdentity {
    pub registration: StoredRegistration,
    pub puid: StoredPuid,
}

impl StoredIdentity {
    /// Mint a fresh identity from the wall clock.
    #[must_use]
    pub fn generate() -> Self {
        ClientIdentity::generate(clock_seed()).into()
    }

    /// Rebuild an identity from a pre-identity file's `puid`, minting the
    /// registration pair that file never stored. The stored PUID is preserved.
    #[must_use]
    fn from_legacy_puid(puid: StoredPuid) -> Self {
        StoredIdentity {
            registration: RegistrationCode::generate(clock_seed()).into(),
            puid,
        }
    }
}

impl From<StoredIdentity> for ClientIdentity {
    fn from(stored: StoredIdentity) -> Self {
        ClientIdentity {
            registration: stored.registration.into(),
            puid: stored.puid.into(),
        }
    }
}

impl From<ClientIdentity> for StoredIdentity {
    fn from(identity: ClientIdentity) -> Self {
        StoredIdentity {
            registration: identity.registration.into(),
            puid: identity.puid.into(),
        }
    }
}

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
    /// The per-install identity. `None` until [`Settings::ensure_identity`]
    /// mints one.
    #[serde(default)]
    pub identity: Option<StoredIdentity>,
    /// The `puid` field of a settings file written before identities existed.
    /// Read once for migration by [`Settings::ensure_identity`]; never written.
    #[serde(default, skip_serializing)]
    pub puid: Option<StoredPuid>,
    /// The credential for an `auth` challenge. Sourced only from
    /// `PALACE_PASSWORD` or `--password`, so it is never read from or written to
    /// the config file.
    #[serde(skip)]
    pub password: Option<Secret>,
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
            identity: None,
            puid: None,
            password: std::env::var("PALACE_PASSWORD")
                .ok()
                .filter(|value| !value.is_empty())
                .map(Secret::new),
        }
    }

    /// Mint and store an identity if this install does not have one yet.
    ///
    /// A file that predates identities still has its old `puid` field, which is
    /// migrated into the identity unchanged while the registration pair it
    /// never stored is minted. Returns `true` when an identity was produced, so
    /// the caller can persist it. An install that already has one keeps it
    /// unchanged forever.
    pub fn ensure_identity(&mut self) -> bool {
        if self.identity.is_some() {
            return false;
        }
        self.identity = Some(match self.puid.take() {
            Some(puid) => StoredIdentity::from_legacy_puid(puid),
            None => StoredIdentity::generate(),
        });
        true
    }

    /// Apply `--host` / `--port` / `--user` / `--soundfont` / `--password` from
    /// the command line.
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
                    "password" => self.password = Some(Secret::new(arg)),
                    _ => {}
                }
                continue;
            }
            match arg.as_str() {
                "--host" => pending = Some("host".to_string()),
                "--port" => pending = Some("port".to_string()),
                "--user" => pending = Some("user".to_string()),
                "--soundfont" => pending = Some("soundfont".to_string()),
                "--password" => pending = Some("password".to_string()),
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
                    } else if let Some(value) = other.strip_prefix("--password=") {
                        self.password = Some(Secret::new(value));
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
    ///
    /// The credential is exempt: it is never in the file, so the environment's
    /// value survives the merge and only the command line can override it.
    #[must_use]
    pub fn resolve(
        defaults: Settings,
        saved: Option<Settings>,
        args: impl Iterator<Item = String>,
    ) -> Settings {
        let mut resolved = saved.unwrap_or_else(|| defaults.clone());
        resolved.password = defaults.password;
        resolved.with_args(args)
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
                warn(&format!(
                    "could not read settings at {}: {error}",
                    path.display()
                ));
            }
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(settings) => Some(settings),
        Err(error) => {
            warn(&format!(
                "ignoring malformed settings at {}: {error}",
                path.display()
            ));
            None
        }
    }
}

/// Read a settings file as a JSON object, preserving key order.
///
/// The file is shared with the original PalaceChat client, so it is read as a
/// generic map rather than through [`Settings`]: a key this client does not
/// know must survive a later write. An absent file yields an empty map; an
/// unreadable, malformed or non-object file yields an empty map and a warning,
/// because a bad file must not stop the app from starting.
#[must_use]
pub fn read_document(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn(&format!(
                    "could not read settings at {}: {error}",
                    path.display()
                ));
            }
            return serde_json::Map::new();
        }
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(document)) => document,
        Ok(_) => {
            warn(&format!(
                "ignoring settings at {}: the JSON is not an object",
                path.display()
            ));
            serde_json::Map::new()
        }
        Err(error) => {
            warn(&format!(
                "ignoring malformed settings at {}: {error}",
                path.display()
            ));
            serde_json::Map::new()
        }
    }
}

/// Read the [`PREFS_KEY`] block, preserving key order.
///
/// An absent block yields an empty map, so callers apply their defaults.
#[must_use]
pub fn read_prefs(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    read_document(path)
        .get(PREFS_KEY)
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Additively merge `patch` into the [`PREFS_KEY`] block and write the file.
///
/// Only the keys named in `patch` change: nested objects are merged key by
/// key, and everything else in the file — including keys another client wrote
/// — is left in place and in order. The block is stamped with
/// [`PREFS_SCHEMA_VERSION_KEY`] when it does not already carry one; an
/// existing version is never overwritten, so a file written by a newer build
/// is not downgraded. The write is atomic and the one-time backup applies as
/// it does to [`save`].
pub fn update_prefs(
    path: &Path,
    patch: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let mut document = read_document(path);
    if !document
        .get(PREFS_KEY)
        .is_some_and(serde_json::Value::is_object)
    {
        document.insert(
            PREFS_KEY.to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }
    let prefs = document
        .get_mut(PREFS_KEY)
        .and_then(serde_json::Value::as_object_mut)
        .expect("the prefs block was just ensured to be an object");
    merge_objects(prefs, patch);
    if !prefs.contains_key(PREFS_SCHEMA_VERSION_KEY) {
        prefs.insert(
            PREFS_SCHEMA_VERSION_KEY.to_string(),
            serde_json::Value::from(PREFS_SCHEMA_VERSION),
        );
    }
    write_document(path, &document)
}

/// Merge `patch` into `base`: objects merge recursively, anything else
/// replaces the value. The position of keys that are already present is kept.
fn merge_objects(
    base: &mut serde_json::Map<String, serde_json::Value>,
    patch: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in patch {
        match (base.get_mut(key), value) {
            (Some(serde_json::Value::Object(nested)), serde_json::Value::Object(patch_nested)) => {
                merge_objects(nested, patch_nested);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Write a settings document atomically, with the one-time backup.
///
/// A document identical to what is already on disk is left untouched, so the
/// shared file is only rewritten when something actually changed; a copy of
/// the original is taken just before the first modified write and never
/// replaced afterwards. The JSON goes to a sibling temp file first and is
/// renamed over the target, so a crash mid-write cannot leave a half-written
/// config behind. `PathBuf` handles the separators, so this is safe with a
/// Windows-style target path.
pub fn write_document(
    path: &Path,
    document: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
    }
    let json = serde_json::to_string_pretty(document).map_err(|error| error.to_string())?;
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == json) {
        return Ok(());
    }
    backup_once(path)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)
        .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("could not replace {}: {error}", path.display())
    })
}

/// Copy the original file next to itself once, before the first modified
/// write. A file that does not exist yet has nothing to preserve; an existing
/// backup is the earliest original and is never replaced.
fn backup_once(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let backup = backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    std::fs::copy(path, &backup).map(|_| ()).map_err(|error| {
        format!(
            "could not back up {} to {}: {error}",
            path.display(),
            backup.display()
        )
    })
}

/// Write the typed settings into `path` without disturbing anything else.
///
/// The file is shared with the original PalaceChat client, so it is rewritten
/// as an additive merge: the existing document is read as a generic map, the
/// fields this struct owns are replaced in place, and every other key — the
/// sibling client's settings, future keys, keys this build does not know —
/// survives with its name, value and position. It is never rebuilt from the
/// struct alone. (Before the merge existed, a save silently dropped every key
/// the struct did not declare.)
///
/// The legacy top level `puid` is dropped once an identity has been persisted,
/// because the migration folded the PUID into the identity; while no identity
/// exists the key is left alone, since it is then the only copy of the
/// install's PUID. The credential is `#[serde(skip)]` and never reaches disk.
/// The write goes through [`write_document`]: atomic, one-time backup, and a
/// no-op when the merged document matches what is already there.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let mut document = read_document(path);
    if settings.identity.is_some() && document.contains_key("puid") {
        // Rebuild without the key so the position of the keys that stay is
        // kept (a plain removal may move the last key into its slot).
        document = document
            .into_iter()
            .filter(|(key, _)| key != "puid")
            .collect();
    }
    let fields = serde_json::to_value(settings).map_err(|error| error.to_string())?;
    let fields = fields
        .as_object()
        .expect("a serialized Settings is always a JSON object");
    for (key, value) in fields {
        document.insert(key.clone(), value.clone());
    }
    write_document(path, &document)
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
            identity: None,
            puid: None,
            password: None,
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
            identity: Some(StoredIdentity {
                registration: StoredRegistration {
                    crc: 0x0102_0304,
                    counter: 0x0a0b_0c0d,
                },
                puid: StoredPuid {
                    ctr: 0x1122_3344,
                    crc: 0x5566_7788,
                },
            }),
            puid: None,
            password: None,
        };
        save(&path, &original).expect("the settings are writable");
        let loaded = load(&path).expect("the settings are readable again");
        assert_eq!(loaded, original);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_password_comes_from_the_environment_and_the_command_line_wins() {
        std::env::set_var("PALACE_PASSWORD", "envpass");
        let env = Settings::from_env();
        std::env::remove_var("PALACE_PASSWORD");
        assert_eq!(env.password, Some(Secret::new("envpass")));

        let cli = env.with_args(args(&["--password", "clipass"]));
        assert_eq!(
            cli.password,
            Some(Secret::new("clipass")),
            "the command line beats the environment"
        );

        let equals = sample().with_args(args(&["--password=eqpass"]));
        assert_eq!(equals.password, Some(Secret::new("eqpass")));
    }

    #[test]
    fn the_config_file_neither_supplies_nor_clears_a_credential() {
        let env = Settings {
            password: Some(Secret::new("envpass")),
            ..sample()
        };
        let resolved = Settings::resolve(env, Some(sample()), args(&[]));
        assert_eq!(
            resolved.password,
            Some(Secret::new("envpass")),
            "a saved file must not become a source of, or a sink for, the credential"
        );

        let resolved =
            Settings::resolve(sample(), Some(sample()), args(&["--password", "clipass"]));
        assert_eq!(resolved.password, Some(Secret::new("clipass")));
    }

    #[test]
    fn a_password_is_never_written_to_the_config_file() {
        let directory = scratch("password");
        let path = directory.join(CONFIG_FILE);
        let original = Settings {
            password: Some(Secret::new("hunter2")),
            ..sample()
        };
        save(&path, &original).expect("the settings are writable");
        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        assert!(!raw.contains("hunter2"), "the password reached disk: {raw}");
        assert!(
            !raw.contains("password"),
            "the field must not even be named: {raw}"
        );
        let loaded = load(&path).expect("the settings are readable again");
        assert_eq!(loaded.password, None, "the file never populates a password");
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
        assert_eq!(
            loaded.identity, None,
            "an install with no identity has none until one is minted"
        );
        assert_eq!(loaded.puid, None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_identity_is_minted_once_and_then_stays() {
        let mut settings = sample();
        assert!(settings.ensure_identity());
        let minted = settings.identity.expect("an identity was minted");
        assert!(
            !settings.ensure_identity(),
            "an existing identity is never replaced"
        );
        assert_eq!(settings.identity, Some(minted));
    }

    #[test]
    fn a_persisted_identity_survives_a_restart() {
        let directory = scratch("identity-persist");
        let path = directory.join(CONFIG_FILE);
        let mut first = sample();
        assert!(first.ensure_identity());
        save(&path, &first).expect("the settings are writable");

        let reloaded = load(&path).expect("the settings are readable again");
        let mut restarted = Settings::resolve(sample(), Some(reloaded), args(&[]));
        assert_eq!(
            restarted.identity, first.identity,
            "the identity was reloaded"
        );
        assert!(
            !restarted.ensure_identity(),
            "a restart must not mint a second identity"
        );
        assert_eq!(restarted.identity, first.identity);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_legacy_puid_is_migrated_into_an_identity_without_changing_it() {
        let directory = scratch("identity-migrate");
        let path = directory.join(CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"host":"h.test","port":4,"username":"N","soundfont":null,
                "puid":{"ctr":287454020,"crc":1432778632}}"#,
        )
        .expect("the legacy file is writable");

        let loaded = load(&path).expect("the legacy file loads");
        assert_eq!(loaded.identity, None, "the legacy file has no identity yet");
        assert_eq!(
            loaded.puid,
            Some(StoredPuid {
                ctr: 0x1122_3344,
                crc: 0x5566_7788,
            })
        );

        let mut migrated = Settings::resolve(sample(), Some(loaded), args(&[]));
        assert!(migrated.ensure_identity());
        let identity = migrated.identity.expect("an identity was produced");
        assert_eq!(
            identity.puid,
            StoredPuid {
                ctr: 0x1122_3344,
                crc: 0x5566_7788,
            },
            "the existing PUID is preserved, not reshuffled"
        );
        assert_ne!(
            identity.registration,
            StoredRegistration {
                crc: 0x32fb_23e9,
                counter: 0xaa18_198f,
            },
            "the registration pair must not be the captured one"
        );
        assert!(!migrated.ensure_identity());

        save(&path, &migrated).expect("the migrated settings are writable");
        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        let doc: serde_json::Value = serde_json::from_str(&raw).expect("the saved JSON is valid");
        assert!(
            doc.get("puid").is_none(),
            "the legacy top-level field must not be written back: {raw}"
        );
        assert!(
            doc.get("identity").is_some(),
            "the identity was written: {raw}"
        );
        let reloaded = load(&path).expect("the migrated file loads");
        assert_eq!(reloaded.identity, Some(identity));
        assert_eq!(reloaded.puid, None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn merging_prefs_keeps_unrelated_keys_and_merges_nested_objects() {
        let mut base = serde_json::json!({
            "appearance": {"theme": "crt-dark", "font_size_px": 13},
            "mute": {"ignore_all": false}
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let patch = serde_json::json!({"appearance": {"font_size_px": 14}})
            .as_object()
            .expect("the literal is an object")
            .clone();

        merge_objects(&mut base, &patch);

        assert_eq!(
            base.get("appearance").and_then(|value| value.get("theme")),
            Some(&serde_json::json!("crt-dark")),
            "a key the patch does not name is untouched"
        );
        assert_eq!(
            base.get("appearance")
                .and_then(|value| value.get("font_size_px")),
            Some(&serde_json::json!(14))
        );
        assert_eq!(
            base.get("mute").and_then(|value| value.get("ignore_all")),
            Some(&serde_json::json!(false))
        );
        assert_eq!(
            base.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["appearance", "mute"],
            "existing keys keep their position"
        );
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

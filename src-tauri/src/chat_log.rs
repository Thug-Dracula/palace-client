//! The chat transcript's file half.
//!
//! Chat logging has **two capabilities** and they must not be conflated:
//!
//! * **The in-memory log** is the backend transcript the runtime already keeps
//!   (`palace-client`'s `SessionState.chat`, capped at `CHAT_SCROLLBACK_CAP`
//!   lines and replayed to a window that opens late). It is always on, it needs
//!   no preference key, and it must keep working with file logging **off**.
//!   Nothing in this module touches it.
//! * **Saving the transcript to a file** is this module. It is off by default,
//!   writes only when `prefs.chat_log.to_file` is true, and is bounded by a
//!   size cap plus a rotation count so it can never grow without bound.
//!
//! # Where it goes
//!
//! The destination is `prefs.chat_log.path` when the user chose one; otherwise
//! it is [`default_path`], a **separate file** from the diagnostic log —
//! `chat-transcript.log` next to `palace-client.log` in the platform data
//! directory (`$XDG_DATA_HOME/org.palace.client/logs/` on this system). The
//! resolved path is announced in the diagnostic log when logging starts, and
//! the Preferences window shows it.
//!
//! # Guarantees
//!
//! * **Every line is flushed before the call returns.** The acceptance check
//!   reads the transcript while the app is still running; a file that only
//!   lands on a clean shutdown would fail it.
//! * **Bounded.** The active file is capped at `max_bytes`; when it fills it is
//!   rotated to `.1`, shifting older archives, and only `rotate_files` are
//!   kept. A cap of `0` rotations truncates the active file instead.
//! * **Never breaks the app.** A failed open or write is reported through the
//!   diagnostic log and turns the transcript off; it is never propagated and
//!   never panics. A write failure never touches the in-memory log.
//! * **Never a password or an identity secret.** The only input is a
//!   [`ChatLine`] — the name, the text, the kind and the sequence the user
//!   already sees in the chat window. The credential in `Settings` has no path
//!   into this module, and a `password` key inside the `chat_log` block is not a
//!   key this group reads.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::State;

use crate::logging::{self, Level};
use palace_client::{ChatKind, ChatLine};

/// The transcript's file name inside the log directory.
pub const CHAT_LOG_FILE: &str = "chat-transcript.log";

/// `prefs.chat_log.max_bytes` default: 8 MiB, matching `PREFERENCES.md`.
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// `prefs.chat_log.rotate_files` default: keep three archives, matching
/// `PREFERENCES.md`.
pub const DEFAULT_ROTATE_FILES: u32 = 3;

/// Smallest cap this build accepts (4 KiB), so a transcript is still useful.
pub const MIN_MAX_BYTES: u64 = 4 * 1024;

/// Largest cap this build accepts (1 GiB), so a bad value cannot fill a disk.
pub const MAX_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Most archives this build accepts.
pub const MAX_ROTATE_FILES: u32 = 20;

/// Longest single line the transcript will write before truncating.
const MAX_LINE: usize = 8192;

/// The four `prefs.chat_log.*` keys, resolved with the shipped defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLogConfig {
    /// `chat_log.to_file`: write the transcript to disk.
    pub to_file: bool,
    /// `chat_log.path`: the chosen destination, or `None` for the default.
    pub path: Option<PathBuf>,
    /// `chat_log.max_bytes`: rotate once the active file reaches this size.
    pub max_bytes: u64,
    /// `chat_log.rotate_files`: how many archives are kept.
    pub rotate_files: u32,
}

impl Default for ChatLogConfig {
    fn default() -> Self {
        ChatLogConfig {
            to_file: false,
            path: None,
            max_bytes: DEFAULT_MAX_BYTES,
            rotate_files: DEFAULT_ROTATE_FILES,
        }
    }
}

impl ChatLogConfig {
    /// Resolve the stored `chat_log` block, clamping values this build cannot
    /// honour and ignoring keys it does not own.
    ///
    /// A missing block, a non-object block or a mistyped value falls back to
    /// the default for that key; a stored number outside the accepted range is
    /// clamped rather than trusted, so a hand-edited file cannot produce an
    /// unbounded transcript. Only the four keys above are read — anything else
    /// in the block (including a stray `password`) is left alone.
    #[must_use]
    pub fn from_prefs(prefs: &Map<String, Value>) -> ChatLogConfig {
        let mut config = ChatLogConfig::default();
        let Some(block) = prefs.get("chat_log").and_then(Value::as_object) else {
            return config;
        };
        if let Some(value) = block.get("to_file").and_then(Value::as_bool) {
            config.to_file = value;
        }
        if let Some(value) = block.get("path").and_then(Value::as_str) {
            let trimmed = value.trim();
            config.path = if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            };
        }
        if let Some(value) = block.get("max_bytes").and_then(Value::as_u64) {
            config.max_bytes = value.clamp(MIN_MAX_BYTES, MAX_MAX_BYTES);
        }
        if let Some(value) = block.get("rotate_files").and_then(Value::as_u64) {
            config.rotate_files = (value as u32).min(MAX_ROTATE_FILES);
        }
        config
    }

    /// The file the transcript is written to: the chosen path, or the default.
    #[must_use]
    pub fn resolved_path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(default_path)
    }
}

/// Reject a `chat_log` patch this build cannot honour.
///
/// The Preferences shell calls this before the additive merge, so a bad value
/// is refused and the file keeps the previous one. A value of the wrong type,
/// or a number outside the accepted range, is an error — never silently
/// clamped at write time, so what the user sees stored is what runs.
pub fn validate_chat_log(block: &Map<String, Value>) -> Result<(), String> {
    if let Some(value) = block.get("to_file") {
        if !value.is_boolean() {
            return Err("prefs.chat_log.to_file must be a boolean".to_string());
        }
    }
    if let Some(value) = block.get("path") {
        if !value.is_null() && !value.is_string() {
            return Err("prefs.chat_log.path must be a string or null".to_string());
        }
    }
    if let Some(value) = block.get("max_bytes") {
        match value.as_u64() {
            Some(bytes) if (MIN_MAX_BYTES..=MAX_MAX_BYTES).contains(&bytes) => {}
            Some(_) => {
                return Err(format!(
                    "prefs.chat_log.max_bytes must be between {MIN_MAX_BYTES} and {MAX_MAX_BYTES}"
                ))
            }
            None => {
                return Err("prefs.chat_log.max_bytes must be a whole number".to_string());
            }
        }
    }
    if let Some(value) = block.get("rotate_files") {
        match value.as_u64() {
            Some(count) if count <= u64::from(MAX_ROTATE_FILES) => {}
            Some(_) => {
                return Err(format!(
                    "prefs.chat_log.rotate_files must be between 0 and {MAX_ROTATE_FILES}"
                ))
            }
            None => {
                return Err("prefs.chat_log.rotate_files must be a whole number".to_string());
            }
        }
    }
    Ok(())
}

/// The default transcript path: the log directory, in its own file.
///
/// The diagnostic log and the chat transcript share a directory but never a
/// file, so a busy room cannot push diagnostics out of the rotation window.
#[must_use]
pub fn default_path() -> PathBuf {
    logging::default_log_dir().join(CHAT_LOG_FILE)
}

/// The token written for a line's kind; stable so `grep` can rely on it.
#[must_use]
pub fn kind_token(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::Talk => "talk",
        ChatKind::Whisper => "whisper",
        ChatKind::System => "system",
        ChatKind::Error => "error",
    }
}

/// One transcript line, formatted for the file.
///
/// `2026-09-20T12:34:56.789Z talk Ada: hello`. The name and text are collapsed
/// to one line and capped, so chat content cannot forge a second line or an
/// unbounded record.
#[must_use]
pub fn format_line(line: &ChatLine) -> String {
    format!(
        "{} {:<8} {}: {}\n",
        logging::timestamp(),
        kind_token(line.kind),
        logging::sanitize(&line.name),
        logging::sanitize(&line.text)
    )
}

/// The longest a formatted line may be, for the tests' bound checks.
#[must_use]
pub fn max_line_len() -> usize {
    MAX_LINE
}

/// An open transcript file with its rotation policy.
///
/// This is the whole file half; it has no opinion about preferences. The
/// [`ChatLogService`] owns one of these while file logging is on.
#[derive(Debug)]
pub struct ChatLogSink {
    path: PathBuf,
    max_bytes: u64,
    rotate_files: u32,
    state: Mutex<SinkState>,
}

/// The open file and how much has been written to it.
#[derive(Debug)]
struct SinkState {
    file: Option<File>,
    written: u64,
}

impl ChatLogSink {
    /// Open `path` for appending, creating its parent directory.
    pub fn open(path: &Path, max_bytes: u64, rotate_files: u32) -> io::Result<ChatLogSink> {
        let state = SinkState::open(path)?;
        Ok(ChatLogSink {
            path: path.to_path_buf(),
            max_bytes,
            rotate_files,
            state: Mutex::new(state),
        })
    }

    /// The file this sink writes to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many bytes the active file holds.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        lock(&self.state).written
    }

    /// Append one already-formatted line, rotating first when the file is at
    /// its cap. The line is flushed before this returns.
    pub fn append(&self, line: &str) -> io::Result<()> {
        let mut state = lock(&self.state);
        state.append(line, &self.path, self.max_bytes, self.rotate_files)
    }
}

impl SinkState {
    fn open(path: &Path) -> io::Result<SinkState> {
        let file = open_append(path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(SinkState {
            file: Some(file),
            written,
        })
    }

    fn append(
        &mut self,
        line: &str,
        path: &Path,
        max_bytes: u64,
        rotate_files: u32,
    ) -> io::Result<()> {
        if self.written >= max_bytes {
            self.rotate(path, rotate_files)?;
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.write_all(line.as_bytes())?;
        file.flush()?;
        self.written = self.written.saturating_add(line.len() as u64);
        Ok(())
    }

    /// Close the active file, shift the archives down, and reopen empty.
    fn rotate(&mut self, path: &Path, rotate_files: u32) -> io::Result<()> {
        self.file = None;
        if rotate_files > 0 {
            let _ = fs::remove_file(archive_path(path, rotate_files));
            for index in (1..rotate_files).rev() {
                let from = archive_path(path, index);
                if from.exists() {
                    let _ = fs::rename(&from, archive_path(path, index + 1));
                }
            }
            let _ = fs::rename(path, archive_path(path, 1));
        } else {
            let _ = fs::remove_file(path);
        }
        self.file = Some(open_append(path)?);
        self.written = 0;
        Ok(())
    }
}

/// Create the parent directory and open `path` for appending.
fn open_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// `chat-transcript.log.1` for `index` 1, and so on.
#[must_use]
pub fn archive_path(path: &Path, index: u32) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// What the Preferences window and the chat panel can show about the file half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatLogStatus {
    /// Whether lines are being written right now.
    pub enabled: bool,
    /// The resolved destination, whether or not logging is on.
    pub path: String,
    /// The active file's cap in bytes.
    pub max_bytes: u64,
    /// How many archives are kept.
    pub rotate_files: u32,
    /// How much the active file holds; `0` while logging is off.
    pub bytes_written: u64,
}

/// The process-wide chat transcript writer, managed as Tauri state.
///
/// `configure` is the only way in: it takes the resolved preference block and
/// either opens, closes or reopens the file. `record` is the hot path — it is
/// called once per chat line by the event pump and is a no-op while logging is
/// off, so the in-memory transcript is untouched by this module's state.
#[derive(Debug, Default)]
pub struct ChatLogService {
    inner: Mutex<ServiceState>,
}

#[derive(Debug, Default)]
struct ServiceState {
    config: ChatLogConfig,
    sink: Option<ChatLogSink>,
}

impl ChatLogService {
    /// A service with file logging off.
    #[must_use]
    pub fn new() -> ChatLogService {
        ChatLogService::default()
    }

    /// Apply a resolved configuration, opening or closing the file as needed.
    ///
    /// Idempotent: an unchanged configuration does nothing, so an unrelated
    /// preference write never reopens the transcript. The resolved path is
    /// announced in the diagnostic log when the file is opened, so the user
    /// never has to guess where it went.
    pub fn configure(&self, config: ChatLogConfig) {
        let mut inner = lock(&self.inner);
        if inner.config == config {
            return;
        }
        if !config.to_file {
            if inner.sink.take().is_some() {
                logging::log(Level::Info, "chat transcript file logging stopped");
            }
            inner.config = config;
            return;
        }
        let path = config.resolved_path();
        match ChatLogSink::open(&path, config.max_bytes, config.rotate_files) {
            Ok(sink) => {
                logging::log(
                    Level::Info,
                    format!(
                        "chat transcript logging to {} (cap {} bytes, {} rotations)",
                        path.display(),
                        config.max_bytes,
                        config.rotate_files
                    ),
                );
                inner.sink = Some(sink);
            }
            Err(error) => {
                logging::log(
                    Level::Error,
                    format!(
                        "could not open the chat transcript at {}: {error}",
                        path.display()
                    ),
                );
                eprintln!(
                    "palace-app: could not open the chat transcript at {}: {error}",
                    path.display()
                );
                inner.sink = None;
            }
        }
        inner.config = config;
    }

    /// Apply the `chat_log` half of a preference block.
    pub fn configure_from_prefs(&self, prefs: &Map<String, Value>) {
        self.configure(ChatLogConfig::from_prefs(prefs));
    }

    /// Whether lines are being written to disk right now.
    #[must_use]
    pub fn enabled(&self) -> bool {
        lock(&self.inner).sink.is_some()
    }

    /// The current state, for the UI.
    #[must_use]
    pub fn status(&self) -> ChatLogStatus {
        let inner = lock(&self.inner);
        ChatLogStatus {
            enabled: inner.sink.is_some(),
            path: inner.config.resolved_path().to_string_lossy().into_owned(),
            max_bytes: inner.config.max_bytes,
            rotate_files: inner.config.rotate_files,
            bytes_written: inner.sink.as_ref().map_or(0, ChatLogSink::bytes_written),
        }
    }

    /// Write one transcript line, if file logging is on.
    ///
    /// The line is flushed before this returns. A write failure turns file
    /// logging off and says so once in the diagnostic log; it is never
    /// propagated, so a full disk cannot stop the room from working.
    pub fn record(&self, line: &ChatLine) {
        let mut inner = lock(&self.inner);
        let Some(sink) = inner.sink.as_ref() else {
            return;
        };
        if let Err(error) = sink.append(&format_line(line)) {
            logging::log(
                Level::Error,
                format!("chat transcript logging stopped after a write failed: {error}"),
            );
            eprintln!("palace-app: chat transcript logging stopped: {error}");
            inner.sink = None;
        }
    }
}

/// The transcript's current state: enabled, resolved path, cap and rotations.
///
/// Read-only: the four keys are written through `set_prefs` like every other
/// live preference, which is what reconfigures the writer.
#[tauri::command]
pub fn chat_log_status(service: State<'_, ChatLogService>) -> ChatLogStatus {
    service.status()
}

/// Lock a mutex, recovering the guard when a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(tag: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "palace-chat-log-{}-{tag}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
        directory
    }

    fn talk(seq: u64, name: &str, text: &str) -> ChatLine {
        ChatLine {
            seq,
            user_id: 1,
            name: name.to_string(),
            text: text.to_string(),
            kind: ChatKind::Talk,
        }
    }

    #[test]
    fn the_defaults_match_the_specification() {
        let config = ChatLogConfig::default();
        assert!(!config.to_file, "file logging is off until asked for");
        assert_eq!(config.path, None);
        assert_eq!(config.max_bytes, DEFAULT_MAX_BYTES);
        assert_eq!(config.max_bytes, 8 * 1024 * 1024);
        assert_eq!(config.rotate_files, DEFAULT_ROTATE_FILES);
        assert_eq!(config.rotate_files, 3);
    }

    #[test]
    fn the_stored_block_resolves_and_clamps() {
        let empty = Map::new();
        assert_eq!(ChatLogConfig::from_prefs(&empty), ChatLogConfig::default());

        let stored = serde_json::json!({
            "chat_log": {
                "to_file": true,
                "path": "  /tmp/room.log  ",
                "max_bytes": 4096,
                "rotate_files": 2
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let config = ChatLogConfig::from_prefs(&stored);
        assert!(config.to_file);
        assert_eq!(config.path, Some(PathBuf::from("/tmp/room.log")));
        assert_eq!(config.max_bytes, MIN_MAX_BYTES);
        assert_eq!(config.rotate_files, 2);
        assert_eq!(config.resolved_path(), PathBuf::from("/tmp/room.log"));

        let wild = serde_json::json!({
            "chat_log": {
                "path": "   ",
                "max_bytes": u64::MAX,
                "rotate_files": 999
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let clamped = ChatLogConfig::from_prefs(&wild);
        assert_eq!(
            clamped.path, None,
            "a blank path means the default location"
        );
        assert_eq!(clamped.max_bytes, MAX_MAX_BYTES);
        assert_eq!(clamped.rotate_files, MAX_ROTATE_FILES);

        let mistyped = serde_json::json!({
            "chat_log": { "to_file": "yes", "max_bytes": "big", "rotate_files": -1 }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        assert_eq!(
            ChatLogConfig::from_prefs(&mistyped),
            ChatLogConfig::default()
        );
    }

    #[test]
    fn a_stray_password_key_is_not_a_chat_log_key() {
        let block = serde_json::json!({
            "chat_log": { "to_file": true, "password": "hunter2" }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let config = ChatLogConfig::from_prefs(&block);
        assert!(config.to_file, "the real key is read");
        let debug = format!("{config:?}");
        assert!(
            !debug.contains("hunter2"),
            "an unrelated key never reaches the config: {debug}"
        );
    }

    #[test]
    fn validation_refuses_values_this_build_cannot_honour() {
        let good = serde_json::json!({
            "chat_log": {
                "to_file": true,
                "path": "/tmp/room.log",
                "max_bytes": 1048576,
                "rotate_files": 3
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_chat_log(&good).expect("well typed values are accepted");

        let null_path = serde_json::json!({ "chat_log": { "path": null } })
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_chat_log(&null_path).expect("the default location is a real choice");

        let zero_rotations = serde_json::json!({ "chat_log": { "rotate_files": 0 } })
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_chat_log(&zero_rotations).expect("truncate instead of rotate is accepted");

        for bad in [
            serde_json::json!({"chat_log": {"to_file": 1}}),
            serde_json::json!({"chat_log": {"path": 7}}),
            serde_json::json!({"chat_log": {"max_bytes": MIN_MAX_BYTES - 1}}),
            serde_json::json!({"chat_log": {"max_bytes": MAX_MAX_BYTES + 1}}),
            serde_json::json!({"chat_log": {"max_bytes": -1}}),
            serde_json::json!({"chat_log": {"max_bytes": 1.5}}),
            serde_json::json!({"chat_log": {"rotate_files": MAX_ROTATE_FILES + 1}}),
            serde_json::json!({"chat_log": {"rotate_files": -1}}),
        ] {
            let block = bad
                .get("chat_log")
                .and_then(Value::as_object)
                .expect("the literal has a chat_log object")
                .clone();
            assert!(
                validate_chat_log(&block).is_err(),
                "should be refused: {bad}"
            );
        }
    }

    #[test]
    fn a_line_is_on_disk_as_soon_as_it_is_written() {
        let directory = scratch("live");
        let path = directory.join(CHAT_LOG_FILE);
        let sink = ChatLogSink::open(&path, 1024 * 1024, 3).expect("the sink opens");
        sink.append(&format_line(&talk(1, "Ada", "first line")))
            .expect("the write succeeds");
        let first = std::fs::read_to_string(&path).expect("readable while still open");
        assert!(first.contains("talk     Ada: first line"), "{first}");
        sink.append(&format_line(&talk(2, "Ada", "second line")))
            .expect("the write succeeds");
        let second = std::fs::read_to_string(&path).expect("readable while still open");
        assert!(second.contains("first line"), "{second}");
        assert!(second.contains("second line"), "{second}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_line_that_spans_lines_stays_on_one() {
        let formatted = format_line(&talk(3, "Ada", "first\nsecond\r\nthird"));
        assert_eq!(formatted.lines().count(), 1, "{formatted}");
        assert!(
            formatted.contains("first\\nsecond\\r\\nthird"),
            "{formatted}"
        );
    }

    #[test]
    fn rotation_bounds_the_file_count_and_the_bytes() {
        let directory = scratch("rotate");
        let path = directory.join(CHAT_LOG_FILE);
        let cap = 400;
        let sink = ChatLogSink::open(&path, cap, 2).expect("the sink opens");
        for index in 0..500 {
            sink.append(&format!("{index:04} {}\n", "x".repeat(60)))
                .expect("the write succeeds");
        }
        assert!(path.is_file(), "the active file remains");
        assert!(archive_path(&path, 1).is_file(), "one archive is kept");
        assert!(archive_path(&path, 2).is_file(), "two archives are kept");
        assert!(
            !archive_path(&path, 3).exists(),
            "no archive past the cap is kept"
        );
        let total: u64 = [&path, &archive_path(&path, 1), &archive_path(&path, 2)]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        assert!(
            total < 500 * 66,
            "the transcript is bounded rather than 500 lines long: {total}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_zero_rotation_cap_truncates_instead_of_archiving() {
        let directory = scratch("truncate");
        let path = directory.join(CHAT_LOG_FILE);
        let sink = ChatLogSink::open(&path, 100, 0).expect("the sink opens");
        for index in 0..20 {
            sink.append(&format!("line {index} {}\n", "y".repeat(40)))
                .expect("the write succeeds");
        }
        assert!(path.is_file(), "the active file remains");
        assert!(
            !archive_path(&path, 1).exists(),
            "no archive is written when none are kept"
        );
        let len = std::fs::metadata(&path)
            .expect("the file has metadata")
            .len();
        assert!(len <= 200, "the active file stays near the cap: {len}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

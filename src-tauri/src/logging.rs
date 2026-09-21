//! The desktop shell's diagnostic log.
//!
//! The window is the only place the runtime's notes were ever visible, which
//! made every bug a copy-paste exercise for the person seeing it. This module
//! puts the same diagnostics on disk, at a stable path, written as they happen
//! so a freeze or a crash still leaves them readable.
//!
//! # Where it goes
//!
//! `PALACE_LOG_DIR` overrides the directory, which is what the tests use.
//! Otherwise the platform's data directory is used, under `org.palace.client`:
//!
//! * Unix: `$XDG_DATA_HOME/org.palace.client/logs/palace-client.log` (on this
//!   system `~/.local/share/org.palace.client/logs/palace-client.log`), or
//!   `$HOME/.local/share/...` when `XDG_DATA_HOME` is unset.
//! * Windows: `%LOCALAPPDATA%\org.palace.client\logs\palace-client.log`, or
//!   `%APPDATA%\...` when `LOCALAPPDATA` is unset.
//!
//! The Windows variables are checked before the `$HOME` fallback: a Git for
//! Windows or MSYS shell sets `HOME`, and with the Unix path checked first it
//! won on Windows, dropping the log in a stray `.local` directory. The temp
//! directory is the last resort.
//!
//! # Verbosity
//!
//! `PALACE_LOG` selects the level: `error`, `warn`, `info` (the default),
//! `debug` or `trace`. Unknown values fall back to `info`.
//!
//! # Window labels
//!
//! A window reports its lifecycle through [`WindowLog`], which prefixes every
//! line with the window's label (`[main]`, `[panel-users]`) so one greppable
//! file answers "which window did what". Creation, destruction, detach,
//! re-attach and layout restore are `info`; move and resize are `debug`, so
//! dragging a window does not flood the default output.
//!
//! # Guarantees
//!
//! * **Every line is flushed before the call returns.** A log that only reaches
//!   disk on a clean shutdown cannot describe a freeze, and a freeze is exactly
//!   when the log matters.
//! * **Panics are captured.** A panic hook records the message, the location
//!   and the thread, then still runs the previous hook so the panic reaches
//!   stderr unchanged.
//! * **Bounded.** The active file is capped at [`MAX_BYTES`]; when it fills it
//!   is rotated to `.1`, shifting older archives, and only [`MAX_ARCHIVES`] are
//!   kept.
//! * **Never breaks the app.** A failed write latches the logger off and says
//!   so once on stderr; it is never propagated and never panics.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The environment variable that selects the level.
pub const ENV_LOG: &str = "PALACE_LOG";

/// The environment variable that overrides the log directory.
pub const ENV_LOG_DIR: &str = "PALACE_LOG_DIR";

/// The log file's name inside the log directory.
pub const LOG_FILE: &str = "palace-client.log";

/// The bundle identifier the data directory is named after.
pub const APP_ID: &str = "org.palace.client";

/// Largest active file before it is rotated.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How many rotated files are kept (`.1` through `.N`).
pub const MAX_ARCHIVES: u32 = 2;

/// Longest single line the logger will write before truncating.
const MAX_LINE: usize = 8192;

/// Longest window label kept in a prefix.
const MAX_LABEL: usize = 64;

/// The prefix used when a window has no usable label.
const UNKNOWN_LABEL: &str = "unknown-window";

/// The process-wide logger, or empty until [`init`] runs.
static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Guards the one-off stderr note about a failed write.
static FAILURE_NOTED: AtomicBool = AtomicBool::new(false);

/// Guards panic-hook installation so two callers cannot chain the hook twice.
static HOOK: Once = Once::new();

/// How much the logger writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nothing at all.
    Off = 0,
    /// Failures that stop the app from doing what was asked.
    Error = 1,
    /// Something recoverable went wrong.
    Warn = 2,
    /// Lifecycle and the runtime's notes: the default.
    Info = 3,
    /// Per-frame detail.
    Debug = 4,
    /// Everything, including the noisiest internals.
    Trace = 5,
}

impl Level {
    /// Parse a `PALACE_LOG` value; `None` for a value that names no level.
    #[must_use]
    pub fn parse(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Level::Off),
            "error" | "err" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }

    /// The level `PALACE_LOG` names, defaulting to `info`.
    #[must_use]
    pub fn from_env() -> Level {
        std::env::var(ENV_LOG)
            .ok()
            .and_then(|value| Level::parse(&value))
            .unwrap_or(Level::Info)
    }

    fn label(self) -> &'static str {
        match self {
            Level::Off => "OFF",
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

/// A window lifecycle milestone worth a line in the log.
///
/// The shell and each panel report these through [`WindowLog`]; the token
/// written for each is stable so `grep` and tooling can rely on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowMilestone {
    /// A window was created.
    Created,
    /// A window was destroyed.
    Destroyed,
    /// A window moved without being resized.
    Moved,
    /// A window was resized.
    Resized,
    /// A panel left the main window and became its own OS window.
    Detached,
    /// A panel was folded back into the main window.
    Reattached,
    /// A saved geometry was applied to a window at startup.
    RestoredFromLayout,
}

impl WindowMilestone {
    /// Every milestone, in the order they are documented.
    pub const ALL: [WindowMilestone; 7] = [
        WindowMilestone::Created,
        WindowMilestone::Destroyed,
        WindowMilestone::Moved,
        WindowMilestone::Resized,
        WindowMilestone::Detached,
        WindowMilestone::Reattached,
        WindowMilestone::RestoredFromLayout,
    ];

    /// The token written to the log for this milestone.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WindowMilestone::Created => "created",
            WindowMilestone::Destroyed => "destroyed",
            WindowMilestone::Moved => "moved",
            WindowMilestone::Resized => "resized",
            WindowMilestone::Detached => "detached",
            WindowMilestone::Reattached => "reattached",
            WindowMilestone::RestoredFromLayout => "restored-from-layout",
        }
    }

    /// The level this milestone writes at unless the caller overrides it.
    ///
    /// Move and resize fire continuously while a window is dragged, so they
    /// stay at `debug`; the rest happen once per session and stay visible at
    /// the default level.
    #[must_use]
    pub fn level(self) -> Level {
        match self {
            WindowMilestone::Moved | WindowMilestone::Resized => Level::Debug,
            _ => Level::Info,
        }
    }
}

/// A window-scoped view of the log: every line it writes carries `[label]`.
///
/// Keep one per window for the window's lifetime, or call [`log_window`] for a
/// single event.
#[derive(Debug, Clone)]
pub struct WindowLog {
    label: String,
}

impl WindowLog {
    /// A logger for the window named `label`.
    ///
    /// The label is cleaned to ASCII identifier characters; an unusable label
    /// becomes `unknown-window`.
    #[must_use]
    pub fn new(label: &str) -> WindowLog {
        WindowLog {
            label: clean_label(label),
        }
    }

    /// The window's cleaned label, without the brackets.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Record a milestone at the level [`WindowMilestone::level`] picks.
    pub fn record(&self, milestone: WindowMilestone, detail: impl fmt::Display) {
        self.record_at(milestone.level(), milestone, detail);
    }

    /// Record a milestone at an explicit level.
    pub fn record_at(&self, level: Level, milestone: WindowMilestone, detail: impl fmt::Display) {
        log(
            level,
            format_window_message(&self.label, milestone, &detail.to_string()),
        );
    }
}

/// Record one window milestone through the process logger.
///
/// Shorthand for a single event; a window that logs repeatedly should keep a
/// [`WindowLog`] instead. The level is [`WindowMilestone::level`].
pub fn log_window(label: &str, milestone: WindowMilestone, detail: impl fmt::Display) {
    WindowLog::new(label).record(milestone, detail);
}

/// The `[label]` prefix for a window line.
#[must_use]
pub fn window_prefix(label: &str) -> String {
    format!("[{}]", clean_label(label))
}

/// One log target with its rotation policy.
struct Logger {
    level: Level,
    path: PathBuf,
    max_bytes: u64,
    max_archives: u32,
    state: Mutex<State>,
    failed: AtomicBool,
}

/// The open file and how much has been written to it.
struct State {
    file: Option<File>,
    written: u64,
}

impl Logger {
    fn open(dir: &Path, level: Level, max_bytes: u64, max_archives: u32) -> io::Result<Logger> {
        let path = dir.join(LOG_FILE);
        let state = State::open(&path)?;
        Ok(Logger {
            level,
            path,
            max_bytes,
            max_archives,
            state: Mutex::new(state),
            failed: AtomicBool::new(false),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Write one line, rotating first when the active file is at its cap.
    fn write(&self, level: Level, message: &str) {
        if level > self.level || self.failed.load(Ordering::Relaxed) {
            return;
        }
        let line = format!(
            "{} {:<5} {}\n",
            timestamp(),
            level.label(),
            sanitize(message)
        );
        let result = match self.state.lock() {
            Ok(mut state) => state.append(&line, &self.path, self.max_bytes, self.max_archives),
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.append(&line, &self.path, self.max_bytes, self.max_archives)
            }
        };
        if let Err(error) = result {
            self.failed.store(true, Ordering::Relaxed);
            if !FAILURE_NOTED.swap(true, Ordering::Relaxed) {
                eprintln!("palace-app: logging stopped after a write failed: {error}");
            }
        }
    }
}

impl State {
    fn open(path: &Path) -> io::Result<State> {
        let file = open_append(path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(State {
            file: Some(file),
            written,
        })
    }

    fn append(
        &mut self,
        line: &str,
        path: &Path,
        max_bytes: u64,
        max_archives: u32,
    ) -> io::Result<()> {
        if self.written >= max_bytes {
            self.rotate(path, max_archives)?;
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
    fn rotate(&mut self, path: &Path, max_archives: u32) -> io::Result<()> {
        self.file = None;
        if max_archives > 0 {
            let _ = fs::remove_file(archive_path(path, max_archives));
            for index in (1..max_archives).rev() {
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

/// `palace-client.log.1` for `index` 1, and so on.
fn archive_path(path: &Path, index: u32) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Write one line through the process logger, if it is installed and the level
/// admits it.
pub fn log(level: Level, message: impl fmt::Display) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    if level > logger.level {
        return;
    }
    logger.write(level, &message.to_string());
}

/// The log file this process is writing to, once [`init`] has run.
#[must_use]
pub fn path() -> Option<&'static Path> {
    LOGGER.get().map(Logger::path)
}

/// Install the logger at `dir`, returning the file it writes to.
///
/// The first caller wins; later callers get the already-installed path. The
/// panic hook is installed once, here, so every panic after startup lands in
/// the file.
pub fn init_at(dir: &Path, level: Level) -> io::Result<PathBuf> {
    if let Some(existing) = LOGGER.get() {
        return Ok(existing.path.clone());
    }
    let logger = Logger::open(dir, level, MAX_BYTES, MAX_ARCHIVES)?;
    let path = logger.path.clone();
    let _ = LOGGER.set(logger);
    install_panic_hook();
    log(
        Level::Info,
        format!(
            "palace-app {} logging to {} (level={})",
            env!("CARGO_PKG_VERSION"),
            path.display(),
            level.label()
        ),
    );
    Ok(path)
}

/// Install the logger at the standard path with the `PALACE_LOG` level.
pub fn init() -> io::Result<PathBuf> {
    init_at(&default_log_dir(), Level::from_env())
}

/// The directory the logs live in: `PALACE_LOG_DIR`, else the platform data
/// directory under [`APP_ID`].
///
/// Shared with the chat transcript (`crate::chat_log::default_path`), which
/// lives in the same directory in its own file: one place to look for this
/// install's logs, two files that cannot push each other out of rotation.
#[must_use]
pub fn default_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_LOG_DIR).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    log_dir_from(
        std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        std::env::temp_dir(),
    )
}

/// `XDG_DATA_HOME`, then `LOCALAPPDATA`, then `APPDATA`, then
/// `$HOME/.local/share`, then the temp dir.
///
/// `XDG_DATA_HOME` stays first because when it is set it is an explicit
/// choice. The Windows variables come before `HOME` so the native location
/// wins on Windows — Git for Windows and MSYS shells do set `HOME`, and with
/// the Unix path first it shadowed `LOCALAPPDATA`. The candidates are passed
/// in rather than read, so the Windows rungs are testable from Linux — a
/// Windows-only `cfg` would leave the precedence unverified on the machine
/// this is developed on.
fn log_dir_from(
    xdg_data_home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    app_data: Option<PathBuf>,
    home: Option<PathBuf>,
    temp: PathBuf,
) -> PathBuf {
    xdg_data_home
        .or(local_app_data)
        .or(app_data)
        .or_else(|| home.map(|home| home.join(".local").join("share")))
        .unwrap_or(temp)
        .join(APP_ID)
        .join("logs")
}

/// Route panics into the log and then run the hook that was installed before.
fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = if let Some(text) = info.payload().downcast_ref::<&str>() {
                (*text).to_string()
            } else if let Some(text) = info.payload().downcast_ref::<String>() {
                text.clone()
            } else {
                "non-string panic payload".to_string()
            };
            let location = info.location().map_or_else(
                || "<unknown>".to_string(),
                |site| format!("{}:{}:{}", site.file(), site.line(), site.column()),
            );
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            log(
                Level::Error,
                format!("panic on thread {name} at {location}: {payload}"),
            );
            previous(info);
        }));
    });
}

/// Reduce a window label to ASCII identifier characters.
///
/// A label carrying a space, a `]` or a newline could otherwise split a line
/// or fake another window's prefix. An empty result becomes [`UNKNOWN_LABEL`]
/// so every line still names a window.
fn clean_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .take(MAX_LABEL)
        .collect();
    if cleaned.is_empty() {
        UNKNOWN_LABEL.to_string()
    } else {
        cleaned
    }
}

/// The message body of a window line, without the timestamp and level the
/// logger adds.
fn format_window_message(label: &str, milestone: WindowMilestone, detail: &str) -> String {
    if detail.is_empty() {
        format!("{} {}", window_prefix(label), milestone.as_str())
    } else {
        format!("{} {} {}", window_prefix(label), milestone.as_str(), detail)
    }
}

/// Collapse a message to one line and cap its length.
pub(crate) fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_LINE));
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
        if out.len() >= MAX_LINE {
            out.push_str("...<truncated>");
            break;
        }
    }
    out
}

/// `2026-09-18T12:34:56.789Z` from the wall clock.
pub(crate) fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (year, month, day, hour, minute, second) = civil_from_secs(now.as_secs() as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        now.subsec_millis()
    )
}

/// Days-to-civil conversion (Howard Hinnant's `civil_from_days`), with the
/// time-of-day split off first.
fn civil_from_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    (
        year,
        month as u32,
        day as u32,
        hour as u32,
        minute as u32,
        second as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn temp_dir(tag: &str) -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("palace-app-log-{}-{tag}-{seq}", std::process::id()))
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("the log file is readable")
    }

    #[test]
    fn levels_parse_by_name_and_order_by_severity() {
        assert_eq!(Level::parse("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("banana"), None);
        assert!(Level::Info < Level::Debug);
        assert!(Level::Error < Level::Info);
        assert!(Level::Off < Level::Error);
    }

    #[test]
    fn the_default_directory_ends_in_the_log_folder() {
        let dir = default_log_dir();
        assert!(dir.ends_with(Path::new(APP_ID).join("logs")), "{dir:?}");
    }

    #[test]
    fn the_default_directory_prefers_the_variable_each_platform_sets() {
        let temp = PathBuf::from("temp-fallback");
        let local = PathBuf::from("C:/Users/x/AppData/Local");
        let roaming = PathBuf::from("C:/Users/x/AppData/Roaming");
        assert_eq!(
            log_dir_from(
                None,
                Some(local.clone()),
                Some(roaming.clone()),
                None,
                temp.clone()
            ),
            local.join(APP_ID).join("logs"),
            "Windows sets LOCALAPPDATA and neither of the Unix variables"
        );

        // The regression this pins: Git for Windows and MSYS set HOME, so the
        // home-relative Unix path used to shadow the Windows data directory.
        let home = PathBuf::from("home");
        let with_home = log_dir_from(
            None,
            Some(local.clone()),
            Some(roaming.clone()),
            Some(home.clone()),
            temp.clone(),
        );
        assert_eq!(with_home, local.join(APP_ID).join("logs"));
        assert!(
            !with_home.starts_with(&home),
            "the Unix home must not shadow LOCALAPPDATA"
        );

        assert_eq!(
            log_dir_from(None, None, Some(roaming.clone()), None, temp.clone()),
            roaming.join(APP_ID).join("logs"),
            "APPDATA is the fallback when LOCALAPPDATA is unset"
        );

        let xdg = PathBuf::from("/xdg");
        assert_eq!(
            log_dir_from(
                Some(xdg.clone()),
                Some(local),
                Some(roaming),
                None,
                temp.clone()
            ),
            xdg.join(APP_ID).join("logs"),
            "an explicit XDG_DATA_HOME wins over the platform default"
        );

        assert_eq!(
            log_dir_from(None, None, None, None, temp.clone()),
            temp.join(APP_ID).join("logs"),
            "the temp dir is the last resort"
        );
    }

    #[test]
    fn a_line_is_on_disk_as_soon_as_it_is_written() {
        let dir = temp_dir("live");
        let logger = Logger::open(&dir, Level::Debug, MAX_BYTES, MAX_ARCHIVES).expect("opens");
        logger.write(Level::Info, "before the process ends");
        let text = read(logger.path());
        assert!(text.contains("INFO  before the process ends"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_level_below_the_threshold_is_not_written() {
        let dir = temp_dir("filter");
        let logger = Logger::open(&dir, Level::Info, MAX_BYTES, MAX_ARCHIVES).expect("opens");
        logger.write(Level::Debug, "too chatty");
        logger.write(Level::Error, "kept");
        let text = read(logger.path());
        assert!(!text.contains("too chatty"), "{text}");
        assert!(text.contains("ERROR kept"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_bounds_the_file_count() {
        let dir = temp_dir("rotate");
        let logger = Logger::open(&dir, Level::Info, 200, 2).expect("opens");
        for index in 0..200 {
            logger.write(Level::Info, &format!("line {index} {}", "x".repeat(40)));
        }
        let active = logger.path().to_path_buf();
        assert!(active.is_file(), "the active file remains");
        let archive_one = archive_path(&active, 1);
        let archive_two = archive_path(&active, 2);
        assert!(archive_one.is_file(), "one archive is kept");
        assert!(archive_two.is_file(), "two archives are kept");
        assert!(
            !archive_path(&active, 3).exists(),
            "no archive past the cap is kept"
        );
        let total: u64 = [&active, &archive_one, &archive_two]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        assert!(total <= 200 * 4, "the total stays within the cap: {total}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_that_spans_lines_stays_on_one() {
        let text = sanitize("first\nsecond\r\nthird");
        assert_eq!(text, "first\\nsecond\\r\\nthird");
        let long = sanitize(&"y".repeat(MAX_LINE + 50));
        assert!(long.ends_with("...<truncated>"), "{long}");
        assert!(long.len() <= MAX_LINE + "...<truncated>".len());
    }

    #[test]
    fn civil_time_converts_a_known_epoch() {
        assert_eq!(
            civil_from_secs(1_789_603_200),
            (2026, 9, 17, 0, 0, 0),
            "the formatter agrees with a known instant"
        );
        assert_eq!(civil_from_secs(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn window_prefixes_name_the_window() {
        assert_eq!(window_prefix("main"), "[main]");
        assert_eq!(window_prefix("panel-users"), "[panel-users]");
        assert_eq!(
            window_prefix("panel users ] [main"),
            "[panelusersmain]",
            "a label cannot forge a second prefix or split the line"
        );
        assert_eq!(window_prefix(""), "[unknown-window]");
        assert_eq!(window_prefix("   "), "[unknown-window]");
        assert_eq!(WindowLog::new("main").label(), "main");
        assert_eq!(WindowLog::new("").label(), UNKNOWN_LABEL);
    }

    #[test]
    fn milestones_have_stable_tokens() {
        let tokens: Vec<&str> = WindowMilestone::ALL.iter().map(|m| m.as_str()).collect();
        assert_eq!(
            tokens,
            vec![
                "created",
                "destroyed",
                "moved",
                "resized",
                "detached",
                "reattached",
                "restored-from-layout",
            ]
        );
        assert_eq!(
            format_window_message("main", WindowMilestone::Destroyed, ""),
            "[main] destroyed",
            "a milestone without detail still names the window"
        );
    }

    #[test]
    fn move_and_resize_stay_quiet_at_the_default_level() {
        assert_eq!(WindowMilestone::Moved.level(), Level::Debug);
        assert_eq!(WindowMilestone::Resized.level(), Level::Debug);
        for milestone in [
            WindowMilestone::Created,
            WindowMilestone::Destroyed,
            WindowMilestone::Detached,
            WindowMilestone::Reattached,
            WindowMilestone::RestoredFromLayout,
        ] {
            assert_eq!(
                milestone.level(),
                Level::Info,
                "{milestone:?} is a lifecycle event"
            );
        }

        let dir = temp_dir("window-levels");
        let logger = Logger::open(&dir, Level::Info, MAX_BYTES, MAX_ARCHIVES).expect("opens");
        logger.write(
            WindowMilestone::Moved.level(),
            &format_window_message("main", WindowMilestone::Moved, "x=1 y=2"),
        );
        logger.write(
            WindowMilestone::Created.level(),
            &format_window_message("main", WindowMilestone::Created, "label=main"),
        );
        let text = read(logger.path());
        assert!(
            !text.contains("moved"),
            "a drag stays below the default level: {text}"
        );
        assert!(text.contains("INFO  [main] created label=main"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_line_carries_its_label_and_milestone() {
        let dir = temp_dir("window-line");
        let logger = Logger::open(&dir, Level::Debug, MAX_BYTES, MAX_ARCHIVES).expect("opens");
        logger.write(
            WindowMilestone::Detached.level(),
            &format_window_message(
                "panel-users",
                WindowMilestone::Detached,
                "label=panel-users",
            ),
        );
        logger.write(
            WindowMilestone::RestoredFromLayout.level(),
            &format_window_message("main", WindowMilestone::RestoredFromLayout, "x=0 y=0"),
        );
        let text = read(logger.path());
        assert!(
            text.contains("INFO  [panel-users] detached label=panel-users"),
            "{text}"
        );
        assert!(
            text.contains("INFO  [main] restored-from-layout x=0 y=0"),
            "{text}"
        );

        let prefix = window_prefix("panel-users");
        assert!(prefix.starts_with('[') && prefix.ends_with(']'));
        assert_eq!(
            prefix.matches(']').count(),
            1,
            "the prefix cannot nest brackets"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

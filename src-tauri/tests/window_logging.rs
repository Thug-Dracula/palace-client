//! Proves window-lifecycle lines reach the log while the process that wrote
//! them is still running.
//!
//! The live proof runs a second copy of this test binary as a real child
//! process: the child installs the logger, reports lifecycle events for `main`
//! and `panel-users`, and stays alive while the parent reads the file. A log
//! buffered until exit would fail that read.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use palace_app_lib::logging::{self, Level, WindowLog, WindowMilestone};

/// Set on the child only; it gates the writer test body.
const CHILD_ENV: &str = "PALACE_WINDOW_LOG_CHILD";

/// The test the parent runs inside the child.
const CHILD_TEST: &str = "child_writer_stays_alive_after_logging";

/// How long the parent waits for the child's lines.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the child stays alive after writing; the parent stops it sooner.
const CHILD_LIFETIME: Duration = Duration::from_secs(120);

/// Stops the child even when an assertion above it fails.
struct ChildGuard {
    child: Child,
    stop: PathBuf,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.stop, b"stop");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn child_writer_stays_alive_after_logging() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let dir = std::env::var_os(logging::ENV_LOG_DIR)
        .map(PathBuf::from)
        .expect("the parent passes the log directory");
    logging::init_at(&dir, Level::from_env()).expect("the child log initializes");

    WindowLog::new("main").record(WindowMilestone::Created, "label=main size=1200x820");
    let users = WindowLog::new("panel-users");
    users.record(WindowMilestone::Created, "label=panel-users detached=true");
    users.record(WindowMilestone::Moved, "x=10 y=20");

    let stop = dir.join("stop");
    let deadline = Instant::now() + CHILD_LIFETIME;
    while !stop.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `2026-09-19T12:34:56.789Z ` — the prefix the logger puts on every line.
fn starts_with_timestamp(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() > 24
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && bytes[24] == b' '
}

#[test]
fn lifecycle_lines_are_readable_while_the_writer_is_still_running() {
    let dir = std::env::temp_dir().join(format!("palace-app-window-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the log directory is created");

    let exe = std::env::current_exe().expect("the test binary path");
    let child = Command::new(exe)
        .arg(CHILD_TEST)
        .env(CHILD_ENV, "1")
        .env(logging::ENV_LOG_DIR, &dir)
        .env(logging::ENV_LOG, "debug")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the child writer starts");
    let mut guard = ChildGuard {
        child,
        stop: dir.join("stop"),
    };

    let log_path = dir.join(logging::LOG_FILE);
    let announcement = format!("logging to {}", log_path.display());
    let deadline = Instant::now() + READ_TIMEOUT;
    let text = loop {
        let current = std::fs::read_to_string(&log_path).unwrap_or_default();
        if current.contains("[main] created")
            && current.contains("[panel-users] created")
            && current.contains("[panel-users] moved")
            && current.contains(&announcement)
        {
            break current;
        }
        if let Some(status) = guard.child.try_wait().expect("the child status") {
            panic!("the child exited before writing every line ({status}):\n{current}");
        }
        assert!(
            Instant::now() < deadline,
            "the child never wrote every line within {READ_TIMEOUT:?}:\n{current}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        guard.child.try_wait().expect("the child status").is_none(),
        "the writer is still running while its lines are read"
    );

    let main_line = text
        .lines()
        .find(|line| line.contains("[main] created"))
        .expect("the main line is present");
    assert!(
        starts_with_timestamp(main_line),
        "the lifecycle line carries a timestamp: {main_line}"
    );
    let users_line = text
        .lines()
        .find(|line| line.contains("[panel-users] created"))
        .expect("the panel line is present");
    assert!(
        starts_with_timestamp(users_line),
        "the panel line carries a timestamp: {users_line}"
    );

    println!("--- log read while the writer is still running ---");
    print!("{text}");
    println!("--- end of live log ---");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panic_lands_in_the_log_and_still_reaches_the_previous_hook() {
    static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

    let dir = std::env::temp_dir().join(format!("palace-app-window-panic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    std::panic::set_hook(Box::new(|info| {
        PREVIOUS_RAN.store(true, Ordering::SeqCst);
        eprintln!("previous hook saw: {info}");
    }));
    logging::init_at(&dir, Level::Debug).expect("the log initializes");

    let caught = std::panic::catch_unwind(|| panic!("window-logging canary"));
    assert!(caught.is_err(), "the panic unwinds");
    assert!(
        PREVIOUS_RAN.load(Ordering::SeqCst),
        "the hook installed before logging still runs"
    );

    let text = std::fs::read_to_string(dir.join(logging::LOG_FILE)).expect("the log is readable");
    assert!(text.contains("window-logging canary"), "{text}");
    assert!(
        text.contains("window_logging.rs"),
        "the panic location is recorded: {text}"
    );

    println!("--- log after a deliberate panic ---");
    print!("{text}");
    println!("--- end of log ---");

    let _ = std::fs::remove_dir_all(&dir);
}

//! Reusable multi-window test harness on a virtual display (plan Task 9).
//!
//! The `tauri::test` mock runtime cannot stand in for real webviews: it models
//! the window registry but has no JS engine and no native window, so it can
//! never prove that a panel boots, receives events, or answers a command. The
//! Phase-0 spike (`.omo/evidence/spike-findings.md`, plan Task 1) settled that.
//! This module is the real thing: it launches a real Tauri app from the
//! production context (same config, capabilities and asset routing) on the
//! real runtime, under a **proven virtual display**, and exposes the verbs
//! later tasks need:
//!
//! * [`Harness::launch`] / [`Harness::launch_with`] — start the app on a
//!   background thread and leave the test thread free to drive it;
//! * [`Harness::wait_until_ready`] — wait until a window's real webview has run
//!   injected JavaScript and called back into Rust;
//! * [`Harness::assert_window`], [`Harness::wait_for_window`],
//!   [`Harness::window_labels`] — assert against the live window registry;
//! * [`Harness::open_panel`] / [`Harness::close_panel`] — open and close a real
//!   second OS window and watch it appear and disappear;
//! * [`Harness::mark_log`], [`Harness::read_log_since`],
//!   [`Harness::wait_for_log_since`] — read the on-disk diagnostic log while
//!   the app is still running;
//! * [`Harness::eval_in_window`] — run JavaScript in a window (the hook a
//!   later task uses to call the production `open_panel` from the SPA side);
//! * [`Harness::quit`] — stop the app and get its exit code.
//!
//! # It never touches the user's session
//!
//! [`virtual_display`] refuses to return unless the display in use is **proven
//! virtual**: `DISPLAY` must be served by an `Xvfb` process, or
//! `WAYLAND_DISPLAY` must belong to a running `gamescope`. A bare desktop
//! session — including the developer's own — is a skip, not a test. The same
//! check runs again inside [`Harness::launch`], so a test cannot bypass it.
//!
//! # It does not depend on a dev server
//!
//! In a debug build Tauri resolves `WebviewUrl::App` against `build.devUrl`
//! and treats that origin as local for IPC. Nothing is normally listening on
//! `localhost:1420` during `cargo test`, and a failed page load means the
//! injected probe never runs — measured: the same spike in a network namespace
//! with nothing listening produced zero reports. The harness therefore starts
//! its own one-page HTTP server on an ephemeral loopback port and points the
//! app's dev URL at it, so every window loads a real document deterministically
//! and a page-load failure is a real failure, never a silent one.
//!
//! # One app per process
//!
//! GTK exports a process-global `GtkApplication` object, so a second Tauri app
//! in the same test binary cannot start. The harness allows one launch per
//! process and returns a clear error for a second attempt; a test binary that
//! needs several real-window scenarios runs each in its own process (the
//! pattern `tests/window_logging.rs` uses for its live-log child).
//!
//! # Running it
//!
//! ```text
//! xvfb-run -a cargo test -p palace-app --test multiwindow_harness -- --nocapture --test-threads=1
//! ```
//!
//! Without a display — or on a real desktop session — every display-gated test
//! prints `SKIP …` and passes, so `cargo test` on a headless box stays green
//! without pretending the harness ran. The gate tests below are pure and run
//! everywhere.
//!
//! # Using it from another integration test
//!
//! ```ignore
//! #[path = "multiwindow_harness.rs"]
//! mod multiwindow_harness;
//!
//! use multiwindow_harness::{virtual_display, Harness};
//! ```
//!
//! The acceptance test at the bottom runs only in the `multiwindow_harness`
//! binary, so an included copy does not re-run it.

#![cfg(target_os = "linux")]
// The module is a library for other test binaries that include it by path, so
// a helper the acceptance test does not use itself is not dead code.
#![allow(dead_code)]

use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use palace_app_lib::logging::{self, Level, WindowLog, WindowMilestone};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// How long a window may take to appear, boot and report ready.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the app may take to build and reach its event loop.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the event loop gets to stop before the harness gives up on it.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Backstop: force the app to exit if a test wedges, so CI cannot hang.
const WATCHDOG: Duration = Duration::from_secs(180);

/// Poll interval while waiting for a window or a log line.
const POLL: Duration = Duration::from_millis(50);

/// How often a window without a ready report is poked with the probe script.
const POKE: Duration = Duration::from_millis(400);

/// The test binary that owns the acceptance test.
const HARNESS_BINARY: &str = "multiwindow_harness";

// ---------------------------------------------------------------------------
// Display gating
// ---------------------------------------------------------------------------

/// A display that has been proven to be virtual, with the value it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayKind {
    /// X11 served by an `Xvfb` process, as `xvfb-run` sets up.
    Xvfb(String),
    /// Wayland served by a headless `gamescope`.
    Gamescope(String),
}

impl DisplayKind {
    /// One greppable line naming the display and the proof it is virtual.
    #[must_use]
    pub fn provenance(&self) -> String {
        match self {
            DisplayKind::Xvfb(display) => format!("xvfb DISPLAY={display}"),
            DisplayKind::Gamescope(wayland) => format!("gamescope WAYLAND_DISPLAY={wayland}"),
        }
    }
}

/// The virtual display the harness may run on, or why it must skip.
///
/// This is the single safety gate: a display is accepted only when a process
/// on this machine proves it is virtual (an `Xvfb` serving the exact
/// `DISPLAY`, or a `gamescope` with `WAYLAND_DISPLAY` set). Everything else —
/// including the developer's real session — returns `Err` so the caller skips.
pub fn virtual_display() -> Result<DisplayKind, String> {
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|value| !value.is_empty());
    let wayland = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|value| !value.is_empty());
    gate_from(display.as_deref(), wayland.as_deref(), &process_commands())
}

/// The pure decision behind [`virtual_display`], with the process list passed
/// in so it is testable without a display.
fn gate_from(
    display: Option<&str>,
    wayland: Option<&str>,
    processes: &[String],
) -> Result<DisplayKind, String> {
    if let Some(display) = display {
        if xvfb_serves(display, processes) {
            return Ok(DisplayKind::Xvfb(display.to_string()));
        }
    }
    if let Some(wayland) = wayland {
        if gamescope_present(processes) {
            return Ok(DisplayKind::Gamescope(wayland.to_string()));
        }
    }
    if display.is_none() && wayland.is_none() {
        return Err("no DISPLAY/WAYLAND_DISPLAY is set; run the harness under \
             `xvfb-run -a cargo test -p palace-app --test multiwindow_harness`"
            .to_string());
    }
    Err(format!(
        "DISPLAY={display:?} / WAYLAND_DISPLAY={wayland:?} is not a proven virtual display \
         (no Xvfb serving it, no gamescope); the harness must never touch the user's session — \
         run it under `xvfb-run -a` or headless gamescope"
    ))
}

/// Whether an `Xvfb` process serves exactly this local X11 display.
fn xvfb_serves(display: &str, processes: &[String]) -> bool {
    let Some((host, rest)) = display.rsplit_once(':') else {
        return false;
    };
    if !host.is_empty() {
        return false; // a remote display is not our local virtual one
    }
    let number = rest.split('.').next().unwrap_or(rest);
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    let token = format!(":{number}");
    processes.iter().any(|command| {
        program_is(command, "Xvfb") && command.split_whitespace().any(|arg| arg == token)
    })
}

/// Whether a `gamescope` process is running.
fn gamescope_present(processes: &[String]) -> bool {
    processes
        .iter()
        .any(|command| program_is(command, "gamescope"))
}

/// Whether the first word of a `/proc/<pid>/cmdline` line names this program.
fn program_is(command: &str, name: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|file| file.to_str())
        .is_some_and(|file| file == name)
}

/// Every process command line, with the argument separators turned into
/// spaces. Empty when `/proc` is unreadable, which the gate treats as "cannot
/// prove virtual" and therefore skips — the safe direction.
fn process_commands() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|ch| ch.is_ascii_digit())
        })
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .collect()
}

// ---------------------------------------------------------------------------
// The page every window loads
// ---------------------------------------------------------------------------

/// A one-page HTTP server on an ephemeral loopback port.
///
/// It exists so `WebviewUrl::App` has something real to resolve against: a
/// debug build points at `build.devUrl`, and without a server the load fails
/// and the injected probe never runs (measured — see the module docs). The
/// listener thread is detached and lives as long as the test process.
struct PageServer {
    origin: String,
}

impl PageServer {
    fn origin(&self) -> &str {
        &self.origin
    }
}

/// The stand-in document. It only has to load: the probe that proves the
/// webview is real is injected by Tauri, not by this page.
const PAGE_HTML: &str = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
<title>Palace harness</title></head>\n<body><p id=\"harness-page\">Palace multi-window \
harness page</p></body></html>\n";

fn start_page_server() -> Result<PageServer, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("could not bind the harness page server: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not read the harness page server port: {error}"))?
        .port();
    std::thread::Builder::new()
        .name("palace-harness-page".to_string())
        .spawn(move || serve(listener))
        .map_err(|error| format!("could not start the harness page server: {error}"))?;
    Ok(PageServer {
        origin: format!("http://127.0.0.1:{port}"),
    })
}

/// Answer every request with the stand-in page. One thread per connection so a
/// client that opens a socket and says nothing cannot stall the rest.
fn serve(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                PAGE_HTML.len(),
                PAGE_HTML
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });
    }
}

// ---------------------------------------------------------------------------
// Reports from inside the webviews
// ---------------------------------------------------------------------------

/// `(window label, report kind)` pairs sent by the injected probe.
static REPORTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// One harness at a time: two live event loops would fight over the same
/// display and the same registry.
static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

/// Whether this process has already started an app.
///
/// GTK exports a process-global `GtkApplication` D-Bus object, so a second
/// Tauri app in the same process cannot start (tao: "An object is already
/// exported for the interface org.gtk.Application"). The harness therefore
/// allows exactly one launch per test binary and says so clearly instead of
/// failing with a platform panic. A test binary that needs several real-window
/// scenarios should run each in its own process, the way
/// `tests/window_logging.rs` re-executes itself for its live-log child.
static LAUNCHED: AtomicBool = AtomicBool::new(false);

fn reports() -> MutexGuard<'static, Vec<(String, String)>> {
    REPORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The command the injected probe calls: proof that a real webview executed
/// our JavaScript and can reach Rust. Application commands are not ACL-gated
/// in this app (no app ACL manifest exists), so no capability is needed.
///
/// It lives in a child module because `#[tauri::command]` emits a
/// `#[macro_export]` macro plus a `use` of it, which collides with itself at a
/// crate root.
pub mod command {
    use palace_app_lib::logging::{self, Level};

    use super::reports;

    #[tauri::command]
    pub fn harness_report(label: String, kind: String) {
        let line = format!("{} harness {kind}", logging::window_prefix(&label));
        logging::log(Level::Info, &line);
        println!("{line}");
        reports().push((label, kind));
    }

    /// A stand-in for a production command a later task registers through
    /// [`Harness::launch_with`]; it only has to land in the log.
    #[tauri::command]
    pub fn harness_ping(note: String) {
        let line = format!("harness_ping note={note}");
        logging::log(Level::Info, &line);
        println!("{line}");
    }
}

/// Whether `label` has reported ready.
fn has_report(label: &str) -> bool {
    reports()
        .iter()
        .any(|(who, kind)| who == label && kind == "ready")
}

/// The probe, as JavaScript. It waits for Tauri's IPC bootstrap, then calls
/// [`harness_report`] once. The label is embedded as a JSON string literal so
/// it can never escape into code.
fn boot_js(label: &str) -> String {
    let label = serde_json::to_string(label).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function () {{
  function boot() {{
    var internals = window.__TAURI_INTERNALS__;
    if (!internals || !internals.invoke) {{ return setTimeout(boot, 40); }}
    if (window.__palaceHarnessReady) {{ return; }}
    window.__palaceHarnessReady = true;
    internals.invoke('harness_report', {{ label: {label}, kind: 'ready' }}).catch(function () {{}});
  }}
  boot();
}})();"#
    )
}

/// Whether a window label is safe to put in a URL and a JS literal.
fn is_safe_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// Whether this test binary is the one that owns the acceptance test. An
/// included copy of the module (see the module docs) sees its own crate name
/// and leaves the acceptance test to the real binary.
fn is_harness_binary() -> bool {
    env!("CARGO_CRATE_NAME") == HARNESS_BINARY
}

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// A running app on the real runtime, driven from the test thread.
///
/// Dropping it stops the app even when an assertion unwinds, so a failing test
/// cannot leave an event loop running in the test process.
pub struct Harness {
    handle: AppHandle,
    display: DisplayKind,
    log_path: PathBuf,
    app_thread: Option<std::thread::JoinHandle<i32>>,
    running: Arc<AtomicBool>,
    exited: bool,
    /// Held for the harness's lifetime so only one event loop runs at a time.
    _serial: MutexGuard<'static, ()>,
}

impl Harness {
    /// Launch the app with only the harness command registered.
    ///
    /// Use [`Harness::launch_with`] to add production state and commands.
    pub fn launch() -> Result<Harness, String> {
        Self::launch_with(|builder| {
            builder.invoke_handler(tauri::generate_handler![command::harness_report])
        })
    }

    /// Launch the app, letting `configure` extend the builder first.
    ///
    /// The builder handed to `configure` already has `.any_thread()` applied
    /// (the event loop runs on a background thread, as a test needs). A caller
    /// that sets its own `invoke_handler` must include
    /// [`command::harness_report`] in the handler, otherwise readiness can
    /// never be observed:
    ///
    /// ```ignore
    /// Harness::launch_with(|builder| {
    ///     builder
    ///         .manage(production_state())
    ///         .invoke_handler(tauri::generate_handler![
    ///             multiwindow_harness::command::harness_report,
    ///             palace_app_lib::windows::open_panel,
    ///         ])
    /// })
    /// ```
    pub fn launch_with<F>(configure: F) -> Result<Harness, String>
    where
        F: FnOnce(tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> + Send + 'static,
    {
        let display = virtual_display()?;
        let serial = SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if LAUNCHED.swap(true, Ordering::SeqCst) {
            return Err(
                "the harness supports one app per process (GTK exports a process-global \
                 GtkApplication); run each real-window scenario in its own test binary"
                    .to_string(),
            );
        }
        reports().clear();

        let log_path = ensure_logger()?;
        let server = start_page_server()?;
        println!("harness_page_server={}", server.origin());
        logging::log(
            Level::Info,
            format!("harness page server at {}", server.origin()),
        );

        let (tx, rx) = mpsc::channel::<Result<AppHandle, String>>();
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = running.clone();
        let origin = server.origin().to_string();

        let app_thread = std::thread::Builder::new()
            .name("palace-harness-app".to_string())
            .spawn(move || {
                let mut context = tauri::generate_context!();
                match origin.parse::<tauri::Url>() {
                    Ok(url) => context.config_mut().build.dev_url = Some(url),
                    Err(error) => {
                        let _ = tx.send(Err(format!("bad harness origin {origin:?}: {error}")));
                        return 3;
                    }
                }
                let builder = configure(tauri::Builder::default().any_thread());
                let app = match builder.build(context) {
                    Ok(app) => app,
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string()));
                        return 3;
                    }
                };
                let handle = app.handle().clone();
                let _ = tx.send(Ok(handle.clone()));

                let watchdog = handle.clone();
                let watchdog_running = running_thread.clone();
                std::thread::spawn(move || {
                    let deadline = Instant::now() + WATCHDOG;
                    while Instant::now() < deadline {
                        if !watchdog_running.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    if watchdog_running.load(Ordering::SeqCst) {
                        eprintln!(
                            "multiwindow_harness: watchdog fired after {}s; forcing exit 3",
                            WATCHDOG.as_secs()
                        );
                        watchdog.exit(3);
                    }
                });

                let code = app.run_return(|_, _| {});
                running_thread.store(false, Ordering::SeqCst);
                code
            })
            .map_err(|error| format!("could not start the harness app thread: {error}"))?;

        let handle = match rx.recv_timeout(LAUNCH_TIMEOUT) {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                running.store(false, Ordering::SeqCst);
                return Err(format!("the real runtime failed to start: {error}"));
            }
            Err(error) => {
                running.store(false, Ordering::SeqCst);
                return Err(format!(
                    "the real runtime did not start within {LAUNCH_TIMEOUT:?}: {error}"
                ));
            }
        };

        Ok(Harness {
            handle,
            display,
            log_path,
            app_thread: Some(app_thread),
            running,
            exited: false,
            _serial: serial,
        })
    }

    /// The display this harness runs on, with its virtual proof.
    #[must_use]
    pub fn display(&self) -> &DisplayKind {
        &self.display
    }

    /// The proof line for the display, for evidence output.
    #[must_use]
    pub fn display_provenance(&self) -> String {
        self.display.provenance()
    }

    /// The app handle, for the rare assertion the harness does not wrap.
    #[must_use]
    pub fn handle(&self) -> &AppHandle {
        &self.handle
    }

    /// The on-disk log this harness reads and writes.
    #[must_use]
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Every window label currently in the live registry, sorted.
    #[must_use]
    pub fn window_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.handle.webview_windows().keys().cloned().collect();
        labels.sort();
        labels
    }

    /// Whether a window with this label exists right now.
    #[must_use]
    pub fn has_window(&self, label: &str) -> bool {
        self.handle.get_webview_window(label).is_some()
    }

    /// Panic unless a window with this label exists, naming what does.
    pub fn assert_window(&self, label: &str) {
        assert!(
            self.has_window(label),
            "expected a window labelled {label:?}, found {:?}",
            self.window_labels()
        );
    }

    /// Wait until a window with this label exists.
    pub fn wait_for_window(&self, label: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.has_window(label) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "no window labelled {label:?} appeared within {timeout:?}; windows: {:?}",
                    self.window_labels()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Wait until this window's real webview has run the probe and called
    /// back. A window that exists but never reports is a failure, not a skip:
    /// it means the native webview did not boot.
    ///
    /// Config windows are created when the event loop starts, so a window that
    /// does not exist yet is waited for; one that existed and then vanished is
    /// an immediate error.
    pub fn wait_until_ready(&self, label: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut last_poke: Option<Instant> = None;
        let mut seen = false;
        loop {
            if has_report(label) {
                return Ok(());
            }
            match self.handle.get_webview_window(label) {
                Some(window) => {
                    seen = true;
                    if last_poke.is_none_or(|last| last.elapsed() >= POKE) {
                        let _ = window.eval(boot_js(label));
                        last_poke = Some(Instant::now());
                    }
                }
                None if seen => {
                    return Err(format!(
                        "window {label:?} disappeared before it reported ready; windows: {:?}",
                        self.window_labels()
                    ));
                }
                None => {}
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "window {label:?} did not run the harness probe within {timeout:?}; \
                     windows: {:?}; log: {}",
                    self.window_labels(),
                    self.log_path.display()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Open a real second OS window, wait for it to appear and boot, and log
    /// the lifecycle line. `label` must be a fresh label; `main` is refused.
    ///
    /// The window loads `index.html#/panel/<name>` from the harness page
    /// server, matching the route convention the production panels use.
    pub fn open_panel(&self, label: &str) -> Result<(), String> {
        if label == "main" {
            return Err("refusing to open a panel labelled \"main\"".to_string());
        }
        if !is_safe_label(label) {
            return Err(format!(
                "panel label {label:?} must be ASCII letters, digits, '-' or '_'"
            ));
        }
        let route = label.strip_prefix("panel-").unwrap_or(label);
        let url = WebviewUrl::App(format!("index.html#/panel/{route}").into());
        let caller = self.handle.clone();
        let app = self.handle.clone();
        let label_owned = label.to_string();
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        caller
            .run_on_main_thread(move || {
                let result = WebviewWindowBuilder::new(&app, label_owned.clone(), url)
                    .title(format!("Harness {label_owned}"))
                    .inner_size(640.0, 480.0)
                    .initialization_script(boot_js(&label_owned))
                    .build()
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                let _ = tx.send(result);
            })
            .map_err(|error| format!("could not ask the event loop to open {label:?}: {error}"))?;
        match rx.recv_timeout(DEFAULT_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(format!(
                    "the real runtime refused to open {label:?}: {error}"
                ))
            }
            Err(error) => return Err(format!("opening {label:?} timed out: {error}")),
        }
        WindowLog::new(label).record(WindowMilestone::Created, format!("label={label} harness"));
        self.wait_for_window(label, DEFAULT_TIMEOUT)?;
        self.wait_until_ready(label, DEFAULT_TIMEOUT)
    }

    /// Close a real window and wait until the registry no longer lists it.
    /// `main` is refused: closing it would end the session, not a panel.
    pub fn close_panel(&self, label: &str) -> Result<(), String> {
        if label == "main" {
            return Err("refusing to close the main window".to_string());
        }
        let window = self.handle.get_webview_window(label).ok_or_else(|| {
            format!(
                "no window labelled {label:?} to close; windows: {:?}",
                self.window_labels()
            )
        })?;
        window
            .close()
            .map_err(|error| format!("could not close {label:?}: {error}"))?;
        self.wait_for_window_gone(label, DEFAULT_TIMEOUT)?;
        WindowLog::new(label).record(WindowMilestone::Destroyed, format!("label={label} harness"));
        Ok(())
    }

    /// Wait until a window with this label is gone from the registry.
    fn wait_for_window_gone(&self, label: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.has_window(label) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "window {label:?} still exists after {timeout:?}; windows: {:?}",
                    self.window_labels()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Run JavaScript in a window's webview. This is how a later task drives
    /// the production commands the way the SPA does, for example
    /// `invoke('open_panel', { panel_id: 'users' })`.
    pub fn eval_in_window(&self, label: &str, js: &str) -> Result<(), String> {
        let window = self.handle.get_webview_window(label).ok_or_else(|| {
            format!(
                "no window labelled {label:?}; windows: {:?}",
                self.window_labels()
            )
        })?;
        window
            .eval(js)
            .map_err(|error| format!("could not evaluate JavaScript in {label:?}: {error}"))
    }

    /// The log's current length in bytes: pass it to [`Harness::read_log_since`]
    /// to read only what happens next.
    #[must_use]
    pub fn mark_log(&self) -> u64 {
        std::fs::metadata(&self.log_path)
            .map(|meta| meta.len())
            .unwrap_or(0)
    }

    /// Everything the log gained since `mark`.
    ///
    /// The logger flushes every line before the call that wrote it returns, so
    /// this reads real content while the app is still running. A `mark` past
    /// the end (the active file rotated) restarts from the beginning.
    #[must_use]
    pub fn read_log_since(&self, mark: u64) -> String {
        let Ok(mut file) = std::fs::File::open(&self.log_path) else {
            return String::new();
        };
        let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let start = if mark > length { 0 } else { mark };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return String::new();
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return String::new();
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Wait until the log gained a line containing `needle`, and return
    /// everything read since `mark`.
    pub fn wait_for_log_since(
        &self,
        mark: u64,
        needle: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let text = self.read_log_since(mark);
            if text.contains(needle) {
                return Ok(text);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the log did not gain {needle:?} within {timeout:?}; read since the mark:\n{text}"
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Stop the app and return its exit code.
    pub fn quit(mut self) -> Result<i32, String> {
        self.handle.exit(0);
        self.exited = true;
        self.running.store(false, Ordering::SeqCst);
        let thread = self
            .app_thread
            .take()
            .ok_or_else(|| "the app has already stopped".to_string())?;
        thread
            .join()
            .map_err(|_| "the app thread panicked".to_string())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if !self.exited {
            self.handle.exit(0);
        }
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.app_thread.take() {
            // Waiting briefly keeps the process from exiting while the event
            // loop still owns GTK. If it refuses to stop, the thread is left
            // detached: the process exits when the test harness finishes, and
            // hanging here would turn a failed assertion into a stuck CI job.
            let deadline = Instant::now() + EXIT_TIMEOUT;
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(POLL);
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

/// Install the process logger, or reuse the one another test already
/// installed. `PALACE_LOG_DIR` chooses the directory; the default is a
/// per-process temp directory so repeated runs do not mix.
fn ensure_logger() -> Result<PathBuf, String> {
    let dir = std::env::var_os(logging::ENV_LOG_DIR)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("palace-harness-{}", std::process::id()))
        });
    logging::init_at(&dir, Level::Debug).map_err(|error| {
        format!(
            "could not open the harness log in {}: {error}",
            dir.display()
        )
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_gate_skips_without_any_display() {
    let decision = gate_from(None, None, &[]);
    let reason = decision.expect_err("no display must skip");
    assert!(
        reason.contains("xvfb-run"),
        "the skip message must name the runnable command: {reason}"
    );
}

#[test]
fn the_gate_refuses_a_real_session_display() {
    // `:0` with an unrelated Xvfb elsewhere is a desktop session: the harness
    // must refuse it, and say why.
    let processes = vec!["/usr/bin/Xvfb :99 -screen 0 1280x1024x24".to_string()];
    let decision = gate_from(Some(":0"), Some("wayland-0"), &processes);
    let reason = decision.expect_err("a bare session display must be refused");
    assert!(
        reason.contains("never touch"),
        "the refusal must state the desktop-safety rule: {reason}"
    );
}

#[test]
fn the_gate_accepts_xvfb_serving_the_display() {
    let processes = vec!["/usr/bin/Xvfb :99 -screen 0 1280x1024x24 -nolisten tcp".to_string()];
    match gate_from(Some(":99"), None, &processes) {
        Ok(DisplayKind::Xvfb(display)) => assert_eq!(display, ":99"),
        other => panic!("expected the Xvfb display to be accepted: {other:?}"),
    }
    assert!(
        gate_from(Some(":0"), None, &processes).is_err(),
        "an Xvfb on another display number is not proof for this one"
    );
    assert!(
        gate_from(Some("builder:99"), None, &processes).is_err(),
        "a remote display is not the local virtual one"
    );
}

#[test]
fn the_gate_accepts_headless_gamescope_on_wayland() {
    let processes = vec!["gamescope --backend headless --expose-wayland".to_string()];
    match gate_from(None, Some("wayland-1"), &processes) {
        Ok(DisplayKind::Gamescope(display)) => assert_eq!(display, "wayland-1"),
        other => panic!("expected the gamescope display to be accepted: {other:?}"),
    }
    assert!(
        gate_from(None, Some("wayland-1"), &[]).is_err(),
        "a bare Wayland session without gamescope must be refused"
    );
}

#[test]
fn the_probe_script_keeps_the_label_inside_a_string_literal() {
    let js = boot_js("panel-users");
    assert!(js.contains("'harness_report'"), "{js}");
    assert!(js.contains("\"panel-users\""), "{js}");

    let hostile = boot_js("panel'); alert(1); //");
    assert!(
        hostile.contains(r#""panel'); alert(1); //""#),
        "the label must stay a JSON string literal: {hostile}"
    );

    assert!(is_safe_label("panel-users"));
    assert!(!is_safe_label("panel users"));
    assert!(!is_safe_label(""));
}

/// The acceptance test: on a proven virtual display, the real runtime opens a
/// real second window, its webview runs the probe, the on-disk log records it
/// while the app runs, and closing the window removes it from the registry.
///
/// It also exercises the integration path a later task uses — `launch_with`
/// registering that task's commands and `eval_in_window` driving them from the
/// window the way the SPA would — because only one app can start per process.
#[test]
fn real_runtime_opens_panel_users_and_sees_it_in_the_window_list() {
    if !is_harness_binary() {
        eprintln!(
            "SKIP multiwindow_harness: this is an included copy of the harness module; \
             the acceptance test runs in the multiwindow_harness test binary"
        );
        return;
    }
    let display = match virtual_display() {
        Ok(display) => display,
        Err(reason) => {
            eprintln!("SKIP multiwindow_harness: {reason}");
            return;
        }
    };
    println!("display_provenance={}", display.provenance());
    assert!(
        display.provenance().starts_with("xvfb") || display.provenance().starts_with("gamescope"),
        "the harness ran on an unproven display: {}",
        display.provenance()
    );

    let harness = Harness::launch_with(|builder| {
        builder.invoke_handler(tauri::generate_handler![
            command::harness_report,
            command::harness_ping
        ])
    })
    .expect("the real runtime launches with the caller's command set");
    let mark = harness.mark_log();

    harness
        .wait_until_ready("main", DEFAULT_TIMEOUT)
        .expect("the configured main window boots a real webview");
    println!("window_labels_after_main={:?}", harness.window_labels());
    assert_eq!(
        harness.window_labels(),
        vec!["main".to_string()],
        "the app starts with exactly the configured main window"
    );

    harness
        .eval_in_window(
            "main",
            "window.__TAURI_INTERNALS__.invoke('harness_ping', { note: 'from-eval' })",
        )
        .expect("the caller's command can be driven from the window");
    let pinged = harness
        .wait_for_log_since(mark, "harness_ping note=from-eval", DEFAULT_TIMEOUT)
        .expect("the caller's command ran in Rust");
    assert!(pinged.contains("harness_ping note=from-eval"), "{pinged}");

    harness
        .open_panel("panel-users")
        .expect("panel-users opens and boots its real webview");
    println!("window_labels_after_open={:?}", harness.window_labels());
    harness.assert_window("panel-users");
    assert_eq!(
        harness.window_labels(),
        vec!["main".to_string(), "panel-users".to_string()],
        "opening a panel adds exactly one window"
    );

    let duplicate = harness.open_panel("panel-users");
    assert!(
        duplicate.is_err(),
        "the real runtime refuses a second window with the same label: {duplicate:?}"
    );

    let log = harness
        .wait_for_log_since(mark, "[panel-users] created", DEFAULT_TIMEOUT)
        .expect("the on-disk log records the panel while the app runs");
    assert!(
        log.contains("[main] harness ready"),
        "the main webview must have run the probe: {log}"
    );
    assert!(
        log.contains("[panel-users] harness ready"),
        "the panel webview must have run the probe: {log}"
    );
    assert!(
        !log.contains("[panel-users] destroyed"),
        "the panel must still be open: {log}"
    );

    harness
        .close_panel("panel-users")
        .expect("panel-users closes cleanly");
    println!("window_labels_after_close={:?}", harness.window_labels());
    assert!(
        !harness.has_window("panel-users"),
        "closing the panel removes it from the real window list"
    );
    harness.assert_window("main");

    let code = harness.quit().expect("the app stops");
    println!("harness_exit_code={code}");
    assert_eq!(code, 0, "the harness app exits cleanly");
}

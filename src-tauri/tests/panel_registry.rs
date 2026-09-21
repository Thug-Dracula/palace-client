//! Task 6: the panel window registry and its capability boundary.
//!
//! Two kinds of proof live here:
//!
//! * a **pure** capability inspection that runs anywhere and pins the fact that
//!   neither `main` nor a `panel-*` window is granted window/webview creation;
//! * a **real-runtime** run (Tauri/WebKitGTK) that opens `panel-users`, refuses a
//!   duplicate, leaves `main` untouched, closes the panel, and confirms the
//!   re-attach signal reaches `main` while the capability still denies a
//!   frontend-initiated window create. That one needs a display; it skips with a
//!   message when neither `DISPLAY` nor `WAYLAND_DISPLAY` is set, exactly as the
//!   Task-1 spike does.
//!
//! Run the real one headless:
//!
//! ```text
//! xvfb-run -a cargo test -p palace-app --test panel_registry -- --nocapture --test-threads=1
//! ```
//!
//! The real-runtime case runs on the shared [`multiwindow_harness`], which
//! refuses any display it cannot *prove* is virtual (an `Xvfb` serving the
//! exact `DISPLAY`, or a running `gamescope`). On the developer's real desktop
//! session the launch returns `Err` and this test prints `SKIP …` and passes,
//! so `cargo test` can never open a window on the user's screen.

#![cfg(any(target_os = "linux", target_os = "windows"))]

#[cfg(target_os = "linux")]
#[path = "multiwindow_harness.rs"]
mod multiwindow_harness;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use palace_app_lib::logging::{self, Level};
use palace_app_lib::windows::{self, CloseOutcome, OpenOutcome, Panel, PANEL_CLOSED_EVENT};
use serde_json::Value;
use tauri::{Emitter, Manager};

/// Reports sent back from the JS injected into the windows.
static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record_report(kind: &str) {
    REPORTS.lock().unwrap().push(kind.to_string());
}

fn has_report(kind: &str) -> bool {
    REPORTS.lock().unwrap().iter().any(|report| report == kind)
}

/// The command the injected JS calls back. Application commands are not
/// ACL-gated in this app, so this needs no capability entry (Task 1, Q2).
#[tauri::command]
fn panel_probe(kind: String, payload: String) {
    let line = format!("panel_probe kind={kind} payload={payload}");
    logging::log(Level::Info, &line);
    println!("{line}");
    record_report(&kind);
}

/// Injected into `main`: registers a label-scoped listener for the re-attach
/// signal and probes two frontend window-creation commands that must be denied.
/// The event name is substituted from the production constant.
const MAIN_JS_TEMPLATE: &str = r#"
(function () {
  function boot() {
    var I = window.__TAURI_INTERNALS__;
    if (!I || !I.invoke || !I.transformCallback) { return setTimeout(boot, 40); }
    if (window.__panelRegistryMainInstalled) { return; }
    window.__panelRegistryMainInstalled = true;
    function report(kind, payload) {
      I.invoke('panel_probe', { kind: kind, payload: String(payload) }).catch(function () {});
    }
    I.invoke('plugin:event|listen', {
      event: '__PANEL_CLOSED_EVENT__',
      target: { kind: 'AnyLabel', label: 'main' },
      handler: I.transformCallback(function (e) { report('main_panel_closed', JSON.stringify(e.payload)); })
    }).catch(function (error) { report('main_listen_failed', String(error)); });

    I.invoke('plugin:webview|create_webview_window', {
      options: { label: 'panel-forbidden-webview' }
    }).then(function () { report('main_create_webview_allowed', 'unexpected'); })
      .catch(function (error) { report('main_create_webview_denied', String(error)); });

    I.invoke('plugin:window|create', {
      options: { label: 'panel-forbidden-window' }
    }).then(function () { report('main_create_window_allowed', 'unexpected'); })
      .catch(function (error) { report('main_create_window_denied', String(error)); });

    report('main_ready', 'main');
  }
  boot();
})();
"#;

fn main_js() -> String {
    MAIN_JS_TEMPLATE.replace("__PANEL_CLOSED_EVENT__", PANEL_CLOSED_EVENT)
}

/// Injected into `panel-users`: registers a label-scoped listener on the shared
/// event channel and reports readiness.
const PANEL_JS: &str = r#"
(function () {
  function boot() {
    var I = window.__TAURI_INTERNALS__;
    if (!I || !I.invoke || !I.transformCallback) { return setTimeout(boot, 40); }
    if (window.__panelRegistryPanelInstalled) { return; }
    window.__panelRegistryPanelInstalled = true;
    function report(kind, payload) {
      I.invoke('panel_probe', { kind: kind, payload: String(payload) }).catch(function () {});
    }
    I.invoke('plugin:event|listen', {
      event: 'palace://event',
      target: { kind: 'AnyLabel', label: 'panel-users' },
      handler: I.transformCallback(function (e) { report('panel_event', JSON.stringify(e.payload)); })
    }).catch(function (error) { report('panel_listen_failed', String(error)); });
    report('panel_ready', 'panel-users');
  }
  boot();
})();
"#;

fn wait_for_reports(kinds: &[&str], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if kinds.iter().all(|kind| has_report(kind)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    kinds.iter().all(|kind| has_report(kind))
}

fn panel_labels<R: tauri::Runtime>(handle: &tauri::AppHandle<R>) -> Vec<String> {
    let mut labels: Vec<String> = handle
        .webview_windows()
        .keys()
        .filter(|label| windows::is_panel_label(label))
        .cloned()
        .collect();
    labels.sort();
    labels
}

fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    condition()
}

/// Runs on a plain thread; window creation goes through the registry, which
/// dispatches safely from any thread.
fn orchestrate(handle: tauri::AppHandle) {
    let Some(main) = handle.get_webview_window("main") else {
        eprintln!("panel_registry: no main window; aborting");
        handle.exit(4);
        return;
    };
    let main_label = main.label().to_string();
    let initial_size = main.inner_size().ok().map(|size| (size.width, size.height));
    println!("main_window label={main_label} initial_inner_size={initial_size:?}");

    // Wait for main's SPA context so the injected probes can install.
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut main_evals = 0;
    while Instant::now() < deadline && !has_report("main_ready") && main_evals < 40 {
        if let Some(main) = handle.get_webview_window("main") {
            let _ = main.eval(main_js());
        }
        main_evals += 1;
        std::thread::sleep(Duration::from_millis(250));
    }

    // The window has now been mapped and settled, so this is the size the panel
    // must leave untouched.
    let main_size = handle
        .get_webview_window("main")
        .and_then(|window| window.inner_size().ok())
        .map(|size| (size.width, size.height));
    println!("main_size_before_panel={main_size:?}");

    // --- open -----------------------------------------------------------------
    let opened = windows::open(&handle, Panel::Users);
    println!("open_first={opened:?}");
    let one = handle.get_webview_window("panel-users").is_some();
    let labels_after_open = panel_labels(&handle);
    if opened == Ok(OpenOutcome::Created) && one && labels_after_open == ["panel-users"] {
        record_report("rust_open_created_single");
    }

    // --- refuse a duplicate ---------------------------------------------------
    let duplicate = windows::open(&handle, Panel::Users);
    println!("open_second={duplicate:?}");
    let labels_after_second = panel_labels(&handle);
    if duplicate == Ok(OpenOutcome::AlreadyOpen) && labels_after_second == ["panel-users"] {
        record_report("rust_duplicate_refused");
    }

    // --- invalid id is rejected, and creates nothing --------------------------
    let invalid = windows::open_panel(handle.clone(), "editor".to_string());
    println!("open_invalid={invalid:?}");
    if invalid.is_err() && handle.get_webview_window("panel-editor").is_none() {
        record_report("rust_invalid_rejected");
    }

    // --- main is untouched ----------------------------------------------------
    let main_after = handle.get_webview_window("main").map(|window| {
        (
            window.label().to_string(),
            window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
        )
    });
    println!("main_after={main_after:?}");
    if main_after == Some((main_label.clone(), main_size)) {
        record_report("rust_main_unchanged");
    }

    // --- panel boots and joins the shared channel -----------------------------
    let panel_deadline = Instant::now() + Duration::from_secs(40);
    let mut panel_evals = 0;
    while Instant::now() < panel_deadline && !has_report("panel_ready") && panel_evals < 40 {
        if let Some(panel) = handle.get_webview_window("panel-users") {
            let _ = panel.eval(PANEL_JS);
        }
        panel_evals += 1;
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = handle.emit(commands_event(), "probe-payload");
    wait_for_reports(&["panel_event"], Duration::from_secs(10));

    wait_for_reports(
        &["main_create_webview_denied", "main_create_window_denied"],
        Duration::from_secs(10),
    );

    // --- close and re-attach signal -------------------------------------------
    let closed = windows::close(&handle, Panel::Users);
    println!("close_first={closed:?}");
    let gone = wait_until(
        || handle.get_webview_window("panel-users").is_none(),
        Duration::from_secs(15),
    );
    println!("panel_gone={gone}");
    if closed == Ok(CloseOutcome::Closed) && gone {
        record_report("rust_close_removed");
    }
    wait_for_reports(&["main_panel_closed"], Duration::from_secs(10));

    // Closing a panel that is not open is a no-op, not an error.
    let closed_again = windows::close(&handle, Panel::Users);
    println!("close_second={closed_again:?}");
    if closed_again == Ok(CloseOutcome::NotOpen) {
        record_report("rust_close_missing_noop");
    }

    handle.exit(0);
}

/// The production event channel name, pulled from the command module so this
/// test notices if it ever forks.
fn commands_event() -> &'static str {
    palace_app_lib::commands::EVENT_NAME
}

// ---------------------------------------------------------------------------
// Capability inspection (no display)
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn permissions(path: &std::path::Path) -> Vec<String> {
    read_json(path)
        .get("permissions")
        .and_then(Value::as_array)
        .expect("permissions is an array")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

const FORBIDDEN: [&str; 6] = [
    "core:window:allow-create",
    "core:window:allow-close",
    "core:window:allow-destroy",
    "core:webview:allow-create-webview-window",
    "core:webview:allow-create-webview",
    "core:webview:allow-webview-close",
];

#[test]
fn no_frontend_capability_can_create_or_close_windows() {
    let main = manifest_dir().join("capabilities/default.json");
    let main_permissions = permissions(&main);
    assert!(
        main_permissions.contains(&"core:event:default".to_string()),
        "the shared event channel must stay granted: {main_permissions:?}"
    );
    for forbidden in FORBIDDEN {
        assert!(
            !main_permissions.contains(&forbidden.to_string()),
            "main must not receive {forbidden}: {main_permissions:?}"
        );
    }
}

#[test]
fn the_panel_capability_matches_panel_windows_only() {
    let panel_path = manifest_dir().join("capabilities/panel.json");
    let panel = read_json(&panel_path);

    let windows: Vec<&str> = panel
        .get("windows")
        .and_then(Value::as_array)
        .expect("windows is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        windows,
        vec!["panel-*"],
        "the capability must target the panel label namespace, not main"
    );

    let panel_permissions = permissions(&panel_path);
    assert_eq!(
        panel_permissions,
        vec!["core:event:default"],
        "a panel is a receiving view: event listen, nothing more"
    );
    for forbidden in FORBIDDEN {
        assert!(
            !panel_permissions.contains(&forbidden.to_string()),
            "a panel must not be able to create or close windows, found {forbidden}"
        );
    }

    // `core:default` is what would transitively leak create/close if it were
    // ever granted here; it is not.
    assert!(!panel_permissions.contains(&"core:default".to_string()));
}

#[test]
fn the_prefs_capability_matches_the_prefs_window_only() {
    let prefs_path = manifest_dir().join("capabilities/prefs.json");
    let prefs = read_json(&prefs_path);

    let windows: Vec<&str> = prefs
        .get("windows")
        .and_then(Value::as_array)
        .expect("windows is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        windows,
        vec!["prefs"],
        "the capability must target the Preferences label only, never the panel namespace"
    );

    let prefs_permissions = permissions(&prefs_path);
    assert_eq!(
        prefs_permissions,
        vec!["core:event:default"],
        "the Preferences window is a receiving view: event listen, nothing more"
    );
    for forbidden in FORBIDDEN {
        assert!(
            !prefs_permissions.contains(&forbidden.to_string()),
            "Preferences must not be able to create or close windows, found {forbidden}"
        );
    }
    assert!(!prefs_permissions.contains(&"core:default".to_string()));
    assert!(
        !prefs_permissions.contains(&"dialog:allow-open".to_string()),
        "the window itself is opened by the Rust command, not by the frontend"
    );
}

// ---------------------------------------------------------------------------
// Real runtime (needs a display)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn registry_lifecycle_on_the_real_runtime() {
    use multiwindow_harness::Harness;

    let harness = match Harness::launch_with(|builder| {
        builder
            .invoke_handler(tauri::generate_handler![
                multiwindow_harness::command::harness_report,
                panel_probe,
                windows::open_panel,
                windows::close_panel
            ])
            .on_window_event(palace_app_lib::handle_window_event)
    }) {
        Ok(harness) => harness,
        Err(reason) => {
            eprintln!("SKIP registry_lifecycle_on_the_real_runtime: {reason}");
            return;
        }
    };
    println!("display_provenance={}", harness.display_provenance());

    let log_path = harness.log_path().to_path_buf();
    println!("panel-registry log: {}", log_path.display());

    // Config windows are created when the event loop starts, and the harness
    // hands back the handle as soon as the app is built, so `main` may not be
    // in the registry yet.
    harness
        .wait_for_window("main", multiwindow_harness::DEFAULT_TIMEOUT)
        .expect("the configured main window appears");

    // The harness runs the event loop on its own background thread, so the test
    // thread is free to drive the registry: `orchestrate` runs here, not in a
    // spawned thread as the hand-rolled builder needed.
    orchestrate(harness.handle().clone());

    let exit_code = harness.quit().expect("the harness app stops");

    let reports = REPORTS.lock().unwrap().clone();
    let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();

    println!("exit_code={exit_code}");
    println!("reports={reports:?}");

    assert_eq!(exit_code, 0, "the run must exit cleanly");

    for expected in [
        "main_ready",
        "panel_ready",
        "panel_event",
        "rust_open_created_single",
        "rust_duplicate_refused",
        "rust_invalid_rejected",
        "rust_main_unchanged",
        "rust_close_removed",
        "rust_close_missing_noop",
        "main_panel_closed",
        "main_create_webview_denied",
        "main_create_window_denied",
    ] {
        assert!(
            reports.iter().any(|report| report == expected),
            "missing {expected}: {reports:?}"
        );
    }
    assert!(
        !reports.iter().any(|report| report.ends_with("_allowed")),
        "no frontend window creation may succeed: {reports:?}"
    );

    // On-disk lifecycle lines, label-prefixed, produced by the production
    // registry and window-event handler this test exercises.
    for expected in [
        "[panel-users] detached",
        "[panel-users] reattached",
        "[panel-users] destroyed",
    ] {
        assert!(
            log_text.contains(expected),
            "the log must record {expected}:\n{log_text}"
        );
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn registry_lifecycle_on_the_real_runtime() {
    eprintln!(
        "SKIP registry_lifecycle_on_the_real_runtime: the multiwindow harness is Linux-only; \
         refusing to open real windows on this platform"
    );
}

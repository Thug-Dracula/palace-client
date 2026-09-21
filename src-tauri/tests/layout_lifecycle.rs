//! Task 18: layout save/restore across a restart, and the window close
//! semantics from `WINDOWS-ARCHITECTURE.md` §6.
//!
//! Four real-window behaviours are proven here, each in its own OS process
//! because GTK allows one Tauri app per process (the pattern
//! `tests/geometry_persistence.rs` established):
//!
//! * **Restore.** Two panels are detached through the production registry,
//!   placed at known rectangles and quit; a second process starts from that
//!   file and both windows are recreated at their saved rectangles.
//! * **Quit closes the panels.** Closing `main` while two panels are open
//!   closes every window and exits, while leaving both panels marked detached
//!   so the next launch reopens them. This is one flow with the restore proof
//!   because both need the same detach-then-quit child.
//! * **Titlebar close re-attaches.** A panel's `CloseRequested` re-docks it,
//!   delivers exactly one `PANEL_CLOSED_EVENT` to `main`, and logs
//!   `via=close` once.
//! * **A dead webview re-docks.** A panel destroyed without a close request
//!   (simulating a crashed webview or a window-manager kill) delivers the same
//!   event and logs `via=destroyed`, so `main` cannot keep a ghost placeholder.
//! * **Memory off stops the writing.** Task 27: with two panels detached, the
//!   Layout memory toggle is turned off, the session is moved and quit, and the
//!   file must stay byte-identical; the relaunch must be the default
//!   single-window layout and must still not write.
//! * **Reset re-docks now.** The "reset layout to default" action destroys
//!   every panel window, delivers the re-dock signal for each, clears the saved
//!   windows and leaves no panel window behind.
//! * **The Preferences window's geometry is remembered.** `prefs` is tracked by
//!   layout memory without becoming a panel: it is not reopened at startup, but
//!   opening it applies the saved rectangle.
//!
//! Run the real ones headless, on a screen big enough for the fixture
//! rectangles:
//!
//! ```text
//! xvfb-run -a -s "-screen 0 1920x1080x24" \
//!   cargo test -p palace-app --test layout_lifecycle -- --nocapture --test-threads=1
//! ```
//!
//! On a screen too small for the fixtures each phase prints `SKIP …` and exits
//! with code 77, which the parent reports as a skip rather than a failure. On
//! the developer's real desktop session the harness refuses the display and the
//! acceptance tests print `SKIP …` and pass.

#![cfg(target_os = "linux")]

#[path = "multiwindow_harness.rs"]
mod multiwindow_harness;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use palace_app_lib::geometry::{self, LAYOUT_FILE};
use palace_app_lib::windows::Panel;

/// A fresh directory under the temp dir so no run can touch the real layout.
fn scratch(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("palace-lifecycle-{tag}-{unique}"));
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

/// Print the display proof, or the skip reason, for a parent acceptance test.
fn proven_display(name: &str) -> bool {
    match multiwindow_harness::virtual_display() {
        Ok(display) => {
            println!("{name}: display_provenance={}", display.provenance());
            true
        }
        Err(reason) => {
            eprintln!("SKIP {name}: {reason}");
            false
        }
    }
}

fn print_child(phase: &str, output: &std::process::Output) {
    println!(
        "--- {phase} phase stdout ---\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    if !output.stderr.is_empty() {
        println!(
            "--- {phase} phase stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Whether a child was stopped by the too-small-screen guard.
fn skipped(output: &std::process::Output) -> bool {
    output.status.code() == Some(real_runtime::SKIP_EXIT)
}

/// Whether a child finished its phase.
///
/// The exit status alone is not enough: when the app has already exited, the
/// harness's `Drop` can end the process with code 0 while a panic is unwinding,
/// so the stdout sentinel is the proof the phase really completed.
fn child_ok(phase: &str, output: &std::process::Output) -> bool {
    output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains(&format!("PHASE_DONE:{phase}"))
}

// ---------------------------------------------------------------------------
// Acceptance tests: each re-executes this binary as one real-window child
// ---------------------------------------------------------------------------

#[test]
fn two_detached_panels_are_restored_after_a_real_restart() {
    let name = "two_detached_panels_are_restored_after_a_real_restart";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("restart");
    let layout = directory.join(LAYOUT_FILE);

    let detach = real_runtime::run_child("detach-two", &layout, &directory.join("log-detach-two"));
    print_child("detach-two", &detach);
    if skipped(&detach) {
        eprintln!("SKIP {name}: the detach phase needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("detach-two", &detach),
        "the detach-two phase failed; see its output above"
    );

    let saved = geometry::load(&layout);
    for panel in [Panel::Users, Panel::Chat] {
        let entry = saved
            .windows
            .get(panel.label())
            .unwrap_or_else(|| panic!("{} was saved", panel.label()));
        assert!(
            entry.detached,
            "{} must stay detached across the quit, so the next launch reopens it",
            panel.label()
        );
    }
    assert_eq!(
        saved.order,
        vec!["users".to_string(), "chat".to_string()],
        "the detach order must survive the quit"
    );

    let restore =
        real_runtime::run_child("restore-two", &layout, &directory.join("log-restore-two"));
    print_child("restore-two", &restore);
    if skipped(&restore) {
        eprintln!("SKIP {name}: the restore phase needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("restore-two", &restore),
        "the restore-two phase failed; see its output above"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn titlebar_close_reattaches_the_panel_and_delivers_the_event() {
    let name = "titlebar_close_reattaches_the_panel_and_delivers_the_event";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("titlebar");
    let layout = directory.join(LAYOUT_FILE);

    let child = real_runtime::run_child("titlebar-close", &layout, &directory.join("log"));
    print_child("titlebar-close", &child);
    if skipped(&child) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("titlebar-close", &child),
        "the titlebar-close phase failed; see its output above"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_dead_panel_webview_re_docks_instead_of_leaving_a_placeholder() {
    let name = "a_dead_panel_webview_re_docks_instead_of_leaving_a_placeholder";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("dead-webview");
    let layout = directory.join(LAYOUT_FILE);

    let child = real_runtime::run_child("dead-webview", &layout, &directory.join("log"));
    print_child("dead-webview", &child);
    if skipped(&child) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("dead-webview", &child),
        "the dead-webview phase failed; see its output above"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn quitting_with_panels_open_closes_every_window() {
    let name = "quitting_with_panels_open_closes_every_window";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("quit");
    let layout = directory.join(LAYOUT_FILE);

    let child = real_runtime::run_child("quit-with-panels", &layout, &directory.join("log"));
    print_child("quit-with-panels", &child);
    if skipped(&child) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("quit-with-panels", &child),
        "the quit-with-panels phase failed; see its output above"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn remember_off_stops_writing_and_the_next_launch_is_the_default_layout() {
    let name = "remember_off_stops_writing_and_the_next_launch_is_the_default_layout";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("remember-off");
    let layout = directory.join(LAYOUT_FILE);

    let off = real_runtime::run_child("remember-off", &layout, &directory.join("log-off"));
    print_child("remember-off", &off);
    if skipped(&off) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("remember-off", &off),
        "the remember-off phase failed; see its output above"
    );

    let frozen = std::fs::read_to_string(&layout).expect("the layout file is readable");
    let saved: serde_json::Value = serde_json::from_str(&frozen).expect("the file is JSON");
    assert_eq!(
        saved.get("remember").and_then(serde_json::Value::as_bool),
        Some(false),
        "the toggle must be on disk: {frozen}"
    );
    assert!(
        saved
            .get("windows")
            .and_then(|windows| windows.get("panel-users"))
            .is_some(),
        "the geometry the user had before the toggle is kept, not destroyed: {frozen}"
    );

    let relaunch = real_runtime::run_child(
        "remember-off-relaunch",
        &layout,
        &directory.join("log-relaunch"),
    );
    print_child("remember-off-relaunch", &relaunch);
    if skipped(&relaunch) {
        eprintln!("SKIP {name}: the relaunch phase needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("remember-off-relaunch", &relaunch),
        "the remember-off-relaunch phase failed; see its output above"
    );

    assert_eq!(
        std::fs::read_to_string(&layout).expect("the layout file is readable"),
        frozen,
        "the relaunch must not write a single byte"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn reset_layout_re_docks_every_panel_immediately_and_leaves_no_panel_window() {
    let name = "reset_layout_re_docks_every_panel_immediately_and_leaves_no_panel_window";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("reset-layout");
    let layout = directory.join(LAYOUT_FILE);

    let child = real_runtime::run_child("reset-layout", &layout, &directory.join("log"));
    print_child("reset-layout", &child);
    if skipped(&child) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("reset-layout", &child),
        "the reset-layout phase failed; see its output above"
    );

    let saved = geometry::load(&layout);
    assert!(
        saved.windows.is_empty(),
        "a reset forgets every remembered window: {:?}",
        saved.windows
    );
    assert!(saved.order.is_empty(), "a reset forgets the detach order");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_preferences_window_geometry_is_remembered_without_making_it_a_panel() {
    let name = "the_preferences_window_geometry_is_remembered_without_making_it_a_panel";
    if !proven_display(name) {
        return;
    }
    let directory = scratch("prefs-geometry");
    let layout = directory.join(LAYOUT_FILE);

    let place = real_runtime::run_child("prefs-geometry", &layout, &directory.join("log-place"));
    print_child("prefs-geometry", &place);
    if skipped(&place) {
        eprintln!("SKIP {name}: this test needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("prefs-geometry", &place),
        "the prefs-geometry phase failed; see its output above"
    );

    let saved = geometry::load(&layout);
    assert!(
        saved.windows.contains_key("prefs"),
        "the Preferences window's rectangle is saved like any other window: {:?}",
        saved.windows
    );
    assert_eq!(
        saved.order,
        Vec::<String>::new(),
        "the Preferences window is not a panel, so it never joins the detach order"
    );

    let relaunch = real_runtime::run_child(
        "prefs-geometry-relaunch",
        &layout,
        &directory.join("log-reopen"),
    );
    print_child("prefs-geometry-relaunch", &relaunch);
    if skipped(&relaunch) {
        eprintln!("SKIP {name}: the reopen phase needs a 1920x1080 (or larger) screen");
        return;
    }
    assert!(
        child_ok("prefs-geometry-relaunch", &relaunch),
        "the prefs-geometry-relaunch phase failed; see its output above"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

// ---------------------------------------------------------------------------
// Child phases: real windows on the virtual display
// ---------------------------------------------------------------------------

mod real_runtime {
    use std::path::Path;
    use std::process::{Command, Output, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use palace_app_lib::geometry::{self, LayoutStore, Rect, WindowGeometry, LAYOUT_ENV};
    use palace_app_lib::logging;
    use palace_app_lib::windows::{
        self, OpenOutcome, Panel, ToolWindow, PANEL_CLOSED_EVENT, PREFS_LABEL,
    };
    use tauri::{Manager, PhysicalPosition, PhysicalSize};

    use super::multiwindow_harness;

    /// The environment variable that tells a child process which phase to run.
    pub const PHASE_ENV: &str = "PALACE_LIFECYCLE_PHASE";

    /// The single child test the parent re-executes itself with.
    const CHILD_TEST: &str = "layout_lifecycle_child_phase";

    /// Where the Users panel is placed and must be restored.
    const USERS_RECT: Rect = Rect {
        x: 120,
        y: 90,
        w: 520,
        h: 430,
    };

    /// Where the Chat panel is placed and must be restored. Different from
    /// Users so a restore that put both at one rectangle would fail.
    const CHAT_RECT: Rect = Rect {
        x: 700,
        y: 140,
        w: 540,
        h: 440,
    };

    /// Where the Preferences window is placed and must be restored. It differs
    /// from the window's 780×620 default so a restore that kept the default
    /// would fail.
    const PREFS_RECT: Rect = Rect {
        x: 760,
        y: 120,
        w: 700,
        h: 560,
    };

    /// How far the OS may be from the requested rectangle and still count.
    const TOLERANCE: i32 = 2;

    /// The exit code a phase uses to report "this environment cannot run me".
    pub const SKIP_EXIT: i32 = 77;

    /// The smallest screen that holds both fixtures without the clamp moving
    /// them: the off-screen clamp is production behaviour, so a smaller screen
    /// would fail the assertions while the app behaved correctly.
    fn required_screen() -> (u32, u32) {
        (
            (USERS_RECT.x + USERS_RECT.w as i32).max(CHAT_RECT.x + CHAT_RECT.w as i32) as u32,
            (USERS_RECT.y + USERS_RECT.h as i32).max(CHAT_RECT.y + CHAT_RECT.h as i32) as u32,
        )
    }

    fn skip_if_screen_too_small(window: &tauri::WebviewWindow) {
        let monitor = window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten());
        let Some(monitor) = monitor else {
            return;
        };
        let size = *monitor.size();
        let (need_w, need_h) = required_screen();
        if size.width >= need_w && size.height >= need_h {
            return;
        }
        eprintln!(
            "SKIP layout lifecycle phase: the virtual screen is {}x{}, but this test needs at \
             least {need_w}x{need_h}. Run it as: xvfb-run -a -s \"-screen 0 1920x1080x24\" \
             cargo test -p palace-app --test layout_lifecycle",
            size.width, size.height
        );
        std::process::exit(SKIP_EXIT);
    }

    /// The Preferences fixture needs a screen of its own: a smaller one would
    /// clamp it and fail an assertion while the app behaved correctly.
    fn skip_if_prefs_screen_too_small(window: &tauri::WebviewWindow) {
        let monitor = window
            .current_monitor()
            .ok()
            .flatten()
            .or_else(|| window.primary_monitor().ok().flatten());
        let Some(monitor) = monitor else {
            return;
        };
        let size = *monitor.size();
        let need_w = (PREFS_RECT.x + PREFS_RECT.w as i32) as u32;
        let need_h = (PREFS_RECT.y + PREFS_RECT.h as i32) as u32;
        if size.width >= need_w && size.height >= need_h {
            return;
        }
        eprintln!(
            "SKIP prefs geometry phase: the virtual screen is {}x{}, but this test needs at \
             least {need_w}x{need_h}. Run it as: xvfb-run -a -s \"-screen 0 1920x1080x24\" \
             cargo test -p palace-app --test layout_lifecycle",
            size.width, size.height
        );
        std::process::exit(SKIP_EXIT);
    }

    /// Launch the app with the same wiring production uses: the layout store is
    /// discovered, managed, autosaved and restored, and the production
    /// window-event handler is attached.
    fn launch_lifecycle_app() -> multiwindow_harness::Harness {
        multiwindow_harness::Harness::launch_with(|builder| {
            builder
                .invoke_handler(tauri::generate_handler![
                    multiwindow_harness::command::harness_report,
                    lifecycle_probe,
                    windows::open_panel,
                    windows::close_panel
                ])
                .setup(|app| {
                    let handle = app.handle().clone();
                    let store = LayoutStore::discover(&handle);
                    let _ = app.manage(store.clone());
                    if let Err(error) = geometry::spawn_autosave(&store) {
                        eprintln!("layout autosave did not start: {error}");
                    }
                    geometry::schedule_restore(&handle);
                    Ok(())
                })
                .on_window_event(palace_app_lib::handle_window_event)
        })
        .expect("the real runtime launches with the layout lifecycle wired")
    }

    /// Reports the payload of a `PANEL_CLOSED_EVENT` the child injected into
    /// `main`, so a re-dock is proven by what the shell actually received.
    static PROBES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    #[tauri::command]
    fn lifecycle_probe(kind: String, payload: String) {
        let line = format!("lifecycle_probe kind={kind} payload={payload}");
        logging::log(logging::Level::Info, &line);
        println!("{line}");
        PROBES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((kind, payload));
    }

    fn probe_payload(kind: &str) -> Option<String> {
        PROBES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|(recorded, _)| recorded == kind)
            .map(|(_, payload)| payload.clone())
    }

    fn wait_for_probe(kind: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(payload) = probe_payload(kind) {
                return payload;
            }
            assert!(
                Instant::now() < deadline,
                "the injected listener never reported {kind:?}; recorded {:?}",
                PROBES
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait until at least `count` reports of `kind` have arrived, and return
    /// every payload seen for it.
    fn wait_for_probe_count(kind: &str, count: usize, timeout: Duration) -> Vec<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let found: Vec<String> = PROBES
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .filter(|(recorded, _)| recorded == kind)
                .map(|(_, payload)| payload.clone())
                .collect();
            if found.len() >= count {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "only {} of {count} {kind:?} reports arrived; got {found:?}",
                found.len()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Inject a label-scoped `PANEL_CLOSED_EVENT` listener into `main` and wait
    /// until it is registered.
    ///
    /// This is the same mechanism `tests/panel_registry.rs` uses; the event
    /// name comes from the production constant so the test notices a fork. The
    /// `listener_ready` report matters: without waiting for the registration to
    /// settle, a panel opened and closed immediately afterwards can fire the
    /// event before the listener exists.
    fn install_close_listener(harness: &multiwindow_harness::Harness) {
        let event =
            serde_json::to_string(PANEL_CLOSED_EVENT).expect("the event name is a JSON string");
        let js = format!(
            r#"
(function () {{
  function boot() {{
    var I = window.__TAURI_INTERNALS__;
    if (!I || !I.invoke || !I.transformCallback) {{ return setTimeout(boot, 40); }}
    if (window.__layoutLifecycleInstalled) {{ return; }}
    window.__layoutLifecycleInstalled = true;
    I.invoke('plugin:event|listen', {{
      event: {event},
      target: {{ kind: 'AnyLabel', label: 'main' }},
      handler: I.transformCallback(function (e) {{
        I.invoke('lifecycle_probe', {{
          kind: 'panel_closed',
          payload: JSON.stringify(e.payload)
        }}).catch(function () {{}});
      }})
    }}).then(function () {{
      I.invoke('lifecycle_probe', {{ kind: 'listener_ready', payload: '' }}).catch(function () {{}});
    }}).catch(function (error) {{
      I.invoke('lifecycle_probe', {{
        kind: 'listen_failed',
        payload: String(error)
      }}).catch(function () {{}});
    }});
  }}
  boot();
}})();"#
        );
        harness
            .eval_in_window("main", &js)
            .expect("the main webview accepts the listener script");
        let _ = wait_for_probe("listener_ready", multiwindow_harness::DEFAULT_TIMEOUT);
    }

    fn current_rect(window: &tauri::WebviewWindow) -> Rect {
        let position = window
            .inner_position()
            .expect("the window has a client position");
        let size = window.inner_size().expect("the window has a client size");
        Rect {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
        }
    }

    fn within(actual: Rect, expected: Rect) -> bool {
        (actual.x - expected.x).abs() <= TOLERANCE
            && (actual.y - expected.y).abs() <= TOLERANCE
            && (actual.w as i32 - expected.w as i32).abs() <= TOLERANCE
            && (actual.h as i32 - expected.h as i32).abs() <= TOLERANCE
    }

    fn wait_for_rect(window: &tauri::WebviewWindow, expected: Rect, timeout: Duration) -> Rect {
        let deadline = Instant::now() + timeout;
        loop {
            let actual = current_rect(window);
            if within(actual, expected) {
                return actual;
            }
            assert!(
                Instant::now() < deadline,
                "the window never reached {expected:?} within {timeout:?}; it is {actual:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn place(window: &tauri::WebviewWindow, rect: Rect) {
        window
            .set_position(PhysicalPosition::new(rect.x, rect.y))
            .expect("the window moves");
        window
            .set_size(PhysicalSize::new(rect.w, rect.h))
            .expect("the window resizes");
    }

    fn monitor_of(window: &tauri::WebviewWindow) -> Option<String> {
        window
            .current_monitor()
            .ok()
            .flatten()
            .and_then(|monitor| monitor.name().cloned())
    }

    fn wait_for_window_gone(
        harness: &multiwindow_harness::Harness,
        label: &str,
        timeout: Duration,
    ) {
        let deadline = Instant::now() + timeout;
        while harness.has_window(label) {
            assert!(
                Instant::now() < deadline,
                "window {label:?} still exists; windows: {:?}",
                harness.window_labels()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A stable-enough fingerprint of a file's contents for the evidence log.
    fn content_hash(text: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Wait until closing `main` has taken every window down.
    fn wait_for_no_windows(harness: &multiwindow_harness::Harness, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !harness.window_labels().is_empty() {
            assert!(
                Instant::now() < deadline,
                "windows remain after main closed: {:?}",
                harness.window_labels()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn layout_path() -> std::path::PathBuf {
        std::env::var_os(LAYOUT_ENV)
            .map(std::path::PathBuf::from)
            .expect("the parent passes PALACE_LAYOUT_FILE")
    }

    /// Phase 1: detach Users and Chat at known rectangles through the
    /// production registry, then quit by closing `main` — which must close both
    /// panels, exit, and leave both marked detached.
    fn detach_two_phase() {
        let path = layout_path();
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();

        for panel in [Panel::Users, Panel::Chat] {
            let opened = windows::open(&handle, panel).expect("the panel opens");
            assert_eq!(opened, OpenOutcome::Created, "{}", panel.label());
        }
        let users = handle
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists");
        let chat = handle
            .get_webview_window(Panel::Chat.label())
            .expect("panel-chat exists");
        skip_if_screen_too_small(&users);

        place(&users, USERS_RECT);
        place(&chat, CHAT_RECT);
        let actual_users = wait_for_rect(&users, USERS_RECT, Duration::from_secs(10));
        let actual_chat = wait_for_rect(&chat, CHAT_RECT, Duration::from_secs(10));
        assert_eq!(actual_users, USERS_RECT, "Xvfb applies the Users rectangle");
        assert_eq!(actual_chat, CHAT_RECT, "Xvfb applies the Chat rectangle");

        let store = handle.state::<LayoutStore>();
        store.record_geometry(
            Panel::Users.label(),
            WindowGeometry::from_rect(USERS_RECT, monitor_of(&users), true),
        );
        store.record_geometry(
            Panel::Chat.label(),
            WindowGeometry::from_rect(CHAT_RECT, monitor_of(&chat), true),
        );
        assert_eq!(
            store.detached_panels(),
            vec![Panel::Users, Panel::Chat],
            "detaching through the registry marks both panels detached"
        );
        assert_eq!(store.snapshot().order, vec!["users", "chat"]);
        store.flush().expect("the layout file is written");

        // Close `main` the way the user does: this must quit and close both
        // panels rather than orphan them over a dead session.
        handle
            .get_webview_window("main")
            .expect("main exists")
            .close()
            .expect("main's close is requested");
        wait_for_no_windows(&harness, Duration::from_secs(15));
        println!(
            "phase=detach-two all_windows_closed={} labels={:?}",
            harness.window_labels().is_empty(),
            harness.window_labels()
        );

        let saved = geometry::load(&path);
        for panel in [Panel::Users, Panel::Chat] {
            let entry = saved
                .windows
                .get(panel.label())
                .unwrap_or_else(|| panic!("{} is saved", panel.label()));
            assert!(
                entry.detached,
                "{} must stay detached when it closes as part of the quit",
                panel.label()
            );
        }
        assert_eq!(saved.order, vec!["users".to_string(), "chat".to_string()]);
        println!(
            "--- layout file after the quit ---\n{}",
            std::fs::read_to_string(&path).expect("the layout file is readable")
        );
        println!("PHASE_DONE:detach-two");
    }

    /// Phase 2: relaunch from the file phase 1 wrote and prove both panels are
    /// recreated at their saved rectangles, in the saved order.
    fn restore_two_phase() {
        let path = layout_path();
        let saved = geometry::load(&path);
        for (panel, expected) in [(Panel::Users, USERS_RECT), (Panel::Chat, CHAT_RECT)] {
            let entry = saved
                .windows
                .get(panel.label())
                .cloned()
                .unwrap_or_else(|| panic!("{} was saved", panel.label()));
            assert!(
                entry.detached,
                "{} must still be detached, otherwise it never reopens",
                panel.label()
            );
            assert_eq!(entry.rect(), expected);
        }

        let harness = launch_lifecycle_app();
        harness
            .wait_for_window(Panel::Users.label(), multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("the detached Users panel is recreated at startup");
        harness
            .wait_for_window(Panel::Chat.label(), multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("the detached Chat panel is recreated at startup");
        let users = harness
            .handle()
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists");
        let chat = harness
            .handle()
            .get_webview_window(Panel::Chat.label())
            .expect("panel-chat exists");
        skip_if_screen_too_small(&users);

        let actual_users = wait_for_rect(&users, USERS_RECT, multiwindow_harness::DEFAULT_TIMEOUT);
        let actual_chat = wait_for_rect(&chat, CHAT_RECT, multiwindow_harness::DEFAULT_TIMEOUT);
        assert_eq!(actual_users, USERS_RECT, "Users is back at its saved rect");
        assert_eq!(actual_chat, CHAT_RECT, "Chat is back at its saved rect");
        assert_eq!(
            harness.window_labels(),
            vec![
                "main".to_string(),
                "panel-chat".to_string(),
                "panel-users".to_string()
            ],
            "restore reopens exactly the two remembered panels, once each"
        );

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        for label in ["panel-users", "panel-chat"] {
            assert!(
                log_text.contains(&format!("[{label}] restored-from-layout")),
                "{label} must log its restore:\n{log_text}"
            );
        }
        assert!(
            log_text.contains("2 panel(s) reopened"),
            "the restore must report both panels:\n{log_text}"
        );
        println!("phase=restore-two users={actual_users:?} chat={actual_chat:?}");
        println!("--- restore lines from the diagnostic log ---");
        for line in log_text
            .lines()
            .filter(|line| line.contains("restored-from-layout") || line.contains("layout restore"))
        {
            println!("{line}");
        }

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the restore phase exits cleanly");
        println!("PHASE_DONE:restore-two");
    }

    /// Phase 3: a panel's titlebar close (`CloseRequested`) re-docks it, tells
    /// `main` once, and clears the detached flag.
    fn titlebar_close_phase() {
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        install_close_listener(&harness);
        let handle = harness.handle().clone();

        let opened = windows::open(&handle, Panel::Users).expect("the panel opens");
        assert_eq!(opened, OpenOutcome::Created);
        harness
            .wait_for_window(Panel::Users.label(), multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("panel-users appears");
        let store = handle.state::<LayoutStore>();
        assert_eq!(store.detached_panels(), vec![Panel::Users]);

        handle
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists")
            .close()
            .expect("the titlebar close is requested");
        wait_for_window_gone(&harness, Panel::Users.label(), Duration::from_secs(15));

        let payload = wait_for_probe("panel_closed", multiwindow_harness::DEFAULT_TIMEOUT);
        assert!(
            payload.contains("panel-users") && payload.contains("\"panel\":\"users\""),
            "main must receive the re-dock payload, got {payload}"
        );
        assert!(
            store.detached_panels().is_empty(),
            "a re-attached panel must not stay marked detached"
        );
        assert_eq!(
            harness.window_labels(),
            vec!["main".to_string()],
            "only main remains after the panel re-attaches"
        );

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        let reattached = log_text
            .lines()
            .filter(|line| line.contains("[panel-users] reattached"))
            .count();
        assert_eq!(
            reattached, 1,
            "a titlebar close re-attaches exactly once, never twice:\n{log_text}"
        );
        assert!(
            log_text.contains("[panel-users] reattached panel=users via=close"),
            "the close path logs via=close:\n{log_text}"
        );
        assert!(
            !log_text.contains("via=destroyed"),
            "the Destroyed event must not signal a second time:\n{log_text}"
        );
        println!("phase=titlebar-close payload={payload} reattached_lines={reattached}");

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the titlebar-close phase exits cleanly");
        println!("PHASE_DONE:titlebar-close");
    }

    /// Phase 4: a panel destroyed with no close request (a crash or a
    /// window-manager kill) re-docks through the same channel.
    fn dead_webview_phase() {
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        install_close_listener(&harness);
        let handle = harness.handle().clone();

        let opened = windows::open(&handle, Panel::Users).expect("the panel opens");
        assert_eq!(opened, OpenOutcome::Created);
        harness
            .wait_for_window(Panel::Users.label(), multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("panel-users appears");
        let store = handle.state::<LayoutStore>();
        assert_eq!(store.detached_panels(), vec![Panel::Users]);

        // `destroy()` bypasses CloseRequested, exactly like a crashed webview
        // or a window-manager kill: only WindowEvent::Destroyed fires.
        handle
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists")
            .destroy()
            .expect("the panel webview is destroyed");
        wait_for_window_gone(&harness, Panel::Users.label(), Duration::from_secs(15));

        let payload = wait_for_probe("panel_closed", multiwindow_harness::DEFAULT_TIMEOUT);
        assert!(
            payload.contains("panel-users") && payload.contains("\"panel\":\"users\""),
            "the dead webview must re-dock through the same payload, got {payload}"
        );
        assert!(
            store.detached_panels().is_empty(),
            "a dead panel must re-dock, not leave a ghost placeholder"
        );
        assert_eq!(harness.window_labels(), vec!["main".to_string()]);

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        assert!(
            log_text.contains("[panel-users] reattached panel=users via=destroyed"),
            "a spontaneous destroy re-docks with via=destroyed:\n{log_text}"
        );
        assert!(
            !log_text.contains("via=close"),
            "no close request happened, so via=close must be absent:\n{log_text}"
        );
        let reattached = log_text
            .lines()
            .filter(|line| line.contains("[panel-users] reattached"))
            .count();
        assert_eq!(
            reattached, 1,
            "the Destroyed event signals exactly once:\n{log_text}"
        );
        println!("phase=dead-webview payload={payload} reattached_lines={reattached}");

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the dead-webview phase exits cleanly");
        println!("PHASE_DONE:dead-webview");
    }

    /// Phase 5: closing `main` with two panels open closes every window and
    /// exits, without touching the detached flags.
    fn quit_with_panels_phase() {
        let path = layout_path();
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();

        for panel in [Panel::Users, Panel::Chat] {
            assert_eq!(
                windows::open(&handle, panel).expect("the panel opens"),
                OpenOutcome::Created
            );
        }
        for panel in [Panel::Users, Panel::Chat] {
            harness
                .wait_for_window(panel.label(), multiwindow_harness::DEFAULT_TIMEOUT)
                .expect("the panel window appears");
        }
        assert_eq!(
            harness.window_labels(),
            vec![
                "main".to_string(),
                "panel-chat".to_string(),
                "panel-users".to_string()
            ]
        );

        handle
            .get_webview_window("main")
            .expect("main exists")
            .close()
            .expect("main's close is requested");
        wait_for_no_windows(&harness, Duration::from_secs(15));
        println!(
            "phase=quit-with-panels windows_after_main_close={:?}",
            harness.window_labels()
        );

        let saved = geometry::load(&path);
        for panel in [Panel::Users, Panel::Chat] {
            assert!(
                saved
                    .windows
                    .get(panel.label())
                    .is_some_and(|entry| entry.detached),
                "{} must stay detached across the quit",
                panel.label()
            );
        }
        println!("phase=quit-with-panels detached_flags_preserved=true");
        println!("PHASE_DONE:quit-with-panels");
    }

    /// Phase 6: with two panels detached, turning layout memory off freezes the
    /// file — a move, a flush and the quit must all leave it byte-identical.
    fn remember_off_phase() {
        let path = layout_path();
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();

        for panel in [Panel::Users, Panel::Chat] {
            assert_eq!(
                windows::open(&handle, panel).expect("the panel opens"),
                OpenOutcome::Created
            );
        }
        for panel in [Panel::Users, Panel::Chat] {
            harness
                .wait_for_window(panel.label(), multiwindow_harness::DEFAULT_TIMEOUT)
                .expect("the panel window appears");
        }
        let users = handle
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists");
        skip_if_screen_too_small(&users);
        let store = handle.state::<LayoutStore>().inner().clone();
        assert!(
            store.armed(),
            "the store is armed once the startup restore has run"
        );
        store.flush().expect("the detached panels are written");
        assert!(
            std::fs::read_to_string(&path)
                .expect("the layout file is readable")
                .contains("\"panel-users\""),
            "the session starts with a saved layout to freeze"
        );

        let state = geometry::set_layout_remember(handle.clone(), false)
            .expect("turning layout memory off succeeds");
        assert!(!state.remember, "the new state reports memory off");
        assert_eq!(
            state.detached,
            vec!["users".to_string(), "chat".to_string()],
            "the detached list reflects the two open panel windows"
        );
        let frozen = std::fs::read_to_string(&path).expect("the layout file is readable");
        assert!(
            frozen.contains("\"remember\": false"),
            "the toggle reaches the file: {frozen}"
        );

        // Move a panel and give the autosave thread time to try to write.
        place(&users, USERS_RECT);
        let _ = wait_for_rect(&users, USERS_RECT, Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(900));
        store
            .flush()
            .expect("a flush while memory is off is a no-op, not an error");
        let after_flush = std::fs::read_to_string(&path).expect("the layout file is readable");
        assert_eq!(
            after_flush, frozen,
            "no layout state may be written while memory is off"
        );

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the remember-off phase exits cleanly");
        let after_quit = std::fs::read_to_string(&path).expect("the layout file is readable");
        assert_eq!(after_quit, frozen, "the quit must not write either");
        println!("--- the frozen layout file ---\n{frozen}");
        println!(
            "phase=remember-off remember=false detached={:?} frozen_bytes={} frozen_hash={:#018x} \
             after_move_and_flush_hash={:#018x} after_quit_hash={:#018x}",
            state.detached,
            frozen.len(),
            content_hash(&frozen),
            content_hash(&after_flush),
            content_hash(&after_quit),
        );
        println!("PHASE_DONE:remember-off");
    }

    /// Phase 7: relaunch from the frozen file. The default single-window layout
    /// must return, no panel may be reopened, and nothing may be written.
    fn remember_off_relaunch_phase() {
        let path = layout_path();
        let frozen = std::fs::read_to_string(&path).expect("the frozen file is readable");

        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();
        // Give a restore more than enough time to reopen a panel if it were
        // going to: the autosave interval plus a launch's worth of slack.
        std::thread::sleep(Duration::from_millis(1500));

        assert_eq!(
            harness.window_labels(),
            vec!["main".to_string()],
            "memory off must give the default single-window layout"
        );
        let store = handle.state::<LayoutStore>().inner().clone();
        assert!(!store.remember(), "the flag survives the relaunch");
        assert!(
            store.startup_snapshot().windows.is_empty(),
            "the saved geometry is not even loaded while memory is off"
        );
        store.flush().expect("the relaunch writes nothing");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the layout file is readable"),
            frozen,
            "the relaunch must leave the frozen file byte-identical"
        );

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        assert!(
            log_text.contains("layout memory is off"),
            "the launch must say why nothing was restored:\n{log_text}"
        );
        println!(
            "phase=remember-off-relaunch windows={:?} wrote_nothing=true",
            harness.window_labels()
        );

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the relaunch phase exits cleanly");
        println!("PHASE_DONE:remember-off-relaunch");
    }

    /// Phase 8: "reset layout to default" re-docks every panel now: each panel
    /// window is destroyed, `main` receives the re-dock signal for each, the
    /// saved windows are cleared and no panel window is left.
    fn reset_layout_phase() {
        let path = layout_path();
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        install_close_listener(&harness);
        let handle = harness.handle().clone();

        for panel in [Panel::Users, Panel::Chat] {
            assert_eq!(
                windows::open(&handle, panel).expect("the panel opens"),
                OpenOutcome::Created
            );
        }
        for panel in [Panel::Users, Panel::Chat] {
            harness
                .wait_for_window(panel.label(), multiwindow_harness::DEFAULT_TIMEOUT)
                .expect("the panel window appears");
        }
        let users = handle
            .get_webview_window(Panel::Users.label())
            .expect("panel-users exists");
        skip_if_screen_too_small(&users);
        let store = handle.state::<LayoutStore>().inner().clone();
        assert_eq!(
            store.detached_panels(),
            vec![Panel::Users, Panel::Chat],
            "the reset starts from two detached panels"
        );

        let state = geometry::reset_layout(handle.clone()).expect("the reset runs");
        assert!(state.remember, "a reset does not turn memory off");
        assert!(
            state.detached.is_empty(),
            "the reset reports no detached panel, got {:?}",
            state.detached
        );

        for panel in [Panel::Users, Panel::Chat] {
            wait_for_window_gone(&harness, panel.label(), Duration::from_secs(15));
        }
        assert_eq!(
            harness.window_labels(),
            vec!["main".to_string()],
            "no panel window may remain after a reset"
        );

        let payloads =
            wait_for_probe_count("panel_closed", 2, multiwindow_harness::DEFAULT_TIMEOUT);
        for panel in [Panel::Users, Panel::Chat] {
            assert!(
                payloads
                    .iter()
                    .any(|payload| payload.contains(panel.label())),
                "main must be told to re-dock {}; got {payloads:?}",
                panel.label()
            );
        }
        assert!(
            store.detached_panels().is_empty(),
            "the store must not keep a panel detached after the reset"
        );

        let saved = geometry::load(&path);
        assert!(
            saved.windows.is_empty() && saved.order.is_empty(),
            "the reset is remembered: {saved:?}"
        );
        println!(
            "--- the layout file after the reset ---\n{}",
            std::fs::read_to_string(&path).expect("the layout file is readable")
        );
        println!(
            "phase=reset-layout windows={:?} payloads={payloads:?} saved_windows={} saved_order={:?}",
            harness.window_labels(),
            saved.windows.len(),
            saved.order,
        );

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the reset phase exits cleanly");
        println!("PHASE_DONE:reset-layout");
    }

    /// Phase 9: the Preferences window is placed, and the move/resize event
    /// path itself must record its rectangle — not a test-only call.
    fn prefs_geometry_phase() {
        let path = layout_path();
        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();

        assert_eq!(
            windows::open_tool_window(&handle, ToolWindow::Preferences)
                .expect("the preferences window opens"),
            OpenOutcome::Created
        );
        harness
            .wait_for_window(PREFS_LABEL, multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("the preferences window appears");
        let prefs = handle
            .get_webview_window(PREFS_LABEL)
            .expect("prefs exists");
        skip_if_prefs_screen_too_small(&prefs);

        place(&prefs, PREFS_RECT);
        let actual = wait_for_rect(&prefs, PREFS_RECT, Duration::from_secs(10));
        assert_eq!(actual, PREFS_RECT, "Xvfb applies the Preferences rectangle");

        let store = handle.state::<LayoutStore>().inner().clone();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let recorded = store
                .snapshot()
                .windows
                .get(PREFS_LABEL)
                .cloned()
                .is_some_and(|entry| within(entry.rect(), PREFS_RECT) && !entry.detached);
            if recorded {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the move/resize events never recorded the prefs window; snapshot keys: {:?}",
                store.snapshot().windows.keys().collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        store.flush().expect("the layout file is written");

        let saved = geometry::load(&path);
        let entry = saved
            .windows
            .get(PREFS_LABEL)
            .expect("the prefs rectangle is on disk");
        assert_eq!(entry.rect(), PREFS_RECT);
        assert!(
            !entry.detached,
            "a tool window is never marked detached, so it cannot reopen"
        );
        println!("phase=prefs-geometry saved={entry:?}");

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the prefs-geometry phase exits cleanly");
        println!("PHASE_DONE:prefs-geometry");
    }

    /// Phase 10: relaunch and open the Preferences window. It must not appear
    /// on its own, and opening it must apply the saved rectangle.
    fn prefs_geometry_relaunch_phase() {
        let path = layout_path();
        let saved = geometry::load(&path);
        let entry = saved
            .windows
            .get(PREFS_LABEL)
            .cloned()
            .expect("phase 9 saved the prefs rectangle");
        assert_eq!(entry.rect(), PREFS_RECT);

        let harness = launch_lifecycle_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");
        let handle = harness.handle().clone();
        assert_eq!(
            harness.window_labels(),
            vec!["main".to_string()],
            "the Preferences window must not be reopened at startup"
        );

        assert_eq!(
            windows::open_tool_window(&handle, ToolWindow::Preferences)
                .expect("the preferences window opens"),
            OpenOutcome::Created
        );
        harness
            .wait_for_window(PREFS_LABEL, multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("the preferences window appears");
        let prefs = handle
            .get_webview_window(PREFS_LABEL)
            .expect("prefs exists");
        let actual = wait_for_rect(&prefs, PREFS_RECT, Duration::from_secs(10));
        assert_eq!(actual, PREFS_RECT, "the prefs window returns to its rect");

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        assert!(
            log_text.contains("[prefs] restored-from-layout"),
            "the restore path must run for the prefs label:\n{log_text}"
        );
        println!("phase=prefs-geometry-relaunch rect={actual:?}");

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the reopen phase exits cleanly");
        println!("PHASE_DONE:prefs-geometry-relaunch");
    }

    #[test]
    fn layout_lifecycle_child_phase() {
        let Some(phase) = std::env::var_os(PHASE_ENV) else {
            return;
        };
        match phase.to_str() {
            Some("detach-two") => detach_two_phase(),
            Some("restore-two") => restore_two_phase(),
            Some("titlebar-close") => titlebar_close_phase(),
            Some("dead-webview") => dead_webview_phase(),
            Some("quit-with-panels") => quit_with_panels_phase(),
            Some("remember-off") => remember_off_phase(),
            Some("remember-off-relaunch") => remember_off_relaunch_phase(),
            Some("reset-layout") => reset_layout_phase(),
            Some("prefs-geometry") => prefs_geometry_phase(),
            Some("prefs-geometry-relaunch") => prefs_geometry_relaunch_phase(),
            other => panic!("unknown layout lifecycle phase {other:?}"),
        }
    }

    /// Re-execute this test binary as a child running one phase.
    ///
    /// `GDK_BACKEND=x11` is forced and `WAYLAND_DISPLAY` removed so GDK cannot
    /// prefer the developer's real Wayland session over the Xvfb (which would
    /// test the wrong backend and touch the real desktop).
    pub fn run_child(phase: &str, layout: &Path, log_dir: &Path) -> Output {
        let exe = std::env::current_exe().expect("the test binary path");
        Command::new(exe)
            .arg(CHILD_TEST)
            .arg("--nocapture")
            .env(PHASE_ENV, phase)
            .env(LAYOUT_ENV, layout)
            .env(logging::ENV_LOG_DIR, log_dir)
            .env(logging::ENV_LOG, "debug")
            .env("GDK_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("the child test process starts")
    }
}

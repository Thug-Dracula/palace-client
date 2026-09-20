//! Task 7: window-layout persistence, the off-screen clamp, and startup
//! restore.
//!
//! Three kinds of proof live here:
//!
//! * the pure file contract — a known layout round-trips through
//!   `window-layout.json`, extra keys are ignored, a malformed file falls back
//!   to empty with a warning, and a file written by a newer schema is never
//!   overwritten;
//! * the clamp's required scenarios through the public API (the exhaustive
//!   unit cases live in `geometry.rs`);
//! * a **real restart** on a proven virtual display: one child process writes
//!   `panel-users` geometry and marks it detached, asserts the file, and quits;
//!   a second child process launches from that file and asserts the window is
//!   recreated at the saved rectangle, with a `restored-from-layout` log line.
//!   Each phase is its own OS process because GTK allows one Tauri app per
//!   process — the pattern `tests/window_logging.rs` established.
//!
//! Run the real one headless:
//!
//! ```text
//! GDK_BACKEND=x11 xvfb-run -a cargo test -p palace-app --test geometry_persistence -- --nocapture --test-threads=1
//! ```
//!
//! Each child process is given `GDK_BACKEND=x11` with `WAYLAND_DISPLAY`
//! removed. GDK otherwise prefers the user's real Wayland session (reachable
//! through `XDG_RUNTIME_DIR`) even when `DISPLAY` names an Xvfb, which would
//! both test the wrong backend and touch the user's desktop.

#![cfg(any(target_os = "linux", target_os = "windows"))]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use palace_app_lib::geometry::{
    self, Layout, LayoutStore, Rect, Screen, WindowGeometry, LAYOUT_FILE,
};
use palace_app_lib::logging::{self, Level};
use palace_app_lib::windows::Panel;

#[cfg(target_os = "linux")]
#[path = "multiwindow_harness.rs"]
mod multiwindow_harness;

/// A fresh directory under the temp dir so no test can touch the real layout.
fn scratch(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("palace-geometry-{tag}-{unique}"));
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

fn screen(name: &str, x: i32, y: i32, w: u32, h: u32) -> Screen {
    Screen {
        name: Some(name.to_string()),
        x,
        y,
        w,
        h,
    }
}

/// A 1920×1080 primary with a second 1920×1080 screen to its right.
fn two_screens() -> Vec<Screen> {
    vec![
        screen("primary", 0, 0, 1920, 1080),
        screen("right", 1920, 0, 1920, 1080),
    ]
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

// ---------------------------------------------------------------------------
// The clamp, through the public API
// ---------------------------------------------------------------------------

#[test]
fn clamp_centres_a_window_whose_saved_monitor_is_gone() {
    let saved = rect(2500, -300, 640, 480);
    let placement = geometry::clamp_placement(saved, Some("unplugged"), &two_screens(), 0);
    assert_eq!(
        placement.rect,
        rect(640, 300, 640, 480),
        "the window must land centred on the primary, fully visible"
    );
    assert!(placement.centred, "a gone monitor means a fresh centre");
    assert_eq!(placement.screen.as_deref(), Some("primary"));
}

#[test]
fn clamp_pulls_a_partly_off_screen_window_fully_back_on_screen() {
    let saved = rect(-120, -80, 640, 480);
    let placement = geometry::clamp_placement(saved, Some("primary"), &two_screens(), 0);
    assert_eq!(
        placement.rect,
        rect(0, 0, 640, 480),
        "the whole rectangle must be visible, not just part of it"
    );
    assert!(placement.moved);
    assert!(
        !placement.centred,
        "the saved monitor still exists, so the window only gets pulled in"
    );
}

#[test]
fn clamp_leaves_a_valid_rectangle_unchanged() {
    let saved = rect(300, 240, 640, 480);
    let placement = geometry::clamp_placement(saved, Some("primary"), &two_screens(), 0);
    assert_eq!(placement.rect, saved);
    assert!(
        !placement.moved,
        "a rectangle already on screen must not be nudged"
    );
}

// ---------------------------------------------------------------------------
// The file contract
// ---------------------------------------------------------------------------

#[test]
fn a_layout_round_trips_through_the_file_and_leaves_no_temp_behind() {
    let directory = scratch("round-trip");
    let path = directory.join(LAYOUT_FILE);
    let mut layout = Layout::default();
    layout.order.push("users".to_string());
    layout.windows.insert(
        "panel-users".to_string(),
        WindowGeometry::from_rect(rect(-20, 30, 500, 400), Some("right".to_string()), true),
    );

    geometry::save(&path, &layout).expect("the layout is writable");
    assert_eq!(geometry::load(&path), layout, "the file round-trips");

    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert!(text.contains("\"panel-users\""), "{text}");
    assert!(text.contains("\"order\""), "{text}");
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the atomic write must not leave its temp file behind"
    );

    geometry::save(&path, &layout).expect("an unchanged layout is a no-op");
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file is still readable"),
        text,
        "saving an unchanged layout must not rewrite the file"
    );

    println!("--- window-layout.json ---");
    print!("{text}");
    println!("\n--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn extra_keys_in_the_layout_file_are_ignored() {
    let directory = scratch("extra-keys");
    let path = directory.join(LAYOUT_FILE);
    std::fs::write(
        &path,
        r#"{"version":1,"future_field":{"nested":true},"windows":{"panel-users":{"x":1,"y":2,"w":3,"h":4,"detached":true,"future_per_window":7}}}"#,
    )
    .expect("the fixture is writable");

    let layout = geometry::load(&path);
    assert_eq!(
        layout.windows.get("panel-users").map(WindowGeometry::rect),
        Some(rect(1, 2, 3, 4)),
        "a file from a future build still loads what this build understands"
    );
    assert!(layout
        .windows
        .get("panel-users")
        .is_some_and(|entry| entry.detached));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_malformed_layout_file_falls_back_to_empty_and_warns() {
    let directory = scratch("malformed");
    let path = directory.join(LAYOUT_FILE);
    let malformed = "{ this is not json";
    std::fs::write(&path, malformed).expect("the bad file is writable");
    let log_dir = scratch("malformed-log");
    let log_path = logging::init_at(&log_dir, Level::Warn).expect("the log opens");

    assert_eq!(
        geometry::load(&path),
        Layout::default(),
        "a malformed file falls back instead of failing"
    );

    let log = std::fs::read_to_string(&log_path).expect("the log is readable");
    assert!(
        log.contains("malformed window layout"),
        "the warning reached the log:\n{log}"
    );
    assert!(
        log.contains(&path.display().to_string()),
        "the warning names the file:\n{log}"
    );

    println!("--- malformed file ---");
    println!("{malformed}");
    println!("--- warning lines from the diagnostic log ---");
    for line in log.lines().filter(|line| line.contains("layout")) {
        println!("{line}");
    }
    println!("--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&log_dir);
}

#[test]
fn a_malformed_file_is_replaced_by_the_next_flush() {
    let directory = scratch("repair");
    let path = directory.join(LAYOUT_FILE);
    std::fs::write(&path, "not json at all").expect("the bad file is writable");

    let store = LayoutStore::open(Some(path.clone()));
    store.set_detached(Panel::Users, true);
    store.flush().expect("a broken file may be replaced");

    let repaired = geometry::load(&path);
    assert_eq!(repaired.order, vec!["users".to_string()]);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_newer_schema_file_is_never_overwritten() {
    let directory = scratch("newer");
    let path = directory.join(LAYOUT_FILE);
    let original = r#"{
  "version": 99,
  "order": ["users"],
  "windows": {
    "panel-users": {
      "x": 1,
      "y": 2,
      "w": 3,
      "h": 4,
      "detached": true
    }
  }
}"#;
    std::fs::write(&path, original).expect("the newer fixture is writable");

    let store = LayoutStore::open(Some(path.clone()));
    assert_eq!(
        store.startup_snapshot(),
        Layout::default(),
        "a newer schema is not interpreted"
    );
    store.set_detached(Panel::Users, true);
    store.flush().expect("a blocked write is not an error");
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file is still readable"),
        original,
        "a file written by a newer build must stay byte-identical"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

// ---------------------------------------------------------------------------
// Real restart on a virtual display
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod real_runtime {
    use std::path::Path;
    use std::process::{Command, Output, Stdio};
    use std::time::{Duration, Instant};

    use palace_app_lib::geometry::{self, LayoutStore, Rect, WindowGeometry, LAYOUT_ENV};
    use palace_app_lib::logging;
    use palace_app_lib::windows::{self, OpenOutcome, Panel};
    use tauri::{Manager, PhysicalPosition, PhysicalSize};

    use super::{multiwindow_harness, scratch};

    /// The environment variable that tells a child process which phase to run.
    const PHASE_ENV: &str = "PALACE_GEOMETRY_PHASE";

    /// The single child test the parent re-executes itself with.
    const CHILD_TEST: &str = "geometry_child_phase";

    /// The rectangle phase 1 stores and phase 2 must restore. It is
    /// deliberately different from the panel's default 640×480 so a restore
    /// that silently kept the default would fail.
    const SAVED: Rect = Rect {
        x: 140,
        y: 110,
        w: 520,
        h: 430,
    };

    /// A second known rectangle, used to prove the debounced move/resize
    /// autosave writes while the app is still running.
    const MOVED: Rect = Rect {
        x: 200,
        y: 180,
        w: 560,
        h: 470,
    };

    /// How far the OS may be from the requested rectangle and still count.
    const TOLERANCE: i32 = 2;

    /// Launch the app with the same layout wiring production uses: the store
    /// is discovered from `PALACE_LAYOUT_FILE`, managed, autosaved, restored on
    /// the main thread, and the production window-event handler is attached.
    fn launch_layout_app() -> multiwindow_harness::Harness {
        multiwindow_harness::Harness::launch_with(|builder| {
            builder
                .invoke_handler(tauri::generate_handler![
                    multiwindow_harness::command::harness_report
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
        .expect("the real runtime launches with layout memory wired")
    }

    /// The window's client rectangle, which is what the geometry layer stores
    /// and restores (`set_position` moves it, `set_size` sets this size).
    ///
    /// The outer rectangle is deliberately not used: a bare Xvfb has no window
    /// manager, so the platform reports the frame as `(0, 0)` and the outer
    /// values carry no information.
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

    /// Wait until the window really sits at `expected`, and return where it is.
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

    fn layout_path() -> std::path::PathBuf {
        std::env::var_os(LAYOUT_ENV)
            .map(std::path::PathBuf::from)
            .expect("the parent passes PALACE_LAYOUT_FILE")
    }

    /// Phase 1: open the panel, place it at [`SAVED`], persist that geometry
    /// and the detached flag, then quit like a user closing the app.
    fn write_phase() {
        let path = layout_path();
        let harness = launch_layout_app();
        harness
            .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("main boots its real webview");

        let handle = harness.handle().clone();
        let opened = windows::open(&handle, Panel::Users).expect("the panel opens");
        assert_eq!(opened, OpenOutcome::Created);
        let label = Panel::Users.label();
        let window = handle
            .get_webview_window(label)
            .expect("the panel window exists");

        window
            .set_position(PhysicalPosition::new(SAVED.x, SAVED.y))
            .expect("the window moves");
        window
            .set_size(PhysicalSize::new(SAVED.w, SAVED.h))
            .expect("the window resizes");
        let actual = wait_for_rect(&window, SAVED, Duration::from_secs(10));
        assert_eq!(actual, SAVED, "Xvfb must apply the requested rectangle");

        let monitor = geometry::capture(&window).and_then(|entry| entry.monitor);
        let store = handle.state::<LayoutStore>();
        store.record_geometry(
            label,
            WindowGeometry::from_rect(SAVED, monitor.clone(), true),
        );
        store.set_detached(Panel::Users, true);
        store.flush().expect("the layout file is written");

        let saved = geometry::load(&path);
        assert_eq!(
            saved.windows.get(label).map(WindowGeometry::rect),
            Some(SAVED),
            "the file holds the rectangle"
        );
        assert!(
            saved.windows.get(label).is_some_and(|entry| entry.detached),
            "the panel is marked detached"
        );
        assert_eq!(saved.order, vec!["users".to_string()]);

        println!("phase=write layout_file={}", path.display());
        println!("phase=write monitor={monitor:?}");
        println!(
            "--- layout file after the write phase ---\n{}",
            std::fs::read_to_string(&path).expect("the layout file is readable")
        );

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the write phase exits cleanly");
    }

    /// Phase 2: relaunch from the file phase 1 wrote and prove the panel is
    /// recreated at the saved rectangle, with a restore line in the log.
    fn restore_phase() {
        let path = layout_path();
        let expected = geometry::load(&path)
            .windows
            .get(Panel::Users.label())
            .cloned()
            .expect("phase 1 saved the panel");
        assert!(
            expected.detached,
            "detach state must survive the quit, otherwise the panel never reopens"
        );
        assert_eq!(expected.rect(), SAVED);

        let harness = launch_layout_app();
        harness
            .wait_for_window(Panel::Users.label(), multiwindow_harness::DEFAULT_TIMEOUT)
            .expect("the detached panel is recreated at startup");
        let window = harness
            .handle()
            .get_webview_window(Panel::Users.label())
            .expect("the restored window exists");
        let actual = wait_for_rect(
            &window,
            expected.rect(),
            multiwindow_harness::DEFAULT_TIMEOUT,
        );
        assert_eq!(actual, SAVED, "the window is back at the saved rectangle");
        assert_eq!(
            harness.window_labels(),
            vec!["main".to_string(), Panel::Users.label().to_string()],
            "restore reopens exactly the remembered panel, once"
        );

        let log_text =
            std::fs::read_to_string(harness.log_path()).expect("the diagnostic log is readable");
        assert!(
            log_text.contains("[panel-users] restored-from-layout"),
            "the restore must be logged:\n{log_text}"
        );
        assert!(
            log_text.contains("clamped=false"),
            "the saved rectangle was on-screen, so it must not be clamped:\n{log_text}"
        );

        // The automatic path: move events must reach the store and the
        // debounced autosave must write them to disk while the app runs.
        window
            .set_position(PhysicalPosition::new(MOVED.x, MOVED.y))
            .expect("the restored window moves");
        window
            .set_size(PhysicalSize::new(MOVED.w, MOVED.h))
            .expect("the restored window resizes");
        let file_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let on_disk = geometry::load(&path)
                .windows
                .get(Panel::Users.label())
                .map(WindowGeometry::rect);
            if on_disk == Some(MOVED) {
                break;
            }
            assert!(
                Instant::now() < file_deadline,
                "the layout file never picked up the live move; it holds {on_disk:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("phase=restore autosave_picked_up={MOVED:?}");

        println!("phase=restore expected={SAVED:?} actual={actual:?}");
        println!("--- restore lines from the diagnostic log ---");
        for line in log_text
            .lines()
            .filter(|line| line.contains("restored-from-layout") || line.contains("layout restore"))
        {
            println!("{line}");
        }

        let code = harness.quit().expect("the app stops");
        assert_eq!(code, 0, "the restore phase exits cleanly");
    }

    /// The child body. It does nothing unless the parent set [`PHASE_ENV`], so
    /// a plain `cargo test` run treats it as a passing no-op.
    #[test]
    fn geometry_child_phase() {
        let Some(phase) = std::env::var_os(PHASE_ENV) else {
            return;
        };
        match phase.to_str() {
            Some("write") => write_phase(),
            Some("restore") => restore_phase(),
            other => panic!("unknown geometry phase {other:?}"),
        }
    }

    /// Re-execute this test binary as a child running one phase.
    ///
    /// `GDK_BACKEND=x11` is forced and `WAYLAND_DISPLAY` removed: GDK prefers a
    /// Wayland session whenever one is reachable through `XDG_RUNTIME_DIR`,
    /// even with `DISPLAY` set to an Xvfb, and on Wayland a client may not
    /// position its own windows — the restart proof would silently test the
    /// wrong backend (and, worse, touch the user's real session).
    fn run_child(phase: &str, layout: &Path, log_dir: &Path) -> Output {
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

    fn print_child(phase: &str, output: &Output) {
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

    /// The acceptance test: a known rectangle survives a real quit and
    /// relaunch, and the panel that was detached is recreated by itself.
    #[test]
    fn panel_geometry_is_restored_after_a_real_restart() {
        let display = match multiwindow_harness::virtual_display() {
            Ok(display) => display,
            Err(reason) => {
                eprintln!("SKIP panel_geometry_is_restored_after_a_real_restart: {reason}");
                return;
            }
        };
        println!("display_provenance={}", display.provenance());

        let directory = scratch("restart");
        let layout = directory.join(super::LAYOUT_FILE);
        let log_dir = directory.join("log");
        std::fs::create_dir_all(&log_dir).expect("the log directory is creatable");

        let write = run_child("write", &layout, &log_dir);
        print_child("write", &write);
        assert!(
            write.status.success(),
            "the write phase failed; see its output above"
        );
        let saved = geometry::load(&layout);
        assert_eq!(
            saved
                .windows
                .get(Panel::Users.label())
                .map(WindowGeometry::rect),
            Some(SAVED),
            "after the write phase the file must hold the saved rectangle"
        );

        let restore = run_child("restore", &layout, &log_dir);
        print_child("restore", &restore);
        assert!(
            restore.status.success(),
            "the restore phase failed; see its output above"
        );

        let final_layout = geometry::load(&layout);
        assert_eq!(
            final_layout
                .windows
                .get(Panel::Users.label())
                .map(WindowGeometry::rect),
            Some(MOVED),
            "the last rectangle the restored app reported must survive its quit"
        );
        let final_text = std::fs::read_to_string(&layout).expect("the layout file is readable");
        println!("--- layout file after both phases ---\n{final_text}");
        println!(
            "restart_evidence=phase write saved {:?}; phase restore reopened it there, moved it to {:?} \
             (autosave), and the file survived the quit",
            SAVED, MOVED
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}

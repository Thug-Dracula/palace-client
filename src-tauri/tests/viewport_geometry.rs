//! Task 17 acceptance: a room-viewport geometry reply reaches only the window
//! that owns the room view, is tagged with an owner and a monotonic epoch, and
//! a zero or nonsense report is never composited for.
//!
//! The pure tests run everywhere. The real-runtime test launches the app on a
//! proven virtual display (see `multiwindow_harness`) and drives three real
//! webviews; without a virtual display it prints `SKIP …` and passes, so a
//! headless `cargo test` stays green without pretending the harness ran.
//!
//! ```text
//! xvfb-run -a cargo test -p palace-app --test viewport_geometry -- --nocapture --test-threads=1
//! ```

#![cfg(target_os = "linux")]

#[path = "multiwindow_harness.rs"]
mod multiwindow_harness;

use palace_app_lib::commands::viewport_report_is_usable;
use palace_app_lib::protocol::{ViewportClaim, ViewportOwnerSlot};
use palace_app_lib::windows::owns_room_view;
use palace_client::{ScreenState, ViewGeometry};
use tauri::Manager;

// ---------------------------------------------------------------------------
// Pure policy
// ---------------------------------------------------------------------------

#[test]
fn only_the_room_window_may_report_a_viewport() {
    assert!(owns_room_view("main"));
    assert!(owns_room_view("panel-room"));
    for other in [
        "panel-users",
        "panel-rooms",
        "panel-chat",
        "panel-props",
        "panel-spike",
        "",
        "panel-",
    ] {
        assert!(
            !owns_room_view(other),
            "{other:?} must not own the room view"
        );
    }
}

#[test]
fn claims_name_the_owner_and_advance_the_epoch_monotonically() {
    let slot = ViewportOwnerSlot::default();
    assert_eq!(slot.current(), None);

    let first = slot.claim("main");
    let second = slot.claim("panel-room");
    assert!(second > first);
    assert_eq!(
        slot.current(),
        Some(ViewportClaim {
            owner: "panel-room".to_string(),
            epoch: second,
        })
    );

    let third = slot.claim("main");
    assert!(third > second);
}

#[test]
fn a_zero_or_nonsense_report_is_not_composited() {
    assert!(!viewport_report_is_usable(0.0, 480.0, 1.0, 1.0));
    assert!(!viewport_report_is_usable(640.0, 0.0, 1.0, 1.0));
    assert!(!viewport_report_is_usable(f64::NAN, 480.0, 1.0, 1.0));
    assert!(!viewport_report_is_usable(640.0, f64::INFINITY, 1.0, 1.0));
    assert!(!viewport_report_is_usable(640.0, 480.0, 0.0, 1.0));
    assert!(!viewport_report_is_usable(640.0, 480.0, 1.0, 0.0));
    assert!(viewport_report_is_usable(640.0, 480.0, 1.0, 1.0));
}

// ---------------------------------------------------------------------------
// Real windows on a virtual display
// ---------------------------------------------------------------------------

fn sample_screen(room_name: &str) -> ScreenState {
    ScreenState {
        version: 1,
        mid_version: None,
        top_version: None,
        room_id: 1,
        room_name: room_name.to_string(),
        avatars: 0,
        loose_props: 0,
        props_pending: 0,
        notes: Vec::new(),
        geometry: ViewGeometry {
            room_w: 100.0,
            room_h: 80.0,
            viewport_w: 640.0,
            viewport_h: 480.0,
            dpr: 1.0,
            zoom: 1.0,
            native: false,
            scale: 1.0,
            content_x: 0.0,
            content_y: 0.0,
            content_w: 100.0,
            content_h: 80.0,
            bitmap_w: 100,
            bitmap_h: 80,
        },
    }
}

/// JavaScript that registers a **label-scoped** listener for the geometry
/// event and reports what it receives, then reports that it is listening.
///
/// The scoped target is the point: a listener registered with the default
/// `Any` target would receive `emit_to` replies meant for another window
/// (Task 1, Q3). Each label is a safe ASCII token, so embedding it as a JSON
/// string literal cannot escape into code.
fn listener_js(label: &str) -> String {
    let label = format!("\"{label}\"");
    format!(
        r#"
(function () {{
  function boot() {{
    var I = window.__TAURI_INTERNALS__;
    if (!I || !I.invoke || !I.transformCallback) {{ return setTimeout(boot, 40); }}
    if (window.__palaceGeometryInstalled) {{ return; }}
    window.__palaceGeometryInstalled = true;
    function report(kind) {{
      I.invoke('harness_report', {{ label: {label}, kind: kind }}).catch(function () {{}});
    }}
    I.invoke('plugin:event|listen', {{
      event: 'palace://geometry',
      target: {{ kind: 'AnyLabel', label: {label} }},
      handler: I.transformCallback(function (e) {{
        var p = (e && e.payload) ? e.payload : {{}};
        var room = (p.screen && p.screen.room_name) ? p.screen.room_name : '?';
        report('geom ' + p.owner + ' ' + p.epoch + ' ' + room);
      }})
    }}).then(function () {{ report('geom-listening'); }})
      .catch(function (error) {{ report('geom-error ' + String(error)); }});
  }}
  boot();
}})();
"#
    )
}

#[test]
fn a_geometry_reply_reaches_only_the_window_that_owns_the_room_view() {
    let display = match multiwindow_harness::virtual_display() {
        Ok(display) => display,
        Err(reason) => {
            eprintln!("SKIP viewport_geometry: {reason}");
            return;
        }
    };
    println!("display_provenance={}", display.provenance());

    let harness = multiwindow_harness::Harness::launch_with(|builder| {
        builder
            .manage(ViewportOwnerSlot::default())
            .invoke_handler(tauri::generate_handler![
                multiwindow_harness::command::harness_report
            ])
    })
    .expect("the real runtime launches with the viewport-owner slot managed");

    harness
        .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
        .expect("main boots its real webview");
    harness
        .open_panel("panel-room")
        .expect("panel-room opens and boots");
    harness
        .open_panel("panel-users")
        .expect("panel-users opens and boots");

    let mark = harness.mark_log();
    let labels = ["main", "panel-room", "panel-users"];
    for label in labels {
        harness
            .eval_in_window(label, &listener_js(label))
            .expect("the listener installs in the window");
    }
    for label in labels {
        harness
            .wait_for_log_since(
                mark,
                &format!("[{label}] harness geom-listening"),
                multiwindow_harness::DEFAULT_TIMEOUT,
            )
            .unwrap_or_else(|error| panic!("{label} did not register its listener: {error}"));
    }

    // No window has claimed the viewport, so a composed screen belongs to no
    // one and must not be delivered anywhere.
    palace_app_lib::forward_screen(harness.handle(), &sample_screen("nobody"));
    std::thread::sleep(std::time::Duration::from_millis(400));
    let unowned = harness.read_log_since(mark);
    assert!(
        !unowned.contains("harness geom "),
        "no window owns the viewport, so no geometry may be delivered: {unowned}"
    );

    // A claim by the room panel targets the room panel only.
    let owners = harness.handle().state::<ViewportOwnerSlot>();
    let room_epoch = owners.claim("panel-room");
    palace_app_lib::forward_screen(harness.handle(), &sample_screen("room-only"));

    let room_log = harness
        .wait_for_log_since(
            mark,
            "harness geom panel-room",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("the room panel receives the geometry it owns");
    assert!(
        room_log.contains(&format!(
            "[panel-room] harness geom panel-room {room_epoch} room-only"
        )),
        "the room panel must receive its own owner+epoch+geometry: {room_log}"
    );
    assert!(
        !room_log.contains("[main] harness geom "),
        "main must not receive geometry targeted at the room panel: {room_log}"
    );
    assert!(
        !room_log.contains("[panel-users] harness geom "),
        "the users panel must never receive room geometry: {room_log}"
    );

    // A later claim by main retargets the reply, and the room panel no longer
    // sees it.
    let main_epoch = owners.claim("main");
    let second = harness.mark_log();
    palace_app_lib::forward_screen(harness.handle(), &sample_screen("main-only"));

    let main_log = harness
        .wait_for_log_since(
            second,
            "harness geom main",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("main receives the geometry it now owns");
    assert!(
        main_log.contains(&format!("[main] harness geom main {main_epoch} main-only")),
        "main must receive its own owner+epoch+geometry: {main_log}"
    );
    assert!(
        !main_log.contains("[panel-room] harness geom "),
        "the room panel must not receive geometry retargeted at main: {main_log}"
    );
    assert!(
        !main_log.contains("[panel-users] harness geom "),
        "the users panel must not receive geometry retargeted at main: {main_log}"
    );

    let code = harness.quit().expect("the app stops");
    assert_eq!(code, 0, "the harness app exits cleanly");
}

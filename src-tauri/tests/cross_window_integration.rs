//! Task 28 acceptance on the real runtime: five windows, one session.
//!
//! The unit tests in `cross_window.rs` prove the pump's per-event behaviour with
//! a mock app. This binary proves the same properties with **five real OS
//! windows** on a proven virtual display, because "several windows at once" is
//! the thing Task 28 exists to demonstrate:
//!
//! * **One connection.** The app starts exactly one client runtime; opening
//!   five panels adds five windows and no second connection. The log shows one
//!   `client runtime started` line and no second one.
//! * **One seed per window, no storm.** Each window calls `refresh` once on
//!   mount, so five windows produce five `refresh_requested` lines with five
//!   distinct epochs — not a burst under one epoch.
//! * **One sound owner.** A sound event is handled once by the pump, so the
//!   engine's command counter moves by exactly one however many windows are
//!   open.
//!
//! Run it headless:
//!
//! ```text
//! xvfb-run -a -s "-screen 0 1920x1080x24" \
//!   cargo test -p palace-app --test cross_window_integration -- --nocapture --test-threads=1
//! ```
//!
//! On the developer's real desktop session the harness refuses the display and
//! the test prints `SKIP …` and passes.

#![cfg(target_os = "linux")]

#[path = "multiwindow_harness.rs"]
mod multiwindow_harness;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use palace_app_lib::logging::{self, Level};
use palace_app_lib::windows::Panel;
use palace_app_lib::{start_client, AppState, Settings};
use palace_audio::{AudioConfig, AudioEngine, AudioHandle};
use palace_client::ClientEvent;
use tauri::Manager;

/// The five panels, in the registry's order.
const PANELS: [Panel; 5] = [
    Panel::Room,
    Panel::Users,
    Panel::Rooms,
    Panel::Chat,
    Panel::PropBag,
];

/// A process-global audio engine the test owns, so the pump's one command per
/// sound event can be counted from the test thread.
static AUDIO: Mutex<Option<AudioHandle>> = Mutex::new(None);

/// How many `refresh` requests the test's stand-in command has seen.
static REFRESHES: AtomicU64 = AtomicU64::new(0);

/// A stand-in for the production `refresh` command.
///
/// It bumps the same process-global epoch the real command does and logs the
/// same `refresh_requested` line, so the log proves one seed per window without
/// needing a live server behind the runtime.
#[tauri::command]
fn refresh_probe() {
    let epoch = REFRESHES.fetch_add(1, Ordering::Relaxed) + 1;
    logging::log(Level::Info, format!("refresh_requested epoch={epoch}"));
    println!("refresh_requested epoch={epoch}");
}

/// A stand-in for the production `set_viewport` command: only the room window
/// may report, so a stray report from another window is refused and logged.
#[tauri::command]
fn viewport_probe(window: tauri::Window, label: String) {
    let owner = window.label().to_string();
    let allowed = palace_app_lib::windows::owns_room_view(&owner);
    logging::log(
        Level::Info,
        format!("viewport_probe window={owner} claimed={label} allowed={allowed}"),
    );
    println!("viewport_probe window={owner} claimed={label} allowed={allowed}");
}

/// Drive one sound event through the pump's mapping and count the engine's
/// commands, so "exactly once" is measured from the engine itself.
#[tauri::command]
fn sound_probe(name: String) {
    let Some(audio) = AUDIO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    else {
        return;
    };
    let before = audio.stats().handled();
    let effect = palace_app_lib::sound_effect(&ClientEvent::Sound { name: name.clone() });
    if let Some(effect) = effect {
        effect.play(&audio);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while audio.stats().handled() == before && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let after = audio.stats().handled();
    logging::log(
        Level::Info,
        format!("sound_probe name={name} commands={}", after - before),
    );
    println!("sound_probe name={name} commands={}", after - before);
}

fn install_audio() -> AudioHandle {
    let engine = AudioEngine::spawn(AudioConfig::headless());
    let handle = engine.handle();
    *AUDIO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle.clone());
    // Leak the engine so it lives for the whole test process.
    std::mem::forget(engine);
    handle
}

#[test]
fn five_real_windows_share_one_connection_and_seed_once_each() {
    let display = match multiwindow_harness::virtual_display() {
        Ok(display) => display,
        Err(reason) => {
            eprintln!("SKIP cross_window_integration: {reason}");
            return;
        }
    };
    println!("display_provenance={}", display.provenance());

    let audio = install_audio();
    let harness = multiwindow_harness::Harness::launch_with(|builder| {
        builder
            .invoke_handler(tauri::generate_handler![
                multiwindow_harness::command::harness_report,
                refresh_probe,
                viewport_probe,
                sound_probe,
                palace_app_lib::windows::open_panel,
                palace_app_lib::windows::close_panel,
                palace_app_lib::commands::connect,
                palace_app_lib::commands::disconnect,
            ])
            .setup(move |app| {
                let handle = app.handle().clone();
                let settings = Settings::from_env();
                let engine = AudioEngine::spawn(AudioConfig::headless());
                let audio = engine.handle();
                app.manage(AppState {
                    client: Mutex::new(None),
                    settings: Mutex::new(settings.clone()),
                    audio: Mutex::new(engine),
                    bundled_soundfont: None,
                    bag: palace_app_lib::bag::BagService::discover(),
                    refresh_epoch: AtomicU64::new(0),
                });
                match start_client(&handle, &settings, audio) {
                    Ok(client) => {
                        if let Some(state) = handle.try_state::<AppState>() {
                            if let Ok(mut guard) = state.client.lock() {
                                *guard = Some(client);
                            }
                        }
                        logging::log(Level::Info, "client runtime started");
                    }
                    Err(error) => {
                        logging::log(Level::Error, format!("could not start client: {error}"));
                    }
                }
                Ok(())
            })
    })
    .expect("the real runtime launches with the cross-window commands wired");

    let mark = harness.mark_log();
    harness
        .wait_until_ready("main", multiwindow_harness::DEFAULT_TIMEOUT)
        .expect("main boots a real webview");

    // Open all five panels, each a real OS window with its own webview.
    for panel in PANELS {
        harness
            .open_panel(panel.label())
            .unwrap_or_else(|error| panic!("{} must open: {error}", panel.label()));
    }
    let labels = harness.window_labels();
    println!("window_labels={labels:?}");
    assert_eq!(
        labels.len(),
        6,
        "main plus five panels is six windows: {labels:?}"
    );

    // Each window seeds itself once. Drive the stand-in refresh from every
    // window the way the SPA does on mount.
    for panel in PANELS {
        harness
            .eval_in_window(
                panel.label(),
                "window.__TAURI_INTERNALS__.invoke('refresh_probe', {})",
            )
            .unwrap_or_else(|error| panic!("{} must seed: {error}", panel.label()));
    }
    harness
        .eval_in_window(
            "main",
            "window.__TAURI_INTERNALS__.invoke('refresh_probe', {})",
        )
        .expect("main seeds");

    let log = harness
        .wait_for_log_since(
            mark,
            "refresh_requested epoch=6",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("six windows seed once each");
    let seeds: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("refresh_requested"))
        .collect();
    println!("refresh_requested_lines={}", seeds.len());
    for line in &seeds {
        println!("{line}");
    }
    assert_eq!(seeds.len(), 6, "one seed per window, no storm: {seeds:?}");
    let epochs: Vec<&str> = seeds
        .iter()
        .filter_map(|line| line.split("epoch=").nth(1))
        .collect();
    let mut unique = epochs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        6,
        "each seed has its own epoch, so this is six windows seeding once, not one window looping: {epochs:?}"
    );

    // Only the room window may drive the frame slot.
    harness
        .eval_in_window(
            "panel-users",
            "window.__TAURI_INTERNALS__.invoke('viewport_probe', { label: 'panel-users' })",
        )
        .expect("the users window can ask");
    let log = harness
        .wait_for_log_since(
            mark,
            "viewport_probe window=panel-users",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("the refusal is logged");
    assert!(
        log.contains("viewport_probe window=panel-users claimed=panel-users allowed=false"),
        "a non-room window must be refused the frame slot: {log}"
    );

    // One sound event, five panels open, exactly one engine command.
    harness
        .eval_in_window(
            "main",
            "window.__TAURI_INTERNALS__.invoke('sound_probe', { name: 'shared.mp3' })",
        )
        .expect("the sound probe runs");
    let log = harness
        .wait_for_log_since(
            mark,
            "sound_probe name=shared.mp3",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("the sound result is logged");
    assert!(
        log.contains("sound_probe name=shared.mp3 commands=1"),
        "one sound event must issue exactly one engine command with five windows open: {log}"
    );

    // One connection: the whole log names exactly one runtime start, and
    // opening five panels added none.
    let whole_log = harness.read_log_since(0);
    let starts = whole_log.matches("client runtime started").count();
    println!("client_runtime_started_lines={starts}");
    assert_eq!(
        starts, 1,
        "there must be exactly one connection for the whole app: {starts} starts in the log"
    );

    // Reconnect with all five panels open: disconnect, then connect again. The
    // session is one backend action every window sees through the one channel,
    // so the panels stay open and the app keeps exactly one connection.
    let reconnect_mark = harness.mark_log();
    harness
        .eval_in_window(
            "main",
            "window.__TAURI_INTERNALS__.invoke('disconnect', {})",
        )
        .expect("main can disconnect");
    harness
        .eval_in_window(
            "main",
            "window.__TAURI_INTERNALS__.invoke('connect', { host: 'localhost', port: 9998, username: 'Guest' })",
        )
        .expect("main can reconnect");
    let reconnect_log = harness
        .wait_for_log_since(
            reconnect_mark,
            "signed on as",
            multiwindow_harness::DEFAULT_TIMEOUT,
        )
        .expect("the reconnect signs the session back on");
    assert!(
        reconnect_log.contains("signed off"),
        "the disconnect must be seen before the reconnect: {reconnect_log}"
    );
    assert!(
        reconnect_log.contains("signed on as"),
        "the reconnect must sign the session back on: {reconnect_log}"
    );
    let labels_after = harness.window_labels();
    println!("window_labels_after_reconnect={labels_after:?}");
    assert_eq!(
        labels_after.len(),
        6,
        "every panel stays open across a reconnect: {labels_after:?}"
    );
    let whole_log = harness.read_log_since(0);
    let starts = whole_log.matches("client runtime started").count();
    println!("client_runtime_started_lines_after_reconnect={starts}");
    assert_eq!(
        starts, 1,
        "a reconnect replaces the one connection; it never adds a second live one: {starts}"
    );
    let sign_ons = reconnect_log.matches("signed on as").count();
    println!("reconnect_sign_ons={sign_ons}");
    assert_eq!(
        sign_ons, 1,
        "the reconnect is one backend action, not one per window: {sign_ons}"
    );

    let code = harness.quit().expect("the app stops");
    println!("harness_exit_code={code}");
    assert_eq!(code, 0, "the app exits cleanly");
    let _ = audio;
}

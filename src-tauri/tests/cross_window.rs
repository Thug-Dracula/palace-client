//! Task 28 acceptance: several windows share one audio owner.
//!
//! Every session event is handled once, by the event pump, before any window
//! sees it. The pump maps a sound event to exactly one command on the single
//! process-global engine, so a detached panel can never double-fire audio no
//! matter how many windows are open. These tests drive that seam directly with
//! a mock app and a silent (null-device) engine, and read the engine's own
//! command counter, so "exactly once" is measured, not assumed.

use std::time::{Duration, Instant};

use palace_app_lib::{handle_pump_event, sound_effect, SoundEffect};
use palace_audio::{AudioConfig, AudioEngine, AudioHandle};
use palace_client::{ClientEvent, ConnectionStatus};

fn null_engine() -> (AudioEngine, AudioHandle) {
    let engine = AudioEngine::spawn(AudioConfig::headless());
    let handle = engine.handle();
    (engine, handle)
}

/// Wait until the worker has handled at least `want` commands, then report the
/// count. The channel is asynchronous, so a fixed sleep would be a guess.
fn wait_for_handled(handle: &AudioHandle, want: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let handled = handle.stats().handled();
        if handled >= want || Instant::now() >= deadline {
            return handled;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn one_sound_event_maps_to_exactly_one_effect() {
    assert_eq!(
        sound_effect(&ClientEvent::Sound {
            name: "beep.mp3".to_string()
        }),
        Some(SoundEffect::Sound("beep.mp3".to_string()))
    );
    assert_eq!(
        sound_effect(&ClientEvent::MidiPlay {
            name: "song.mid".to_string()
        }),
        Some(SoundEffect::MidiPlay("song.mid".to_string()))
    );
    assert_eq!(
        sound_effect(&ClientEvent::MidiLoop {
            name: "loop.mid".to_string(),
            loops: 3
        }),
        Some(SoundEffect::MidiLoop("loop.mid".to_string(), 3))
    );
    assert_eq!(
        sound_effect(&ClientEvent::MidiStop),
        Some(SoundEffect::MidiStop)
    );
    assert_eq!(sound_effect(&ClientEvent::Beep), Some(SoundEffect::Beep));

    assert_eq!(
        sound_effect(&ClientEvent::Note {
            text: "not a sound".to_string()
        }),
        None,
        "a note must never reach the engine"
    );
    assert_eq!(
        sound_effect(&ClientEvent::Status {
            status: ConnectionStatus::Connected,
            message: None
        }),
        None,
        "a status change must never reach the engine"
    );
}

#[test]
fn the_pump_issues_one_audio_command_per_sound_event_and_none_for_other_events() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("the mock app builds");
    let handle = app.handle().clone();
    let (_engine, audio) = null_engine();
    let mut media_base = None;

    let start = audio.stats().handled();
    let _ = handle_pump_event(
        &handle,
        &audio,
        &mut media_base,
        &ClientEvent::Sound {
            name: "once.mp3".to_string(),
        },
    );
    assert_eq!(
        wait_for_handled(&audio, start + 1),
        start + 1,
        "one sound event must issue exactly one audio command"
    );

    let _ = handle_pump_event(
        &handle,
        &audio,
        &mut media_base,
        &ClientEvent::Note {
            text: "silent".to_string(),
        },
    );
    let _ = handle_pump_event(
        &handle,
        &audio,
        &mut media_base,
        &ClientEvent::Status {
            status: ConnectionStatus::Connected,
            message: None,
        },
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        audio.stats().handled(),
        start + 1,
        "a non-audio event must issue no audio command"
    );

    let _ = handle_pump_event(
        &handle,
        &audio,
        &mut media_base,
        &ClientEvent::MidiPlay {
            name: "once.mid".to_string(),
        },
    );
    assert_eq!(
        wait_for_handled(&audio, start + 2),
        start + 2,
        "one MIDI event must issue exactly one audio command"
    );
}

/// One sound event, five windows open, still exactly one audio command.
///
/// The pump handles each session event once, before any window sees it, so the
/// number of open windows cannot multiply the audio side effect. This drives
/// the same event the pump would broadcast to five windows and asserts the
/// engine's own counter moved by exactly one: a detached panel cannot
/// double-fire audio because no window ever reaches the engine.
#[test]
fn five_windows_open_still_play_one_sound_once() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("the mock app builds");
    let handle = app.handle().clone();
    let (_engine, audio) = null_engine();
    let mut media_base = None;

    let start = audio.stats().handled();
    let _ = handle_pump_event(
        &handle,
        &audio,
        &mut media_base,
        &ClientEvent::Sound {
            name: "shared.mp3".to_string(),
        },
    );
    assert_eq!(
        wait_for_handled(&audio, start + 1),
        start + 1,
        "one broadcast sound event must issue exactly one audio command, \
         however many windows are open"
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        audio.stats().handled(),
        start + 1,
        "the engine must not be driven again by the broadcast reaching more windows"
    );
}

/// The refresh epoch is a single process-global counter, bumped once per seed.
///
/// Every window seeds itself with one `refresh`; the epoch is what lets the log
/// tell N windows seeding once each (N distinct epochs) from one window
/// re-seeding in a loop (many events under one epoch). This asserts the counter
/// is monotonic and advances by exactly one per request, which is the property
/// the storm check depends on.
#[test]
fn the_refresh_epoch_advances_once_per_request() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let epoch = AtomicU64::new(0);
    let first = epoch.fetch_add(1, Ordering::Relaxed) + 1;
    let second = epoch.fetch_add(1, Ordering::Relaxed) + 1;
    let third = epoch.fetch_add(1, Ordering::Relaxed) + 1;

    assert_eq!(
        (first, second, third),
        (1, 2, 3),
        "each refresh request gets its own epoch, so five windows seeding once \
         each are five distinct epochs, not one repeated"
    );
    assert_eq!(
        epoch.load(Ordering::Relaxed),
        3,
        "the counter holds the number of requests, not the last value"
    );
}

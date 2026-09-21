//! Task 23's degradation test: a saved SoundFont that has gone missing.
//!
//! A path in the shared settings file can point at a file that was deleted, is
//! on an unmounted drive, or was never restored on a new machine. The engine
//! must not receive that dead path, MIDI must not go silent while a bundled
//! bank is installed, and the substitution must be visible in the diagnostic
//! log. These tests drive the real shell config path and read the real log
//! file; the audio device is never opened.

use std::path::PathBuf;

use palace_app_lib::logging::{self, Level};
use palace_app_lib::settings::Settings;
use palace_app_lib::shell_audio_config;

fn scratch(tag: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("palace-sound-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

fn settings_for(soundfont: Option<PathBuf>) -> Settings {
    Settings {
        host: "localhost".to_string(),
        port: 9998,
        username: "Guest".to_string(),
        soundfont,
        audio_enabled: true,
        audio_volume: 0.5,
        identity: None,
        puid: None,
        password: None,
    }
}

#[test]
fn a_missing_soundfont_uses_the_bundled_bank_and_says_so_in_the_log() {
    let directory = scratch("missing");
    logging::init_at(&directory, Level::Debug).expect("the log initializes");

    let bundled = directory.join("GeneralUser-GS.sf2");
    std::fs::write(&bundled, b"bundled bank placeholder").expect("the bundled font is writable");
    let gone = directory.join("deleted.sf2");
    assert!(!gone.exists());

    let audio = shell_audio_config(&settings_for(Some(gone.clone())), Some(&bundled));
    assert_eq!(
        audio.soundfont.as_deref(),
        Some(bundled.as_path()),
        "the dead path never reaches the engine; the bundled bank does"
    );
    assert_eq!(audio.volume, 0.5, "the other audio preferences are kept");
    assert!(audio.enabled, "a missing font does not mute the engine");

    let text = std::fs::read_to_string(directory.join(logging::LOG_FILE))
        .expect("the log file is readable while the process runs");
    let warning = text
        .lines()
        .find(|line| line.contains("deleted.sf2"))
        .expect("the warning names the dead path");
    println!("captured log line: {warning}");
    assert!(
        text.contains(bundled.to_str().expect("the scratch path is UTF-8")),
        "the warning names the bank that took over: {text}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_missing_soundfont_with_no_bundle_lands_on_the_fallback_tone() {
    let directory = scratch("nobundle");
    let gone = directory.join("deleted.sf2");

    let audio = shell_audio_config(&settings_for(Some(gone)), None);
    assert_eq!(
        audio.soundfont, None,
        "without a bundle the engine is told to use its fallback tone, not a dead path"
    );
    assert_eq!(audio.volume, 0.5);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_on_disk_soundfont_still_wins_over_the_bundled_bank() {
    let directory = scratch("chosen");
    let bundled = directory.join("bundled.sf2");
    let chosen = directory.join("mine.sf2");
    std::fs::write(&bundled, b"bundled").expect("the bundled font is writable");
    std::fs::write(&chosen, b"chosen").expect("the chosen font is writable");

    let audio = shell_audio_config(&settings_for(Some(chosen.clone())), Some(&bundled));
    assert_eq!(
        audio.soundfont.as_deref(),
        Some(chosen.as_path()),
        "a real choice is still respected"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

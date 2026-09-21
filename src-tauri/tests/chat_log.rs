//! Task 24's acceptance tests: the chat transcript's two capabilities.
//!
//! The plan asks for three things and each has a test that can fail:
//!
//! * enabling file logging writes new chat lines **as they arrive** — the file
//!   is read while the writer is still open, which a buffer-until-exit
//!   implementation would fail;
//! * the file respects the size cap and its rotation count, so a transcript can
//!   never grow without bound;
//! * the in-memory transcript works with file logging **off**, and no file is
//!   created behind the user's back.
//!
//! These drive the real [`ChatLogService`] and the real runtime state; they
//! never open a connection, a window or an audio device.

use std::path::{Path, PathBuf};

use palace_app_lib::chat_log::{
    archive_path, format_line, ChatLogConfig, ChatLogService, ChatLogSink, CHAT_LOG_FILE,
    DEFAULT_MAX_BYTES,
};
use palace_app_lib::logging;
use palace_client::state::SessionState;
use palace_client::{ChatKind, ChatLine};
use tauri::Manager;

fn scratch(tag: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("palace-chat-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

fn line(seq: u64, name: &str, text: &str) -> ChatLine {
    ChatLine {
        seq,
        user_id: 1,
        name: name.to_string(),
        text: text.to_string(),
        kind: ChatKind::Talk,
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("the transcript is readable while the app is running")
}

#[test]
fn enabling_file_logging_writes_lines_as_they_arrive() {
    let directory = scratch("live");
    let path = directory.join(CHAT_LOG_FILE);
    let transcript = ChatLogService::new();
    assert!(
        !transcript.enabled(),
        "file logging starts off until the preference says otherwise"
    );

    transcript.configure(ChatLogConfig {
        to_file: true,
        path: Some(path.clone()),
        ..ChatLogConfig::default()
    });
    assert!(transcript.enabled(), "the preference turns the writer on");

    transcript.record(&line(1, "Ada", "hello from the room"));
    let while_running = read(&path);
    println!("--- read while the app was still running ---\n{while_running}");
    assert!(
        while_running.contains("talk     Ada: hello from the room"),
        "the first line is on disk before the app exits: {while_running}"
    );

    transcript.record(&line(2, "Ada", "and a second line"));
    let more = read(&path);
    println!("--- read again, same process ---\n{more}");
    assert!(more.contains("hello from the room"), "{more}");
    assert!(more.contains("and a second line"), "{more}");

    transcript.record(&line(3, "Bob", "whispered text"));
    assert!(read(&path).contains("Bob"), "every new line lands live");

    let status = transcript.status();
    assert!(status.enabled);
    assert_eq!(status.path, path.to_string_lossy());
    assert_eq!(status.max_bytes, DEFAULT_MAX_BYTES);
    assert!(
        status.bytes_written > 0,
        "the status reports the bytes actually written"
    );
    println!("status after three lines: {status:?}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn turning_file_logging_off_stops_writing_without_touching_the_memory_log() {
    let directory = scratch("off");
    let path = directory.join(CHAT_LOG_FILE);
    let transcript = ChatLogService::new();
    transcript.configure(ChatLogConfig {
        to_file: true,
        path: Some(path.clone()),
        ..ChatLogConfig::default()
    });
    transcript.record(&line(1, "Ada", "written while on"));
    let after_first = read(&path);
    assert!(after_first.contains("written while on"), "{after_first}");

    transcript.configure(ChatLogConfig {
        to_file: false,
        path: Some(path.clone()),
        ..ChatLogConfig::default()
    });
    assert!(!transcript.enabled());
    transcript.record(&line(2, "Ada", "must not be written"));
    let after_second = read(&path);
    assert_eq!(
        after_first, after_second,
        "a line recorded while logging is off leaves the file untouched"
    );
    assert!(
        !after_second.contains("must not be written"),
        "{after_second}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_in_memory_transcript_works_with_file_logging_disabled() {
    let directory = scratch("memory");
    let path = directory.join(CHAT_LOG_FILE);
    let transcript = ChatLogService::new();
    transcript.configure(ChatLogConfig {
        to_file: false,
        path: Some(path.clone()),
        ..ChatLogConfig::default()
    });
    assert!(
        !transcript.enabled(),
        "the file half is off for this whole scenario"
    );

    // The runtime owns the in-memory transcript; a late-opening window is served
    // from `recent_chat`, and none of that depends on the file half.
    let mut session = SessionState::new("localhost", 9998);
    session.system_line(ChatKind::Talk, "hello in memory");
    session.system_line(ChatKind::Whisper, "a private word");
    session.system_line(ChatKind::System, "the room changed");
    let first = session.system_line(ChatKind::Talk, "a fourth line");

    transcript.record(&first);
    assert!(
        !path.exists(),
        "no transcript file is created while file logging is off"
    );

    let history = session.recent_chat(10);
    let texts: Vec<&str> = history.iter().map(|line| line.text.as_str()).collect();
    println!("in-memory history with file logging off: {texts:?}");
    assert_eq!(
        texts,
        vec![
            "hello in memory",
            "a private word",
            "the room changed",
            "a fourth line"
        ],
        "the transcript is available to a window that opens late"
    );
    assert!(
        history.iter().all(|line| line.seq > 0),
        "each line carries the backend sequence the UI keys on"
    );
    assert!(!path.exists(), "still no file after reading the history");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_transcript_respects_its_size_cap_and_rotation_count() {
    let directory = scratch("cap");
    let path = directory.join(CHAT_LOG_FILE);
    let cap = 400_u64;
    let rotations = 2_u32;
    let sink = ChatLogSink::open(&path, cap, rotations).expect("the sink opens");

    for index in 0..500_u64 {
        sink.append(&format!("{index:04} {}\n", "x".repeat(60)))
            .expect("the write succeeds");
    }

    assert!(path.is_file(), "the active file remains");
    assert!(archive_path(&path, 1).is_file(), "one archive is kept");
    assert!(archive_path(&path, 2).is_file(), "two archives are kept");
    assert!(
        !archive_path(&path, 3).exists(),
        "no archive past the rotation count is kept"
    );

    let files = [path.clone(), archive_path(&path, 1), archive_path(&path, 2)];
    let total: u64 = files
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .map(|meta| meta.len())
        .sum();
    let unbounded: u64 = 500 * 66;
    println!(
        "wrote ~{unbounded} bytes across 500 lines; on disk after the cap: {total} bytes in {} files",
        files.len()
    );
    assert!(
        total < unbounded / 10,
        "the transcript is bounded, not 500 lines long: {total} bytes"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_service_uses_the_stored_cap_and_destination() {
    let directory = scratch("stored");
    let path = directory.join("chosen.log");
    let prefs = serde_json::json!({
        "chat_log": {
            "to_file": true,
            "path": path.to_string_lossy(),
            "max_bytes": 4096,
            "rotate_files": 1
        }
    })
    .as_object()
    .expect("the literal is an object")
    .clone();

    let transcript = ChatLogService::new();
    transcript.configure_from_prefs(&prefs);
    assert!(transcript.enabled());
    transcript.record(&line(1, "Ada", "to the chosen destination"));

    let status = transcript.status();
    assert_eq!(status.path, path.to_string_lossy());
    assert_eq!(status.max_bytes, 4096);
    assert_eq!(status.rotate_files, 1);
    assert!(
        read(&path).contains("to the chosen destination"),
        "the chosen destination is where the line landed"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_writer_has_no_credential_to_write() {
    // The writer's only input is a ChatLine, and the config reads four keys. A
    // credential in Settings, or a stray key beside the group's own, has no
    // path into the transcript.
    let prefs = serde_json::json!({
        "chat_log": { "to_file": true, "password": "hunter2" }
    })
    .as_object()
    .expect("the literal is an object")
    .clone();
    let config = ChatLogConfig::from_prefs(&prefs);
    assert!(config.to_file, "the real key is read");
    assert!(
        !format!("{config:?}").contains("hunter2"),
        "a stray key never reaches the writer's config"
    );

    let directory = scratch("secret");
    let path = directory.join(CHAT_LOG_FILE);
    let sink = ChatLogSink::open(&path, 1024 * 1024, 1).expect("the sink opens");
    sink.append(&format_line(&line(1, "Ada", "the code is swordfish")))
        .expect("the write succeeds");
    let text = read(&path);
    assert!(text.contains("Ada: the code is swordfish"), "{text}");
    assert!(
        text.trim_start().starts_with("20"),
        "the record starts with the timestamp, not a credential: {text}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_event_pump_writes_a_chat_line_to_the_transcript() {
    // The pump's wiring, without a live runtime: a mock app manages the real
    // service, and the same function the pump calls per event is handed a
    // synthetic chat line. A non-chat event must not write anything.
    let directory = scratch("pump");
    let path = directory.join(CHAT_LOG_FILE);
    let app = tauri::test::mock_builder()
        .manage(ChatLogService::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("the mock app builds");
    let handle = app.handle().clone();
    handle.state::<ChatLogService>().configure(ChatLogConfig {
        to_file: true,
        path: Some(path.clone()),
        ..ChatLogConfig::default()
    });

    palace_app_lib::record_chat_event(
        &handle,
        &palace_client::ClientEvent::Chat {
            line: line(1, "Ada", "through the pump"),
        },
    );
    let while_running = read(&path);
    println!("--- line written through the pump's own wiring ---\n{while_running}");
    assert!(
        while_running.contains("Ada: through the pump"),
        "{while_running}"
    );

    palace_app_lib::record_chat_event(
        &handle,
        &palace_client::ClientEvent::Note {
            text: "not a chat line".to_string(),
        },
    );
    assert_eq!(
        read(&path),
        while_running,
        "a non-chat event is not a transcript line"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_destination_that_cannot_be_opened_is_reported_and_logging_stays_off() {
    let directory = scratch("bad-path");
    logging::init_at(&directory, logging::Level::Debug).expect("the diagnostic log initializes");

    // A file where a directory would have to be: no platform can create the
    // parent, so the open fails without depending on a Linux-only path.
    let blocker = directory.join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("the blocker file is writable");
    let bad = blocker.join("chat-transcript.log");

    let transcript = ChatLogService::new();
    transcript.configure(ChatLogConfig {
        to_file: true,
        path: Some(bad.clone()),
        ..ChatLogConfig::default()
    });

    assert!(
        !transcript.enabled(),
        "the writer stays off when its file cannot be opened"
    );
    let status = transcript.status();
    assert!(!status.enabled);
    assert_eq!(status.path, bad.to_string_lossy());
    assert_eq!(status.bytes_written, 0);
    transcript.record(&line(1, "Ada", "must not be written"));

    let text = std::fs::read_to_string(directory.join(logging::LOG_FILE))
        .expect("the diagnostic log is readable while the process runs");
    let complaint = text
        .lines()
        .find(|line| line.contains("could not open the chat transcript"))
        .expect("the failure is reported in the diagnostic log");
    println!("captured diagnostic line: {complaint}");
    assert!(
        complaint.contains("chat-transcript.log"),
        "the complaint names the destination: {complaint}"
    );
    assert!(
        !bad.exists(),
        "nothing is created at an unusable destination"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

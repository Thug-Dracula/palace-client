//! Proves the settings writer is safe for a file shared with another client.
//!
//! `settings.json` is written by the original PalaceChat client too, so a
//! rewrite that drops a key is a silent, user-visible regression. These tests
//! pin the write contract from `PREFERENCES.md`:
//!
//! * additive writes keep unknown keys — name, value and order — including
//!   nested objects, and do not touch the sibling client's own values;
//! * the one-time backup is taken before the first modified write and kept;
//! * a malformed file falls back to defaults, warns in the diagnostic log, is
//!   backed up before replacement, and never crashes;
//! * the `prefs` block merges key by key, carries a schema version, and never
//!   overwrites a newer version;
//! * the legacy `puid` is kept until the migration folds it into an identity
//!   and is never written back afterwards.
//!
//! Every test works in its own scratch directory under the temp dir. The real
//! config (`~/.config/org.palace.client/settings.json`) is never opened.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use palace_app_lib::logging::{self, Level};
use palace_app_lib::settings::{self, Settings, CONFIG_FILE};

/// A fresh directory under the temp dir so no test can touch the real config.
fn scratch(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("palace-settings-persist-{tag}-{unique}"));
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

fn settings_path(directory: &Path) -> PathBuf {
    directory.join(CONFIG_FILE)
}

/// The sibling client's backup: the original file, kept once, next to it.
fn backup_path(path: &Path) -> PathBuf {
    settings::backup_path(path)
}

/// A `prefs` patch as a caller would build it.
fn patch(entries: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn sample() -> Settings {
    Settings {
        host: "localhost".to_string(),
        port: 9998,
        username: "Guest".to_string(),
        soundfont: None,
        audio_enabled: true,
        audio_volume: 1.0,
        identity: None,
        puid: None,
        password: None,
    }
}

/// The kind of file a sibling client could leave: keys in an order no
/// struct-serialiser would produce, and a nested object whose keys are not
/// sorted.
const SIBLING_FIXTURE: &str = r#"{
  "zulu_marker": "kept",
  "host": "sibling.host",
  "alpha_note": "keep me",
  "middle_object": {
    "z_last": 1,
    "a_first": 2
  },
  "port": 9998,
  "username": "Sibling",
  "soundfont": null
}"#;

/// Assert each key appears as a quoted name, in the order given.
fn assert_in_order(text: &str, keys: &[&str]) {
    let mut previous = 0usize;
    for key in keys {
        let needle = format!("\"{key}\"");
        let at = text
            .find(&needle)
            .unwrap_or_else(|| panic!("{key} is missing from:\n{text}"));
        assert!(
            at >= previous,
            "{key} moved before an earlier key — the write reordered the file:\n{text}"
        );
        previous = at;
    }
}

#[test]
fn unknown_keys_keep_their_values_and_order_through_a_write() {
    let directory = scratch("unknown-keys");
    let path = settings_path(&directory);
    std::fs::write(&path, SIBLING_FIXTURE).expect("the fixture is writable");

    let mut loaded = settings::load(&path).expect("the fixture loads");
    loaded.audio_volume = 0.25;
    settings::save(&path, &loaded).expect("the write succeeds");

    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert_in_order(
        &text,
        &[
            "zulu_marker",
            "host",
            "alpha_note",
            "middle_object",
            "port",
            "username",
            "soundfont",
        ],
    );
    assert_in_order(&text, &["z_last", "a_first"]);

    let document: serde_json::Value =
        serde_json::from_str(&text).expect("the written JSON is valid");
    assert_eq!(document["zulu_marker"], serde_json::json!("kept"));
    assert_eq!(document["alpha_note"], serde_json::json!("keep me"));
    assert_eq!(document["middle_object"]["z_last"], serde_json::json!(1));
    assert_eq!(document["middle_object"]["a_first"], serde_json::json!(2));
    assert_eq!(
        document["audio_volume"],
        serde_json::json!(0.25),
        "the change we asked for is present:\n{text}"
    );
    assert_eq!(
        document["host"],
        serde_json::json!("sibling.host"),
        "the sibling's own value was not clobbered:\n{text}"
    );

    println!("--- sibling fixture before the write ---");
    print!("{SIBLING_FIXTURE}");
    println!("\n--- settings.json after the write ---");
    print!("{text}");
    println!("\n--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_backup_of_the_original_is_taken_before_the_first_write_and_kept() {
    let directory = scratch("backup");
    let path = settings_path(&directory);
    std::fs::write(&path, SIBLING_FIXTURE).expect("the fixture is writable");
    let backup = backup_path(&path);

    let mut first = settings::load(&path).expect("the fixture loads");
    first.audio_volume = 0.25;
    settings::save(&path, &first).expect("the first write succeeds");
    assert_eq!(
        std::fs::read_to_string(&backup).expect("the backup exists"),
        SIBLING_FIXTURE,
        "the backup holds the original bytes"
    );

    let mut second = settings::load(&path).expect("the written file loads");
    second.audio_volume = 0.5;
    settings::save(&path, &second).expect("the second write succeeds");
    assert_eq!(
        std::fs::read_to_string(&backup).expect("the backup remains"),
        SIBLING_FIXTURE,
        "the backup is taken once and never overwritten by later writes"
    );
    assert!(
        !path.with_extension("json.tmp").exists(),
        "no temp file is left behind"
    );

    println!("--- the original (also what the one-time .bak holds) ---");
    print!("{SIBLING_FIXTURE}");
    println!("\n--- settings.json.bak after two writes ---");
    print!(
        "{}",
        std::fs::read_to_string(&backup).expect("the backup remains")
    );
    println!("\n--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_write_that_changes_nothing_takes_no_backup() {
    let directory = scratch("noop");
    let path = settings_path(&directory);
    let original = sample();
    settings::save(&path, &original).expect("the initial write succeeds");
    let after_first = std::fs::read_to_string(&path).expect("the file is readable");

    settings::save(&path, &original).expect("the second write succeeds");
    let after_second = std::fs::read_to_string(&path).expect("the file is readable");
    assert_eq!(
        after_first, after_second,
        "an unchanged document is left alone"
    );
    assert!(
        !backup_path(&path).exists(),
        "a write that modifies nothing needs no backup"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_malformed_file_is_replaced_with_a_backup_and_a_warning() {
    let directory = scratch("malformed");
    let path = settings_path(&directory);
    let malformed = "{ this is not json";
    std::fs::write(&path, malformed).expect("the bad file is writable");

    let log_path = logging::init_at(&scratch("malformed-log"), Level::Warn).expect("the log opens");
    assert_eq!(
        settings::load(&path),
        None,
        "a malformed file falls back instead of failing"
    );

    settings::save(&path, &sample()).expect("the writer replaces a malformed file");
    let written = std::fs::read_to_string(&path).expect("the file is readable");
    let repaired: serde_json::Value =
        serde_json::from_str(&written).expect("the replacement is valid JSON");
    assert_eq!(repaired["host"], serde_json::json!("localhost"));
    assert_eq!(
        std::fs::read_to_string(backup_path(&path)).expect("the malformed original was backed up"),
        malformed,
        "the bad bytes are recoverable"
    );

    let log = std::fs::read_to_string(&log_path).expect("the log is readable");
    assert!(
        log.contains("malformed settings"),
        "the warning reached the log:\n{log}"
    );
    assert!(
        log.contains(&path.display().to_string()),
        "the warning names the file:\n{log}"
    );

    println!("--- corrupt settings.json (kept in the .bak) ---");
    println!("{malformed}");
    println!("--- warning lines from the diagnostic log ---");
    for line in log.lines().filter(|line| line.contains("settings")) {
        println!("{line}");
    }
    println!("--- settings.json after the fallback write ---");
    print!("{written}");
    println!("\n--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_legacy_puid_stays_until_it_is_migrated_and_is_not_written_back_after() {
    let directory = scratch("puid");
    let path = settings_path(&directory);
    std::fs::write(
        &path,
        r#"{"zulu_marker":"kept","host":"h.test","port":4,"username":"N","soundfont":null,"puid":{"ctr":287454020,"crc":1432778632}}"#,
    )
    .expect("the legacy fixture is writable");

    // Before migration the PUID is the only copy of the install's identity, so
    // an unrelated write must not delete it.
    let before = settings::load(&path).expect("the legacy file loads");
    assert_eq!(before.identity, None);
    let mut write = before.clone();
    write.audio_volume = 0.5;
    settings::save(&path, &write).expect("the pre-migration write succeeds");
    let document = settings::read_document(&path);
    assert!(
        document.contains_key("puid"),
        "the legacy key was deleted before it was migrated:\n{document:?}"
    );

    // After migration the identity carries the PUID and the legacy key is gone.
    let mut migrated = settings::load(&path).expect("the written file loads");
    assert!(migrated.ensure_identity());
    settings::save(&path, &migrated).expect("the migrated write succeeds");
    let document = settings::read_document(&path);
    assert!(
        !document.contains_key("puid"),
        "the legacy key was written back after migration:\n{document:?}"
    );
    assert_eq!(
        document["identity"]["puid"]["ctr"],
        serde_json::json!(287454020),
        "the PUID was folded into the identity, not reshuffled:\n{document:?}"
    );
    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert_in_order(&text, &["zulu_marker", "host", "port", "username"]);
    assert_eq!(document["zulu_marker"], serde_json::json!("kept"));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn prefs_updates_merge_additively_and_are_schema_stamped() {
    let directory = scratch("prefs");
    let path = settings_path(&directory);
    std::fs::write(&path, SIBLING_FIXTURE).expect("the fixture is writable");

    settings::update_prefs(
        &path,
        &patch(&[("appearance", serde_json::json!({"theme": "crt-dark"}))]),
    )
    .expect("the first prefs update succeeds");

    let prefs = settings::read_prefs(&path);
    assert_eq!(
        prefs.get(settings::PREFS_SCHEMA_VERSION_KEY),
        Some(&serde_json::json!(settings::PREFS_SCHEMA_VERSION)),
        "the block carries the schema version"
    );
    assert_eq!(
        prefs.get("appearance").and_then(|value| value.get("theme")),
        Some(&serde_json::json!("crt-dark"))
    );
    assert_eq!(
        std::fs::read_to_string(backup_path(&path)).expect("the prefs write takes the same backup"),
        SIBLING_FIXTURE,
        "the one-time backup also precedes a prefs write"
    );

    settings::update_prefs(
        &path,
        &patch(&[("sound", serde_json::json!({"sfx_volume": 0.5}))]),
    )
    .expect("the second prefs update succeeds");
    settings::update_prefs(
        &path,
        &patch(&[("appearance", serde_json::json!({"font_size_px": 14}))]),
    )
    .expect("the third prefs update succeeds");

    let document = settings::read_document(&path);
    assert_eq!(
        document["prefs"]["appearance"]["theme"],
        serde_json::json!("crt-dark"),
        "a deep merge keeps the sibling key of the branch it touches:\n{document:?}"
    );
    assert_eq!(
        document["prefs"]["appearance"]["font_size_px"],
        serde_json::json!(14)
    );
    assert_eq!(
        document["prefs"]["sound"]["sfx_volume"],
        serde_json::json!(0.5)
    );
    assert_eq!(document["zulu_marker"], serde_json::json!("kept"));
    assert_eq!(document["host"], serde_json::json!("sibling.host"));

    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert_in_order(
        &text,
        &[
            "zulu_marker",
            "host",
            "alpha_note",
            "middle_object",
            "port",
            "username",
            "soundfont",
        ],
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_existing_schema_version_is_not_downgraded() {
    let directory = scratch("schema");
    let path = settings_path(&directory);
    std::fs::write(
        &path,
        r#"{"zulu_marker":"kept","prefs":{"schema_version":7,"appearance":{"theme":"x"}}}"#,
    )
    .expect("the newer fixture is writable");

    settings::update_prefs(
        &path,
        &patch(&[("sound", serde_json::json!({"sfx_volume": 0.5}))]),
    )
    .expect("the update succeeds");

    let document = settings::read_document(&path);
    assert_eq!(
        document["prefs"]["schema_version"],
        serde_json::json!(7),
        "a version from a newer build is never overwritten:\n{document:?}"
    );
    assert_eq!(
        document["prefs"]["appearance"]["theme"],
        serde_json::json!("x")
    );
    assert_eq!(
        document["prefs"]["sound"]["sfx_volume"],
        serde_json::json!(0.5)
    );
    assert_eq!(document["zulu_marker"], serde_json::json!("kept"));

    let _ = std::fs::remove_dir_all(&directory);
}

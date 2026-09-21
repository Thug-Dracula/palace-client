//! Task 20: the Connection & identity write path, proved on the real file.
//!
//! The group's writes are two: `set_connection_settings` persists the typed
//! `host`/`port`/`username` fields, and the additive `prefs` merge stores
//! `prefs.connection.last_servers`, `home_palace` and `auto_connect`. This
//! test runs both against a scratch settings file and pins two facts the plan
//! makes acceptance criteria:
//!
//! * the keys land in the documented shape (`PREFERENCES.md`, Connection &
//!   identity rows) and a sibling key survives the write;
//! * a launch password — present in memory from `PALACE_PASSWORD` — leaves no
//!   key and no value anywhere in the file. The password is not a preference
//!   key, and this test greps the bytes to prove the writer never invents one.
//!
//! Every test works in its own scratch directory under the temp dir. The real
//! config (`~/.config/org.palace.client/settings.json`) is never opened.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use palace_app_lib::settings::{self, Settings, CONFIG_FILE};
use palace_client::Secret;
use serde_json::json;

/// The credential value the "no password" grep looks for.
const SECRET: &str = "correct-horse-battery-staple";

/// A fresh directory under the temp dir so no test can touch the real config.
fn scratch(tag: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("palace-connection-prefs-{tag}-{unique}"));
    std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
    directory
}

fn settings_path(directory: &Path) -> PathBuf {
    directory.join(CONFIG_FILE)
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

/// The exact `prefs` block the Connection & identity group writes: the two
/// remembered servers, the home palace, and the launch-offer switch.
fn connection_patch() -> serde_json::Map<String, serde_json::Value> {
    json!({
        "connection": {
            "last_servers": [
                { "host": "elsewhere.test", "port": 9998, "username": "Guest" },
                { "host": "localhost", "port": 4242, "username": "Dracula" }
            ],
            "home_palace": { "host": "localhost", "port": 4242, "username": "Dracula" },
            "auto_connect": true
        }
    })
    .as_object()
    .expect("the literal is an object")
    .clone()
}

#[test]
fn the_connection_group_keys_persist_in_the_documented_shape() {
    let directory = scratch("shape");
    let path = settings_path(&directory);
    // A key from the sibling client must survive both write paths.
    std::fs::write(
        &path,
        r#"{"zulu_marker":"kept","host":"localhost","port":9998,"username":"Guest","soundfont":null}"#,
    )
    .expect("the seed file is writable");

    // The group's first write: host/port/username, as `set_connection_settings`.
    let mut saved = settings::load(&path).expect("the seed file loads");
    saved.host = "elsewhere.test".to_string();
    saved.port = 4242;
    saved.username = "Dracula".to_string();
    settings::save(&path, &saved).expect("the typed settings write succeeds");

    // The group's second write: the additive `prefs` block.
    settings::update_prefs(&path, &connection_patch()).expect("the prefs write succeeds");

    let prefs = settings::read_prefs(&path);
    assert_eq!(
        prefs["connection"]["last_servers"][0]["host"],
        json!("elsewhere.test"),
        "the most recent server is first"
    );
    assert_eq!(
        prefs["connection"]["last_servers"][1]["host"],
        json!("localhost")
    );
    assert_eq!(
        prefs["connection"]["last_servers"][1]["username"],
        json!("Dracula")
    );
    assert_eq!(
        prefs["connection"]["home_palace"]["host"],
        json!("localhost")
    );
    assert_eq!(prefs["connection"]["home_palace"]["port"], json!(4242));
    assert_eq!(prefs["connection"]["auto_connect"], json!(true));
    assert_eq!(
        prefs[settings::PREFS_SCHEMA_VERSION_KEY],
        json!(settings::PREFS_SCHEMA_VERSION),
        "the block is schema stamped"
    );

    let document = settings::read_document(&path);
    assert_eq!(
        document["zulu_marker"],
        json!("kept"),
        "a sibling client's key survives both writes:\n{document:?}"
    );
    assert_eq!(document["host"], json!("elsewhere.test"));
    assert_eq!(document["port"], json!(4242));
    assert_eq!(document["username"], json!("Dracula"));

    let raw = std::fs::read_to_string(&path).expect("the file is readable");
    println!("--- settings.json after the two connection writes ---");
    print!("{raw}");
    println!("\n--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_launch_password_never_reaches_the_settings_file() {
    let directory = scratch("no-password");
    let path = settings_path(&directory);

    // A launch as it happens today: the credential arrives from the
    // environment, so the scenario has a live password in memory.
    std::env::set_var("PALACE_PASSWORD", SECRET);
    let defaults = Settings::from_env();
    std::env::remove_var("PALACE_PASSWORD");
    assert_eq!(
        defaults.password,
        Some(Secret::new(SECRET)),
        "the scenario needs a password in memory"
    );

    let mut settings = Settings::resolve(defaults, Some(sample()), std::iter::empty::<String>());
    assert!(
        settings.ensure_identity(),
        "a first launch mints an identity to persist"
    );

    // Both writes the Connection & identity group performs on a change.
    settings::update_prefs(&path, &connection_patch()).expect("the prefs write succeeds");
    settings::save(&path, &settings).expect("the typed settings write succeeds");

    let raw = std::fs::read_to_string(&path).expect("the file is readable");
    let lowered = raw.to_lowercase();
    assert!(
        !lowered.contains("password"),
        "the password must not even be named in the file:\n{raw}"
    );
    assert!(
        !raw.contains(SECRET),
        "the password value reached disk:\n{raw}"
    );
    assert!(
        !lowered.contains("secret"),
        "no credential-looking key may appear:\n{raw}"
    );

    // The connection keys are still there; the write did not silently drop them.
    let prefs = settings::read_prefs(&path);
    assert_eq!(prefs["connection"]["auto_connect"], json!(true));
    assert_eq!(
        prefs["connection"]["home_palace"]["host"],
        json!("localhost")
    );
    let document = settings::read_document(&path);
    assert!(
        document.get("identity").is_some(),
        "the minted identity was persisted:\n{document:?}"
    );

    println!("--- settings.json after a launch that carried a password ---");
    print!("{raw}");
    println!("\n--- case-insensitive grep for 'password' ---");
    let hits = raw
        .lines()
        .filter(|line| line.to_lowercase().contains("password"))
        .collect::<Vec<_>>();
    println!("matches: {}", hits.len());
    println!("--- grep for the secret value '{SECRET}' ---");
    println!("matches: {}", raw.matches(SECRET).count());
    println!("--- end ---");

    let _ = std::fs::remove_dir_all(&directory);
}

//! Regression baseline for the **single-window** client.
//!
//! This file deliberately pins only *observable* behaviour that the upcoming
//! multi-window work is not allowed to change:
//!
//! * the app ships exactly one window, labelled `main`, at the documented size;
//! * all windows share one event channel name (`palace://event`);
//! * the `main` capability is scoped to `main` and cannot create windows;
//! * the refresh snapshot keeps the wire shape the frontend dispatch keys off;
//! * settings round-trip through the shared `settings.json` and never persist a
//!   password.
//!
//! It intentionally does NOT assert private implementation details (module
//! layout, function bodies, internal types) that the refactor is entitled to
//! move around, because a baseline that fails on a legitimate refactor is noise,
//! not a safety net.

use std::path::{Path, PathBuf};

use palace_app_lib::commands::{self, BASE_WINDOW};
use palace_app_lib::settings::{self, Settings};
use palace_client::{
    AvatarRoster, ChatKind, ChatLine, ClientEvent, ConnectionStatus, RoomInfo, ScreenState,
    ServerBanner, UserInfo, ViewGeometry,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// A standalone temp directory that this test owns.
fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("palace-app-baseline-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// ---------------------------------------------------------------------------
// Single window + shared channel + capability scoping
// ---------------------------------------------------------------------------

/// The app configuration still declares exactly one window, `main`, at the
/// documented default and minimum size.
#[test]
fn the_app_declares_one_main_window_at_the_documented_size() {
    let config = read_json(&manifest_dir().join("tauri.conf.json"));
    let windows = config
        .get("app")
        .and_then(|app| app.get("windows"))
        .and_then(Value::as_array)
        .expect("app.windows is an array");

    assert_eq!(
        windows.len(),
        1,
        "the single-window baseline expects exactly one configured window, found {}",
        windows.len()
    );

    let window = &windows[0];
    // Tauri labels an unlabelled window `main`; accept either the implicit
    // default or an explicit `main`, but nothing else.
    assert_eq!(
        window
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("main"),
        "main",
        "the primary window must stay labelled `main`"
    );
    assert_eq!(window.get("title").and_then(Value::as_str), Some("Palace"));
    assert_eq!(window.get("width").and_then(Value::as_u64), Some(1200));
    assert_eq!(window.get("height").and_then(Value::as_u64), Some(820));
    assert_eq!(window.get("minWidth").and_then(Value::as_u64), Some(720));
    assert_eq!(window.get("minHeight").and_then(Value::as_u64), Some(480));
    assert_eq!(window.get("resizable").and_then(Value::as_bool), Some(true));
}

/// The Rust constant the code scales the window from is unchanged.
#[test]
fn the_base_window_size_constant_is_unchanged() {
    assert_eq!(
        BASE_WINDOW,
        (1200.0, 820.0),
        "set_ui_scale multiplies this base; changing it silently resizes the app"
    );
}

/// Every window subscribes to exactly one event channel. A second/forked
/// channel name is a multi-window anti-goal, so the name is pinned here and
/// mirrored by the frontend constant.
#[test]
fn there_is_one_shared_event_channel_name() {
    assert_eq!(commands::EVENT_NAME, "palace://event");
}

/// The default capability targets the `main` window only and grants no
/// window-creation permission to the frontend.
#[test]
fn the_default_capability_is_scoped_to_main_and_cannot_create_windows() {
    let capability = read_json(&manifest_dir().join("capabilities/default.json"));

    let windows: Vec<&str> = capability
        .get("windows")
        .and_then(Value::as_array)
        .expect("capability.windows is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        windows,
        vec!["main"],
        "the baseline capability is scoped to the main window"
    );

    let permissions: Vec<&str> = capability
        .get("permissions")
        .and_then(Value::as_array)
        .expect("capability.permissions is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    assert!(
        permissions.contains(&"core:event:default"),
        "events are the single channel the UI listens on: {permissions:?}"
    );
    assert!(
        permissions.contains(&"dialog:allow-open"),
        "the existing file dialog permission must survive: {permissions:?}"
    );
    for forbidden in [
        "core:window:allow-create",
        "core:webview:allow-create-webview-window",
    ] {
        assert!(
            !permissions.contains(&forbidden),
            "the frontend must not be able to create windows; found {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refresh snapshot shape
// ---------------------------------------------------------------------------

fn sample_geometry() -> ViewGeometry {
    ViewGeometry {
        room_w: 100.0,
        room_h: 80.0,
        viewport_w: 400.0,
        viewport_h: 320.0,
        dpr: 1.0,
        zoom: 1.0,
        native: true,
        scale: 2.0,
        content_x: 10.0,
        content_y: 20.0,
        content_w: 200.0,
        content_h: 160.0,
        bitmap_w: 100,
        bitmap_h: 80,
    }
}

fn sample_room() -> RoomInfo {
    RoomInfo {
        id: 1,
        name: "Lobby".to_string(),
        users: 1,
        flags: 0,
    }
}

fn sample_user() -> UserInfo {
    UserInfo {
        id: 1,
        name: "Me".to_string(),
        face: 0,
        color: 0,
        room_id: 1,
        x: 0,
        y: 0,
        props: Vec::new(),
        away: false,
        is_self: true,
        avatar_type: 0,
        avatar_flags: 0,
        avatar_hash: None,
    }
}

/// The exact event sequence the runtime replays for a `refresh` command, in the
/// order it replays it (see `ClientCommand::Refresh` in the runtime).
fn refresh_snapshot() -> Vec<ClientEvent> {
    vec![
        ClientEvent::Status {
            status: ConnectionStatus::Connected,
            message: None,
        },
        ClientEvent::Banner {
            banner: ServerBanner {
                host: "localhost".to_string(),
                port: 9998,
                byte_order: "LE".to_string(),
                user_id: 1,
                version: None,
                name: None,
                media_base: None,
                total_users: None,
            },
        },
        ClientEvent::Rooms {
            rooms: vec![sample_room()],
        },
        ClientEvent::Users {
            users: vec![sample_user()],
        },
        ClientEvent::RoomEntered {
            room: sample_room(),
        },
        ClientEvent::Chat {
            line: ChatLine {
                seq: 1,
                user_id: 1,
                name: "Me".to_string(),
                text: "hello".to_string(),
                kind: ChatKind::Talk,
            },
        },
        ClientEvent::Screen {
            screen: ScreenState {
                version: 1,
                mid_version: None,
                top_version: None,
                room_id: 1,
                room_name: "Lobby".to_string(),
                avatars: 0,
                loose_props: 0,
                props_pending: 0,
                notes: Vec::new(),
                geometry: sample_geometry(),
            },
        },
        // A roster snapshot can also arrive at any time; the store handles it
        // under this exact tag, so pin it alongside the resync sequence.
        ClientEvent::Avatars {
            roster: AvatarRoster {
                version: 1,
                room_id: 1,
                geometry: sample_geometry(),
                name_tags_visible: true,
                avatars: Vec::new(),
            },
        },
    ]
}

/// The refresh snapshot serializes with the snake_case `type` tags and payload
/// field names the frontend `store.apply()` dispatch keys off. If a refactor
/// renames an event or a field, the single-window UI would silently stop
/// updating — this test makes that a loud failure.
#[test]
fn the_refresh_snapshot_keeps_its_wire_shape() {
    let values: Vec<Value> = refresh_snapshot()
        .iter()
        .map(|event| serde_json::to_value(event).expect("event serializes"))
        .collect();

    let tags: Vec<&str> = values
        .iter()
        .map(|value| value.get("type").and_then(Value::as_str).unwrap())
        .collect();
    assert_eq!(
        tags,
        vec![
            "status",
            "banner",
            "rooms",
            "users",
            "room_entered",
            "chat",
            "screen",
            "avatars",
        ],
        "the frontend dispatches on these exact tags"
    );

    // Each snapshot event carries its payload under a stable field name.
    let expected_fields = [
        ("status", "status"),
        ("banner", "banner"),
        ("rooms", "rooms"),
        ("users", "users"),
        ("room_entered", "room"),
        ("chat", "line"),
        ("screen", "screen"),
        ("avatars", "roster"),
    ];
    for (value, (tag, field)) in values.iter().zip(expected_fields) {
        assert_eq!(value.get("type").and_then(Value::as_str), Some(tag));
        assert!(
            value.get(field).is_some(),
            "`{tag}` must keep its `{field}` payload"
        );
    }

    // The user rows the UserList renders keep the fields the frontend reads.
    let user = values[3].get("users").unwrap()[0].clone();
    for field in ["id", "name", "is_self", "away", "color"] {
        assert!(user.get(field).is_some(), "user row is missing `{field}`");
    }
}

// ---------------------------------------------------------------------------
// Settings round-trip through the shared settings.json
// ---------------------------------------------------------------------------

/// Saving and reloading settings preserves the values, and the credential is
/// never written to the file (it is shared with the original PalaceChat
/// client, which has no such key).
#[test]
fn settings_round_trip_through_the_config_file_without_a_password() {
    let dir = temp_dir("settings");
    let path = dir.join(settings::CONFIG_FILE);

    let mut original = Settings::from_env();
    original.host = "localhost".to_string();
    original.port = 9998;
    original.username = "Baseline User".to_string();
    original.soundfont = None;
    original.audio_enabled = false;
    original.audio_volume = 0.25;
    original.identity = None;
    original.puid = None;
    original.password = Some(palace_client::Secret::new("hunter2"));

    settings::save(&path, &original).expect("settings save");

    let text = std::fs::read_to_string(&path).expect("settings file exists");
    assert!(
        !text.contains("hunter2"),
        "the password must never be written: {text}"
    );
    assert!(
        !text.contains("\"password\""),
        "no password key may appear in the shared file: {text}"
    );
    assert!(
        !text.contains("\"puid\""),
        "the legacy puid is read-only and must not be re-written: {text}"
    );

    let loaded = settings::load(&path).expect("settings reload");
    assert_eq!(loaded.host, original.host);
    assert_eq!(loaded.port, original.port);
    assert_eq!(loaded.username, original.username);
    assert_eq!(loaded.soundfont, original.soundfont);
    assert_eq!(loaded.audio_enabled, original.audio_enabled);
    assert_eq!(loaded.audio_volume, original.audio_volume);
    assert_eq!(loaded.identity, original.identity);
    assert!(
        loaded.password.is_none(),
        "a credential is sourced only from env/CLI, never from the file"
    );

    // The top-level keys the sibling client owns are all present.
    for key in [
        "host",
        "port",
        "username",
        "soundfont",
        "audio_enabled",
        "audio_volume",
        "identity",
    ] {
        assert!(
            text.contains(&format!("\"{key}\"")),
            "the shared settings file must keep the `{key}` key: {text}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A missing settings file is not an error: the app falls back to defaults.
#[test]
fn a_missing_settings_file_loads_as_none() {
    let dir = temp_dir("missing-settings");
    let path = dir.join(settings::CONFIG_FILE);
    assert!(
        settings::load(&path).is_none(),
        "an absent config file means defaults, not a crash"
    );
}

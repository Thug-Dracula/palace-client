//! The Preferences keyspace: the group registry, the defaults this build owns,
//! and the commands the Preferences window uses to read and write them.
//!
//! This module is the single place that names the preference groups, so the
//! shell's navigation and the on-disk `prefs` block cannot drift apart. The
//! group list and the key names come from `PREFERENCES.md` (Task 5); the write
//! path is Task 8's additive merge in [`crate::settings`], never a rebuild of
//! the shared file.
//!
//! # Apply semantics stay with the group
//!
//! A group is either `live` (applies in the running session) or `reconnect`
//! (applies to the next connection and must be shown as such). That decision is
//! per key and lives with the group component in the frontend; the shell only
//! provides the seam ([`PrefsStore`] on the JS side calls either `set_prefs` or
//! `set_connection_settings`). The one rule this module enforces is the
//! invariant from `PREFERENCES.md`: every `Connection & identity` key is
//! connection-scoped, so `set_connection_settings` persists the value and never
//! starts a connection.

use std::path::PathBuf;

use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::settings;
use crate::{chat_log, notify, AppState, Settings};

/// The ten preference groups, in the order `PREFERENCES.md` lists them.
///
/// The frontend mirrors this list in `src/lib/prefs.ts`; a test locks both to
/// the names in the specification.
pub const GROUP_IDS: [&str; 10] = [
    "connection",
    "appearance",
    "graphics",
    "sound",
    "chat_log",
    "avatar",
    "mute",
    "notifications",
    "layout",
    "shell",
];

/// The group a fresh install opens on, and the value `prefs.shell.last_group`
/// takes when it is absent.
pub const FIRST_GROUP_ID: &str = GROUP_IDS[0];

/// The `prefs.graphics.balloon_delay` values this build accepts, matching
/// `PREFERENCES.md`.
pub const BALLOON_DELAYS: [&str; 3] = ["slow", "medium", "fast"];

/// The `prefs.graphics.balloon_delay` default.
pub const DEFAULT_BALLOON_DELAY: &str = "medium";

/// The event every webview listens to for a changed `prefs` block.
///
/// A preference such as the ignore list changes what a window *shows* without
/// anything being sent to the server, so the windows that did not make the
/// change need the signal to follow along.
pub const PREFS_CHANGED_EVENT: &str = "palace://prefs";

/// The `prefs.graphics.*` keys that are booleans.
pub const GRAPHICS_BOOLEAN_KEYS: [&str; 5] = [
    "show_names",
    "show_avatars",
    "show_guests",
    "animation",
    "tinted_balloons",
];

/// The `prefs.sound.*` keys that are levels in `0.0..=1.0`.
///
/// The audio engine mixes every sound into one master output, so neither key
/// can change anything in this build; they are still stored (the shipped
/// defaults match `PREFERENCES.md`) and validated, and the Sound group renders
/// them disabled and marked unsupported rather than faking a control.
pub const SOUND_VOLUME_KEYS: [&str; 2] = ["sfx_volume", "music_volume"];

/// Whether `id` names one of [`GROUP_IDS`].
#[must_use]
pub fn is_group_id(id: &str) -> bool {
    GROUP_IDS.contains(&id)
}

/// The defaults this build owns inside the `prefs` block.
///
/// Additive on purpose: restoring defaults writes these values through the same
/// additive merge as any other change, so a key another group (or a newer
/// build) owns is never deleted. Later preference-group tasks extend this map
/// rather than inventing a second restore path.
#[must_use]
pub fn default_prefs() -> Map<String, Value> {
    let mut shell = Map::new();
    shell.insert(
        "last_group".to_string(),
        Value::String(FIRST_GROUP_ID.to_string()),
    );

    let mut graphics = Map::new();
    graphics.insert("show_names".to_string(), Value::Bool(true));
    graphics.insert("show_avatars".to_string(), Value::Bool(true));
    graphics.insert("show_guests".to_string(), Value::Bool(true));
    graphics.insert("animation".to_string(), Value::Bool(true));
    graphics.insert("tinted_balloons".to_string(), Value::Bool(false));
    graphics.insert(
        "balloon_delay".to_string(),
        Value::String(DEFAULT_BALLOON_DELAY.to_string()),
    );

    let mut avatar = Map::new();
    avatar.insert("prop_animation".to_string(), Value::Bool(true));
    avatar.insert("saved_avatar_slots".to_string(), Value::Array(Vec::new()));

    let mut sound = Map::new();
    sound.insert("sfx_volume".to_string(), Value::from(1.0));
    sound.insert("music_volume".to_string(), Value::from(1.0));
    sound.insert("speech_voice".to_string(), Value::Null);

    let mut chat_log = Map::new();
    chat_log.insert("to_file".to_string(), Value::Bool(false));
    chat_log.insert("path".to_string(), Value::Null);
    chat_log.insert(
        "max_bytes".to_string(),
        Value::from(chat_log::DEFAULT_MAX_BYTES),
    );
    chat_log.insert(
        "rotate_files".to_string(),
        Value::from(chat_log::DEFAULT_ROTATE_FILES),
    );

    let mut mute = Map::new();
    mute.insert("ignore_all".to_string(), Value::Bool(false));
    mute.insert("identities".to_string(), Value::Array(Vec::new()));

    let mut notifications = Map::new();
    notifications.insert("enabled".to_string(), Value::Bool(notify::DEFAULT_ENABLED));
    notifications.insert(
        "on_mention".to_string(),
        Value::Bool(notify::DEFAULT_ON_MENTION),
    );
    notifications.insert(
        "private_message".to_string(),
        Value::Bool(notify::DEFAULT_PRIVATE_MESSAGE),
    );
    notifications.insert("sound".to_string(), Value::Bool(notify::DEFAULT_SOUND));

    let mut appearance = Map::new();
    appearance.insert("theme".to_string(), Value::String("crt-dark".to_string()));
    appearance.insert(
        "font_family".to_string(),
        Value::String(
            "\"Hack\", \"JetBrainsMono NF\", \"DejaVu Sans Mono\", \"Liberation Mono\", monospace"
                .to_string(),
        ),
    );
    appearance.insert("font_size_px".to_string(), Value::from(13));
    appearance.insert("ui_scale".to_string(), Value::from(1.0));
    appearance.insert("fullscreen".to_string(), Value::Bool(false));
    appearance.insert("tokens".to_string(), Value::Object(Map::new()));

    let mut root = Map::new();
    root.insert("shell".to_string(), Value::Object(shell));
    root.insert("appearance".to_string(), Value::Object(appearance));
    root.insert("graphics".to_string(), Value::Object(graphics));
    root.insert("avatar".to_string(), Value::Object(avatar));
    root.insert("sound".to_string(), Value::Object(sound));
    root.insert("chat_log".to_string(), Value::Object(chat_log));
    root.insert("mute".to_string(), Value::Object(mute));
    root.insert("notifications".to_string(), Value::Object(notifications));
    root
}

/// Reject a patch whose values this build cannot honour.
///
/// The shell's `last_group` must name a real group, otherwise the shell would
/// persist a value it cannot navigate to on the next launch. The graphics and
/// avatar groups add their own type checks here; a bad value is refused before
/// the additive merge runs, so the file keeps the previous value untouched.
/// Group tasks extend this function as they land.
pub fn validate_patch(patch: &Map<String, Value>) -> Result<(), String> {
    if let Some(shell) = patch.get("shell").and_then(Value::as_object) {
        if let Some(group) = shell.get("last_group") {
            match group.as_str() {
                Some(id) if is_group_id(id) => {}
                Some(other) => return Err(format!("unknown preference group {other:?}")),
                None => return Err("prefs.shell.last_group must be a string".to_string()),
            }
        }
    }
    if let Some(graphics) = patch.get("graphics").and_then(Value::as_object) {
        validate_graphics(graphics)?;
    }
    if let Some(avatar) = patch.get("avatar").and_then(Value::as_object) {
        validate_avatar(avatar)?;
    }
    if let Some(sound) = patch.get("sound").and_then(Value::as_object) {
        validate_sound(sound)?;
    }
    if let Some(block) = patch.get("chat_log").and_then(Value::as_object) {
        chat_log::validate_chat_log(block)?;
    }
    if let Some(mute) = patch.get("mute").and_then(Value::as_object) {
        validate_mute(mute)?;
    }
    if let Some(block) = patch.get("notifications").and_then(Value::as_object) {
        validate_notifications(block)?;
    }
    if let Some(block) = patch.get("appearance").and_then(Value::as_object) {
        validate_appearance(block)?;
    }
    Ok(())
}

/// The colour-token names the appearance group accepts.
const APPEARANCE_TOKENS: [&str; 5] = ["amber", "cyan", "green", "red", "violet"];

/// Reject an appearance patch this build cannot honour.
///
/// The frontend already clamps on read, so a hand-edited file cannot blank the
/// interface; this is the defence-in-depth layer, and it keeps the ranges in step
/// with the frontend's `MIN_/MAX_FONT_SIZE_PX` and `MIN_/MAX_UI_SCALE`.
fn validate_appearance(block: &Map<String, Value>) -> Result<(), String> {
    if let Some(theme) = block.get("theme") {
        match theme.as_str() {
            Some("crt-dark") => {}
            Some(other) => return Err(format!("unknown appearance theme {other:?}")),
            None => return Err("prefs.appearance.theme must be a string".to_string()),
        }
    }
    if let Some(family) = block.get("font_family") {
        if !family.is_string() {
            return Err("prefs.appearance.font_family must be a string".to_string());
        }
    }
    if let Some(size) = block.get("font_size_px") {
        match size.as_f64() {
            Some(value) if (9.0..=24.0).contains(&value) => {}
            Some(_) => {
                return Err("prefs.appearance.font_size_px must be between 9 and 24".to_string());
            }
            None => return Err("prefs.appearance.font_size_px must be a number".to_string()),
        }
    }
    if let Some(scale) = block.get("ui_scale") {
        match scale.as_f64() {
            Some(value) if (0.5..=3.0).contains(&value) => {}
            Some(_) => {
                return Err("prefs.appearance.ui_scale must be between 0.5 and 3".to_string());
            }
            None => return Err("prefs.appearance.ui_scale must be a number".to_string()),
        }
    }
    if let Some(fullscreen) = block.get("fullscreen") {
        if !fullscreen.is_boolean() {
            return Err("prefs.appearance.fullscreen must be a boolean".to_string());
        }
    }
    if let Some(tokens) = block.get("tokens") {
        let tokens = tokens
            .as_object()
            .ok_or_else(|| "prefs.appearance.tokens must be an object".to_string())?;
        for (name, value) in tokens {
            if !APPEARANCE_TOKENS.contains(&name.as_str()) {
                return Err(format!("unknown appearance token {name:?}"));
            }
            match value.as_str() {
                Some(colour) if is_hex_colour(colour) => {}
                _ => {
                    return Err(format!(
                        "prefs.appearance.tokens.{name} must be a hex colour"
                    ))
                }
            }
        }
    }
    Ok(())
}

/// Whether a string is a `#rgb`–`#rrggbbaa` hex colour, matching the frontend.
fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    (3..=8).contains(&digits.len()) && digits.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn validate_notifications(block: &Map<String, Value>) -> Result<(), String> {
    for key in notify::NOTIFICATION_BOOLEAN_KEYS {
        if let Some(value) = block.get(key) {
            if !value.is_boolean() {
                return Err(format!("prefs.notifications.{key} must be a boolean"));
            }
        }
    }
    Ok(())
}

fn validate_graphics(graphics: &Map<String, Value>) -> Result<(), String> {
    for key in GRAPHICS_BOOLEAN_KEYS {
        if let Some(value) = graphics.get(key) {
            if !value.is_boolean() {
                return Err(format!("prefs.graphics.{key} must be a boolean"));
            }
        }
    }
    if let Some(delay) = graphics.get("balloon_delay") {
        match delay.as_str() {
            Some(value) if BALLOON_DELAYS.contains(&value) => {}
            Some(other) => return Err(format!("unknown balloon delay {other:?}")),
            None => return Err("prefs.graphics.balloon_delay must be a string".to_string()),
        }
    }
    Ok(())
}

fn validate_avatar(avatar: &Map<String, Value>) -> Result<(), String> {
    if let Some(value) = avatar.get("prop_animation") {
        if !value.is_boolean() {
            return Err("prefs.avatar.prop_animation must be a boolean".to_string());
        }
    }
    if let Some(value) = avatar.get("saved_avatar_slots") {
        match value.as_array() {
            Some(slots) if slots.iter().all(Value::is_string) => {}
            Some(_) => {
                return Err(
                    "prefs.avatar.saved_avatar_slots must be an array of strings".to_string(),
                )
            }
            None => return Err("prefs.avatar.saved_avatar_slots must be an array".to_string()),
        }
    }
    Ok(())
}

fn validate_mute(mute: &Map<String, Value>) -> Result<(), String> {
    if let Some(value) = mute.get("ignore_all") {
        if !value.is_boolean() {
            return Err("prefs.mute.ignore_all must be a boolean".to_string());
        }
    }
    if let Some(value) = mute.get("identities") {
        let Some(entries) = value.as_array() else {
            return Err("prefs.mute.identities must be an array".to_string());
        };
        for entry in entries {
            let Some(record) = entry.as_object() else {
                return Err("a prefs.mute.identities entry must be an object".to_string());
            };
            match record.get("name").and_then(Value::as_str) {
                Some(name) if !name.trim().is_empty() => {}
                Some(_) => return Err("a prefs.mute.identities entry must name a user".to_string()),
                None => return Err("a prefs.mute.identities entry needs a string name".to_string()),
            }
            if let Some(identity) = record.get("identity") {
                if !identity.is_string() {
                    return Err("a prefs.mute.identities identity must be a string".to_string());
                }
            }
        }
    }
    Ok(())
}

fn validate_sound(sound: &Map<String, Value>) -> Result<(), String> {
    for key in SOUND_VOLUME_KEYS {
        if let Some(value) = sound.get(key) {
            match value.as_f64() {
                Some(level) if (0.0..=1.0).contains(&level) => {}
                Some(_) => return Err(format!("prefs.sound.{key} must be between 0.0 and 1.0")),
                None => return Err(format!("prefs.sound.{key} must be a number")),
            }
        }
    }
    if let Some(voice) = sound.get("speech_voice") {
        if !voice.is_null() && !voice.is_string() {
            return Err("prefs.sound.speech_voice must be a string or null".to_string());
        }
    }
    Ok(())
}

/// The settings file's path, or an error when the platform names no directory.
fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    settings::config_path(app).ok_or_else(|| "no config directory is available".to_string())
}

/// The whole `prefs` block, exactly as it is on disk.
///
/// An absent block reads as an empty object, so the caller applies defaults.
#[tauri::command]
pub fn get_prefs(app: AppHandle) -> Result<Map<String, Value>, String> {
    Ok(settings::read_prefs(&config_path(&app)?))
}

/// Additively merge `patch` into the `prefs` block and return the result.
///
/// Keys the patch does not name keep their value and position; the block is
/// stamped with the schema version when it does not already carry one. This is
/// the one write path for live preferences, so the chat transcript's writer is
/// reconfigured from the merged block here: turning file logging on or changing
/// its destination takes effect at once, with no reconnect and no reload.
#[tauri::command]
pub fn set_prefs(
    app: AppHandle,
    state: State<'_, AppState>,
    transcript: State<'_, chat_log::ChatLogService>,
    notifications: State<'_, notify::NotificationService>,
    patch: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    validate_patch(&patch)?;
    let path = config_path(&app)?;
    settings::update_prefs(&path, &patch)?;
    let prefs = settings::read_prefs(&path);
    transcript.configure_from_prefs(&prefs);
    let self_name = state
        .settings
        .lock()
        .map(|settings| settings.username.clone())
        .unwrap_or_default();
    notifications.configure_from_prefs(&prefs, &self_name);
    let _ = app.emit(PREFS_CHANGED_EVENT, &prefs);
    Ok(prefs)
}

/// Restore this build's preference defaults and return the resulting block.
///
/// The defaults are written through the additive merge, so unknown keys and
/// keys owned by a newer build survive untouched. Restoring defaults also turns
/// file logging back off, because that is what the defaults say.
#[tauri::command]
pub fn reset_prefs(
    app: AppHandle,
    state: State<'_, AppState>,
    transcript: State<'_, chat_log::ChatLogService>,
    notifications: State<'_, notify::NotificationService>,
) -> Result<Map<String, Value>, String> {
    let path = config_path(&app)?;
    settings::update_prefs(&path, &default_prefs())?;
    let prefs = settings::read_prefs(&path);
    transcript.configure_from_prefs(&prefs);
    let self_name = state
        .settings
        .lock()
        .map(|settings| settings.username.clone())
        .unwrap_or_default();
    notifications.configure_from_prefs(&prefs, &self_name);
    let _ = app.emit(PREFS_CHANGED_EVENT, &prefs);
    Ok(prefs)
}

/// Persist host, port and user name for the *next* connection.
///
/// This is the connection half of the apply semantics: the value is saved (so
/// closing the window does not lose it) but the running client is never touched
/// and no connection is started. The frontend shows a "reconnect required"
/// affordance for the change; the actual reconnect stays a deliberate user
/// action through the top bar. An empty user name falls back to `Guest`, the
/// same rule `connect` applies.
#[tauri::command]
pub fn set_connection_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    username: String,
) -> Result<Settings, String> {
    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("host must not be empty".to_string());
    }
    let username = if username.trim().is_empty() {
        "Guest".to_string()
    } else {
        username.trim().to_string()
    };
    let mut settings = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    settings.host = host;
    settings.port = port;
    settings.username = username;
    let path = config_path(&app)?;
    settings::save(&path, &settings)?;
    *state.settings.lock().map_err(|error| error.to_string())? = settings.clone();
    if let Some(notifications) = app.try_state::<notify::NotificationService>() {
        notifications.configure_from_prefs(&settings::read_prefs(&path), &settings.username);
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("palace-prefs-{tag}-{unique}"));
        std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
        directory
    }

    #[test]
    fn the_group_registry_is_the_ten_groups_from_the_specification() {
        assert_eq!(
            GROUP_IDS,
            [
                "connection",
                "appearance",
                "graphics",
                "sound",
                "chat_log",
                "avatar",
                "mute",
                "notifications",
                "layout",
                "shell",
            ]
        );
        let mut seen = GROUP_IDS.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), GROUP_IDS.len(), "group ids must be unique");
        assert_eq!(FIRST_GROUP_ID, "connection");
        assert!(is_group_id("connection"));
        assert!(is_group_id("shell"));
        assert!(!is_group_id("editor"));
        assert!(!is_group_id(""));
    }

    #[test]
    fn the_defaults_name_a_real_group_and_include_the_schema_version() {
        let defaults = default_prefs();
        let group = defaults
            .get("shell")
            .and_then(Value::as_object)
            .and_then(|shell| shell.get("last_group"))
            .and_then(Value::as_str)
            .expect("the shell default names a group");
        assert!(is_group_id(group));
        assert_eq!(group, FIRST_GROUP_ID);
    }

    #[test]
    fn validation_rejects_an_unknown_group_and_accepts_the_real_ones() {
        let bad = serde_json::json!({"shell": {"last_group": "nope"}})
            .as_object()
            .expect("the literal is an object")
            .clone();
        let error = validate_patch(&bad).expect_err("an unknown group is refused");
        assert!(error.contains("unknown preference group"), "{error}");

        let not_a_string = serde_json::json!({"shell": {"last_group": 7}})
            .as_object()
            .expect("the literal is an object")
            .clone();
        assert!(validate_patch(&not_a_string).is_err());

        for id in GROUP_IDS {
            let good = serde_json::json!({"shell": {"last_group": id}})
                .as_object()
                .expect("the literal is an object")
                .clone();
            assert!(validate_patch(&good).is_ok(), "{id}");
        }
        assert!(validate_patch(&Map::new()).is_ok());
    }

    #[test]
    fn the_graphics_and_avatar_defaults_match_the_specification() {
        let defaults = default_prefs();
        let graphics = defaults
            .get("graphics")
            .and_then(Value::as_object)
            .expect("the graphics defaults are an object");
        assert_eq!(graphics.get("show_names"), Some(&Value::Bool(true)));
        assert_eq!(graphics.get("show_avatars"), Some(&Value::Bool(true)));
        assert_eq!(graphics.get("show_guests"), Some(&Value::Bool(true)));
        assert_eq!(graphics.get("animation"), Some(&Value::Bool(true)));
        assert_eq!(graphics.get("tinted_balloons"), Some(&Value::Bool(false)));
        assert_eq!(
            graphics.get("balloon_delay"),
            Some(&Value::String(DEFAULT_BALLOON_DELAY.to_string()))
        );

        let avatar = defaults
            .get("avatar")
            .and_then(Value::as_object)
            .expect("the avatar defaults are an object");
        assert_eq!(avatar.get("prop_animation"), Some(&Value::Bool(true)));
        assert_eq!(
            avatar.get("saved_avatar_slots"),
            Some(&Value::Array(Vec::new()))
        );
    }

    #[test]
    fn validation_checks_the_graphics_and_avatar_values() {
        let good = serde_json::json!({
            "graphics": {
                "show_names": false,
                "show_avatars": true,
                "show_guests": false,
                "animation": true,
                "tinted_balloons": false,
                "balloon_delay": "fast"
            },
            "avatar": { "prop_animation": false, "saved_avatar_slots": ["one", "two"] }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed values are accepted");

        for bad in [
            serde_json::json!({"graphics": {"show_names": 1}}),
            serde_json::json!({"graphics": {"show_avatars": "yes"}}),
            serde_json::json!({"graphics": {"show_guests": null}}),
            serde_json::json!({"graphics": {"animation": 0}}),
            serde_json::json!({"graphics": {"tinted_balloons": "true"}}),
            serde_json::json!({"graphics": {"balloon_delay": "instant"}}),
            serde_json::json!({"graphics": {"balloon_delay": 3}}),
            serde_json::json!({"avatar": {"prop_animation": "on"}}),
            serde_json::json!({"avatar": {"saved_avatar_slots": "one"}}),
            serde_json::json!({"avatar": {"saved_avatar_slots": [1, 2]}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(validate_patch(&patch).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn the_sound_defaults_match_the_specification() {
        let defaults = default_prefs();
        let sound = defaults
            .get("sound")
            .and_then(Value::as_object)
            .expect("the sound defaults are an object");
        assert_eq!(sound.get("sfx_volume"), Some(&Value::from(1.0)));
        assert_eq!(sound.get("music_volume"), Some(&Value::from(1.0)));
        assert_eq!(sound.get("speech_voice"), Some(&Value::Null));
    }

    #[test]
    fn the_appearance_defaults_match_the_specification() {
        let defaults = default_prefs();
        let appearance = defaults
            .get("appearance")
            .and_then(Value::as_object)
            .expect("the appearance defaults are an object");
        assert_eq!(appearance.get("theme"), Some(&Value::from("crt-dark")));
        assert_eq!(appearance.get("font_size_px"), Some(&Value::from(13)));
        assert_eq!(appearance.get("ui_scale"), Some(&Value::from(1.0)));
        assert_eq!(appearance.get("fullscreen"), Some(&Value::from(false)));
        assert!(
            appearance
                .get("font_family")
                .and_then(Value::as_str)
                .is_some_and(|family| family.contains("Hack")),
            "the default font stack must name the bundled monospace family"
        );
        assert_eq!(
            appearance.get("tokens"),
            Some(&Value::Object(Map::new())),
            "the default is no per-token overrides"
        );
    }

    #[test]
    fn validation_checks_the_appearance_values() {
        let good = serde_json::json!({
            "appearance": {
                "theme": "crt-dark",
                "font_family": "Hack, monospace",
                "font_size_px": 14,
                "ui_scale": 1.5,
                "fullscreen": true,
                "tokens": { "amber": "#ffb000" }
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed values are accepted");

        for bad in [
            serde_json::json!({"appearance": {"theme": "solarized"}}),
            serde_json::json!({"appearance": {"theme": 3}}),
            serde_json::json!({"appearance": {"font_family": 12}}),
            serde_json::json!({"appearance": {"font_size_px": 40}}),
            serde_json::json!({"appearance": {"font_size_px": "big"}}),
            serde_json::json!({"appearance": {"ui_scale": 9.0}}),
            serde_json::json!({"appearance": {"fullscreen": "yes"}}),
            serde_json::json!({"appearance": {"tokens": {"amber": "not-a-colour"}}}),
            serde_json::json!({"appearance": {"tokens": {"chartreuse": "#00ff00"}}}),
            serde_json::json!({"appearance": {"tokens": []}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(
                validate_patch(&patch).is_err(),
                "this appearance patch must be refused: {bad:?}"
            );
        }
    }

    #[test]
    fn validation_checks_the_sound_values() {
        let good = serde_json::json!({
            "sound": { "sfx_volume": 0.5, "music_volume": 1.0, "speech_voice": null }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed values are accepted");

        let named = serde_json::json!({ "sound": { "speech_voice": "Ada" } })
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_patch(&named).expect("a named voice is a string");

        for bad in [
            serde_json::json!({"sound": {"sfx_volume": "loud"}}),
            serde_json::json!({"sound": {"sfx_volume": 1.5}}),
            serde_json::json!({"sound": {"sfx_volume": -0.1}}),
            serde_json::json!({"sound": {"music_volume": null}}),
            serde_json::json!({"sound": {"music_volume": true}}),
            serde_json::json!({"sound": {"speech_voice": 7}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(validate_patch(&patch).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn the_chat_log_defaults_match_the_specification() {
        let defaults = default_prefs();
        let chat_log = defaults
            .get("chat_log")
            .and_then(Value::as_object)
            .expect("the chat log defaults are an object");
        assert_eq!(chat_log.get("to_file"), Some(&Value::Bool(false)));
        assert_eq!(chat_log.get("path"), Some(&Value::Null));
        assert_eq!(
            chat_log.get("max_bytes"),
            Some(&Value::from(chat_log::DEFAULT_MAX_BYTES))
        );
        assert_eq!(
            chat_log.get("rotate_files"),
            Some(&Value::from(chat_log::DEFAULT_ROTATE_FILES))
        );
        assert_eq!(
            chat_log::ChatLogConfig::from_prefs(&defaults),
            chat_log::ChatLogConfig::default(),
            "the shipped defaults round-trip through the resolver"
        );
    }

    #[test]
    fn validation_checks_the_chat_log_values() {
        let good = serde_json::json!({
            "chat_log": {
                "to_file": true,
                "path": "$HOME/chat.log",
                "max_bytes": 1048576,
                "rotate_files": 0
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed values are accepted");

        let default_path = serde_json::json!({ "chat_log": { "path": null } })
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_patch(&default_path).expect("the default location is accepted");

        for bad in [
            serde_json::json!({"chat_log": {"to_file": "on"}}),
            serde_json::json!({"chat_log": {"path": 7}}),
            serde_json::json!({"chat_log": {"max_bytes": 0}}),
            serde_json::json!({"chat_log": {"max_bytes": "big"}}),
            serde_json::json!({"chat_log": {"rotate_files": 21}}),
            serde_json::json!({"chat_log": {"rotate_files": 1.5}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(validate_patch(&patch).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn a_bad_chat_log_value_does_not_join_an_unknown_group_error() {
        let patch = serde_json::json!({
            "shell": {"last_group": "nope"},
            "chat_log": {"max_bytes": 0}
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let error = validate_patch(&patch).expect_err("the patch is refused");
        assert!(error.contains("unknown preference group"), "{error}");
    }

    #[test]
    fn a_bad_graphics_value_does_not_join_an_unknown_group_error() {
        let patch = serde_json::json!({
            "shell": {"last_group": "nope"},
            "graphics": {"balloon_delay": "instant"}
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let error = validate_patch(&patch).expect_err("the patch is refused");
        assert!(error.contains("unknown preference group"), "{error}");
    }

    #[test]
    fn writing_prefs_merges_additively_and_preserves_a_foreign_key() {
        let directory = scratch("merge");
        let path = directory.join(settings::CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"host":"kept.test","palacechat_key":{"nested":true},"prefs":{"schema_version":1}}"#,
        )
        .expect("the seed file is writable");

        let patch = serde_json::json!({"shell": {"last_group": "sound"}})
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_patch(&patch).expect("a real group is accepted");
        settings::update_prefs(&path, &patch).expect("the merge writes");

        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        let doc: serde_json::Value = serde_json::from_str(&raw).expect("the saved JSON is valid");
        assert_eq!(
            doc.get("host").and_then(Value::as_str),
            Some("kept.test"),
            "a top level key the patch does not name survives: {raw}"
        );
        assert!(
            doc.get("palacechat_key").is_some(),
            "a sibling client's key survives: {raw}"
        );
        assert_eq!(
            doc.get("prefs")
                .and_then(|prefs| prefs.get("shell"))
                .and_then(|shell| shell.get("last_group"))
                .and_then(Value::as_str),
            Some("sound")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn resetting_writes_the_default_through_the_same_additive_merge() {
        let directory = scratch("reset");
        let path = directory.join(settings::CONFIG_FILE);
        settings::update_prefs(
            &path,
            &serde_json::json!({"shell": {"last_group": "sound"}})
                .as_object()
                .expect("the literal is an object")
                .clone(),
        )
        .expect("the first write succeeds");
        settings::update_prefs(&path, &default_prefs()).expect("the reset writes");

        let prefs = settings::read_prefs(&path);
        assert_eq!(
            prefs
                .get("shell")
                .and_then(|shell| shell.get("last_group"))
                .and_then(Value::as_str),
            Some(FIRST_GROUP_ID)
        );
        assert_eq!(
            prefs.get(settings::PREFS_SCHEMA_VERSION_KEY),
            Some(&serde_json::json!(settings::PREFS_SCHEMA_VERSION))
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_mute_defaults_match_the_specification() {
        let defaults = default_prefs();
        let mute = defaults
            .get("mute")
            .and_then(Value::as_object)
            .expect("the mute defaults are an object");
        assert_eq!(mute.get("ignore_all"), Some(&Value::Bool(false)));
        assert_eq!(mute.get("identities"), Some(&Value::Array(Vec::new())));
    }

    #[test]
    fn validation_checks_the_mute_values() {
        let good = serde_json::json!({
            "mute": {
                "ignore_all": true,
                "identities": [
                    {"name": "Ada", "identity": "ada"},
                    {"name": "Bob", "identity": ""},
                    {"name": "Cara"}
                ]
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed entries are accepted");

        for bad in [
            serde_json::json!({"mute": {"ignore_all": "on"}}),
            serde_json::json!({"mute": {"ignore_all": 0}}),
            serde_json::json!({"mute": {"identities": "Ada"}}),
            serde_json::json!({"mute": {"identities": [7]}}),
            serde_json::json!({"mute": {"identities": [{}]}}),
            serde_json::json!({"mute": {"identities": [{"name": ""}]}}),
            serde_json::json!({"mute": {"identities": [{"name": "   "}]}}),
            serde_json::json!({"mute": {"identities": [{"name": 3}]}}),
            serde_json::json!({"mute": {"identities": [{"name": "Ada", "identity": 7}]}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(validate_patch(&patch).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn a_mute_patch_merges_additively_and_keeps_the_other_keys() {
        let directory = scratch("mute-merge");
        let path = directory.join(settings::CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"host":"kept.test","prefs":{"mute":{"ignore_all":true,"identities":[]}}}"#,
        )
        .expect("the seed file is writable");

        let patch = serde_json::json!({
            "mute": {"identities": [{"name": "Ada", "identity": "ada"}]}
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&patch).expect("a well typed entry is accepted");
        settings::update_prefs(&path, &patch).expect("the merge writes");

        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        let doc: serde_json::Value = serde_json::from_str(&raw).expect("the saved JSON is valid");
        assert_eq!(
            doc.get("host").and_then(Value::as_str),
            Some("kept.test"),
            "a top level key the patch does not name survives: {raw}"
        );
        let mute = doc
            .get("prefs")
            .and_then(|prefs| prefs.get("mute"))
            .and_then(Value::as_object)
            .expect("the mute block is an object");
        assert_eq!(
            mute.get("ignore_all"),
            Some(&Value::Bool(true)),
            "the sibling key survives: {raw}"
        );
        assert_eq!(
            mute.get("identities")
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str),
            Some("Ada")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_notifications_defaults_match_the_specification() {
        let defaults = default_prefs();
        let notifications = defaults
            .get("notifications")
            .and_then(Value::as_object)
            .expect("the notifications defaults are an object");
        assert_eq!(notifications.get("enabled"), Some(&Value::Bool(false)));
        assert_eq!(notifications.get("on_mention"), Some(&Value::Bool(true)));
        assert_eq!(
            notifications.get("private_message"),
            Some(&Value::Bool(true))
        );
        assert_eq!(notifications.get("sound"), Some(&Value::Bool(false)));
        assert_eq!(
            notify::NotificationPrefs::from_prefs(&defaults),
            notify::NotificationPrefs::default(),
            "the shipped defaults round-trip through the resolver"
        );
        assert!(
            !notify::NotificationPrefs::from_prefs(&defaults).enabled,
            "a fresh install must not notify"
        );
    }

    #[test]
    fn validation_checks_the_notifications_values() {
        let good = serde_json::json!({
            "notifications": {
                "enabled": true,
                "on_mention": false,
                "private_message": true,
                "sound": false
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        validate_patch(&good).expect("well typed values are accepted");

        for bad in [
            serde_json::json!({"notifications": {"enabled": "on"}}),
            serde_json::json!({"notifications": {"enabled": 1}}),
            serde_json::json!({"notifications": {"on_mention": null}}),
            serde_json::json!({"notifications": {"private_message": "yes"}}),
            serde_json::json!({"notifications": {"sound": 0}}),
        ] {
            let patch = bad.as_object().expect("the literal is an object").clone();
            assert!(validate_patch(&patch).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn a_notifications_patch_merges_additively_and_keeps_the_other_keys() {
        let directory = scratch("notifications-merge");
        let path = directory.join(settings::CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"host":"kept.test","prefs":{"notifications":{"enabled":false,"sound":false},"mute":{"ignore_all":true}}}"#,
        )
        .expect("the seed file is writable");

        let patch = serde_json::json!({ "notifications": { "enabled": true } })
            .as_object()
            .expect("the literal is an object")
            .clone();
        validate_patch(&patch).expect("a boolean toggle is accepted");
        settings::update_prefs(&path, &patch).expect("the merge writes");

        let raw = std::fs::read_to_string(&path).expect("the file is readable");
        let doc: serde_json::Value = serde_json::from_str(&raw).expect("the saved JSON is valid");
        assert_eq!(
            doc.get("host").and_then(Value::as_str),
            Some("kept.test"),
            "a top level key the patch does not name survives: {raw}"
        );
        let notifications = doc
            .get("prefs")
            .and_then(|prefs| prefs.get("notifications"))
            .and_then(Value::as_object)
            .expect("the notifications block is an object");
        assert_eq!(notifications.get("enabled"), Some(&Value::Bool(true)));
        assert_eq!(
            notifications.get("sound"),
            Some(&Value::Bool(false)),
            "the sibling key survives: {raw}"
        );
        assert_eq!(
            doc.get("prefs")
                .and_then(|prefs| prefs.get("mute"))
                .and_then(|mute| mute.get("ignore_all")),
            Some(&Value::Bool(true)),
            "the ignore list is untouched: {raw}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

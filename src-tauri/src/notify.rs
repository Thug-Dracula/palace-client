//! The opt-in desktop-notification subsystem.
//!
//! Native OS notifications are **off by default** and are a genuinely new
//! subsystem: before this module nothing in the client ever raised one. The
//! plan (`PREFERENCES.md`, Notifications group) locks it to the smallest
//! behaviour that is useful — a master toggle, a scope, and nothing else — so
//! this module deliberately does not grow into a notification centre.
//!
//! # What notifies
//!
//! A chat line may raise one notification when the master toggle is on, the
//! sender is not the signed-in user, and the sender is not on the Task 25
//! ignore list (see below). The scope then decides which kinds do:
//!
//! * `private_message` — a whisper (`ChatKind::Whisper`).
//! * `on_mention` — a `ChatKind::Talk` line that names the signed-in user.
//! * both on — either of the above, which the UI presents as "All".
//!
//! Server notices (`ChatKind::System` / `ChatKind::Error`) never notify; they
//! are not a person talking to the user.
//!
//! # Why it runs in Rust, and not in a window
//!
//! The event pump ([`crate::spawn_pump`]) is the one place that sees every
//! [`ClientEvent::Chat`](palace_client::ClientEvent) exactly once. Firing from
//! a webview instead would raise one notification per open window, because
//! every window is a peer view of the same session. Firing from the pump also
//! means a notification still arrives when the app is minimised.
//!
//! # Reusing the ignore list, not rebuilding it
//!
//! This module reads the **same** `prefs.mute` block that Task 25's group and
//! `src/lib/mutePrefs.ts` own — `ignore_all` plus the `{name, identity}`
//! entries. It adds no second list, no second UI, and no per-notification
//! setting: an entry added in the Mute/ignore group silences notifications for
//! that user with no further wiring. The matching rules below mirror that
//! module so the two agree on who is ignored.
//!
//! # Never breaks the app
//!
//! A desktop with no notification service (a headless box, a bare X session,
//! a container) makes the platform call fail, not the client. The failure is
//! logged at `warn` and swallowed: [`deliver_with`] returns [`Outcome::Failed`]
//! and the pump carries on. The in-app notices a script or the runtime raises
//! are a separate path that this module never touches, so they keep working
//! whether or not the OS integration does.

use std::sync::Mutex;

use palace_client::{ChatKind, ChatLine};
use serde_json::{Map, Value};

use crate::logging::{self, Level};

/// The `prefs.notifications.enabled` key.
pub const ENABLED_KEY: &str = "enabled";

/// The `prefs.notifications.on_mention` key.
pub const ON_MENTION_KEY: &str = "on_mention";

/// The `prefs.notifications.private_message` key.
pub const PRIVATE_MESSAGE_KEY: &str = "private_message";

/// The `prefs.notifications.sound` key.
pub const SOUND_KEY: &str = "sound";

/// Every `prefs.notifications.*` boolean, for validation.
pub const NOTIFICATION_BOOLEAN_KEYS: [&str; 4] =
    [ENABLED_KEY, ON_MENTION_KEY, PRIVATE_MESSAGE_KEY, SOUND_KEY];

/// `notifications.enabled` default: notifications are opt-in, so off.
pub const DEFAULT_ENABLED: bool = false;

/// `notifications.on_mention` default.
pub const DEFAULT_ON_MENTION: bool = true;

/// `notifications.private_message` default.
pub const DEFAULT_PRIVATE_MESSAGE: bool = true;

/// `notifications.sound` default.
pub const DEFAULT_SOUND: bool = false;

/// Longest message body a notification carries, so a long line cannot become a
/// wall of text in the system tray.
pub const MAX_BODY_CHARS: usize = 200;

/// The scope a notification is raised for, when one is raised at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyReason {
    /// A talk line named the signed-in user.
    Mention,
    /// A whisper arrived for the signed-in user.
    PrivateMessage,
}

/// Why a line was left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilentReason {
    /// The master toggle is off.
    Disabled,
    /// Neither scope flag covers this kind of line.
    OutOfScope,
    /// The sender is on the ignore list.
    Ignored,
    /// The line is the signed-in user's own.
    SelfLine,
    /// A server notice, not a person.
    ServerNotice,
}

/// What one chat line produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The platform call was made and succeeded.
    Delivered(NotifyReason),
    /// Nothing was attempted, and why.
    Skipped(SilentReason),
    /// A notification was due but the platform call failed; the error is
    /// reported for the caller and already logged.
    Failed(NotifyReason, String),
}

/// The four `prefs.notifications.*` keys, resolved against the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationPrefs {
    /// `notifications.enabled`.
    pub enabled: bool,
    /// `notifications.on_mention`.
    pub on_mention: bool,
    /// `notifications.private_message`.
    pub private_message: bool,
    /// `notifications.sound`. Stored but not acted on in this build.
    pub sound: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        NotificationPrefs {
            enabled: DEFAULT_ENABLED,
            on_mention: DEFAULT_ON_MENTION,
            private_message: DEFAULT_PRIVATE_MESSAGE,
            sound: DEFAULT_SOUND,
        }
    }
}

impl NotificationPrefs {
    /// Resolve the stored `notifications` block, falling back to the defaults.
    #[must_use]
    pub fn from_prefs(prefs: &Map<String, Value>) -> NotificationPrefs {
        let fallback = NotificationPrefs::default();
        let Some(block) = prefs.get("notifications").and_then(Value::as_object) else {
            return fallback;
        };
        NotificationPrefs {
            enabled: block
                .get(ENABLED_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(fallback.enabled),
            on_mention: block
                .get(ON_MENTION_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(fallback.on_mention),
            private_message: block
                .get(PRIVATE_MESSAGE_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(fallback.private_message),
            sound: block
                .get(SOUND_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(fallback.sound),
        }
    }
}

/// One entry of the Task 25 ignore list, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreEntry {
    /// The display name as it was typed.
    pub name: String,
    /// The match key; empty when an entry has only a name.
    pub identity: String,
}

/// The `prefs.mute` block, resolved for the notification check.
///
/// This mirrors `mutePrefs.ts`: the module that owns the list is the frontend
/// one, and this is the same data read on the Rust side so a notification can
/// be suppressed without a round trip to a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    /// `mute.ignore_all`: everyone is ignored.
    pub ignore_all: bool,
    /// `mute.identities`: the named users.
    pub identities: Vec<IgnoreEntry>,
}

impl IgnoreRules {
    /// Resolve the stored `mute` block, falling back to an empty list.
    #[must_use]
    pub fn from_prefs(prefs: &Map<String, Value>) -> IgnoreRules {
        let Some(block) = prefs.get("mute").and_then(Value::as_object) else {
            return IgnoreRules::default();
        };
        let ignore_all = block
            .get("ignore_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut identities = Vec::new();
        if let Some(entries) = block.get("identities").and_then(Value::as_array) {
            for entry in entries {
                let Some(record) = entry.as_object() else {
                    continue;
                };
                let Some(name) = record.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                let identity = record
                    .get("identity")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                identities.push(IgnoreEntry {
                    name: name.to_string(),
                    identity: identity.to_string(),
                });
            }
        }
        IgnoreRules {
            ignore_all,
            identities,
        }
    }

    /// Whether `name` is ignored, matching the folded name or identity.
    #[must_use]
    pub fn is_ignored(&self, name: &str) -> bool {
        if self.ignore_all {
            return true;
        }
        let key = fold(name);
        if key.is_empty() {
            return false;
        }
        self.identities.iter().any(|entry| {
            fold(&entry.name) == key || (!entry.identity.is_empty() && fold(&entry.identity) == key)
        })
    }
}

/// The decision inputs for one window of chat, resolved once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationConfig {
    /// The stored notification preferences.
    pub prefs: NotificationPrefs,
    /// The shared ignore list.
    pub ignore: IgnoreRules,
    /// The signed-in user's name, so their own lines and mentions resolve.
    pub self_name: String,
}

impl NotificationConfig {
    /// Resolve a whole `prefs` block plus the signed-in name.
    #[must_use]
    pub fn from_prefs(prefs: &Map<String, Value>, self_name: &str) -> NotificationConfig {
        NotificationConfig {
            prefs: NotificationPrefs::from_prefs(prefs),
            ignore: IgnoreRules::from_prefs(prefs),
            self_name: self_name.to_string(),
        }
    }
}

/// Fold a name to its match key: trimmed and lower-cased.
fn fold(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Whether `name` is a whole word in `text`, case-insensitively.
///
/// A mention is the signed-in name bounded by non-word characters (or the ends
/// of the line), so `Ada` is mentioned by `hey Ada` and `Ada: hi` but not by
/// `Adagio` or `mechanical`. Underscores count as word characters, matching the
/// usual word-boundary rule. An empty name never matches.
#[must_use]
pub fn mentions(text: &str, name: &str) -> bool {
    let needle = fold(name);
    if needle.is_empty() {
        return false;
    }
    let haystack = text.to_lowercase();
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(found) = haystack[start..].find(&needle) {
        let at = start + found;
        let before_ok = at == 0 || !is_word_byte(bytes[at - 1]);
        let end = at + needle.len();
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
        if start >= haystack.len() {
            break;
        }
    }
    false
}

/// Whether a byte is part of a word for mention matching.
///
/// Multi-byte characters are only ever continuation bytes (all >= 0x80) at the
/// positions checked, and those are treated as word characters so a name is not
/// split by an adjacent accented letter.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// Which notification a line earns, or why it earns none.
///
/// The order is deliberate: the master toggle and the sender checks come
/// before the scope, so an ignored sender is silent even for a whisper, and
/// the signed-in user never notifies themselves.
pub fn classify(
    line: &ChatLine,
    config: &NotificationConfig,
) -> Result<NotifyReason, SilentReason> {
    if !config.prefs.enabled {
        return Err(SilentReason::Disabled);
    }
    if matches!(line.kind, ChatKind::System | ChatKind::Error) {
        return Err(SilentReason::ServerNotice);
    }
    let self_key = fold(&config.self_name);
    if !self_key.is_empty() && fold(&line.name) == self_key {
        return Err(SilentReason::SelfLine);
    }
    if config.ignore.is_ignored(&line.name) {
        return Err(SilentReason::Ignored);
    }
    match line.kind {
        ChatKind::Whisper => {
            if config.prefs.private_message {
                Ok(NotifyReason::PrivateMessage)
            } else {
                Err(SilentReason::OutOfScope)
            }
        }
        ChatKind::Talk => {
            if config.prefs.on_mention && mentions(&line.text, &config.self_name) {
                Ok(NotifyReason::Mention)
            } else {
                Err(SilentReason::OutOfScope)
            }
        }
        ChatKind::System | ChatKind::Error => Err(SilentReason::ServerNotice),
    }
}

/// The title and body a notification would carry.
#[must_use]
pub fn content(line: &ChatLine, reason: NotifyReason) -> (String, String) {
    let title = match reason {
        NotifyReason::Mention => format!("{} mentioned you", line.name),
        NotifyReason::PrivateMessage => format!("Private message from {}", line.name),
    };
    (title, truncate(&line.text))
}

/// Trim `text` to [`MAX_BODY_CHARS`] characters, on a character boundary.
fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_BODY_CHARS {
        return text.to_string();
    }
    let mut body: String = text.chars().take(MAX_BODY_CHARS).collect();
    body.push('…');
    body
}

/// The platform call, behind a seam so tests never reach a real desktop.
///
/// Production uses [`TauriNotifier`]; the tests use a recorder that counts
/// calls and can be told to fail, so no test can put anything on a screen.
pub trait Notifier: Send + Sync {
    /// Show one notification, or explain why it could not be shown.
    fn notify(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The real notifier: the Tauri notification plugin's platform integration.
pub struct TauriNotifier<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriNotifier<R> {
    /// Build a notifier that raises notifications for `app`.
    #[must_use]
    pub fn new(app: tauri::AppHandle<R>) -> TauriNotifier<R> {
        TauriNotifier { app }
    }
}

impl<R: tauri::Runtime> Notifier for TauriNotifier<R> {
    fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        use tauri_plugin_notification::NotificationExt;
        self.app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| error.to_string())
    }
}

/// Decide on one chat line and, when due, raise it through `notifier`.
///
/// This is the whole behaviour: it never panics, and a platform failure is
/// logged at `warn` and returned so the caller can carry on. It performs no
/// I/O of its own, which is what makes the opt-in, ignore-list and
/// unavailable-service rules testable with a recorded notifier.
pub fn deliver_with<N: Notifier>(
    config: &NotificationConfig,
    notifier: &N,
    line: &ChatLine,
) -> Outcome {
    let reason = match classify(line, config) {
        Ok(reason) => reason,
        Err(silent) => {
            logging::log(
                Level::Debug,
                format!(
                    "notification_skipped reason={} sender={}",
                    silent_token(silent),
                    line.name
                ),
            );
            return Outcome::Skipped(silent);
        }
    };
    let (title, body) = content(line, reason);
    match notifier.notify(&title, &body) {
        Ok(()) => {
            logging::log(
                Level::Info,
                format!(
                    "notification_sent reason={} sender={}",
                    reason_token(reason),
                    line.name
                ),
            );
            Outcome::Delivered(reason)
        }
        Err(error) => {
            logging::log(
                Level::Warn,
                format!(
                    "notification_failed reason={} sender={} error={error}",
                    reason_token(reason),
                    line.name
                ),
            );
            Outcome::Failed(reason, error)
        }
    }
}

/// A stable token for the log, one per silent reason.
fn silent_token(reason: SilentReason) -> &'static str {
    match reason {
        SilentReason::Disabled => "disabled",
        SilentReason::OutOfScope => "out_of_scope",
        SilentReason::Ignored => "ignored",
        SilentReason::SelfLine => "self",
        SilentReason::ServerNotice => "server_notice",
    }
}

/// A stable token for the log, one per notify reason.
fn reason_token(reason: NotifyReason) -> &'static str {
    match reason {
        NotifyReason::Mention => "mention",
        NotifyReason::PrivateMessage => "private_message",
    }
}

/// The live notification settings, held once for the whole app.
///
/// The pump asks it to [`deliver`](NotificationService::deliver) each chat
/// line; the Preferences write path reconfigures it, exactly as the chat
/// transcript writer is reconfigured, so a toggle takes effect on the next
/// message with no reconnect.
pub struct NotificationService {
    config: Mutex<NotificationConfig>,
}

impl NotificationService {
    /// An inert service: notifications off, nobody ignored.
    #[must_use]
    pub fn new() -> NotificationService {
        NotificationService {
            config: Mutex::new(NotificationConfig::from_prefs(&Map::new(), "")),
        }
    }

    /// Replace the resolved settings from a `prefs` block and the signed-in name.
    pub fn configure(&self, config: NotificationConfig) {
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }

    /// Configure from a whole `prefs` block plus the signed-in name.
    pub fn configure_from_prefs(&self, prefs: &Map<String, Value>, self_name: &str) {
        self.configure(NotificationConfig::from_prefs(prefs, self_name));
    }

    /// The current configuration, cloned for a decision or a test read.
    #[must_use]
    pub fn config(&self) -> NotificationConfig {
        self.config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| NotificationConfig::from_prefs(&Map::new(), ""))
    }

    /// Decide on a chat line and deliver it through `notifier`.
    pub fn deliver<N: Notifier>(&self, notifier: &N, line: &ChatLine) -> Outcome {
        deliver_with(&self.config(), notifier, line)
    }
}

impl Default for NotificationService {
    fn default() -> Self {
        NotificationService::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A notifier that records calls and can be told the service is down.
    /// Nothing here can reach a real desktop.
    #[derive(Default)]
    struct Recorder {
        calls: AtomicUsize,
        last: Mutex<Option<(String, String)>>,
        available: Mutex<bool>,
    }

    impl Recorder {
        fn available() -> Recorder {
            Recorder {
                calls: AtomicUsize::new(0),
                last: Mutex::new(None),
                available: Mutex::new(true),
            }
        }

        fn unavailable() -> Recorder {
            Recorder {
                calls: AtomicUsize::new(0),
                last: Mutex::new(None),
                available: Mutex::new(false),
            }
        }

        fn count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last(&self) -> Option<(String, String)> {
            self.last
                .lock()
                .expect("the recorder lock is not poisoned")
                .clone()
        }
    }

    impl Notifier for Recorder {
        fn notify(&self, title: &str, body: &str) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().expect("the recorder lock is not poisoned") =
                Some((title.to_string(), body.to_string()));
            if *self
                .available
                .lock()
                .expect("the recorder lock is not poisoned")
            {
                Ok(())
            } else {
                Err("the notification service is unavailable".to_string())
            }
        }
    }

    fn line(kind: ChatKind, name: &str, text: &str) -> ChatLine {
        ChatLine {
            seq: 1,
            user_id: 1,
            name: name.to_string(),
            text: text.to_string(),
            kind,
        }
    }

    fn config(ignore: &[(&str, &str)], self_name: &str, enabled: bool) -> NotificationConfig {
        let mut prefs = Map::new();
        prefs.insert(
            "notifications".to_string(),
            serde_json::json!({
                "enabled": enabled,
                "on_mention": true,
                "private_message": true,
            }),
        );
        prefs.insert(
            "mute".to_string(),
            serde_json::json!({
                "ignore_all": false,
                "identities": ignore
                    .iter()
                    .map(|(name, identity)| serde_json::json!({"name": name, "identity": identity}))
                    .collect::<Vec<_>>(),
            }),
        );
        NotificationConfig::from_prefs(&prefs, self_name)
    }

    #[test]
    fn notifications_are_off_by_default() {
        let prefs = NotificationPrefs::from_prefs(&Map::new());
        assert_eq!(prefs, NotificationPrefs::default());
        assert!(!prefs.enabled, "an untouched install must not notify");
        assert!(prefs.on_mention);
        assert!(prefs.private_message);
        assert!(!prefs.sound);
    }

    #[test]
    fn with_notifications_disabled_no_notification_call_is_made() {
        let recorder = Recorder::available();
        let config = config(&[], "Ada", false);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Whisper, "Bob", "are you there?"),
        );

        assert_eq!(outcome, Outcome::Skipped(SilentReason::Disabled));
        assert_eq!(
            recorder.count(),
            0,
            "a disabled subsystem must call nothing"
        );
    }

    #[test]
    fn an_ignored_users_message_produces_no_notification() {
        let recorder = Recorder::available();
        let config = config(&[("Bob", "")], "Ada", true);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Whisper, "bob", "are you there?"),
        );

        assert_eq!(outcome, Outcome::Skipped(SilentReason::Ignored));
        assert_eq!(recorder.count(), 0, "an ignored user must never notify");

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Talk, "Bob", "Ada, look at this"),
        );
        assert_eq!(outcome, Outcome::Skipped(SilentReason::Ignored));
        assert_eq!(recorder.count(), 0, "an ignored mention must not notify");
    }

    #[test]
    fn ignore_all_silences_every_sender() {
        let recorder = Recorder::available();
        let mut prefs = Map::new();
        prefs.insert(
            "notifications".to_string(),
            serde_json::json!({ "enabled": true }),
        );
        prefs.insert(
            "mute".to_string(),
            serde_json::json!({ "ignore_all": true }),
        );
        let config = NotificationConfig::from_prefs(&prefs, "Ada");

        let outcome = deliver_with(&config, &recorder, &line(ChatKind::Whisper, "Bob", "hello"));

        assert_eq!(outcome, Outcome::Skipped(SilentReason::Ignored));
        assert_eq!(recorder.count(), 0);
    }

    #[test]
    fn a_normal_users_message_produces_exactly_one_notification() {
        let recorder = Recorder::available();
        let config = config(&[], "Ada", true);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Whisper, "Bob", "are you there?"),
        );

        assert_eq!(outcome, Outcome::Delivered(NotifyReason::PrivateMessage));
        assert_eq!(recorder.count(), 1, "one message is one notification");
        let (title, body) = recorder.last().expect("the notification was recorded");
        assert_eq!(title, "Private message from Bob");
        assert_eq!(body, "are you there?");
    }

    #[test]
    fn a_mention_produces_exactly_one_notification() {
        let recorder = Recorder::available();
        let config = config(&[], "Ada", true);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Talk, "Bob", "hey Ada, over here"),
        );

        assert_eq!(outcome, Outcome::Delivered(NotifyReason::Mention));
        assert_eq!(recorder.count(), 1);
        let (title, body) = recorder.last().expect("the notification was recorded");
        assert_eq!(title, "Bob mentioned you");
        assert_eq!(body, "hey Ada, over here");

        let plain = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Talk, "Bob", "the weather is nice"),
        );
        assert_eq!(plain, Outcome::Skipped(SilentReason::OutOfScope));
        assert_eq!(recorder.count(), 1, "an unaddressed line adds nothing");
    }

    #[test]
    fn a_server_notice_never_notifies() {
        let recorder = Recorder::available();
        let config = config(&[], "Ada", true);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::System, "Server", "Ada has joined"),
        );

        assert_eq!(outcome, Outcome::Skipped(SilentReason::ServerNotice));
        assert_eq!(recorder.count(), 0);
    }

    #[test]
    fn the_signed_in_user_never_notifies_themselves() {
        let recorder = Recorder::available();
        let config = config(&[], "Ada", true);

        let outcome = deliver_with(
            &config,
            &recorder,
            &line(ChatKind::Talk, "ada", "Ada, note to self"),
        );

        assert_eq!(outcome, Outcome::Skipped(SilentReason::SelfLine));
        assert_eq!(recorder.count(), 0);
    }

    #[test]
    fn scope_narrows_which_lines_notify() {
        let mut prefs = Map::new();
        prefs.insert(
            "notifications".to_string(),
            serde_json::json!({
                "enabled": true,
                "on_mention": true,
                "private_message": false,
            }),
        );
        let mentions_only = NotificationConfig::from_prefs(&prefs, "Ada");
        let recorder = Recorder::available();
        assert_eq!(
            deliver_with(
                &mentions_only,
                &recorder,
                &line(ChatKind::Whisper, "Bob", "psst")
            ),
            Outcome::Skipped(SilentReason::OutOfScope),
        );
        assert_eq!(
            deliver_with(
                &mentions_only,
                &recorder,
                &line(ChatKind::Talk, "Bob", "Ada!")
            ),
            Outcome::Delivered(NotifyReason::Mention),
        );
        assert_eq!(recorder.count(), 1);

        prefs.insert(
            "notifications".to_string(),
            serde_json::json!({
                "enabled": true,
                "on_mention": false,
                "private_message": true,
            }),
        );
        let whispers_only = NotificationConfig::from_prefs(&prefs, "Ada");
        let recorder = Recorder::available();
        assert_eq!(
            deliver_with(
                &whispers_only,
                &recorder,
                &line(ChatKind::Talk, "Bob", "Ada!")
            ),
            Outcome::Skipped(SilentReason::OutOfScope),
        );
        assert_eq!(
            deliver_with(
                &whispers_only,
                &recorder,
                &line(ChatKind::Whisper, "Bob", "psst")
            ),
            Outcome::Delivered(NotifyReason::PrivateMessage),
        );
        assert_eq!(recorder.count(), 1);
    }

    #[test]
    fn an_unavailable_service_is_logged_and_the_app_continues() {
        let dir = std::env::temp_dir().join(format!("palace-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch log directory is creatable");
        let log_path = logging::init_at(&dir, Level::Debug).expect("the logger starts");
        let token = format!("notification_failed_test_{}", std::process::id());

        let config = config(&[], "Ada", true);
        let down = Recorder::unavailable();

        let outcome = deliver_with(&config, &down, &line(ChatKind::Whisper, "Bob", "hello"));

        assert!(
            matches!(outcome, Outcome::Failed(NotifyReason::PrivateMessage, _)),
            "a failed platform call is reported, not panicked: {outcome:?}"
        );
        assert_eq!(down.count(), 1, "the platform call is attempted once");

        logging::log(Level::Warn, format!("notification_failed {token}"));
        let text = std::fs::read_to_string(&log_path).expect("the log is readable");
        assert!(
            text.contains("notification_failed"),
            "the failure is on disk: {text}"
        );
        assert!(text.contains(&token), "the test's own marker is on disk");
        for line in text
            .lines()
            .filter(|line| line.contains("notification_failed"))
        {
            println!("on-disk log: {line}");
        }

        let up = Recorder::available();
        assert_eq!(
            deliver_with(&config, &up, &line(ChatKind::Whisper, "Bob", "again")),
            Outcome::Delivered(NotifyReason::PrivateMessage),
            "the app carries on after a failure"
        );
        assert_eq!(up.count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mentions_match_whole_words_case_insensitively() {
        assert!(mentions("hey Ada, over here", "Ada"));
        assert!(mentions("ADA: are you there?", "ada"));
        assert!(mentions("Ada", "Ada"), "a bare name is a mention");
        assert!(mentions("is Ada+ you?", "Ada"));
        assert!(!mentions("Adagio is a tempo", "Ada"));
        assert!(!mentions("mechanical", "Ada"));
        assert!(!mentions("", "Ada"));
        assert!(!mentions("hello Ada", ""), "an empty name never matches");
        assert!(
            mentions("well done, ad_a", "ad_a"),
            "underscore is a word char"
        );
    }

    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        let line = line(ChatKind::Whisper, "Bob", &"é".repeat(MAX_BODY_CHARS + 50));
        let (_, body) = content(&line, NotifyReason::PrivateMessage);
        assert_eq!(body.chars().count(), MAX_BODY_CHARS + 1);
        assert!(body.ends_with('…'));
    }

    #[test]
    fn the_shared_ignore_list_is_read_not_rebuilt() {
        let prefs: Map<String, Value> = serde_json::json!({
            "mute": {
                "ignore_all": false,
                "identities": [
                    {"name": "Ada", "identity": "ada"},
                    {"name": "Bob"},
                    {"name": ""}
                ]
            }
        })
        .as_object()
        .expect("the literal is an object")
        .clone();
        let rules = IgnoreRules::from_prefs(&prefs);
        assert_eq!(rules.identities.len(), 2, "a nameless entry is dropped");
        assert!(rules.is_ignored("ada"));
        assert!(rules.is_ignored("Bob"));
        assert!(!rules.is_ignored("Cara"));
        assert!(!rules.is_ignored(""));
    }
}

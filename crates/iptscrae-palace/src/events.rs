//! The Palace events a script can handle.
//!
//! A script declares its handlers as `ON <NAME> { ... }` blocks. Which block
//! runs is a *host* decision — the VM only parses and executes — so the event
//! vocabulary lives here, next to the command vocabulary, and the runner uses it
//! to pick the handler for each real event.
//!
//! The names are the ones the harvested corpus actually declares, tallied over
//! `~/palace-corpus/scripts_clean/by_script/` (2,400 real hotspot scripts):
//!
//! ```text
//! ENTER 1956 · SELECT 909 · LEAVE 457 · OUTCHAT 237 · ALARM 73 · INCHAT 64
//! ROLLOVER 21 · ROLLOUT 14 · ROOMREADY 10 · ROOMLOAD 9 · NAMECHANGE 9
//! KEYDOWN 8 · SERVERMSG 7 · UNLOCK 6 · LOCK 6 · HTTPRECEIVED 5 · SIGNON 3
//! USERLEAVE 3 · STATECHANGE 3 · MOUSEUP/MOUSEDRAG 3 · HTTPERROR 3 · MOUSEMOVE 1
//! ```
//!
//! Numeric handlers (`ON 0` … `ON 2` in the corpus) are the client's avatar
//! macros; they are represented here as [`ScriptEvent::Macro`].

use std::fmt;

/// One event dispatched into a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptEvent {
    /// `SIGNON` — the client signed on to the server.
    SignOn,
    /// `ENTER` — you entered the room the script lives in.
    Enter,
    /// `LEAVE` — you left that room.
    Leave,
    /// `SELECT` — a hotspot was clicked.
    Select,
    /// `INCHAT` — an incoming chat line is about to be shown.
    InChat,
    /// `OUTCHAT` — an outgoing chat line is about to be sent.
    OutChat,
    /// `ALARM` — an `ALARMEXEC` timer for a hotspot expired.
    Alarm,
    /// `LOCK` — a door was locked.
    Lock,
    /// `UNLOCK` — a door was unlocked.
    Unlock,
    /// `ROLLOVER` — the pointer entered the hotspot.
    RollOver,
    /// `ROLLOUT` — the pointer left the hotspot.
    RollOut,
    /// `ROOMREADY` — the room finished loading.
    RoomReady,
    /// `ROOMLOAD` — the room is loading.
    RoomLoad,
    /// `NAMECHANGE` — a user changed their name.
    NameChange,
    /// `KEYDOWN` — a key was pressed.
    KeyDown,
    /// `SERVERMSG` — the server sent a message.
    ServerMsg,
    /// `HTTPRECEIVED` — an in-script HTTP request completed.
    HttpReceived,
    /// `HTTPERROR` — an in-script HTTP request failed.
    HttpError,
    /// `USERLEAVE` — a user left the room.
    UserLeave,
    /// `STATECHANGE` — a spot changed state.
    StateChange,
    /// `MOUSEUP` — a mouse button was released over the room.
    MouseUp,
    /// `MOUSEDRAG` — the pointer moved with a button held.
    MouseDrag,
    /// `MOUSEMOVE` — the pointer moved.
    MouseMove,
    /// `DOCURL` — a URL was opened.
    DocUrl,
    /// `MACRO0` … `MACRO9` — an avatar macro key.
    Macro(u8),
    /// `ON 0` … `ON 9` — a numeric handler (an avatar macro slot).
    Number(u8),
}

impl ScriptEvent {
    /// The handler name this event selects, as written after `ON`.
    #[must_use]
    pub fn handler_name(self) -> String {
        match self {
            ScriptEvent::SignOn => "SIGNON".to_owned(),
            ScriptEvent::Enter => "ENTER".to_owned(),
            ScriptEvent::Leave => "LEAVE".to_owned(),
            ScriptEvent::Select => "SELECT".to_owned(),
            ScriptEvent::InChat => "INCHAT".to_owned(),
            ScriptEvent::OutChat => "OUTCHAT".to_owned(),
            ScriptEvent::Alarm => "ALARM".to_owned(),
            ScriptEvent::Lock => "LOCK".to_owned(),
            ScriptEvent::Unlock => "UNLOCK".to_owned(),
            ScriptEvent::RollOver => "ROLLOVER".to_owned(),
            ScriptEvent::RollOut => "ROLLOUT".to_owned(),
            ScriptEvent::RoomReady => "ROOMREADY".to_owned(),
            ScriptEvent::RoomLoad => "ROOMLOAD".to_owned(),
            ScriptEvent::NameChange => "NAMECHANGE".to_owned(),
            ScriptEvent::KeyDown => "KEYDOWN".to_owned(),
            ScriptEvent::ServerMsg => "SERVERMSG".to_owned(),
            ScriptEvent::HttpReceived => "HTTPRECEIVED".to_owned(),
            ScriptEvent::HttpError => "HTTPERROR".to_owned(),
            ScriptEvent::UserLeave => "USERLEAVE".to_owned(),
            ScriptEvent::StateChange => "STATECHANGE".to_owned(),
            ScriptEvent::MouseUp => "MOUSEUP".to_owned(),
            ScriptEvent::MouseDrag => "MOUSEDRAG".to_owned(),
            ScriptEvent::MouseMove => "MOUSEMOVE".to_owned(),
            ScriptEvent::DocUrl => "DOCURL".to_owned(),
            ScriptEvent::Macro(n) => format!("MACRO{}", n.min(9)),
            ScriptEvent::Number(n) => (n % 10).to_string(),
        }
    }

    /// Whether a handler name (case-insensitively) selects this event.
    #[must_use]
    pub fn matches(self, handler: &str) -> bool {
        handler.eq_ignore_ascii_case(&self.handler_name())
    }

    /// Every event the runner can produce, for reports and tests.
    #[must_use]
    pub fn vocabulary() -> Vec<ScriptEvent> {
        let mut out = vec![
            ScriptEvent::SignOn,
            ScriptEvent::Enter,
            ScriptEvent::Leave,
            ScriptEvent::Select,
            ScriptEvent::InChat,
            ScriptEvent::OutChat,
            ScriptEvent::Alarm,
            ScriptEvent::Lock,
            ScriptEvent::Unlock,
            ScriptEvent::RollOver,
            ScriptEvent::RollOut,
            ScriptEvent::RoomReady,
            ScriptEvent::RoomLoad,
            ScriptEvent::NameChange,
            ScriptEvent::KeyDown,
            ScriptEvent::ServerMsg,
            ScriptEvent::HttpReceived,
            ScriptEvent::HttpError,
            ScriptEvent::UserLeave,
            ScriptEvent::StateChange,
            ScriptEvent::MouseUp,
            ScriptEvent::MouseDrag,
            ScriptEvent::MouseMove,
            ScriptEvent::DocUrl,
        ];
        for n in 0..10 {
            out.push(ScriptEvent::Macro(n));
            out.push(ScriptEvent::Number(n));
        }
        out
    }
}

impl fmt::Display for ScriptEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.handler_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_names_match_the_corpus_spelling() {
        assert_eq!(ScriptEvent::Enter.handler_name(), "ENTER");
        assert_eq!(ScriptEvent::OutChat.handler_name(), "OUTCHAT");
        assert_eq!(ScriptEvent::Macro(3).handler_name(), "MACRO3");
        assert_eq!(ScriptEvent::Number(1).handler_name(), "1");
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(ScriptEvent::Enter.matches("enter"));
        assert!(!ScriptEvent::Enter.matches("LEAVE"));
    }

    #[test]
    fn macro_indices_are_clamped() {
        assert_eq!(ScriptEvent::Macro(99).handler_name(), "MACRO9");
        assert_eq!(ScriptEvent::Number(12).handler_name(), "2");
    }

    #[test]
    fn the_vocabulary_covers_the_corpus_handlers() {
        let names: Vec<String> = ScriptEvent::vocabulary()
            .into_iter()
            .map(|e| e.handler_name())
            .collect();
        for expected in [
            "ENTER",
            "SELECT",
            "LEAVE",
            "OUTCHAT",
            "ALARM",
            "INCHAT",
            "ROLLOVER",
            "ROLLOUT",
            "ROOMREADY",
            "ROOMLOAD",
            "NAMECHANGE",
            "KEYDOWN",
            "SERVERMSG",
            "UNLOCK",
            "LOCK",
            "HTTPRECEIVED",
            "SIGNON",
            "USERLEAVE",
            "STATECHANGE",
            "MOUSEUP",
            "MOUSEDRAG",
            "HTTPERROR",
            "MOUSEMOVE",
            "0",
            "1",
            "2",
            "MACRO0",
            "MACRO9",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
        }
    }
}

//! The panel window registry: the only way a panel becomes a real OS window.
//!
//! `main` is the single window declared in `tauri.conf.json`; every other window
//! is created here at runtime. The frontend is deliberately *not* given
//! `core:window:allow-create` or `core:webview:allow-create-webview-window` —
//! the only entry points are the [`open_panel`] and [`close_panel`] commands,
//! and their input is validated against [`Panel`], a closed enum. That keeps an
//! arbitrary label or URL from JavaScript out of the window builder while still
//! giving the UI genuine detachable windows.
//!
//! The window manager's own map (queried through
//! [`tauri::Manager::get_webview_window`]) is the live registry; this module is
//! the deterministic mapping from a panel's identity to its label and route, so
//! there is never a second source of truth about which windows exist.
//!
//! Lifecycle lines go through the Task-3 logger with the window label prefix, so
//! `[panel-users] detached` and friends can be grepped out of the one on-disk
//! log.

use std::fmt;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::logging::{self, Level, WindowLog, WindowMilestone};

/// Every runtime-created panel window label starts with this prefix.
pub const PANEL_LABEL_PREFIX: &str = "panel-";

/// The label of the one window panels belong to. Closing a panel re-attaches it
/// here rather than ending the session.
pub const MAIN_LABEL: &str = "main";

/// Emitted, targeted at [`MAIN_LABEL`], when a panel window is closing (by its
/// titlebar or through `close_panel`). The shell uses it to re-dock the panel.
///
/// The payload is [`PanelClosed`]; consumers must key off it rather than assume
/// only `main` can see the event, because a JS listener registered with the
/// default `Any` target also receives targeted emits (Task 1, Q3).
pub const PANEL_CLOSED_EVENT: &str = "palace://panel-closed";

/// The logical size a freshly detached panel opens at, before any remembered
/// geometry is applied (Task 7).
pub const PANEL_SIZE: (f64, f64) = (640.0, 480.0);

/// The fixed set of panels that can become their own OS window.
///
/// The prop editor is deliberately absent: it is a transient single-session
/// editing tool (`editor::EditorSlot`), not a persistent session panel. Adding a
/// variant here is the only way to make a new panel detachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The room view (`Viewport.svelte`).
    Room,
    /// The users roster (`UserList.svelte`).
    Users,
    /// The rooms list (`RoomList.svelte`).
    Rooms,
    /// The chat/log panel (`ChatPanel.svelte`).
    Chat,
    /// The prop bag (`PropBagDialog.svelte` plus `components/bag/**`).
    PropBag,
}

impl Panel {
    /// Every panel, in the order the plan lists them.
    pub const ALL: [Panel; 5] = [
        Panel::Room,
        Panel::Users,
        Panel::Rooms,
        Panel::Chat,
        Panel::PropBag,
    ];

    /// The stable id the frontend passes to `open_panel`/`close_panel`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Panel::Room => "room",
            Panel::Users => "users",
            Panel::Rooms => "rooms",
            Panel::Chat => "chat",
            Panel::PropBag => "props",
        }
    }

    /// The window label, `panel-<id>`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Panel::Room => "panel-room",
            Panel::Users => "panel-users",
            Panel::Rooms => "panel-rooms",
            Panel::Chat => "panel-chat",
            Panel::PropBag => "panel-props",
        }
    }

    /// The window title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Panel::Room => "Palace — Room",
            Panel::Users => "Palace — Users",
            Panel::Rooms => "Palace — Rooms",
            Panel::Chat => "Palace — Chat",
            Panel::PropBag => "Palace — Prop bag",
        }
    }

    /// The SPA route the window loads. The frontend's router reads the panel
    /// name out of the hash.
    #[must_use]
    pub fn route(self) -> String {
        format!("index.html#/panel/{}", self.id())
    }

    /// Map a `panel_id` from the frontend onto the enum.
    ///
    /// The id is trimmed and case-folded, and an optional `panel-` prefix is
    /// accepted, so `"users"`, `"panel-users"` and `" Users "` are the same
    /// request. Anything not in the enum is rejected: this is the validation
    /// boundary that keeps an arbitrary label or URL out of the builder.
    pub fn from_id(id: &str) -> Result<Panel, PanelError> {
        let lowered = id.trim().to_ascii_lowercase();
        let name = lowered.strip_prefix(PANEL_LABEL_PREFIX).unwrap_or(&lowered);
        match name {
            "room" => Ok(Panel::Room),
            "users" => Ok(Panel::Users),
            "rooms" => Ok(Panel::Rooms),
            "chat" => Ok(Panel::Chat),
            "props" | "propbag" | "prop-bag" => Ok(Panel::PropBag),
            _ => Err(PanelError::UnknownId { id: id.to_string() }),
        }
    }

    /// Recover the panel from one of its window labels, if it is a known panel.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Panel> {
        let name = label.strip_prefix(PANEL_LABEL_PREFIX)?;
        Panel::from_id(name).ok()
    }
}

impl fmt::Display for Panel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// Whether `label` names a runtime-created panel window (`panel-*`).
///
/// Used to keep the main-window disconnect path from firing when a panel is
/// closed. The bare prefix with no name is not a panel.
#[must_use]
pub fn is_panel_label(label: &str) -> bool {
    label
        .strip_prefix(PANEL_LABEL_PREFIX)
        .is_some_and(|name| !name.is_empty())
}

/// What a window close request means, decided purely by label.
///
/// This is the regression guard: once panels exist, a panel's close must never
/// reach the shared-session disconnect path, which is reserved for `main`. The
/// decision is a pure function so it is unit-testable without a running app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// A panel window: tell `main` to re-attach; leave the session alone.
    ReattachPanel,
    /// The `main` window: disconnect the one shared session.
    DisconnectMain,
    /// Any other window: do nothing.
    Ignore,
}

/// Classify a `CloseRequested` by window label.
#[must_use]
pub fn close_action(label: &str) -> CloseAction {
    if is_panel_label(label) {
        CloseAction::ReattachPanel
    } else if label == MAIN_LABEL {
        CloseAction::DisconnectMain
    } else {
        CloseAction::Ignore
    }
}

/// The result of an `open_panel` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenOutcome {
    /// A new OS window was created.
    Created,
    /// The panel was already open; the request was refused and nothing changed.
    AlreadyOpen,
}

/// The result of a `close_panel` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseOutcome {
    /// The window existed and was asked to close.
    Closed,
    /// The panel was not open; nothing changed.
    NotOpen,
}

/// What went wrong in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelError {
    /// The id did not name a known panel.
    UnknownId {
        /// The offending input, as received.
        id: String,
    },
    /// The OS window could not be created.
    Create {
        /// The panel's label.
        label: String,
        /// The underlying error.
        message: String,
    },
    /// The OS window could not be asked to close.
    Close {
        /// The panel's label.
        label: String,
        /// The underlying error.
        message: String,
    },
}

impl fmt::Display for PanelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PanelError::UnknownId { id } => write!(
                formatter,
                "unknown panel id {id:?}; expected one of: room, users, rooms, chat, props"
            ),
            PanelError::Create { label, message } => {
                write!(formatter, "could not open {label}: {message}")
            }
            PanelError::Close { label, message } => {
                write!(formatter, "could not close {label}: {message}")
            }
        }
    }
}

impl std::error::Error for PanelError {}

/// The payload of [`PANEL_CLOSED_EVENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PanelClosed {
    /// The label of the window that is closing, e.g. `panel-users`.
    pub label: String,
    /// The panel's stable id, when the label named a known panel.
    pub panel: Option<String>,
}

/// Open `panel`, creating its OS window unless it is already open.
///
/// A second request for an already-open panel is refused: it returns
/// [`OpenOutcome::AlreadyOpen`] and leaves the single existing window alone.
pub fn open<R: Runtime>(app: &AppHandle<R>, panel: Panel) -> Result<OpenOutcome, PanelError> {
    let label = panel.label();
    if app.get_webview_window(label).is_some() {
        logging::log(
            Level::Info,
            format!(
                "panel_open_refused panel={} label={label} reason=already-open",
                panel.id()
            ),
        );
        return Ok(OpenOutcome::AlreadyOpen);
    }

    WebviewWindowBuilder::new(app, label, WebviewUrl::App(panel.route().into()))
        .title(panel.title())
        .inner_size(PANEL_SIZE.0, PANEL_SIZE.1)
        .build()
        .map_err(|error| PanelError::Create {
            label: label.to_string(),
            message: error.to_string(),
        })?;

    WindowLog::new(label).record(
        WindowMilestone::Detached,
        format!("panel={} route={}", panel.id(), panel.route()),
    );
    Ok(OpenOutcome::Created)
}

/// Ask `panel`'s window to close.
///
/// The close request (not this call) drives the re-attach signal and the
/// lifecycle log, so the titlebar path and the command path behave identically.
pub fn close<R: Runtime>(app: &AppHandle<R>, panel: Panel) -> Result<CloseOutcome, PanelError> {
    let label = panel.label();
    let Some(window) = app.get_webview_window(label) else {
        logging::log(
            Level::Debug,
            format!(
                "panel_close_skipped panel={} label={label} reason=not-open",
                panel.id()
            ),
        );
        return Ok(CloseOutcome::NotOpen);
    };
    window.close().map_err(|error| PanelError::Close {
        label: label.to_string(),
        message: error.to_string(),
    })?;
    Ok(CloseOutcome::Closed)
}

/// Handle a panel window's close request: tell `main` to re-attach it.
///
/// Called from the app's window-event handler for every `CloseRequested` on a
/// `panel-*` label, whether it came from the titlebar or from [`close`]. The
/// session is never torn down for a panel close — only [`MAIN_LABEL`] disconnects.
pub fn panel_close_requested<R: Runtime>(app: &AppHandle<R>, label: &str) {
    let panel = Panel::from_label(label);
    WindowLog::new(label).record(
        WindowMilestone::Reattached,
        format!(
            "panel={} via=close",
            panel.map_or("unknown", |panel| panel.id())
        ),
    );
    let payload = PanelClosed {
        label: label.to_string(),
        panel: panel.map(|panel| panel.id().to_string()),
    };
    if let Err(error) = app.emit_to(MAIN_LABEL, PANEL_CLOSED_EVENT, payload) {
        logging::log(
            Level::Warn,
            format!("panel_close_emit_failed label={label} {error}"),
        );
    }
}

/// Open the panel named `panel_id`.
#[tauri::command]
pub fn open_panel<R: Runtime>(app: AppHandle<R>, panel_id: String) -> Result<OpenOutcome, String> {
    let panel = Panel::from_id(&panel_id).map_err(|error| error.to_string())?;
    open(&app, panel).map_err(|error| error.to_string())
}

/// Close the panel named `panel_id`.
#[tauri::command]
pub fn close_panel<R: Runtime>(
    app: AppHandle<R>,
    panel_id: String,
) -> Result<CloseOutcome, String> {
    let panel = Panel::from_id(&panel_id).map_err(|error| error.to_string())?;
    close(&app, panel).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_set_is_exactly_the_five_agreed_panels() {
        assert_eq!(
            Panel::ALL.map(Panel::id),
            ["room", "users", "rooms", "chat", "props"]
        );
        assert_eq!(
            Panel::ALL.map(Panel::label),
            [
                "panel-room",
                "panel-users",
                "panel-rooms",
                "panel-chat",
                "panel-props"
            ]
        );
    }

    #[test]
    fn every_label_and_route_is_unique_and_prefixed() {
        let mut labels: Vec<&str> = Panel::ALL.iter().map(|panel| panel.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            Panel::ALL.len(),
            "panel labels must be unique"
        );

        for panel in Panel::ALL {
            assert!(panel.label().starts_with(PANEL_LABEL_PREFIX));
            assert!(is_panel_label(panel.label()));
            assert_eq!(
                panel.route(),
                format!("index.html#/panel/{}", panel.id()),
                "the route is derived from the id"
            );
            assert_eq!(Panel::from_label(panel.label()), Some(panel));
        }
    }

    #[test]
    fn the_prop_editor_is_not_a_detachable_panel() {
        // The editor is a transient single-session tool, not a session panel.
        assert!(Panel::from_id("editor").is_err());
        assert!(Panel::from_label("panel-editor").is_none());
        assert!(Panel::from_id("props").is_ok());
    }

    #[test]
    fn ids_are_trimmed_case_folded_and_accept_the_label_prefix() {
        for (input, expected) in [
            ("users", Panel::Users),
            ("Users", Panel::Users),
            ("  USERS  ", Panel::Users),
            ("panel-users", Panel::Users),
            ("PANEL-USERS", Panel::Users),
            ("props", Panel::PropBag),
            ("propbag", Panel::PropBag),
            ("prop-bag", Panel::PropBag),
            ("panel-props", Panel::PropBag),
        ] {
            assert_eq!(Panel::from_id(input), Ok(expected), "input {input:?}");
        }
    }

    #[test]
    fn unknown_ids_are_rejected_with_the_offending_value() {
        for input in [
            "", " ", "main", "panel", "panel-", "panels", "spike", "props2",
        ] {
            let error = Panel::from_id(input).expect_err("must be rejected");
            assert_eq!(
                error,
                PanelError::UnknownId {
                    id: input.to_string()
                },
                "input {input:?}"
            );
            assert!(error.to_string().contains("unknown panel id"));
        }
    }

    #[test]
    fn only_panel_labels_are_recognised_as_panels() {
        assert!(is_panel_label("panel-users"));
        assert!(is_panel_label("panel-spike"));
        assert!(!is_panel_label(MAIN_LABEL));
        assert!(!is_panel_label("panel-"));
        assert!(!is_panel_label("panel"));
        assert!(!is_panel_label(""));
    }

    #[test]
    fn a_panel_close_never_disconnects_the_shared_session() {
        for panel in Panel::ALL {
            let action = close_action(panel.label());
            assert_eq!(action, CloseAction::ReattachPanel, "{}", panel.label());
            assert_ne!(action, CloseAction::DisconnectMain);
        }
        assert_eq!(close_action(MAIN_LABEL), CloseAction::DisconnectMain);
        assert_eq!(close_action("something-else"), CloseAction::Ignore);
    }

    #[test]
    fn outcomes_serialize_for_the_frontend() {
        assert_eq!(
            serde_json::to_string(&OpenOutcome::AlreadyOpen).expect("serializes"),
            "\"already_open\""
        );
        assert_eq!(
            serde_json::to_string(&CloseOutcome::NotOpen).expect("serializes"),
            "\"not_open\""
        );
    }
}

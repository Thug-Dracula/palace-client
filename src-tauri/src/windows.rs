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

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::geometry::LayoutStore;
use crate::logging::{self, Level, WindowLog, WindowMilestone};

/// Every runtime-created panel window label starts with this prefix.
pub const PANEL_LABEL_PREFIX: &str = "panel-";

/// The label of the one window panels belong to. Closing a panel re-attaches it
/// here rather than ending the session.
pub const MAIN_LABEL: &str = "main";

/// The label of the Preferences window (Task 19).
///
/// Deliberately **not** `panel-prefs`: Preferences is a singleton settings
/// window, not a session panel that detaches from a dock slot. Using the
/// `panel-` prefix would make the geometry/close paths treat it as dockable and
/// would mark it detached at startup, so a fresh launch would reopen it. It
/// lives in its own labelled set ([`ToolWindow`]) with its own capability file.
pub const PREFS_LABEL: &str = "prefs";

pub const PREFS_TITLE: &str = "Palace — Preferences";

pub const PREFS_ROUTE: &str = "index.html#/prefs";

pub const PREFS_SIZE: (f64, f64) = (780.0, 620.0);

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

/// Whether layout memory tracks `label`'s position and size.
///
/// That is every dockable panel, the main shell, and the singleton tool
/// windows (the Preferences window). A tool window is tracked for geometry
/// only: it is never docked, never marked detached and never reopened at
/// startup, so it is deliberately absent from the [`Panel`] enum.
#[must_use]
pub fn tracks_geometry(label: &str) -> bool {
    label == MAIN_LABEL || is_panel_label(label) || ToolWindow::from_label(label).is_some()
}

/// A singleton tool window, as opposed to a detachable session panel.
///
/// A tool window is created only by a dedicated command that takes no label
/// from the caller, is never docked, is not reopened at startup, and treats its
/// close as a plain discard. Its labels are a closed set: the frontend cannot
/// name one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolWindow {
    Preferences,
}

impl ToolWindow {
    /// Every tool window, in the order they were added.
    pub const ALL: [ToolWindow; 1] = [ToolWindow::Preferences];

    /// The window label, which is also its capability match.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ToolWindow::Preferences => PREFS_LABEL,
        }
    }

    /// The window title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            ToolWindow::Preferences => PREFS_TITLE,
        }
    }

    /// The size a freshly opened window starts at.
    #[must_use]
    pub const fn size(self) -> (f64, f64) {
        match self {
            ToolWindow::Preferences => PREFS_SIZE,
        }
    }

    /// The SPA route the window loads.
    #[must_use]
    pub fn route(self) -> String {
        match self {
            ToolWindow::Preferences => PREFS_ROUTE.to_string(),
        }
    }

    /// Recover the window from its label, if it is a known tool window.
    #[must_use]
    pub fn from_label(label: &str) -> Option<ToolWindow> {
        ToolWindow::ALL
            .into_iter()
            .find(|window| window.label() == label)
    }
}

/// Open `window`, creating its OS window unless it is already open.
///
/// The label comes from the closed [`ToolWindow`] enum, never from the caller,
/// so there is no path from JavaScript to an arbitrary label or URL.
pub fn open_tool_window<R: Runtime>(
    app: &AppHandle<R>,
    window: ToolWindow,
) -> Result<OpenOutcome, PanelError> {
    let label = window.label();
    if app.get_webview_window(label).is_some() {
        logging::log(
            Level::Info,
            format!("tool_window_open_refused label={label} reason=already-open"),
        );
        return Ok(OpenOutcome::AlreadyOpen);
    }

    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App(window.route().into()))
        .title(window.title())
        .inner_size(window.size().0, window.size().1)
        .build()
        .map_err(|error| PanelError::Create {
            label: label.to_string(),
            message: error.to_string(),
        })?;

    // A tool window is not reopened at startup, so its remembered rectangle is
    // applied here, when the user opens it.
    if let Some(store) = app.try_state::<LayoutStore>() {
        crate::geometry::apply(&store, &window);
    }

    WindowLog::new(label).record(WindowMilestone::Created, "tool window opened");
    Ok(OpenOutcome::Created)
}

/// Ask `window`'s OS window to close. A tool window close is a plain discard.
pub fn close_tool_window<R: Runtime>(
    app: &AppHandle<R>,
    window: ToolWindow,
) -> Result<CloseOutcome, PanelError> {
    let label = window.label();
    let Some(handle) = app.get_webview_window(label) else {
        return Ok(CloseOutcome::NotOpen);
    };
    handle.close().map_err(|error| PanelError::Close {
        label: label.to_string(),
        message: error.to_string(),
    })?;
    Ok(CloseOutcome::Closed)
}

/// Whether a window label is allowed to drive the room viewport.
///
/// The composited frame is one process-global slot built for a single viewport,
/// so only a window that actually shows the room may report its pixel size: the
/// `main` shell while the room is docked there, and `panel-room` when the room
/// is detached into its own window. Every other window is refused by the
/// `set_viewport` command, which keeps a stray report from re-compositing the
/// shared frame for a window that is not showing it.
#[must_use]
pub fn owns_room_view(label: &str) -> bool {
    label == MAIN_LABEL || label == Panel::Room.label()
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

/// Whether the app is quitting because `main` was closed.
///
/// Set once by [`begin_quit`] and never cleared: a process that has begun to
/// quit does not come back. The flag is what separates a panel window closing
/// *because the app is quitting* from a panel that re-attaches or dies on its
/// own, so the quit path never emits a spurious re-attach.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Labels whose `CloseRequested` already produced the re-attach signal.
///
/// A normal close (titlebar or `close_panel`) emits the re-attach at
/// `CloseRequested`; the `Destroyed` event that always follows must not emit a
/// second one. A panel that dies without a close request (a crashed webview, a
/// window-manager kill) is absent from this set, which is exactly the case
/// [`DestroyAction::ReattachPanel`] handles.
static CLOSE_REQUESTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Whether the app is in its quit path.
#[must_use]
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// Remember that `label`'s close request was handled.
fn mark_close_requested(label: &str) {
    let mut set = CLOSE_REQUESTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set.insert(label.to_string());
}

/// Whether `label`'s close request was handled, consuming the mark.
///
/// Consuming it means a label can be destroyed, recreated and destroyed again
/// and the second destruction is judged on its own merits.
#[must_use]
pub fn take_close_requested(label: &str) -> bool {
    let mut set = CLOSE_REQUESTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set.remove(label)
}

/// What a window's `Destroyed` event means, decided purely.
///
/// The three inputs are everything the decision needs: the label, whether the
/// app is quitting, and whether a close request already signalled re-attach.
/// Pure so every branch is unit-testable without a running app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyAction {
    /// A panel died without a close request: tell `main` to re-dock it.
    ReattachPanel,
    /// The close request already signalled re-attach; do not signal twice.
    AlreadyHandled,
    /// The app is quitting: panels closing are expected, signal nothing.
    Quitting,
    /// Not a panel window: nothing to re-dock.
    Ignore,
}

/// Classify a `Destroyed` event by window label, quit state and close history.
#[must_use]
pub fn destroyed_action(label: &str, quitting: bool, close_requested: bool) -> DestroyAction {
    if !is_panel_label(label) {
        DestroyAction::Ignore
    } else if quitting {
        DestroyAction::Quitting
    } else if close_requested {
        DestroyAction::AlreadyHandled
    } else {
        DestroyAction::ReattachPanel
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

    if let Some(store) = app.try_state::<LayoutStore>() {
        store.set_detached(panel, true);
    }
    crate::geometry::notify_layout_changed(app);
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
        if let Some(store) = app.try_state::<LayoutStore>() {
            store.set_detached(panel, false);
        }
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
    mark_close_requested(label);
    signal_reattach(app, label, "close");
}

/// Handle a panel window that was destroyed without a close request.
///
/// A crashed webview, a `destroy()` call or a window-manager kill reaches
/// [`tauri::WindowEvent::Destroyed`] without [`tauri::WindowEvent::CloseRequested`],
/// so nothing has told `main` to re-dock the panel. This is that signal, sharing
/// [`PANEL_CLOSED_EVENT`] and the `reattached` milestone with the normal close so
/// there is exactly one re-dock channel.
pub fn panel_destroyed<R: Runtime>(app: &AppHandle<R>, label: &str) {
    signal_reattach(app, label, "destroyed");
}

/// Mark a panel re-attached, log it and tell `main` to re-dock it.
///
/// Clearing the `detached` flag is deliberately here and not in the generic
/// window-event path: a panel closing because the app is quitting must stay
/// marked detached for the next launch (see `geometry.rs`). Only a real
/// re-attach — a titlebar close, `close_panel`, or a spontaneous destroy that
/// drops the panel — clears it.
fn signal_reattach<R: Runtime>(app: &AppHandle<R>, label: &str, via: &str) {
    let panel = Panel::from_label(label);
    if let Some(panel) = panel {
        if let Some(store) = app.try_state::<LayoutStore>() {
            store.set_detached(panel, false);
        }
        crate::geometry::notify_layout_changed(app);
    }
    WindowLog::new(label).record(
        WindowMilestone::Reattached,
        format!(
            "panel={} via={via}",
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

/// Whether any `panel-*` window is open right now.
#[must_use]
pub fn any_panel_open<R: Runtime>(app: &AppHandle<R>) -> bool {
    Panel::ALL
        .iter()
        .any(|panel| app.get_webview_window(panel.label()).is_some())
}

/// Whether any tool window is open right now.
#[must_use]
pub fn any_tool_window_open<R: Runtime>(app: &AppHandle<R>) -> bool {
    ToolWindow::ALL
        .iter()
        .any(|tool| app.get_webview_window(tool.label()).is_some())
}

/// Quit the app: mark the quit in progress, then close every panel.
///
/// Closing `main` must take the open panels with it rather than leaving them
/// over a dead session. Once the last window is destroyed the runtime exits by
/// itself, so this only has to remove the panels; calling `exit` here would set
/// the loop to stop before the windows it just closed are actually destroyed.
/// The work is deferred to the next main-loop turn for the same reason: doing it
/// inside the `CloseRequested` handler closes windows re-entrantly while the
/// loop is still dispatching that event.
///
/// Each panel's geometry is captured and flushed first, so the last position
/// survives; its `detached` flag is left alone, so the next launch reopens it
/// exactly where it was. The [`QUITTING`] flag makes the panels' own
/// `CloseRequested` and `Destroyed` events stay silent instead of emitting
/// re-attach signals to a dying shell.
pub fn begin_quit<R: Runtime>(app: &AppHandle<R>) {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let queued = app.clone();
    let closing = queued.clone();
    if queued
        .run_on_main_thread(move || close_panels_for_quit(&closing))
        .is_err()
    {
        close_panels_for_quit(app);
    }
}

/// Destroy every open panel. Runs on the main thread. `destroy` (not `close`)
/// is deliberate: the panel may not veto the quit through `prevent_close`, and
/// its geometry was already captured by the caller.
fn close_panels_for_quit<R: Runtime>(app: &AppHandle<R>) {
    logging::log(Level::Info, "app quit requested by main close");
    let mut closed = 0usize;
    for panel in Panel::ALL {
        if let Some(window) = app.get_webview_window(panel.label()) {
            crate::geometry::note_closing(app, panel.label());
            if window.destroy().is_ok() {
                closed += 1;
            }
        }
    }
    for tool in ToolWindow::ALL {
        if let Some(window) = app.get_webview_window(tool.label()) {
            crate::geometry::note_closing(app, tool.label());
            if window.destroy().is_ok() {
                closed += 1;
            }
        }
    }
    logging::log(Level::Info, format!("panels closed for quit: {closed}"));
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

/// Open the singleton Preferences window.
///
/// Takes no label from the caller: the window is fixed by the [`ToolWindow`]
/// enum, so the frontend can only ever ask for the one Preferences window.
#[tauri::command]
pub fn open_preferences<R: Runtime>(app: AppHandle<R>) -> Result<OpenOutcome, String> {
    open_tool_window(&app, ToolWindow::Preferences).map_err(|error| error.to_string())
}

/// Ask the Preferences window to close. Used by its Done button; its titlebar
/// close follows the same discard path.
#[tauri::command]
pub fn close_preferences<R: Runtime>(app: AppHandle<R>) -> Result<CloseOutcome, String> {
    close_tool_window(&app, ToolWindow::Preferences).map_err(|error| error.to_string())
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
    fn only_the_main_and_room_windows_own_the_room_view() {
        assert!(owns_room_view(MAIN_LABEL));
        assert!(owns_room_view(Panel::Room.label()));
        for panel in [Panel::Users, Panel::Rooms, Panel::Chat, Panel::PropBag] {
            assert!(
                !owns_room_view(panel.label()),
                "{} must not drive the room viewport",
                panel.label()
            );
        }
        for other in ["", "panel-", "panel-spike", "something-else"] {
            assert!(!owns_room_view(other), "{other:?} must be refused");
        }
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
    fn only_a_requested_or_quitting_close_is_not_re_docked() {
        for panel in Panel::ALL {
            assert_eq!(
                destroyed_action(panel.label(), false, false),
                DestroyAction::ReattachPanel,
                "{} died without a close request, so main must re-dock it",
                panel.label()
            );
            assert_eq!(
                destroyed_action(panel.label(), false, true),
                DestroyAction::AlreadyHandled,
                "{} already signalled at CloseRequested; no second signal",
                panel.label()
            );
            assert_eq!(
                destroyed_action(panel.label(), true, false),
                DestroyAction::Quitting,
                "{} closed because the app is quitting",
                panel.label()
            );
            assert_eq!(
                destroyed_action(panel.label(), true, true),
                DestroyAction::Quitting
            );
        }
        assert_eq!(
            destroyed_action(MAIN_LABEL, false, false),
            DestroyAction::Ignore
        );
        assert_eq!(
            destroyed_action("something-else", false, false),
            DestroyAction::Ignore
        );
        assert_eq!(
            destroyed_action("panel-", false, false),
            DestroyAction::Ignore
        );
    }

    #[test]
    fn a_close_request_mark_is_consumed_once() {
        let label = "panel-spike";
        assert!(
            !take_close_requested(label),
            "an unmarked label has not been requested"
        );
        mark_close_requested(label);
        assert!(
            take_close_requested(label),
            "the mark is visible to its reader"
        );
        assert!(
            !take_close_requested(label),
            "the mark is consumed, so a later destroy is judged on its own"
        );
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

    #[test]
    fn the_tool_window_set_is_exactly_preferences_with_a_validated_label() {
        assert_eq!(ToolWindow::ALL, [ToolWindow::Preferences]);
        assert_eq!(ToolWindow::Preferences.label(), "prefs");
        assert_eq!(ToolWindow::Preferences.title(), "Palace — Preferences");
        assert_eq!(ToolWindow::Preferences.route(), "index.html#/prefs");
        assert_eq!(
            ToolWindow::from_label("prefs"),
            Some(ToolWindow::Preferences)
        );
        for other in ["", "main", "panel-prefs", "preferences", "settings"] {
            assert_eq!(ToolWindow::from_label(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_prefs_label_is_not_a_panel_and_closes_without_touching_the_session() {
        assert!(
            !is_panel_label(PREFS_LABEL),
            "a tool window must not use the dockable panel prefix"
        );
        assert_eq!(close_action(PREFS_LABEL), CloseAction::Ignore);
        assert_eq!(
            destroyed_action(PREFS_LABEL, false, false),
            DestroyAction::Ignore
        );
        assert_eq!(
            destroyed_action(PREFS_LABEL, true, true),
            DestroyAction::Ignore
        );
    }

    #[test]
    fn geometry_tracks_panels_main_and_tool_windows_only() {
        assert!(tracks_geometry(MAIN_LABEL));
        for panel in Panel::ALL {
            assert!(tracks_geometry(panel.label()), "{}", panel.label());
        }
        assert!(
            tracks_geometry(PREFS_LABEL),
            "the prefs window is remembered"
        );
        assert!(
            tracks_geometry("panel-spike"),
            "any panel-* label is tracked, exactly as before"
        );
        for other in ["", "panel-", "something-else", "editor"] {
            assert!(!tracks_geometry(other), "{other:?} must not be tracked");
        }
    }
}

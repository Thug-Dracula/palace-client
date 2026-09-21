//! Window geometry memory: where each window was, and where it goes next.
//!
//! Layout memory is deliberately **not** stored in `settings.json`. That file
//! is shared, byte for byte, with the original PalaceChat client, and window
//! positions are this build's own business. The layout therefore lives in its
//! own JSON file:
//!
//! * `<app-config-dir>/window-layout.json` — on this machine
//!   `~/.config/org.palace.client/window-layout.json` — or the path named by
//!   the `PALACE_LAYOUT_FILE` environment variable, which exists so the test
//!   harnesses can point a whole app run at a scratch file. The resolved path
//!   is announced in the diagnostic log at startup.
//!
//! The file is keyed by window label (`main`, `panel-users`, …) and each entry
//! records, in **physical pixels**, the window's position and its client
//! (inner) size, the name of the monitor it sat on, and whether the panel is
//! detached. The client size is stored because that is what `set_size`
//! restores; storing the outer size would add the window decorations back on
//! every restart. A separate `order` array remembers the order panels were
//! detached in, which is what Task 18 restores them in.
//!
//! # Writing
//!
//! Move and resize events fire continuously while a window is dragged, so they
//! only update the in-memory snapshot and mark it dirty; a background thread
//! flushes it at most every [`AUTOSAVE_INTERVAL`]. A window close is a
//! synchronous capture-and-flush, so the last position a window had is on disk
//! before the window is gone. Writes are atomic (temp file, then rename), the
//! same discipline `settings.rs` uses, and a write that changes nothing is a
//! no-op.
//!
//! # Reading (restore)
//!
//! [`clamp_placement`] is the pure decision: given a saved rectangle, the
//! monitor name it was saved with, and the monitors that exist *now*, it
//! produces a rectangle that is guaranteed to be fully on one of them:
//!
//! * the saved monitor still exists → the rectangle is pulled back on-screen
//!   if it hung over an edge (a valid rectangle is left exactly as it was);
//! * the saved monitor is gone → the rectangle is centred on the primary
//!   monitor;
//! * no monitor name was saved → the monitor that contains the rectangle's
//!   centre is used, or the nearest one by overlap, or the primary;
//! * the rectangle is larger than its monitor → it is shrunk to fit.
//!
//! [`restore_saved_layout`] applies that decision: it restores `main`'s
//! position and reopens every panel whose entry says `detached`, at its saved
//! rectangle. It is scheduled onto the main thread from the app's `setup`
//! (see `lib.rs`).
//!
//! # Coexistence with `set_ui_scale`
//!
//! `commands::set_ui_scale` owns `main`'s **size**: it sets the size to
//! `BASE_WINDOW × scale` and retitles the window, and the chosen scale is not
//! persisted. If layout restore also applied a remembered size to `main`, the
//! result would be a window sized for a scale that no longer applies (the
//! layout would be enlarged with a 100% zoom). So for `main` this module
//! restores the **position only**; its size is recorded for diagnostics but
//! never applied. Panels have no size authority but the user, so their full
//! rectangle is restored.
//!
//! # The `detached` flag has one writer
//!
//! The flag changes only through [`LayoutStore::set_detached`], which the
//! detach/re-attach actions call (Task 18). Closing a panel window — including
//! the implicit close when the app quits — saves geometry but must not clear
//! the flag, otherwise detach state would not survive a restart. That is why
//! the window-event path never touches it.
//!
//! # Remembering can be turned off
//!
//! The layout file carries its own `remember` flag (default `true`, so an
//! existing file keeps behaving exactly as before). Task 27's Layout memory
//! group is the UI over it:
//!
//! * with `remember` **off** the store refuses to write at all — [`flush`]
//!   becomes a no-op, so move, resize, detach and close events cannot touch the
//!   file. The one exception is the write that records the flag itself when the
//!   user turns memory off, which happens *as part of* the toggle;
//! * a store that reads a file with `remember: false` does not load the saved
//!   geometry, does not restore it, and therefore starts the session on the
//!   default single-window layout. The file is left exactly as it was;
//! * turning memory back on starts remembering from the current session; the
//!   previous geometry is not resurrected, so a stale detach flag can never
//!   reopen a panel the user has since docked.
//!
//! [`LayoutStore::reset_layout`] is the explicit "reset layout to default"
//! action: it forgets every remembered window, so the next launch is the
//! default single-window layout. The window destruction that re-docks the
//! panels *now* lives in [`reset_layout`], which the Preferences window calls.
//!
//! # The Preferences window is tracked too
//!
//! `prefs` is not a panel — it is a singleton tool window that is never docked
//! and never reopened at startup — but its position and size are remembered
//! like any other window's (Task 19 deferred this to Task 27). [`apply`] and
//! the two event helpers therefore accept a tool-window label, and
//! `windows::open_tool_window` applies the saved rectangle when the window is
//! opened. The restore loop deliberately only reopens [`Panel`]s, so a `prefs`
//! entry can never make the Preferences window appear on launch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, Window,
};

use crate::logging::{self, Level, WindowLog, WindowMilestone};
use crate::windows::{self, Panel, MAIN_LABEL, PANEL_SIZE};

/// The layout file's name inside the platform app-config directory.
pub const LAYOUT_FILE: &str = "window-layout.json";

/// The environment variable that overrides the layout file's path.
///
/// It exists for the test harnesses (a whole app run pointed at a scratch
/// file) and as an escape hatch; unset or empty means the app-config path.
pub const LAYOUT_ENV: &str = "PALACE_LAYOUT_FILE";

/// The schema version this build writes and the newest it reads.
pub const LAYOUT_SCHEMA_VERSION: u64 = 1;

/// The event every webview may listen to for a change in layout memory.
///
/// The Preferences window shows which panels are detached and whether memory is
/// on; the main shell already learns about re-attaches through
/// `palace://panel-closed`. This event carries the same facts as
/// [`LayoutMemory`] so the Preferences window follows a detach that happened in
/// another window without polling.
pub const LAYOUT_CHANGED_EVENT: &str = "palace://layout";

/// How often the background thread flushes a dirty snapshot.
const AUTOSAVE_INTERVAL: Duration = Duration::from_millis(600);

/// How long a just-created window gets to be mapped before a deferred geometry
/// application is abandoned.
const MAP_RETRY_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the deferred application checks whether the window has mapped.
const MAP_POLL: Duration = Duration::from_millis(25);

/// A window rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge, relative to the full virtual desktop.
    pub x: i32,
    /// Top edge, relative to the full virtual desktop.
    pub y: i32,
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
}

/// One monitor, as the clamp needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// The monitor's platform name, when it has one.
    pub name: Option<String>,
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl Screen {
    /// One past the right edge.
    fn right(&self) -> i32 {
        self.x.saturating_add(self.w as i32)
    }

    /// One past the bottom edge.
    fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h as i32)
    }
}

/// What [`clamp_placement`] decided, with enough detail to log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The rectangle to apply, guaranteed fully on the chosen screen.
    pub rect: Rect,
    /// The name of the screen it was placed on, when the platform named one.
    pub screen: Option<String>,
    /// Whether the rectangle was centred (the saved monitor is gone, or the
    /// rectangle touched no screen at all).
    pub centred: bool,
    /// Whether the result differs from the input rectangle.
    pub moved: bool,
}

/// Decide where a saved rectangle belongs on the monitors that exist now.
///
/// This is the pure core of the off-screen clamp: no window handles, no
/// platform calls, so every branch is unit-testable. `primary` is the index
/// into `screens` of the primary monitor (out of range values are clamped, so
/// a caller that could not identify the primary may pass `0`). An empty
/// `screens` cannot be reasoned about and returns the rectangle unchanged.
#[must_use]
pub fn clamp_placement(
    rect: Rect,
    saved_monitor: Option<&str>,
    screens: &[Screen],
    primary: usize,
) -> Placement {
    if screens.is_empty() {
        return Placement {
            rect,
            screen: None,
            centred: false,
            moved: false,
        };
    }
    let primary = primary.min(screens.len() - 1);
    let named = saved_monitor.and_then(|name| {
        screens
            .iter()
            .find(|screen| screen.name.as_deref() == Some(name))
    });
    let (target, centred) = match (saved_monitor, named) {
        // The saved monitor still exists: keep the window on it.
        (Some(_), Some(screen)) => (screen, false),
        // The saved monitor is gone: centre on the primary.
        (Some(_), None) => (&screens[primary], true),
        // No monitor was remembered: stay where the rectangle is, if a screen
        // is there; otherwise centre on the primary.
        (None, _) => match covering(rect, screens) {
            Some(screen) => (screen, false),
            None => (&screens[primary], true),
        },
    };

    let w = rect.w.min(target.w);
    let h = rect.h.min(target.h);
    let (x, y) = if centred {
        (
            target.x + ((target.w - w) / 2) as i32,
            target.y + ((target.h - h) / 2) as i32,
        )
    } else {
        (
            clamp_axis(rect.x, target.x, target.w, w),
            clamp_axis(rect.y, target.y, target.h, h),
        )
    };
    let placed = Rect { x, y, w, h };
    Placement {
        rect: placed,
        screen: target.name.clone(),
        centred,
        moved: placed != rect,
    }
}

/// Clamp one axis so `value..value+size` fits inside `origin..origin+span`.
fn clamp_axis(value: i32, origin: i32, span: u32, size: u32) -> i32 {
    let maximum = origin.saturating_add(span.saturating_sub(size) as i32);
    value.clamp(origin, maximum)
}

/// The screen that contains the rectangle's centre, or overlaps it most.
fn covering(rect: Rect, screens: &[Screen]) -> Option<&Screen> {
    let centre_x = rect.x.saturating_add(rect.w as i32 / 2);
    let centre_y = rect.y.saturating_add(rect.h as i32 / 2);
    if let Some(screen) = screens.iter().find(|screen| {
        centre_x >= screen.x
            && centre_x < screen.right()
            && centre_y >= screen.y
            && centre_y < screen.bottom()
    }) {
        return Some(screen);
    }
    screens
        .iter()
        .max_by_key(|screen| overlap_area(rect, screen))
        .filter(|screen| overlap_area(rect, screen) > 0)
}

/// The overlap of a rectangle and a screen, in square physical pixels.
fn overlap_area(rect: Rect, screen: &Screen) -> u64 {
    let left = rect.x.max(screen.x);
    let top = rect.y.max(screen.y);
    let right = rect.x.saturating_add(rect.w as i32).min(screen.right());
    let bottom = rect.y.saturating_add(rect.h as i32).min(screen.bottom());
    if right <= left || bottom <= top {
        0
    } else {
        (right - left) as u64 * (bottom - top) as u64
    }
}

/// One window's remembered rectangle, keyed by its label in [`Layout`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Left edge of the window, in physical pixels.
    pub x: i32,
    /// Top edge of the window, in physical pixels.
    pub y: i32,
    /// Client (inner) width, in physical pixels: what `set_size` restores.
    pub w: u32,
    /// Client (inner) height, in physical pixels: what `set_size` restores.
    pub h: u32,
    /// The name of the monitor the window sat on, when the platform named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<String>,
    /// Whether the panel was detached (panels only; always false for `main`).
    #[serde(default)]
    pub detached: bool,
}

impl WindowGeometry {
    /// The rectangle part, for the clamp.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }

    /// Build an entry from a placed rectangle.
    #[must_use]
    pub fn from_rect(rect: Rect, monitor: Option<String>, detached: bool) -> WindowGeometry {
        WindowGeometry {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            monitor,
            detached,
        }
    }
}

/// The whole layout file: the schema version, whether memory is on, the detach
/// order and every remembered window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// The schema version the file was written with.
    #[serde(default = "schema_version")]
    pub version: u64,
    /// Whether window positions are remembered at all.
    ///
    /// Defaults to `true`, so a file written before this flag existed keeps
    /// its behaviour. `false` freezes the file: nothing is recorded or
    /// restored until the user turns memory back on.
    #[serde(default = "remember_default")]
    pub remember: bool,
    /// Panel ids in the order they were detached, oldest first.
    #[serde(default)]
    pub order: Vec<String>,
    /// Window label → remembered geometry.
    #[serde(default)]
    pub windows: BTreeMap<String, WindowGeometry>,
}

/// The default value for a missing `version` field.
fn schema_version() -> u64 {
    LAYOUT_SCHEMA_VERSION
}

/// The default value for a missing `remember` field: memory on.
fn remember_default() -> bool {
    true
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            version: LAYOUT_SCHEMA_VERSION,
            remember: true,
            order: Vec::new(),
            windows: BTreeMap::new(),
        }
    }
}

/// The layout-memory state the Preferences window shows.
///
/// `detached` is read from the live window registry, not from the saved flags:
/// it says which panels are in their own window *right now*, which is the
/// question the group answers. The order is [`Panel::ALL`]'s, so the list is
/// stable no matter when a panel was detached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutMemory {
    /// Whether positions and sizes are being remembered.
    pub remember: bool,
    /// The ids of the panels currently in their own window.
    pub detached: Vec<String>,
    /// The file the layout is written to, when the platform names one.
    pub path: Option<String>,
}

/// The panels a saved layout asks to reopen, in the saved detach order.
///
/// Only [`Panel`] labels can appear: a tool window such as `prefs` is tracked
/// for geometry but is never reopened at startup, and an entry with a
/// `detached` flag of `false` is skipped.
#[must_use]
pub fn panels_to_reopen(layout: &Layout) -> Vec<Panel> {
    let mut order = layout.order.clone();
    for label in layout.windows.keys() {
        if let Some(panel) = Panel::from_label(label) {
            let id = panel.id().to_string();
            if !order.contains(&id) {
                order.push(id);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| Panel::from_id(&id).ok())
        .filter(|panel| {
            layout
                .windows
                .get(panel.label())
                .is_some_and(|geometry| geometry.detached)
        })
        .collect()
}

/// The layout file's path: the [`LAYOUT_ENV`] override, else the app-config
/// directory. `None` means the platform named no config directory and geometry
/// simply is not persisted.
#[must_use]
pub fn path_for<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(LAYOUT_ENV) {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    app.path()
        .app_config_dir()
        .ok()
        .map(|directory| directory.join(LAYOUT_FILE))
}

/// What the layout file looked like when it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    /// No file yet: a normal first run.
    Missing,
    /// A readable file of this schema — or an older one.
    Current,
    /// A file written by a newer schema. It is left untouched.
    Newer,
    /// Unreadable is not JSON. Replacing it is safe.
    Malformed,
}

/// Read the layout file.
///
/// A missing file is normal (first run). A malformed file, a non-object, or a
/// file written by a newer schema is reported once and treated as empty: the
/// app must start even when the file is unreadable.
#[must_use]
pub fn load(path: &Path) -> Layout {
    read_file(path).0
}

/// Read the file and say what state it was in, so a store knows whether it may
/// ever write over it.
fn read_file(path: &Path) -> (Layout, FileState) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn(&format!(
                    "could not read the window layout at {}: {error}",
                    path.display()
                ));
                return (Layout::default(), FileState::Malformed);
            }
            return (Layout::default(), FileState::Missing);
        }
    };
    match serde_json::from_str::<Layout>(&text) {
        Ok(layout) if layout.version <= LAYOUT_SCHEMA_VERSION => (layout, FileState::Current),
        Ok(layout) => {
            warn(&format!(
                "ignoring the window layout at {}: schema version {} is newer than {LAYOUT_SCHEMA_VERSION}; it will not be overwritten",
                path.display(),
                layout.version
            ));
            (Layout::default(), FileState::Newer)
        }
        Err(error) => {
            warn(&format!(
                "ignoring a malformed window layout at {}: {error}",
                path.display()
            ));
            (Layout::default(), FileState::Malformed)
        }
    }
}

/// Write the layout file atomically.
///
/// The JSON goes to a sibling temp file first and is renamed over the target,
/// so a crash mid-write cannot leave a half-written layout behind. A document
/// identical to what is already on disk is left untouched.
pub fn save(path: &Path, layout: &Layout) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
    }
    let json = serde_json::to_string_pretty(layout).map_err(|error| error.to_string())?;
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == json) {
        return Ok(());
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)
        .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("could not replace {}: {error}", path.display())
    })
}

/// Report a recoverable layout problem on stderr and in the diagnostic log.
fn warn(message: &str) {
    eprintln!("palace: {message}");
    logging::log(Level::Warn, message);
}

/// Read a window's position, client size and monitor name.
///
/// The size is the **inner** (client) size, because that is what
/// [`WebviewWindow::set_size`] controls on restore; storing the outer size
/// would add the window decorations back on every restart. The position is the
/// outer origin, because that is what [`WebviewWindow::set_position`] controls;
/// when the platform cannot report a frame, the client origin is used instead
/// (a bare X server with no window manager reports the frame as `(0, 0)` even
/// after a move, while the client origin is exact).
///
/// `detached` is carried by the caller: the window itself does not know
/// whether it represents a detached panel.
#[must_use]
pub fn capture<R: Runtime>(window: &WebviewWindow<R>) -> Option<WindowGeometry> {
    let position = window_position(window)?;
    let size = window.inner_size().ok()?;
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .and_then(|monitor| monitor.name().cloned());
    Some(WindowGeometry::from_rect(
        Rect {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
        },
        monitor,
        false,
    ))
}

/// The window's origin: the outer position, unless that is a frame-less
/// `(0, 0)` while the client origin says otherwise.
fn window_position<R: Runtime>(window: &WebviewWindow<R>) -> Option<PhysicalPosition<i32>> {
    let outer = window.outer_position().ok();
    let inner = window.inner_position().ok();
    match (outer, inner) {
        (Some(outer), Some(inner)) if outer == PhysicalPosition::new(0, 0) && inner != outer => {
            Some(inner)
        }
        (Some(outer), _) => Some(outer),
        (None, Some(inner)) => Some(inner),
        (None, None) => None,
    }
}

/// The live monitors, and the index of the primary one (or 0).
fn screens_of<R: Runtime>(window: &WebviewWindow<R>) -> (Vec<Screen>, usize) {
    let monitors = window.available_monitors().unwrap_or_default();
    let screens: Vec<Screen> = monitors
        .iter()
        .map(|monitor| Screen {
            name: monitor.name().cloned(),
            x: monitor.position().x,
            y: monitor.position().y,
            w: monitor.size().width,
            h: monitor.size().height,
        })
        .collect();
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|monitor| {
            screens.iter().position(|screen| {
                screen.x == monitor.position().x
                    && screen.y == monitor.position().y
                    && screen.w == monitor.size().width
                    && screen.h == monitor.size().height
            })
        })
        .unwrap_or(0);
    (screens, primary)
}

/// Apply one window's saved rectangle, clamped on-screen.
///
/// Returns whether an entry existed and was applied. `main` is position-only
/// (see the module docs); any known panel gets its full saved rectangle.
///
/// A just-created window is often not mapped yet, and a platform is free to
/// drop geometry requests made before that point (measured on GTK: the size is
/// kept as a default and the position hint is ignored when no window manager
/// is there to honour it). So when the window reports no size yet, a short
/// background retry applies the same placement again once it has mapped.
pub fn apply<R: Runtime>(store: &LayoutStore, window: &WebviewWindow<R>) -> bool {
    let label = window.label().to_string();
    if !windows::tracks_geometry(&label) {
        return false;
    }
    let Some(saved) = store.snapshot().windows.get(&label).cloned() else {
        return false;
    };
    let (screens, primary) = screens_of(window);
    if screens.is_empty() {
        warn(&format!(
            "no monitors are known; leaving {label} where the platform put it"
        ));
        return false;
    }
    let placement = clamp_placement(saved.rect(), saved.monitor.as_deref(), &screens, primary);
    let resizable = label != MAIN_LABEL;
    place(window, &placement, resizable);
    let mapped = is_mapped(window);
    if !mapped {
        apply_when_mapped(window, placement.clone(), resizable);
    }
    WindowLog::new(&label).record(
        WindowMilestone::RestoredFromLayout,
        format!(
            "x={} y={} w={} h={} screen={} centred={} clamped={} size={} mapped={}",
            placement.rect.x,
            placement.rect.y,
            placement.rect.w,
            placement.rect.h,
            placement.screen.as_deref().unwrap_or("unknown"),
            placement.centred,
            placement.moved,
            if resizable {
                "restored"
            } else {
                "owned-by-ui-scale"
            },
            mapped,
        ),
    );
    true
}

/// Move and size a window to a decided placement.
///
/// `main` is never resized: `set_ui_scale` owns that window's size.
fn place<R: Runtime>(window: &WebviewWindow<R>, placement: &Placement, resizable: bool) {
    let label = window.label();
    if resizable {
        if let Err(error) = window.set_size(PhysicalSize::new(placement.rect.w, placement.rect.h)) {
            warn(&format!("could not size {label}: {error}"));
        }
    }
    if let Err(error) =
        window.set_position(PhysicalPosition::new(placement.rect.x, placement.rect.y))
    {
        warn(&format!("could not move {label}: {error}"));
    }
}

/// Whether the platform has given the window a real size yet.
fn is_mapped<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    window
        .outer_size()
        .is_ok_and(|size| size.width > 0 && size.height > 0)
}

/// Re-apply a placement once a freshly created window has mapped.
///
/// Runs on a short-lived background thread so the caller (the main thread
/// during startup restore) is never blocked; window calls are safe from any
/// thread. Gives up after [`MAP_RETRY_TIMEOUT`].
fn apply_when_mapped<R: Runtime>(window: &WebviewWindow<R>, placement: Placement, resizable: bool) {
    let window = window.clone();
    let label = window.label().to_string();
    let thread_label = label.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("palace-layout-{label}"))
        .spawn(move || {
            let deadline = Instant::now() + MAP_RETRY_TIMEOUT;
            while Instant::now() < deadline {
                if is_mapped(&window) {
                    place(&window, &placement, resizable);
                    WindowLog::new(&thread_label).record(
                        WindowMilestone::RestoredFromLayout,
                        format!(
                            "stage=after-map x={} y={} w={} h={}",
                            placement.rect.x, placement.rect.y, placement.rect.w, placement.rect.h
                        ),
                    );
                    return;
                }
                std::thread::sleep(MAP_POLL);
            }
        });
    if let Err(error) = spawned {
        warn(&format!(
            "could not start the deferred geometry apply for {label}: {error}"
        ));
    }
}

/// Reopen every detached panel at its saved rectangle and move `main` back to
/// its remembered position. Called once at startup, on the main thread.
///
/// Panels are reopened in the saved detach order, then any extra entries in
/// the file (defensive: an entry with no order position) in label order. The
/// store stays disarmed until this returns, so the first platform move events
/// cannot overwrite what is about to be restored.
pub fn restore_saved_layout<R: Runtime>(app: &AppHandle<R>) {
    let Some(store) = app.try_state::<LayoutStore>() else {
        return;
    };
    let layout = store.startup_snapshot();

    if !layout.remember {
        store.arm();
        logging::log(
            Level::Info,
            "layout memory is off; starting from the default single-window layout",
        );
        return;
    }

    if layout.windows.contains_key(MAIN_LABEL) {
        if let Some(main) = app.get_webview_window(MAIN_LABEL) {
            apply(&store, &main);
        }
    }

    let mut reopened = 0usize;
    for panel in panels_to_reopen(&layout) {
        match windows::open(app, panel) {
            Ok(_) => {
                if let Some(window) = app.get_webview_window(panel.label()) {
                    apply(&store, &window);
                }
                reopened += 1;
            }
            Err(error) => {
                logging::log(
                    Level::Warn,
                    format!("could not restore {}: {error}", panel.label()),
                );
            }
        }
    }
    store.arm();
    logging::log(
        Level::Info,
        format!(
            "window layout restore complete: {} remembered, {reopened} panel(s) reopened",
            layout.windows.len()
        ),
    );
}

/// Ask the main thread to run [`restore_saved_layout`] after `setup` returns.
///
/// Window creation and geometry belong on the main thread. If the call cannot
/// be scheduled, restore runs inline instead of being lost.
pub fn schedule_restore<R: Runtime>(app: &AppHandle<R>) {
    let scheduled = app.clone();
    let result = app.run_on_main_thread(move || restore_saved_layout(&scheduled));
    if let Err(error) = result {
        logging::log(
            Level::Warn,
            format!("could not schedule the window layout restore: {error}"),
        );
        restore_saved_layout(app);
    }
}

/// Record a window that moved or resized, when layout memory is armed.
pub fn note_geometry_changed<R: Runtime>(window: &Window<R>) {
    let label = window.label();
    if !windows::tracks_geometry(label) {
        return;
    }
    let Some(store) = window.try_state::<LayoutStore>() else {
        return;
    };
    let Some(webview) = window.app_handle().get_webview_window(label) else {
        return;
    };
    store.note_window(&webview);
}

/// Capture a closing window's geometry and flush it before the window is gone.
///
/// This never changes the `detached` flag: closing a panel on app quit must
/// leave the panel marked detached so the next launch reopens it.
pub fn note_window_closing<R: Runtime>(window: &Window<R>) {
    note_closing(window.app_handle(), window.label());
}

/// [`note_window_closing`], addressed by the app handle and a window label.
///
/// The quit path closes panels from an [`AppHandle`] rather than a `Window`, so
/// it needs the same capture-and-flush without a window reference. The
/// `detached` flag is left untouched, exactly as [`note_window_closing`] does.
pub fn note_closing<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if !windows::tracks_geometry(label) {
        return;
    }
    let Some(store) = app.try_state::<LayoutStore>() else {
        return;
    };
    let Some(webview) = app.get_webview_window(label) else {
        return;
    };
    store.note_window(&webview);
    if let Err(error) = store.flush() {
        warn(&format!(
            "could not save the window layout on {label}'s close: {error}"
        ));
    }
}

/// The layout-memory state the Preferences window shows.
///
/// The detached list comes from the live window registry: it answers "which
/// panels are in their own window right now", which is what the group displays.
#[must_use]
pub fn layout_memory_state<R: Runtime>(app: &AppHandle<R>) -> LayoutMemory {
    let store = app.try_state::<LayoutStore>();
    let remember = store.as_ref().is_none_or(|store| store.remember());
    let path = store
        .as_ref()
        .and_then(|store| store.path())
        .map(|path| path.display().to_string());
    let detached = Panel::ALL
        .iter()
        .filter(|panel| app.get_webview_window(panel.label()).is_some())
        .map(|panel| panel.id().to_string())
        .collect();
    LayoutMemory {
        remember,
        detached,
        path,
    }
}

/// Tell every window that layout memory changed.
///
/// The Preferences window follows detaches that happen in another window
/// through this event instead of polling; the payload is a [`LayoutMemory`].
pub fn notify_layout_changed<R: Runtime>(app: &AppHandle<R>) {
    let state = layout_memory_state(app);
    if let Err(error) = app.emit(LAYOUT_CHANGED_EVENT, &state) {
        logging::log(
            Level::Debug,
            format!("layout event was not delivered: {error}"),
        );
    }
}

/// Read the layout-memory state. Backs the Layout memory preference group.
#[tauri::command]
pub fn get_layout_memory<R: Runtime>(app: AppHandle<R>) -> Result<LayoutMemory, String> {
    Ok(layout_memory_state(&app))
}

/// Turn layout memory on or off and report the new state.
///
/// Turning it off is a real stop, not a hidden control: the store refuses to
/// write anything after this returns, so the next launch starts from the
/// default single-window layout.
#[tauri::command]
pub fn set_layout_remember<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<LayoutMemory, String> {
    if let Some(store) = app.try_state::<LayoutStore>() {
        store.set_remember(enabled)?;
    }
    logging::log(
        Level::Info,
        format!(
            "layout memory turned {}",
            if enabled { "on" } else { "off" }
        ),
    );
    let state = layout_memory_state(&app);
    let _ = app.emit(LAYOUT_CHANGED_EVENT, &state);
    Ok(state)
}

/// Reset the layout to default: re-dock every panel now and forget every
/// remembered window.
///
/// The panel windows are destroyed rather than asked to close, so nothing
/// captures geometry on the way out; the resulting `Destroyed` events re-dock
/// each panel through the one existing re-dock channel. The store is cleared
/// afterwards, so the next launch is the default single-window layout. With
/// memory off nothing is written, because memory off means nothing is written.
#[tauri::command]
pub fn reset_layout<R: Runtime>(app: AppHandle<R>) -> Result<LayoutMemory, String> {
    let mut closed = 0usize;
    let mut failed = Vec::new();
    for panel in Panel::ALL {
        if let Some(window) = app.get_webview_window(panel.label()) {
            match window.destroy() {
                Ok(()) => closed += 1,
                Err(error) => {
                    failed.push(panel.id().to_string());
                    warn(&format!(
                        "could not close {} while resetting the layout: {error}",
                        panel.label()
                    ));
                }
            }
        }
    }
    if let Some(store) = app.try_state::<LayoutStore>() {
        store.reset_layout();
        if let Err(error) = store.flush() {
            warn(&format!("could not write the reset window layout: {error}"));
        }
    }
    logging::log(
        Level::Info,
        format!(
            "layout reset to default: {closed} panel window(s) closed, {} still open",
            failed.len()
        ),
    );
    // The destroys are asynchronous, so the live registry can still list the
    // windows at this point. The answer is the set the reset guarantees: only a
    // panel whose window refused to close stays detached.
    let mut state = layout_memory_state(&app);
    state.detached = failed;
    let _ = app.emit(LAYOUT_CHANGED_EVENT, &state);
    Ok(state)
}

/// The in-memory layout and its file, shared by every window.
#[derive(Clone)]
pub struct LayoutStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// A file written by a newer schema is never overwritten.
    write_blocked: bool,
    /// Set on the first failed write so the log is not flooded by the
    /// autosave thread; cleared by the next successful write.
    write_failed: AtomicBool,
}

struct State {
    /// The live snapshot: what a flush would write.
    layout: Layout,
    /// What the file said at startup, frozen before any window moved. The
    /// restore reads this, not `layout`, because the platform can deliver
    /// move events for the config window before `setup` runs.
    startup: Layout,
    /// Whether `layout` differs from what is on disk.
    dirty: bool,
    /// Every mutation bumps this; a flush only clears `dirty` when the
    /// revision it wrote is still the current one.
    revision: u64,
    /// Whether layout events may be recorded. False until the startup restore
    /// has run, so the first platform move events cannot clobber the file.
    armed: bool,
}

impl LayoutStore {
    /// Load the store for `app`, logging the resolved file path.
    #[must_use]
    pub fn discover<R: Runtime>(app: &AppHandle<R>) -> LayoutStore {
        LayoutStore::open(path_for(app))
    }

    /// Load the store from an explicit path (`None` disables persistence).
    #[must_use]
    pub fn open(path: Option<PathBuf>) -> LayoutStore {
        let (mut layout, file_state) = match path.as_deref() {
            Some(path) => read_file(path),
            None => (Layout::default(), FileState::Missing),
        };
        if !layout.remember {
            // Memory is off: the saved geometry is deliberately not loaded, so
            // this session starts from the default single-window layout and
            // turning memory back on cannot resurrect a stale detach flag. The
            // file itself is left exactly as it was.
            layout = Layout {
                remember: false,
                ..Layout::default()
            };
        }
        match path.as_deref() {
            Some(path) => logging::log(
                Level::Info,
                format!(
                    "window layout file: {} (remember={})",
                    path.display(),
                    layout.remember
                ),
            ),
            None => logging::log(
                Level::Info,
                "window layout: no config directory is available; geometry will not persist",
            ),
        }
        LayoutStore {
            inner: Arc::new(Inner {
                path,
                state: Mutex::new(State {
                    startup: layout.clone(),
                    layout,
                    dirty: false,
                    revision: 0,
                    armed: false,
                }),
                write_blocked: file_state == FileState::Newer,
                write_failed: AtomicBool::new(false),
            }),
        }
    }

    /// The file this store writes, when it has one.
    #[must_use]
    pub fn path(&self) -> Option<PathBuf> {
        self.inner.path.clone()
    }

    /// A copy of the live snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Layout {
        self.state().layout.clone()
    }

    /// A copy of what the file said at startup.
    #[must_use]
    pub fn startup_snapshot(&self) -> Layout {
        self.state().startup.clone()
    }

    /// Start recording automatic move/resize events. Until this is called the
    /// startup restore is still in flight and platform events are ignored.
    pub fn arm(&self) {
        self.state().armed = true;
    }

    /// Whether automatic recording is armed.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.state().armed
    }

    /// Whether there are unsaved changes.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.state().dirty
    }

    /// Whether positions and sizes are being remembered.
    #[must_use]
    pub fn remember(&self) -> bool {
        self.state().layout.remember
    }

    /// Turn layout memory on or off, persisting the flag itself.
    ///
    /// Turning it off writes the current snapshot one last time — the flag has
    /// to reach the disk for the next launch to see it — and every later
    /// [`flush`](Self::flush) becomes a no-op. Turning it back on resumes
    /// recording; the snapshot it starts from is whatever the session has now,
    /// because a store that was opened with memory off never loaded the file's
    /// geometry.
    pub fn set_remember(&self, enabled: bool) -> Result<(), String> {
        {
            let mut state = self.state();
            if state.layout.remember == enabled {
                return Ok(());
            }
            state.layout.remember = enabled;
            state.revision = state.revision.wrapping_add(1);
            state.dirty = true;
        }
        self.flush_inner(true)
    }

    /// Forget every remembered window and the detach order.
    ///
    /// This is the store half of "reset layout to default": the next launch
    /// starts from the default single-window layout. The window destruction
    /// that re-docks panels *now* is [`reset_layout`]'s job. When memory is
    /// off the store is still cleared in memory but nothing is written.
    pub fn reset_layout(&self) {
        let mut state = self.state();
        state.layout.windows.clear();
        state.layout.order.clear();
        state.revision = state.revision.wrapping_add(1);
        state.dirty = true;
    }

    /// Store one window's rectangle, keeping its `detached` flag.
    ///
    /// Called from the free event helpers; a window that did not move or resize
    /// costs one capture and no write.
    fn note_window<R: Runtime>(&self, window: &WebviewWindow<R>) {
        if !self.armed() {
            return;
        }
        let label = window.label().to_string();
        let Some(mut geometry) = capture(window) else {
            return;
        };
        let mut state = self.state();
        geometry.detached = state
            .layout
            .windows
            .get(&label)
            .is_some_and(|saved| saved.detached);
        if state.layout.windows.get(&label) == Some(&geometry) {
            return;
        }
        state.layout.windows.insert(label, geometry);
        state.revision = state.revision.wrapping_add(1);
        state.dirty = true;
    }

    /// Record one window's geometry explicitly (bypasses the armed gate).
    pub fn record_geometry(&self, label: &str, geometry: WindowGeometry) {
        let mut state = self.state();
        state.layout.windows.insert(label.to_string(), geometry);
        state.revision = state.revision.wrapping_add(1);
        state.dirty = true;
    }

    /// Mark a panel detached or re-attached, in place or at the end of the
    /// saved detach order. The only writer of the `detached` flag.
    pub fn set_detached(&self, panel: Panel, detached: bool) {
        let id = panel.id().to_string();
        let label = panel.label().to_string();
        let mut state = self.state();
        if detached {
            if !state.layout.order.iter().any(|entry| entry == &id) {
                state.layout.order.push(id);
            }
            let entry = state.layout.windows.entry(label).or_insert_with(|| {
                WindowGeometry::from_rect(
                    Rect {
                        x: 0,
                        y: 0,
                        w: PANEL_SIZE.0 as u32,
                        h: PANEL_SIZE.1 as u32,
                    },
                    None,
                    true,
                )
            });
            entry.detached = true;
        } else {
            state.layout.order.retain(|entry| entry != &id);
            if let Some(entry) = state.layout.windows.get_mut(&label) {
                entry.detached = false;
            }
        }
        state.revision = state.revision.wrapping_add(1);
        state.dirty = true;
    }

    /// The panels currently marked detached, in the order they were detached.
    #[must_use]
    pub fn detached_panels(&self) -> Vec<Panel> {
        let state = self.state();
        state
            .layout
            .order
            .iter()
            .filter_map(|id| Panel::from_id(id).ok())
            .filter(|panel| {
                state
                    .layout
                    .windows
                    .get(panel.label())
                    .is_some_and(|geometry| geometry.detached)
            })
            .collect()
    }

    /// Write the snapshot if it changed. A no-op when clean, when no path is
    /// configured, or when layout memory is off.
    pub fn flush(&self) -> Result<(), String> {
        self.flush_inner(false)
    }

    /// Write the snapshot; `force` bypasses the memory-off gate so the flag
    /// that turns memory off can itself reach the disk.
    fn flush_inner(&self, force: bool) -> Result<(), String> {
        if self.inner.write_blocked {
            return Ok(());
        }
        let (path, layout, revision) = {
            let state = self.state();
            if !state.dirty {
                return Ok(());
            }
            if !force && !state.layout.remember {
                return Ok(());
            }
            (
                self.inner.path.clone(),
                state.layout.clone(),
                state.revision,
            )
        };
        let Some(path) = path else {
            return Ok(());
        };
        match save(&path, &layout) {
            Ok(()) => {
                let mut state = self.state();
                if state.revision == revision {
                    state.dirty = false;
                }
                self.inner.write_failed.store(false, Ordering::SeqCst);
                Ok(())
            }
            Err(error) => {
                if !self.inner.write_failed.swap(true, Ordering::SeqCst) {
                    warn(&format!(
                        "could not write the window layout to {}: {error}",
                        path.display()
                    ));
                }
                Err(error)
            }
        }
    }

    /// Lock the state, treating a poisoned mutex as usable.
    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Start the background thread that flushes a dirty snapshot every
/// [`AUTOSAVE_INTERVAL`]. The thread lives as long as the process; a store
/// with no path simply does nothing.
pub fn spawn_autosave(store: &LayoutStore) -> std::io::Result<()> {
    let store = store.clone();
    std::thread::Builder::new()
        .name("palace-layout-autosave".to_string())
        .spawn(move || loop {
            std::thread::sleep(AUTOSAVE_INTERVAL);
            let _ = store.flush();
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, x: i32, y: i32, w: u32, h: u32) -> Screen {
        Screen {
            name: Some(name.to_string()),
            x,
            y,
            w,
            h,
        }
    }

    fn primary_and_secondary() -> Vec<Screen> {
        vec![
            screen("primary", 0, 0, 1920, 1080),
            screen("right", 1920, 0, 1920, 1080),
        ]
    }

    #[test]
    fn a_valid_rectangle_is_left_exactly_alone() {
        let rect = Rect {
            x: 100,
            y: 120,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("primary"), &primary_and_secondary(), 0);
        assert_eq!(placement.rect, rect);
        assert!(!placement.moved, "nothing had to move");
        assert!(!placement.centred);
    }

    #[test]
    fn a_partly_offscreen_rectangle_is_pulled_fully_on_screen() {
        let rect = Rect {
            x: -100,
            y: -40,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("primary"), &primary_and_secondary(), 0);
        assert_eq!(
            placement.rect,
            Rect {
                x: 0,
                y: 0,
                w: 640,
                h: 480
            },
            "a window hanging over the top-left edge is pulled in whole"
        );
        assert!(placement.moved);
        assert!(!placement.centred);

        let over_right = Rect {
            x: 1800,
            y: 1000,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(over_right, Some("primary"), &primary_and_secondary(), 0);
        assert_eq!(
            placement.rect,
            Rect {
                x: 1280,
                y: 600,
                w: 640,
                h: 480
            },
            "the whole rectangle ends flush with the right and bottom edges"
        );
    }

    #[test]
    fn a_rectangle_whose_saved_monitor_is_gone_is_centred_on_the_primary() {
        let rect = Rect {
            x: 2500,
            y: 200,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("unplugged"), &primary_and_secondary(), 0);
        assert_eq!(
            placement.rect,
            Rect {
                x: 640,
                y: 300,
                w: 640,
                h: 480
            },
            "centred on the 1920×1080 primary"
        );
        assert!(placement.centred);
        assert!(placement.moved);
        assert_eq!(placement.screen.as_deref(), Some("primary"));
    }

    #[test]
    fn a_window_on_a_secondary_monitor_stays_there() {
        let rect = Rect {
            x: 2000,
            y: 300,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("right"), &primary_and_secondary(), 0);
        assert_eq!(placement.rect, rect);
        assert_eq!(placement.screen.as_deref(), Some("right"));
    }

    #[test]
    fn a_rectangle_larger_than_its_monitor_is_shrunk_to_fit() {
        let rect = Rect {
            x: 10,
            y: 10,
            w: 4000,
            h: 3000,
        };
        let placement = clamp_placement(rect, Some("primary"), &primary_and_secondary(), 0);
        assert_eq!(
            placement.rect,
            Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080
            }
        );
        assert!(placement.moved);
    }

    #[test]
    fn without_a_monitor_name_the_containing_screen_wins() {
        let rect = Rect {
            x: 2100,
            y: 100,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, None, &primary_and_secondary(), 0);
        assert_eq!(
            placement.rect, rect,
            "the rectangle is already on the secondary screen"
        );
        assert_eq!(placement.screen.as_deref(), Some("right"));

        // In dead space between arrangements: centre on the primary.
        let lost = Rect {
            x: -5000,
            y: -5000,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(lost, None, &primary_and_secondary(), 0);
        assert!(placement.centred);
        assert_eq!(placement.screen.as_deref(), Some("primary"));
        assert_eq!(placement.rect.x, 640);
    }

    #[test]
    fn without_monitors_the_rectangle_is_returned_unchanged() {
        let rect = Rect {
            x: 40,
            y: 50,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("primary"), &[], 0);
        assert_eq!(placement.rect, rect);
        assert!(!placement.moved);
        assert!(!placement.centred);
    }

    #[test]
    fn an_out_of_range_primary_index_falls_back_to_the_last_screen() {
        let rect = Rect {
            x: 9000,
            y: 9000,
            w: 640,
            h: 480,
        };
        let placement = clamp_placement(rect, Some("gone"), &primary_and_secondary(), 99);
        assert_eq!(placement.rect.x, 1920 + 640);
        assert!(placement.centred);
    }

    #[test]
    fn a_layout_round_trips_through_serde() {
        let mut layout = Layout::default();
        layout.windows.insert(
            "panel-users".to_string(),
            WindowGeometry::from_rect(
                Rect {
                    x: -20,
                    y: 30,
                    w: 500,
                    h: 400,
                },
                Some("right".to_string()),
                true,
            ),
        );
        layout.order.push("users".to_string());
        let text = serde_json::to_string_pretty(&layout).expect("serializes");
        let parsed: Layout = serde_json::from_str(&text).expect("parses");
        assert_eq!(parsed, layout);
        assert!(text.contains("\"version\": 1"), "{text}");
        assert!(text.contains("\"panel-users\""), "{text}");
    }

    #[test]
    fn detach_order_is_remembered_and_cleared() {
        let store = LayoutStore::open(None);
        assert!(!store.armed());
        store.set_detached(Panel::Users, true);
        store.set_detached(Panel::Chat, true);
        store.set_detached(Panel::Users, false);
        assert_eq!(store.detached_panels(), vec![Panel::Chat]);
        assert_eq!(store.snapshot().order, vec!["chat".to_string()]);
        assert!(store.dirty());

        store.set_detached(Panel::Chat, false);
        assert!(store.detached_panels().is_empty());
        assert!(store.snapshot().order.is_empty());
    }

    #[test]
    fn automatic_recording_waits_for_the_arm() {
        let store = LayoutStore::open(None);
        assert!(!store.armed(), "a fresh store must not record yet");
        store.arm();
        assert!(store.armed());
    }

    fn scratch(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("palace-geometry-{tag}-{unique}"));
        std::fs::create_dir_all(&directory).expect("a scratch directory is creatable");
        directory
    }

    fn seeded_layout() -> Layout {
        let mut layout = Layout::default();
        layout.windows.insert(
            MAIN_LABEL.to_string(),
            WindowGeometry::from_rect(
                Rect {
                    x: 40,
                    y: 60,
                    w: 900,
                    h: 700,
                },
                Some("primary".to_string()),
                false,
            ),
        );
        layout.windows.insert(
            Panel::Users.label().to_string(),
            WindowGeometry::from_rect(
                Rect {
                    x: 120,
                    y: 90,
                    w: 520,
                    h: 430,
                },
                Some("primary".to_string()),
                true,
            ),
        );
        layout.order.push("users".to_string());
        layout
    }

    #[test]
    fn a_file_without_the_remember_flag_keeps_remembering() {
        let layout: Layout = serde_json::from_str(r#"{"version":1,"windows":{}}"#)
            .expect("an older file still parses");
        assert!(layout.remember, "memory defaults to on");
        assert!(Layout::default().remember);
        assert!(serde_json::to_string(&Layout::default())
            .expect("serializes")
            .contains("\"remember\":true"));
    }

    #[test]
    fn remember_off_freezes_the_file_and_the_next_launch_is_the_default_layout() {
        let directory = scratch("remember-off");
        let path = directory.join(LAYOUT_FILE);
        save(&path, &seeded_layout()).expect("the seed layout is written");

        let store = LayoutStore::open(Some(path.clone()));
        assert!(store.remember());
        assert_eq!(
            panels_to_reopen(&store.startup_snapshot()),
            vec![Panel::Users],
            "with memory on the detached panel is part of the next launch"
        );

        store
            .set_remember(false)
            .expect("turning memory off persists the flag");
        let frozen = std::fs::read_to_string(&path).expect("the layout file is readable");
        assert!(frozen.contains("\"remember\": false"), "{frozen}");

        // Everything a live session does after the toggle must not reach disk.
        store.arm();
        store.set_detached(Panel::Chat, true);
        store.record_geometry(
            Panel::Chat.label(),
            WindowGeometry::from_rect(
                Rect {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4,
                },
                None,
                true,
            ),
        );
        store
            .flush()
            .expect("a flush while memory is off is a no-op, not an error");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the layout file is readable"),
            frozen,
            "no layout state may be written while memory is off"
        );

        // The next launch reads the same file: default single-window layout.
        let relaunched = LayoutStore::open(Some(path.clone()));
        assert!(!relaunched.remember());
        assert!(
            relaunched.startup_snapshot().windows.is_empty(),
            "the saved geometry is not loaded while memory is off"
        );
        assert!(
            panels_to_reopen(&relaunched.startup_snapshot()).is_empty(),
            "the next launch must be the default single-window layout"
        );
        relaunched.arm();
        relaunched.flush().expect("the relaunch writes nothing");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the layout file is readable"),
            frozen,
            "the relaunch must leave the frozen file exactly as it was"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn turning_remember_back_on_starts_from_the_current_session() {
        let directory = scratch("remember-on");
        let path = directory.join(LAYOUT_FILE);
        save(&path, &seeded_layout()).expect("the seed layout is written");

        // The user turns memory off in one session …
        let first = LayoutStore::open(Some(path.clone()));
        first
            .set_remember(false)
            .expect("turning memory off persists the flag");

        // … and launches again: the saved geometry is not loaded.
        let store = LayoutStore::open(Some(path.clone()));
        assert!(!store.remember());
        assert!(store.startup_snapshot().windows.is_empty());

        // Turning it back on now remembers this session, not the old file.
        store
            .set_remember(true)
            .expect("turning memory on persists again");
        store.set_detached(Panel::Chat, true);
        store
            .flush()
            .expect("what the session does after memory is on is written");

        let reloaded = LayoutStore::open(Some(path.clone()));
        assert!(reloaded.remember());
        assert!(
            reloaded
                .startup_snapshot()
                .windows
                .contains_key(Panel::Chat.label()),
            "what the session detaches after memory is on is remembered"
        );
        assert!(
            !reloaded
                .startup_snapshot()
                .windows
                .contains_key(Panel::Users.label()),
            "the stale detach flag from before the toggle must not come back"
        );
        assert_eq!(
            panels_to_reopen(&reloaded.startup_snapshot()),
            vec![Panel::Chat]
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn reset_layout_forgets_every_window_and_the_detach_order() {
        let store = LayoutStore::open(None);
        store.set_detached(Panel::Users, true);
        store.record_geometry(MAIN_LABEL, seeded_layout().windows[MAIN_LABEL].clone());
        assert!(store.dirty());
        assert_eq!(store.detached_panels(), vec![Panel::Users]);

        store.reset_layout();

        assert!(store.snapshot().windows.is_empty());
        assert!(store.snapshot().order.is_empty());
        assert!(store.detached_panels().is_empty());
        assert!(store.dirty(), "the reset must reach the file");
    }

    #[test]
    fn the_preferences_window_is_tracked_for_geometry_but_never_reopened() {
        let mut layout = seeded_layout();
        layout.windows.insert(
            crate::windows::ToolWindow::Preferences.label().to_string(),
            WindowGeometry::from_rect(
                Rect {
                    x: 700,
                    y: 500,
                    w: 780,
                    h: 620,
                },
                None,
                false,
            ),
        );

        assert_eq!(
            panels_to_reopen(&layout),
            vec![Panel::Users],
            "a prefs entry must never make the Preferences window appear at startup"
        );
        assert!(
            windows::tracks_geometry(crate::windows::ToolWindow::Preferences.label()),
            "the prefs window's own geometry is remembered"
        );
        assert!(!windows::is_panel_label(
            crate::windows::ToolWindow::Preferences.label()
        ));
    }
}

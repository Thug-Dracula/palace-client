# Windows Architecture: the multi-window state contract

Status: Task 2 deliverable, written 2026-09-19 against commit `1dd56b5`.
Reconciled to shipped behaviour by Task 29 (2026-09-21) against save point
`44d76ac` + the Task 29 route fix: the `refresh()` replay list is now the
shipped one, the Preferences window is documented as a tool window, the
destroy→re-dock semantics and the measured startup order are recorded, and the
routes are stated as fragment-only.
Scope: this is a contract and decision document. It does not design UI and does
not propose code. Every claim below was checked against the current sources;
citations are `file:line`.

---

## 1. The contract in one paragraph

The Rust backend owns all session state and the one and only network
connection. The frontend is a set of views. The `main` window and every detached
panel window are peers: each runs its own webview, therefore its own empty copy
of the frontend store, and each is responsible for seeding itself the same way.
A view seeds itself by (a) subscribing to the single broadcast channel
`palace://event` and (b) calling the `refresh` command once on mount. `refresh`
does not return a snapshot. It asks the runtime to re-emit the session events
(status, banner, room list, user list, current room, the most recent chat lines,
and the last composed screen) to every subscriber. State that no event carries
(settings, audio, the prop bag, the editor session, and the per-window viewport)
is fetched per window with its own command. No window opens a second connection
and no window keeps authoritative state of its own.

---

## 2. Who owns what

### 2.1 Owned by the backend, process-global, shared by every window

| State | Owner | Citation |
| --- | --- | --- |
| The one network connection and the session it drives | `ClientRuntime`, held in `AppState.client` | `src-tauri/src/lib.rs:31`, `src-tauri/src/lib.rs:184-198` |
| Settings (host, port, username, soundfont, audio flags, identity) | `AppState.settings` plus `settings.rs`, one file on disk | `src-tauri/src/lib.rs:32`, `src-tauri/src/settings.rs` |
| Audio engine and its state | `AppState.audio` | `src-tauri/src/lib.rs:33`, `src-tauri/src/commands.rs:588` |
| Frame, mid, top, catalog, avatar-image and face slots | `protocol.rs` slots (`FrameSlot`, `CatalogSlot`, `AvatarImageSlot`) | `src-tauri/src/protocol.rs:17,42,68` |
| Prop bag catalog and shelf health | `protocol::BagSlot` and `bag::BagService` | `src-tauri/src/protocol.rs:97`, `src-tauri/src/bag.rs:51,367`, `src-tauri/src/lib.rs:224-228` |
| One prop editor session | `editor::EditorSlot` | `src-tauri/src/editor.rs:502`, `src-tauri/src/lib.rs:226` |
| Chat scrollback transcript | `ClientState.chat`, capped at 500 lines | `crates/palace-client/src/state.rs:292-306` |

The bag and editor are process-global on the Rust side. The JavaScript arrays
that hold a bag snapshot (`store.bagCatalog` and friends) live in one webview's
store only. They are a cache in front of the backend, never the source of truth.

### 2.2 Per-window, never synced

Each webview has an independent module instance of `src/lib/store.svelte.ts`, so
every window starts with the defaults at `src/lib/store.svelte.ts:53-88`. The
following are view-only or window-only and are deliberately not shared between
windows:

- `scale` and `native` (the Fit / 1:1 toggle): `src/lib/store.svelte.ts:83-84`.
- `showNames` and `showAvatars`: `src/lib/store.svelte.ts:85-86`. These do change
  backend rendering through `set_visibility` (`crates/palace-client/src/runtime.rs:1006`),
  but the local boolean is per window. Two windows can disagree about the
  checkbox; the room frame stays consistent because the backend value is
  authoritative for rendering.
- `roomFilter` (the Rooms list filter): `src/lib/store.svelte.ts:73`.
- `viewport` (the measured pixel size of the room view): `src/lib/store.svelte.ts:88`,
  reported by `Viewport.svelte:114-115`.
- Chat draft text and scroll offset: `ChatPanel.svelte:5,10-12`.

### 2.3 The single connection

There is exactly one `palace://event` channel (`src/lib/api.ts:6`,
`src-tauri/src/commands.rs:22`) and one broadcast site
(`src-tauri/src/lib.rs:298`, `app.emit`). Every window listens to the same
channel. A detached window must never call `connect`, must never own a socket,
and must never publish its own event channel.

**Backend change-notifications.** Alongside the session channel there are three
backend-emitted notifications. Each carries one subsystem's state to every open
window, so a change made in one window is visible in the others without a reload:

| Event | Emitted from | Carries |
|---|---|---|
| `palace://geometry` | Rust, when the room view is measured | the viewport's pixel size |
| `palace://prefs` | `prefs.rs`, on every settings write | the merged `prefs` block |
| `palace://layout` | `geometry.rs`, on a layout change | `{remember, detached[], path}` |

They are **not** session events and they are **not** window-created: the backend
emits them, any window may listen, and no window may emit one. `palace://event`
remains the only channel carrying session state, and the only one a window
subscribes to for the session. A window that needs state it does not receive should
re-read the authoritative source on focus rather than invent another channel.

---

## 3. The event channel and what each event carries

`ClientEvent` is the union at `src/lib/api.ts:150-172` (Rust counterpart at
`crates/palace-client/src/runtime.rs:271-330`). The store's `apply()` maps each
to a field at `src/lib/store.svelte.ts:302-353`.

| Event | Field(s) it fills | Emitted from |
| --- | --- | --- |
| `status` | `status`, `statusMessage` | runtime resync and session transitions |
| `banner` | `banner` | runtime resync and banner frame |
| `rooms` | `rooms` | runtime resync and room-list frame |
| `users` | `users` | runtime resync and roster frame |
| `room_entered` | `room` (and clears `avatars`) | runtime resync and room entry |
| `chat` | `chat`, via `pushLine` | runtime resync and each talk/whisper line |
| `screen` | `screen` and `notes` | runtime resync and every recompose |
| `avatars` | `avatars` roster | roster trigger sites, not the resync block |
| `tooltip` | `tooltip` | room pointer handling |
| `script` / `note` | `notices` (last 6) | script and asset-pipeline notices |
| `sound`, `midi_play`, `midi_loop`, `midi_stop`, `beep` | audio playback, not store fields | the event pump |

---

## 4. The per-panel required-state matrix

Two tables together answer "what does each window need, where does it come
from, and does `refresh` supply it".

### 4.1 Where every required field comes from

Legend for "refresh supplies": **yes** means the resync block re-emits it;
**partial** means it re-emits some but not the full window; **no** means only a
command or a per-window action produces it.

| Required state | Canonical source | refresh supplies | Citation |
| --- | --- | --- | --- |
| status | `status` event | yes | `crates/palace-client/src/runtime.rs:1118` |
| banner | `banner` event | yes | `crates/palace-client/src/runtime.rs:1122` |
| rooms | `rooms` event | yes | `crates/palace-client/src/runtime.rs:1125` |
| users | `users` event | yes | `crates/palace-client/src/runtime.rs:1128` |
| room | `room_entered` event | yes if in a room | `crates/palace-client/src/runtime.rs:1131` |
| screen | `screen` event | yes | `crates/palace-client/src/runtime.rs:1138`, `:2350-2374` |
| avatars | `avatars` event | **no** | emitted only at roster triggers, `crates/palace-client/src/runtime.rs:1008-1753` |
| notes | `screen.notes` folded into the `screen` event | yes | `src/lib/store.svelte.ts:330-332`, `crates/palace-client/src/runtime.rs:1138` |
| notices | `note` / `script` events | **no** | `src/lib/store.svelte.ts:339-351` |
| chat | `chat` events | **partial: last 120 only** | `crates/palace-client/src/runtime.rs:1134` |
| settings | `get_settings` command | no | `src-tauri/src/commands.rs:36`, `src/lib/api.ts:450` |
| audio | `get_audio_state` command | no | `src-tauri/src/commands.rs:588`, `src/lib/api.ts:519` |
| viewport | the window measures its own DOM and calls `set_viewport` | no | `Viewport.svelte:114-115`, `src-tauri/src/commands.rs:547` |
| bag snapshot | `bag_*` / `outfits_*` commands, gathered by `store.loadBag()` | no | `src/lib/api.ts:378-448`, `src/lib/store.svelte.ts:193-213` |
| editor session | `editor_*` commands, opened by `editor_state` | no | `src/lib/api.ts:607`, `src-tauri/src/editor.rs:1994` |

### 4.2 Which window needs which field

Y = required and replayed by `refresh`. C = required, fetched by command (not
replayed). N = required but not delivered by `refresh` (a gap, see section 5).
L = per-window local, not shared. `-` = not required.

| Window | status | banner | rooms | users | room | screen | avatars | notes | notices | chat | settings | audio | viewport | bag | editor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| main | Y | Y | Y | Y | Y | Y | N | Y | N | Y (120/500) | C | C | L | C | C |
| panel-room (Room view) | Y | - | - | Y | Y | Y | N | Y | N | - | C | - | L | C | C |
| panel-users (Users list) | Y | - | - | Y | - | - | - | - | - | - | - | - | - | - | - |
| panel-rooms (Rooms list) | Y | - | Y | - | Y | - | - | - | - | - | - | - | - | - | - |
| panel-chat (Chat/log) | Y | Y | - | - | - | - | - | - | - | Y (120/500) | C | - | - | - | - |
| panel-props (Prop bag) | Y | - | - | Y | - | - | - | - | - | - | - | - | - | C | - |

Row justifications:

- **main** hosts the whole layout, the TopBar (which is the only caller of
  `get_settings`, `TopBar.svelte:12-20`), the Audio dialog (the only caller of
  `get_audio_state`, `AudioDialog.svelte:15-24`), and the docked panels.
- **panel-room** is `Viewport.svelte`. It draws the screen and the avatar
  sprites (`Viewport.svelte:258-300`), shows notes and notices
  (`Viewport.svelte:319-323`), reports its own viewport
  (`Viewport.svelte:114-115`), reads `store.self` for gather/wear
  (`Viewport.svelte:78`), and opens the Avatar dialog, the Prop bag dialog and
  the Editor dialog from its own toolbar and context menu
  (`Viewport.svelte:239,342,355-360`).
- **panel-users** is `UserList.svelte`: it renders `store.users` and its empty
  state keys off `store.connected` (`UserList.svelte:8,11,19-21`).
- **panel-rooms** is `RoomList.svelte`: it renders `store.rooms`, highlights
  `store.room`, disables entry when disconnected, and binds its filter to the
  per-window `store.roomFilter` (`RoomList.svelte:16-36`).
- **panel-chat** is `ChatPanel.svelte`: it renders `store.chat`, keys the empty
  state off `store.connected`, and `echoLocal` reads `store.settings.username`
  and `store.banner.user_id` (`ChatPanel.svelte:17,28,35,50`; `store.svelte.ts:370-383`).
  It therefore needs settings and banner as well as chat, even though they do
  not ride the chat event.
- **panel-props** is `PropBagDialog.svelte` plus `components/bag/**`. It reads
  `store.bagCatalog`, `store.bagCollections`, `store.bagShelves`, `store.bagTrash`,
  `store.outfits` and `store.self.props` (`PropBagDialog.svelte:41-56,356`), and
  it calls `store.loadBag()` on mount (`PropBagDialog.svelte:308-321`). Wearing
  goes through `store.toggleBagProp` -> `set_props`, which needs `store.self`
  from the `users` event.

### 4.3 The Preferences window is a tool window, not a panel

`prefs` (`src-tauri/src/windows.rs`, `ToolWindow::Preferences`) is a peer view
but deliberately **not** a `Panel`: it is never docked, never marked detached,
never reopened at startup, and its close is a plain discard (Task 19). Because
it is not a `Panel`, it has no column in the §4.2 matrix. What it needs and
where it gets it:

- `prefs` values: the `get_prefs` / `set_prefs` / `reset_prefs` /
  `set_connection_settings` commands (never `refresh`).
- session state: none. It shows settings, not the session.
- its own geometry: remembered like every other tracked window
  (`windows::tracks_geometry` returns true for a tool-window label,
  `windows.rs:191-193`), so move/resize/close are recorded and its saved
  rectangle is re-applied when it is opened again (Task 27). It is still never
  part of the detach order and never reopened by the restore loop.
- route: `#/prefs`; label `prefs`; capability `capabilities/prefs.json` grants
  `core:event:default` and no window create/close/destroy or dialog
  permission (Task 19).

### 4.4 The editor is not a panel

The prop editor (`components/editor/EditorDialog.svelte`) is deliberately not in
the detachable set, per the plan's recon decision. It stays a modal opened from
`Viewport.svelte:239`. Its state is one process-global `EditorSlot`
(`src-tauri/src/editor.rs:502`), so even though the room panel can open the
modal, the underlying session is a single shared document. Two windows editing
at once would fight over one session. That is out of scope and must not be
turned into a detached panel by a later task without revisiting this contract.

---

## 5. The `refresh()` replay (as shipped in `44d76ac`)

`refresh` is an event replay, not a snapshot. The command is
`src-tauri/src/commands.rs:629`; the runtime handles `ClientCommand::Refresh`
and runs the resync block at `crates/palace-client/src/runtime.rs:1148-1178`.
What that block re-emits, in order:

status, banner, rooms, users, room (if any), the **full** 500-line chat
scrollback (`CHAT_REPLAY_LIMIT = CHAT_SCROLLBACK_CAP`,
`runtime.rs:1165`, `state.rs:42`), the retained notices
(`runtime.rs:1168`), the avatar roster (`runtime.rs:1171`), and the last
composed screen.

That closes the two real gaps this section listed when it was written:

1. **Chat scrollback.** The replay window was 120 against a 500 cap; Task 10
   raised it to the full cap (`runtime.rs:67`).
2. **Avatar roster.** The resync block never used to emit `avatars`, so a
   late-opening Room view drew no sprites until the next roster trigger;
   Task 10 re-emits it (`runtime.rs:1171`).

The remaining items were decisions, not gaps, and still hold:

3. **Tooltip is not replayed.** It is hover state
   (`src/lib/store.svelte.ts:91`); dropping it on open is correct.
4. **Settings are not replayed, but every window that reads them fetches
   them.** `ChatPanel.svelte:50` calls `get_settings` itself for its local
   echo, as `TopBar.svelte:13` does in the shell
   (`src-tauri/src/commands.rs`, `get_settings`).
5. **Audio state is not replayed.** `get_audio_state` belongs to the Audio
   dialog (`store.svelte.ts:448`, `AudioDialog.svelte:15`); no detached panel
   owns audio controls.
6. **The bag snapshot is not replayed.** It is fetched with the `bag_*` and
   `outfits_*` commands and gathered by `store.loadBag()`
   (`PropBagDialog.svelte:113,141`), which every mount calls.
7. **The editor session is not replayed.** `EditorDialog` fetches it with
   `editor_state` on mount; `editor_state` returns `null` when no session is
   open (`src-tauri/src/editor.rs`).
8. **The viewport is not replayed and never should be.** Each window measures
   its own DOM and calls `set_viewport` (`Viewport.svelte:132`). The
   `screen.geometry` a window receives is computed for the viewport it last
   reported; the geometry reply is tagged with the owning label and a
   monotonic epoch so a stale reply is ignored (Task 17).
9. **Per-window UI state is not replayed by design.** `scale`, `native`,
   `showNames`, `showAvatars` and `roomFilter` are not synchronized
   (section 2.2). Layout memory (Task 27) persists geometry and detach state,
   and broadcasts changes on `palace://layout`, but the values above stay
   per-window.

### 5.1 Scrollback owner, stated unambiguously

The chat transcript is backend-owned. It lives in `ClientState.chat` and is
capped at 500 lines (`crates/palace-client/src/state.rs:292-306`). The frontend
mirrors it in `store.chat` with the same 500 cap (`src/lib/store.svelte.ts:49`),
and that mirror is per window. The backend is the only authority; a window that
opens late fills its mirror from the replay that `refresh` triggers. No window
may treat its local `store.chat` as the transcript of record.

---

## 6. Lifecycle semantics

**Detach.** The main window asks Rust to open the panel
(`open_panel(panel_id)`, Task 6). The main window swaps the panel's dock slot for
a placeholder. The new window boots a fresh SPA context, subscribes to
`palace://event`, calls `refresh` to seed event-sourced state, and calls
whatever command fetch its panel needs (section 4). From then on it is a
peer view: same connection, same channel, no special case.

**Re-attach.** The main window asks Rust to close the panel
(`close_panel(panel_id)`). The detached window is destroyed. The main window
removes the placeholder and renders the live panel again. Because the backend
held all state the whole time, nothing is lost and nothing needs to be copied
back.

**Titlebar close.** Closing a panel window from its own titlebar is defined as a
re-attach, not a discard. The close is intercepted and turned into the same
path as the Re-attach button. The main window must end up with exactly one
docked panel and no duplicate window (Task 14's edge-case test).

**Spontaneous death — destroy means re-dock.** A panel's webview can die
without any close request (a crashed or killed renderer). `WindowEvent::
Destroyed` classifies that case and re-docks the panel, so `main` never keeps a
ghost placeholder over a window that no longer exists. The signal reuses
`PANEL_CLOSED_EVENT` and logs `reattached … via=destroyed` to distinguish it
from a user close (Task 18, `layout_lifecycle.rs`). The classifier is a pure
function with three outcomes: re-dock (spontaneous death), silent (the app is
quitting), ignore (the close was already handled, so no duplicate signal). A
`main` webview that dies spontaneously runs the same quit path, so panels are
never orphaned over a dead session.

**App quit.** All windows are destroyed, including `prefs`. `main`'s close
request destroys the open panels rather than close-requesting them, so a panel
cannot veto the quit. Geometry and detach state are saved on move, resize and
close (debounced), then restored on next launch with off-screen clamping.
Detached flags are deliberately preserved across a quit, so the layout survives.
Geometry lives in its own file, never in the shared `settings.json`; the path
and format are fixed in section 6.1.

**Apply semantics for options.** This document does not enumerate preference
keys; `PREFERENCES.md` (Task 5) owns the per-key live-versus-reconnect column.
The rule from this contract is: a change that alters the connection parameters
or the live session cannot apply without a reconnect; everything else (appearance,
sound, logging, layout, notifications) applies live. No window may reconnect
independently; a reconnect is a single backend action that every window sees
through the event channel.

### 6.1 Window layout memory (Task 7, implemented in `src-tauri/src/geometry.rs`)

**Where.** `<app-config-dir>/window-layout.json` — on Linux
`~/.config/org.palace.client/window-layout.json`. The `PALACE_LAYOUT_FILE`
environment variable overrides the whole path, which is how the test harnesses
point a run at a scratch file. The resolved path is announced in the diagnostic
log at startup. `settings.json` is never touched by this feature.

**Format.** Schema version 1, keyed by the same window labels the registry uses
(`main`, `panel-users`, …):

```json
{
  "version": 1,
  "order": ["users", "chat"],
  "windows": {
    "main":        { "x": 40, "y": 30, "w": 1200, "h": 820, "monitor": "DP-1", "detached": false },
    "panel-users": { "x": 140, "y": 110, "w": 520, "h": 430, "monitor": "DP-1", "detached": true }
  }
}
```

`x`/`y` are the window's physical screen position (the outer origin where the
platform reports one, otherwise the client origin); `w`/`h` are the physical
**client** size. The client size is deliberate: `set_size` restores that, and
storing the outer size would add the decorations back on every restart. The
`order` array is the detach order Task 18 restores panels in. A file written by
a newer schema is left byte-identical, never overwritten; a malformed file is
replaced on the next write with a warning in the log.

**When.** Move and resize events only mark the in-memory snapshot dirty; a
background thread flushes at most every 600 ms. A window close is a synchronous
capture-and-flush, so the last known rectangle is on disk before the window is
destroyed. Writes are atomic (temp file, then rename) and a write that changes
nothing is a no-op.

**Clamp on restore.** The pure function `geometry::clamp_placement` decides,
before any platform call: a saved rectangle whose monitor still exists is pulled
fully on-screen; one whose monitor is gone is centred on the primary; one with
no recorded monitor goes to the screen that contains it, else the primary; a
rectangle larger than its monitor is shrunk to fit. The placement is applied
immediately and, if the platform has not mapped the window yet, once more from
a short-lived background thread (a platform may drop geometry requests made
before a window is mapped).

**`main` and `set_ui_scale`.** `set_ui_scale` owns `main`'s size
(`BASE_WINDOW × scale`) and its title. The chosen scale is not persisted, so
restoring a remembered size for `main` would produce a window sized for a scale
that no longer applies, with the page at 100% zoom. Layout memory therefore
restores `main`'s **position only**; its size is recorded for diagnostics and
never applied. Panels have no size authority but the user, so their full
rectangle is restored.

**Who sets `detached`.** Only `LayoutStore::set_detached(panel, bool)`, called by
the detach/re-attach actions (Task 18). The window-event path saves geometry but
never changes the flag: a panel closing because the app is quitting must stay
marked detached, or the next launch would not reopen it.

**Remembering can be turned off.** The layout file carries its own `remember`
flag (default `true`). With it off, `LayoutStore::flush` is a no-op, so move,
resize, detach and close cannot touch the file; the one exception is the write
that records the flag itself. A store that reads `remember: false` deliberately
does not load the saved geometry, so the session starts from the default
single-window layout and turning memory back on cannot resurrect a stale detach
flag. This is the Layout memory preference group (Task 27); the flag lives in
this file, never in `settings.json`.

### 6.2 Startup order (measured on `44d76ac`)

This is the real order, observed in a launch log
(`.omo/evidence/task-29-boot.txt`) and traced to `src-tauri/src/lib.rs`. The
plan's prose ("layout restore → window creation → connection") is an
idealisation; the differences are called out below.

1. **Logging first.** `logging::init()` runs before the Tauri builder
   (`lib.rs:472`), so a failure to open the log goes to stderr and everything
   after it is captured.
2. **Process-global slots and catalogs**, before any window exists: prop
   catalog, bag service, editor slot, and settings defaults plus the command
   line (`lib.rs:475-510`).
3. **Window creation precedes `setup`.** Tauri creates the configured `main`
   window when the event loop starts; `.setup` only *logs* it
   (`lib.rs:641-645`). The window therefore exists before the layout file is
   read.
4. **`setup` runs, in this order** (`lib.rs:638-714`): log "palace-app
   starting"; load settings from disk and resolve defaults/saved/args; ensure
   an identity exists (persisting a generated one); log the target and spawn
   the audio engine; `manage(AppState)`; read preferences and configure the
   chat-log transcript and notification services **before the pump starts**, so
   the session's first line is written; `LayoutStore::discover` + manage;
   `spawn_autosave`; `geometry::schedule_restore`; `start_client` (pump +
   connection); log "client runtime started".
5. **The layout restore runs inline, between `schedule_restore` and the
   "client runtime started" line.** `schedule_restore` asks
   `AppHandle::run_on_main_thread` to run it "after `setup` returns"
   (`geometry.rs:788`), but `tauri-runtime-wry` executes that message
   immediately when the caller is already the main thread
   (`send_user_message`, `tauri-runtime-wry 2.x`, `src/lib.rs:239`), and
   `.setup` is on the main thread. So the restore — `main`'s remembered
   position, then each detached panel reopened in saved order at its saved
   rectangle (`geometry.rs:736-782`) — completes **before** `start_client` is
   called. Nothing deadlocks; it is simply synchronous here.
6. **Windows that were not mapped yet get a deferred second apply.** Immediately
   after the placement, `apply_when_mapped` starts a short-lived background
   thread that re-applies it once the window reports a real size
   (`geometry.rs:699-727`).
7. **Each window seeds itself once.** Every webview (the shell and every panel)
   subscribes to `palace://event` and calls `refresh` exactly once on mount
   (`src/lib/session.ts`, `PanelWindow.svelte`); the command logs
   `refresh_requested epoch=N` per window (`commands.rs:629-632`).
8. **`on_window_event(handle_window_event)` is attached before `.run()`**
   (`lib.rs:716`), so move/resize/close/destroy from the very first frame are
   seen by the geometry and lifecycle paths.

One practical consequence: because the restore is synchronous with `setup`, a
slow monitor query or a stuck window-placement call would delay `start_client`.
The order above is what a launch log proves; it is not a claim that the phases
are concurrent.

**Route note.** Panel and tool-window labels load `#/panel/<id>` and
`#/prefs`. The route must be a bare fragment: Tauri only treats the literal
`index.html` as the root document, so `index.html#/panel/<id>` made the
document's path `/index.html`, and SvelteKit's router rendered its own 404 page
instead of the panel (found and fixed in Task 29, `windows.rs`).

---

## 7. Forbidden designs

- No second connection. There is one `AppState.client` and one socket.
- No per-window authoritative state. A window is a view; the backend is the
  source of truth.
- No window-created event channel. There is one `palace://event`; the backend's
  change-notifications (§2.3) are emitted by Rust, never by a window.
- No detached panel that renders the full layout, and no full `main` layout
  inside a panel window.
- No sharing of UI-only state (scroll offset, input draft, selected tab, room
  filter) across windows.
- No second editor session. `EditorSlot` holds one session and the editor is
  not a panel.

---

## Appendix: verification notes

The matrix was built by reading, not by assuming. Task 29 re-checked the claims
that Tasks 13-28 changed and corrected the ones that no longer held; the two
entries marked "(Task 29)" are the corrected ones.

- The event union and the `refresh` / `onEvent` wrappers were read at
  `src/lib/api.ts:150-172`, `:475`, `:529`.
- The exact replay set came from the resync block at
  `crates/palace-client/src/runtime.rs:1148-1178`, the transcript cap from
  `crates/palace-client/src/state.rs:42`, and the replay count from
  `CHAT_REPLAY_LIMIT` (`runtime.rs:67`). (Task 29: the block now also replays
  notices and the avatar roster, and sends the full 500-line scrollback.)
- The seed pattern was read in `src/lib/session.ts` and `PanelWindow.svelte`;
  the old `App.svelte:60-80` citation is superseded.
- ~~The avatar-roster gap was confirmed…~~ (Task 29: closed — `emit_avatar_roster`
  runs inside the resync block at `runtime.rs:1171`.)
- Bag and editor state were traced from
  `PropBagDialog.svelte:113,141` -> `store.loadBag()` -> `bag_*` commands, and
  from `EditorDialog.svelte` -> `api.editorState()` -> `editor_state`
  (`src-tauri/src/editor.rs`).
- Settings and audio fetch sites were traced to `TopBar.svelte:13`,
  `ChatPanel.svelte:50` and `store.svelte.ts:448`.
- The startup order was checked against a real launch log; see
  `.omo/evidence/task-29-boot.txt` for the raw lines and the step mapping.

# Windows Architecture: the multi-window state contract

Status: Task 2 deliverable, written 2026-09-19 against commit `1dd56b5` plus the
uncommitted prop bag and prop editor work.
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
(`src-tauri/src/lib.rs:176`, `app.emit`). Every window listens to the same
channel. A detached window must never call `connect`, must never own a socket,
and must never publish its own event channel.

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

### 4.3 The editor is not a panel

The prop editor (`components/editor/EditorDialog.svelte`) is deliberately not in
the detachable set, per the plan's recon decision. It stays a modal opened from
`Viewport.svelte:239`. Its state is one process-global `EditorSlot`
(`src-tauri/src/editor.rs:502`), so even though the room panel can open the
modal, the underlying session is a single shared document. Two windows editing
at once would fight over one session. That is out of scope and must not be
turned into a detached panel by a later task without revisiting this contract.

---

## 5. The `refresh()` gap list (this is Task 10's specification)

`refresh` is an event replay, not a snapshot. The command is
`src-tauri/src/commands.rs:562-564`; the runtime handles `ClientCommand::Refresh`
at `crates/palace-client/src/runtime.rs:1072` and runs the resync block at
`crates/palace-client/src/runtime.rs:1117-1144`. What that block re-emits is:
status, banner, rooms, users, room (if any), the most recent 120 chat lines, and
the last composed screen. Everything below is what a late-opening window is
still missing.

1. **Chat scrollback: 120 replayed against a 500 cap.** The replay window is
   120 (`crates/palace-client/src/runtime.rs:1134`); the frontend holds up to 500
   (`CHAT_LIMIT`, `src/lib/store.svelte.ts:49`) and the backend transcript is
   capped at 500 (`crates/palace-client/src/state.rs:302`). A detached Chat
   window can open showing at most 120 prior lines. Task 10 must raise the
   replay window to at least 500 or record a deliberate smaller justification.
2. **Avatar roster is not replayed.** The resync block never emits `avatars`.
   The roster is published only at the roster trigger sites
   (`crates/palace-client/src/runtime.rs:1008-1753`, definition at `:2616-2623`). A late
   Room view therefore gets the room frame and geometry from `screen`, but its
   sprite layer (`Viewport.svelte:267-280`) stays empty until the next roster
   trigger (someone moves, enters, changes props or face). This is a real gap
   the plan's PRE-START note did not name. Task 10 should re-emit the roster
   during resync, or the room panel will look wrong on open in the normal
   sprite-drawing mode.
3. **Notices are not replayed.** `notices` fills from `note` and `script`
   events (`src/lib/store.svelte.ts:339-351`), which resync never re-sends. A
   detached Room view opens with no notices until new ones fire. Acceptable as
   transient, but the decision must be explicit.
4. **Tooltip is not replayed.** It is hover state (`src/lib/store.svelte.ts:63`,
   `crates/palace-client/src/runtime.rs:1488`). Dropping it on open is correct.
5. **Settings are not replayed.** Only `TopBar.svelte:12-20` calls
   `get_settings`. Any detached panel that reads `store.settings` (notably the
   Chat panel for its local echo name, and the Status bar if a panel shows it)
   must call `get_settings` itself on mount.
6. **Audio state is not replayed.** Only `AudioDialog.svelte:15-24` calls
   `get_audio_state`. No detached panel owns audio controls, so no action unless
   that changes.
7. **The bag snapshot is not replayed.** It is fetched with the `bag_*` and
   `outfits_*` commands and gathered by `store.loadBag()`
   (`src/lib/store.svelte.ts:193-213`). A detached Prop bag panel must call
   `loadBag()` on mount exactly as the modal does today, or it opens empty.
8. **The editor session is not replayed.** `EditorDialog` fetches it with
   `editor_state` on mount (`EditorDialog.svelte:741-756`), and `editor_state`
   returns `null` when no session is open (`src-tauri/src/editor.rs:1994`).
9. **The viewport is not replayed and never should be.** Each window measures
   its own DOM and calls `set_viewport` (`Viewport.svelte:114-115`). The
   `screen.geometry` a window receives is computed for the viewport it last
   reported. A detached Room window must report its own size before trusting
   geometry; this is the cross-talk risk Task 17 owns.
10. **Per-window UI state is not replayed by design.** `scale`, `native`,
    `showNames`, `showAvatars` and `roomFilter` are not persisted and not
    synchronized (section 2.2). Only the Layout memory group (Task 27) will
    persist geometry and detach state, and even that is not shared live.

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
docked panel and no duplicate window. Task 6 wires the signal; Task 14 has the
edge-case test.

**App quit.** All windows close. Geometry and detach state are saved on
move, resize and close (debounced), then restored on next launch with off-screen
clamping. Geometry lives in its own file, never in the shared `settings.json`;
Task 7 fixes the exact path and format and documents it here when implemented.

**Apply semantics for options.** This document does not enumerate preference
keys; `PREFERENCES.md` (Task 5) owns the per-key live-versus-reconnect column.
The rule from this contract is: a change that alters the connection parameters
or the live session cannot apply without a reconnect; everything else (appearance,
sound, logging, layout, notifications) applies live. No window may reconnect
independently; a reconnect is a single backend action that every window sees
through the event channel.

---

## 7. Forbidden designs

- No second connection. There is one `AppState.client` and one socket.
- No per-window authoritative state. A window is a view; the backend is the
  source of truth.
- No window-created event channel. There is one `palace://event`.
- No detached panel that renders the full layout, and no full `main` layout
  inside a panel window.
- No sharing of UI-only state (scroll offset, input draft, selected tab, room
  filter) across windows.
- No second editor session. `EditorSlot` holds one session and the editor is
  not a panel.

---

## Appendix: verification notes

The matrix was built by reading, not by assuming:

- The event union and the `refresh` / `onEvent` wrappers were read at
  `src/lib/api.ts:150-172`, `:475`, `:529`.
- The exact replay set came from the resync block at
  `crates/palace-client/src/runtime.rs:1117-1144`, and the transcript cap from
  `crates/palace-client/src/state.rs:292-306`.
- The seed pattern was read in `App.svelte:60-80`.
- The avatar-roster gap was confirmed by checking every `emit_avatar_roster`
  call site (`crates/palace-client/src/runtime.rs:1008,1021,1028,1040,1046,1296,1427,1440,1483,1497,1753`)
  against the resync block: none is inside it.
- Bag and editor state were traced from
  `PropBagDialog.svelte:308-321` -> `store.loadBag()` (`store.svelte.ts:193-213`)
  -> `bag_*` commands (`api.ts:378-448`), and from
  `EditorDialog.svelte:741-756` -> `api.editorState()` -> `editor_state`
  (`editor.rs:1994`).
- Settings and audio fetch sites were traced to `TopBar.svelte:12-20` and
  `AudioDialog.svelte:15-24`.

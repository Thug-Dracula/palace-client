# Status / handoff — Palace Tauri client

**Last updated:** 2026-09-16 (interactive client milestone)
**Scope, decisions and design:** `$CORPUS/TAURI-CLIENT-SCOPE.md` ← read that too. It is the design authority; this file is only "where things stand right now". (Left untouched per the read-only rule for `$CORPUS/`.)

This file exists so a **fresh session** can resume without carrying a long conversation.

---

## Update 2026-09-17 (late) — read this before the sections below

**Everything below predates this.** Those sections describe the 2026-09-16 interactive-client
milestone; the day since changed the client materially, and the live record of that work is
`palace-todo.md` §2.11–§2.24. Summary, so a fresh session is not misled by the older text:

**Shipped, tested and published:**
- `LOADSCRIPT`/`HTTPGET` fetch-and-execute (`b62de07`): the response *is* a script when its
  content-type says so, and `HTTPRECEIVED` fires at the fetching hotspot. Together with the ~40
  previously unreachable command names now registered (`af6035f`), rooms can load their own logic.
- `ISLOCKED` answers from the room (`7802e8e`): the id must name a hotspot that is a shuttable (2) or
  lockable (3) door in state 1. Mutation-verified — note the pre-existing test had passed for the
  wrong reason, because the stub answered `false` to everything.
- `CLEARLOOSEPROPS` is no longer claimed as a wire effect (`b8b2c21`): it was the only effect claiming
  wire-backing with no encoder, and the protocol has only a client-side event for it.
- Props: bag reader (`e059f33`), `USER_PROP`/`ASSET_REGI` encoders (`2c780b3`), wearing reaching the
  server (`8837789`). `IPTVERSION` now reports 2 (`81180db`).
- Tracing to a file via `PALACE_TRACE` (`ea295a4`); `DRAW` decoded and rasterized (`ddbf9e7`,
  `f682ca5`); the click-pointer fix that stopped room ejections (`cbf7608`).
- Release **`v0.2.0-alpha.2`** is Latest (AppImage + both Windows artifacts) and **verified to contain
  the two fixes above**. `v0.1.0` and `v0.2.0-alpha` are older snapshots.

**The hard blocker is unchanged:** authentication is unimplemented (tracker §2.1) and needs a
**decision** about where a credential lives before any code is written.

**Three premises that were wrong — recorded so they are not acted on:**
1. `SETPICLOCLOCAL` is *not* missing; it is dispatched at `host.rs:423`.
2. `LOADPROPS` pushes nothing **by design** — a prefetch with a 500-id limit. It is not a broken no-op.
3. A release is a snapshot: `v0.2.0-alpha` did *not* contain that day's fixes, which is why
   `v0.2.0-alpha.2` was cut. Never assume "Latest" includes work committed after it.

**Verification lessons, all of which cost real time:**
- **Local clippy is weaker than CI** unless the toolchains match. CI tracks rolling stable and was on
  1.98 while this machine was on 1.95, so `unneeded_wildcard_pattern` passed locally and failed the
  release. Now on 1.98.1, verified clean across every crate except `palace-app`.
- **A test that derives its expectation from the table it checks cannot catch an error in that table.**
  The wire-set test asserted "exactly twenty" and counted a bad flag, locking the mistake in.
- **Check the cheapest thing first.** Three completion claims in this session were contradicted by one
  command — run afterwards instead of before.

**Lost, and must be re-derived:** two 36-minute research passes (the hit-test rule and its blast
radius; the constant-stub semantics). Their outputs were cleaned up and subagent access is refused, so
go to `$CORPUS/reference/repos/sparky/index.js` and OpenPalace's spot class directly.

**State when this was written:** tree clean; `cargo test -p palace-host` green; clippy 1.98.1 clean
across every non-Tauri crate; public source scrub-verified at 132 commits; `v0.2.0-alpha.2` released.

---

## HARD BOUNDARIES — agents must not cross these

These exist because a subagent, trying to satisfy a "watch the window resize" instruction, went hunting for desktop input-injection tooling and reached for **`input-tool` (kernel-level mouse injection) on a live desktop**. It abandoned the path after ~11s and used the app's own command path instead — but it should never have been available to it.

**Note:**
- **Inject input into the user's desktop.** No `input-tool`, `input-tool`, `input-tool mousemove/click/key`, `xte`, or any synthetic input. Not to click a button, not to prove a UI works. This is a machine the user is actively using.
- **Unlock, wake, or inhibit the user's screen.** A locked screen ends the QA run — stop and report.
- **Screenshot the whole desktop.** If a capture is needed, grab the app window only (`import -window <id>`). Never full-screen `spectacle -b -n` — those captures contain the user's private windows.
- **Activate, raise or focus the user's windows** (`wmctrl -a`, KWin D-Bus window scripting).
- **Ask for or use the user's password**, or any credential.

**DO INSTEAD, for UI verification:**
- Drive the app's own command/state path (`invoke("set_viewport", …)`) — the identical code the button calls. This is the honest verification anyway.
- Verify numerically (geometry readouts, transform values) and via headless renders.
- Say plainly in the report that a human still needs to click the real control once, rather than manufacturing a click.

**Root cause to remember:** the instruction that triggered this was a "definition of done" that demanded *watching* the GUI resize. **Never write a definition of done that can only be satisfied by driving the user's desktop.** Specify the command path instead.

---

## Where the code is

**Everything described below is on `master`.** The only worktree is `$REPO`: `palace-client-ui` and `palace-client-ipt` were removed once their branches (`feat/palace-ui`, `feat/events`) were verified contained in master. The branches are kept as a safety net, so nothing is unrecoverable.

```bash
git worktree list              # $REPO [master]
git branch --no-merged master  # empty — every branch is in master
```

### Running the app

```bash
cd $REPO
bun install          # first time only
bun run dev          # Vite on :1420 — REQUIRED for a debug build
cargo run -p palace-app
```

**Gotcha that cost real time:** a debug `cargo build`/`cargo run` loads `devUrl` (`http://localhost:1420`) from `tauri.conf.json`, **not** the bundled frontend. Without Vite running you get a WebKit page reading `Could not connect to localhost: Connection refused` — which looks like a WebKitGTK failure but is not. For a standalone binary use `cargo build --release -p palace-app` or `bun run tauri build`, which use `frontendDist`.

**Trap:** never `pkill -f "<pattern>"` where the pattern appears in your own command string — it kills the shell running it (documented in `~/AGENTS.md`; it killed a tmux server here). Use `pgrep -x`/`pkill -x` or kill by PID.

**621 tests passing, clippy clean, `cargo check` clean.** The frontend's `svelte-check` was clean when last run in the `palace-client-ui` worktree; that worktree is gone, so run `bun install && bun run check` here to re-confirm it.

### The eight original crates (unchanged contracts)

| Crate | State | Evidence |
|---|---|---|
| `palace-wire` | ✅ done | framing, `ByteOrder`, 75-opcode table, codecs. **`navr_frame` fixed** (see below). |
| `palace-probe` | ✅ done | live logon; 81 rooms / 2 users. |
| `palace-room` | ✅ done | 804/804 corpus records parse clean. |
| `palace-prop` | ✅ done | 5 decoders + S20 encoder; 227,874-prop differential = 0 disagreements (7 rejections, the same props the reference rejects). |
| `palace-asset` | ✅ done | `qAst`/`sAst`/`rAst`, paced scheduler, media HTTP. |
| `palace-render` | ✅ done | composites rooms → PNG; coordinate mapping round-trip tested. |
| `iptscrae` | ✅ done | lexer (718 lines), parser, VM, stack, budget enforcement, capability trait, regex engine; 144 tests (plus 1 doctest). **Zero dependencies.** |
| `iptscrae-palace` | ✅ done | 497-line host trait, 371 lines of Palace commands, CLI, corpus harness; 33 tests (plus 1 doctest). Corpus: **2396/2400 files parse · 3791/3805 handlers clean · 0 tokenizer gaps.** |

### What this milestone added

| Path | What |
|---|---|
| `crates/palace-client/` | **Headless runtime.** Connection supervisor thread (socket + `palace-asset` pipeline + `SceneBuilder`), a media HTTP worker thread, session state machine, frame store. Emits `ClientEvent`s; no Tauri types. |
| `src-tauri/` (`palace-app`) | **Tauri v2 shell.** Commands, the `palace://` URI scheme that serves the frame as PNG, and a pump that forwards runtime events to the webview. |
| `src/` | **SvelteKit 5 frontend** (Vite, adapter-static, Bun). Room viewport, room list, user list, chat, status bar, connect form. |
| `crates/palace-client/src/bin/live-smoke.rs` | Live smoke harness (`cargo run -p palace-client --bin live-smoke`). Also sweeps viewports/zoom/1:1. |

---

## How the pieces fit

```text
TcpStream ─► Connection (palace-wire framing)
                │  frames
                ├─► SessionState::apply ──► ClientEvent (status/banner/rooms/users/chat/screen)
                ├─► AssetPipeline (palace-asset) ──► prop blobs ──► SceneBuilder.props_mut()
                └─► media worker thread (blocking ureq) ──► files ──► SceneBuilder.media_mut()
                                        │
                                   compose(): SceneBuilder.build_with → render() → Canvas::to_png_bytes()
                                        │
                                   FrameStore (Arc<Mutex<latest PNG + version>>)
                                        │
             Tauri: palace://localhost/frame?v=N ──► <img> in the webview (one bitmap)
```

* **Frame delivery**: `register_asynchronous_uri_scheme_protocol("palace", …)`,
  answered on a worker thread, `Content-Type: image/png`, `Cache-Control: no-store`.
  The frontend points an `<img>` at `palace://localhost/frame?v=<version>`.
  **No `Vec<u8>` ever crosses a Tauri command** — commands only carry small JSON.
* **Threading**: the connection/render loop and the media fetch are plain blocking
  threads; the event pump is a `tauri::async_runtime` task. No command handler
  blocks the UI thread.
* **Presentation**: the frontend never computes a scale. `ViewGeometry` (built from
  `palace-render`'s tested `ViewTransform`) reports `content_x/y/w/h`, and the
  frontend applies it with `image-rendering: pixelated`. Resizing reports the new
  viewport via `set_viewport`; a self-healing effect re-reports if the runtime's
  geometry disagrees with the measured element.

### palace-render extensions (additive, tested)

* `Canvas::to_png_bytes()` — in-memory twin of `write_png`, for the URI scheme.
* `PropStore::insert_blob(id, blob)` — insert a prop received over the wire.
* `MediaStore::insert_path(name, path)` — register a file fetched after indexing.
* `SceneBuilder::media_mut()` / `props_mut()` — let a live client fill the stores.
* `PropBackend::Memory` — an in-memory prop source.

All additions; no existing behaviour changed. The six Palace protocol and render crates (`palace-wire`, `palace-probe`, `palace-room`, `palace-prop`, `palace-asset`, `palace-render`) now total 373 tests, excluding doctests and ignored, and all pass.

---

## Bug found and fixed: `palace-wire::navr_frame`

`navr_frame` built a **6-byte body** `[u32 2][u16 room_id]`, putting the length
constant *inside* the payload. Both reference encoders write that `2` as the frame
**length field**:

* `$CORPUS/tools/palace_walker.py::make_navr` →
  `struct.pack("<III", GOTO_ROOM, 2, refid) + struct.pack("<H", room_id)`
* OpenPalace `actuallyGotoRoom` →
  `writeInt(GOTO_ROOM); writeInt(2); writeInt(id); writeShort(roomId)`

The old form made the server read destination room **2**, so navigation silently
did nothing (observed live: the room never changed). Fixed to a 2-byte payload;
`crates/palace-wire/tests/navr_encoding.rs` pins the exact wire bytes. Room
switching now works live (`901 Balamb Garden → 817 Balamb Hotel`).

## Gap filled: `xtlk` / `xwis` decryption

`palace-wire` decodes plaintext `talk`/`whis` but has **no** cipher for the
encrypted variants, so they degraded to `Message::Unknown`. The runtime now
implements the Park–Miller Lehmer keystream (seed `0xa2c2a`, 512-byte table,
reverse iteration, two table bytes per char) in `crates/palace-client/src/xtlk.rs`,
ported from `QPalace`'s `QPCodec` and OpenPalace's `PalaceEncryption`. The LUT
prefix is asserted against independently computed values and the round trip is
tested. Outgoing chat still uses plaintext `talk` (the server relays it).

---

## What is verified, and how

**Hermetic (no network), `cargo test` — 621 pass** (run it in the main worktree; see the `--workspace` caution under operational rules):

* `crates/palace-client/tests/fixture_replay.rs` replays every server frame of
  `fixtures/logon-run1/` through `SessionState` and asserts 81 rooms, Balamb
  Garden #901, `sqoom23.gif`, 6 hotspots, 1 overlay, server v1.22, media base.
* `xtlk` round-trip + LUT prefix + payload framing; synthetic plaintext/encrypted
  talk, ping→pong, malformed-body tolerance, unknown-opcode silence.
* `ViewGeometry::compute` across fit/1:1/zoom/non-4:3/DPR-clamp.
* `navr` wire bytes; `Settings::with_args` CLI parsing.

**Live (against `localhost:9998`, Balamb Garden), via the app and via
`live-smoke`:**

* Window shows a real composited room; media fetched live over HTTPS from the
  server's advertised media base (`sqoom23.gif`, `notebar.gif`, then `balent.gif`,
  `bent1..3.gif` for room 817).
* Room switching works (`goto_room`), room list populates (81 rooms with real
  names), user list shows the logged-on user with a face colour.
* Chat round-trips: `live-smoke ping` came back as a `talk` from the server.
* Viewport sweep (through the same `set_viewport` command the UI uses):

  | vp | dpr | zoom | mode | scale | content | offset | buffer |
  |---|---|---|---|---|---|---|---|
  | 960×540 | 1 | 1.0 | fit | 1.4062 | 720×540 | (120,0) | 512×384 |
  | 960×540 | 1 | 0.5 | fit | 0.7031 | 360×270 | (300,135) | 512×384 |
  | 960×540 | 1 | 3.0 | fit | 4.2188 | 2160×1620 | (−600,−540) | 512×384 |
  | 960×540 | 1 | — | **1:1** | **1.0000** | 512×384 | (224,78) | 512×384 |
  | 640×480 | 2 | 1.0 | fit | 1.2500 | 640×480 | (0,0) | **1024×768** |
  | 1920×1080 | 2 | 2.0 | fit | 5.6250 | 2880×2160 | (−480,−540) | 1024×768 |

  Every value matches the independently tested `ViewTransform`.

* Visual: screenshots at `/tmp/work/palace-crop2.png` (Balamb Garden live in
  the window, room list 81/81, "Member 21 (you)", status bar
  `connected · Balamb Garden · v1.22 · endian little · frame #4`) and
  `/tmp/work/palace-room901.png`, `palace-room817.png` (headless composites).

## Not verified / rough edges

* **The last visual pass (zoom/1:1/resize screenshots) was done through the
  runtime command path, not a mouse drag.** The session's screen locked from
  inactivity partway through QA and unlocking needs the user's password, so the
  final zoom/1:1/resize evidence is the numeric sweep above plus unit tests.
  Everything is wired to the same `set_viewport` command, but a human should
  still click the slider once.
* **Avatar drawing exists; avatar *art transfer* is the untested part.** The
  composite pipeline draws avatars y-ordered (`draw_avatar`, layer 6), and
  `renders/1672-ludo-ii-props-avatars.png` shows three. What is unproven live is
  art for other users: on Balamb Garden the logged-on user arrives with
  `props=0`, so there is nothing to draw, and the runtime hides avatars whose
  prop art has not arrived rather than filling the room with magenta
  placeholders. That needs another user online, which QA never had.
* **Server churn during QA.** Repeated connections from one host in quick
  succession made the server send `bye` (len 4, ref = the *previous* session's
  user id, i.e. a server-side logoff notice) and close. The client
  auto-reconnects with backoff and now **restores the room the user was in**
  (`Shared::last_room`). A single normal session is unaffected; the first smoke
  runs ran 22 s cleanly.
* **Still out of scope:** sound and MIDI playback (`SOUND`, `MIDIPLAY`,
  `MIDILOOP`, `MIDISTOP` and `BEEP` are dispatched and reported, but nothing is
  played), `LINE` / `LINETO` rasterization (the effects are dispatched and
  dropped), deco, a prop editor UI, web panes (`WEBEMBED` is not registered, and
  `GOTOURL` is reported rather than opened), and theming. IPTSCRAE dispatch and
  hotspot clicks are no longer on this list; see the dispatch section below.
* `Message::Logoff` still prints a diagnostic transcript line rather than being
  interpreted; see the note above.
* Name tags and chat text are not rasterized into the frame (presentation-layer
  font work, as the renderer documents).

## Known pre-existing gaps (carried over)

* Multi-block asset transfer: reference-derived only, no real capture exists.
* 16-bit props: zero real samples.
* All 155 loose-prop IDs in the local corpus are missing from local stores.
* `pserver_full.prp` is systematically corrupt.
* Draw commands / name tags / chat text are not rasterized.
* Avatar anchor offset `x−22, y−22`; one reference uses −21 (1 px ambiguity).

## Event dispatch readiness (measured 2026-09-16)

Measured against the 2,400-script corpus, not assumed.

**Events actually used** — 24 distinct, by frequency:

| Event | n | Event | n |
|---|---|---|---|
| `ENTER` | 1956 | `LOCK` / `UNLOCK` | 6 / 6 |
| `SELECT` | 909 | `HTTPRECEIVED` | 5 |
| `LEAVE` | 457 | `MOUSEUP` / `MOUSEDRAG` | 3 / 3 |
| `OUTCHAT` | 237 | `HTTPERROR` | 3 |
| `ALARM` | 73 | `USERLEAVE` | 3 |
| `INCHAT` | 64 | `STATECHANGE` | 3 |
| `ROLLOVER` / `ROLLOUT` | 21 / 14 | `SIGNON` | 3 |
| `ROOMREADY` | 10 | `HTTPRECIEVED` (sic) | 1 |
| `NAMECHANGE` / `ROOMLOAD` | 9 / 9 | `MOUSEMOVE` | 1 |
| `KEYDOWN` | 8 | | |
| `SERVERMSG` | 7 | | |

`ENTER` + `SELECT` + `LEAVE` + `OUTCHAT` + `ALARM` + `INCHAT` alone are ~92% of all handler invocations.

**Command coverage: 100%.** All 102 distinct commands the corpus uses — 391,129 occurrences — are already implemented. The registry holds **192 names**: 72 core in `iptscrae/src/registry.rs` + 120 Palace bindings in `iptscrae-palace/src/commands.rs`. **No command implementation work remains for dispatch.**

**So event dispatch was a wiring job, not an implementation job** — which is what the next section did. The script text was already extracted and validated (`crates/palace-room/tests/corpus_scripts.rs`, 2400/2400), the `ON`-block splitter and VM already existed, and every command was registered. What was missing was the mapping from the 24 event names above onto runtime events, and the host.

## Event dispatch (merged from `feat/events`)

`crates/palace-host/` is the dispatch layer:

| Module | Role |
|---|---|
| `load` | splits the `ON <NAME> { … }` blocks out of each hotspot's script text and reports the ones that fail |
| `engine::ScriptEngine` | fires `ScriptEvent`s, keeps globals across handlers, runs `SETALARM` / `ALARMEXEC` timers |
| `host::ScriptHost` | implements `PalaceHost` over a `view::HostView` snapshot, recording every effect; it never touches the socket |
| `wire` | encodes recorded effects as protocol frames |

`fire()` returns a `DispatchReport` carrying `fired`, per-run `runs` (each with its
error), `effects`, and `chat_string` — the `CHATSTR` value after the run, which is how
a script rewrites or clears an `ON INCHAT` / `ON OUTCHAT` line. Dispatch is wired into
the live runtime, and `ClientEvent::Script` reports what fired to the UI.

**Untrusted scripts have no ambient authority:** the host cannot open a socket, read a
file or spawn a process, and every effect is a value the runtime may refuse. The VM
budgets still apply.

The runtime-event mapping the layer answers:

| Runtime event | Script event |
|---|---|
| `RoomEntered` | `ENTER` |
| `Chat` where the speaker is self | `OUTCHAT` |
| `Chat` otherwise (`Talk` / `Whisper`) | `INCHAT` |
| `Chat` of kind `System` / `Error` | `SERVERMSG` |
| `Status::Connected` | `SIGNON` |

### Correction: "100% command coverage" ≠ "the host implements it"

Coverage above means every command *lexes and is registered*. `PalaceHost` **refuses**
a command it does not implement rather than ignoring it. Running the corpus against a
two-command host (`chat`, `goto_room`) turned **1264 of 1951** `ON ENTER` handlers into
refusals — which is how the host's remaining debt got measured. **That debt is now
closed**: `ScriptHost` implements the surface, including every command in the list
below. It was measured from `ON ENTER` only, so treat it as a lower bound and as a
record of the method rather than an open to-do.

| Command | refusals | | Command | refusals |
|---|---|---|---|---|
| `PAINTCLEAR` | 331 | | `GETSPOTSTATE` | 16 |
| `ALARMEXEC` | 308 | | `NBRROOMUSERS` | 15 |
| `ME` | 151 | | `CLEARLOOSEPROPS` | 10 |
| `ISGOD` | 118 | | `SETPOS` | 9 |
| `SOUND` | 92 | | `SETSPOTSTATELOCAL` | 8 |
| `LOCALMSG` | 63 | | `STATUSMSG` | 6 |
| `DIMROOM` | 45 | | `MIDILOOP` | 4 |
| `SETPROPS` | 42 | | `OPENPALACE` / `PENCOLOR` | 1 / 1 |
| `ROOMID` | 23 | | | |
| `MIDISTOP` | 20 | | | |

### Process lesson: inspect an aborted agent's worktree before writing its work off

This layer was built on `feat/events` and then **stranded** when the agent producing it
was aborted and its result was recorded as "aborted, no task_id, do not resume". It sat
unmerged in the `palace-client-ipt` worktree while a smaller duplicate was written by
hand on master — and that worktree was nearly deleted as "leftover cleanup".

Three commands would have caught it immediately:

```bash
git worktree list                  # which branch each worktree is actually on
git branch --merged master         # what is NOT in master
git log --oneline master..<branch> # the stranded commits
```

An aborted agent may have committed real work before dying. "No task_id" means the
*session* cannot be resumed — it does **not** mean nothing was produced.

### Measurement caution — two earlier attempts at this metric were wrong

1. Counting every uppercase token as a command reported **43.3% coverage**. Wrong: it counted `CHATSTR` (a special *variable*), the `ON`-block *event names*, and user variables such as `DT`, `HP`, `X2`, `S1`, `SR`.
2. Comparing only against `iptscrae-palace/src/commands.rs` reported **54.5% missing**. Wrong: it missed the entire 72-command core registry in `iptscrae/src/registry.rs` — `GLOBAL`, `IF`, `IFELSE`, `AND`, `NOT`, `EXEC`, `WHILE`, `DUP` — which is where most of the "missing" commands actually live.

The correct method: use the reference registry (OpenPalace `IptDefaultCommands.as` + `PalaceIptscraeCommands.as`) as the command vocabulary, then compare against the **union** of both Rust registries. Any future coverage claim must do the same.

## The effect-application gap (found 2026-09-16, **closed** 2026-09-16)

`apply_effect()` in `crates/palace-client/src/runtime.rs` used to end with a
catch-all, `other => vec![ClientEvent::Note { … }]`, so **any `Effect` with no
explicit match arm was logged and nothing else**: the transcript said "script: X"
and the room did not change.

All 16 missing arms are now written and **the catch-all is gone**, so the match is
exhaustive. Deleting a single arm now fails the build with `E0004` — verified by
deleting the `LOCK` arm and reading the error. That is what stops a new `Effect`
from silently doing nothing.

| Variants | Now |
|---|---|
| `AddLooseProp`, `RemoveLooseProp`, `MoveLooseProp`, `ClearLooseProps`, `DropProp` | Applied to `room_desc.loose_props`, which `build.rs` draws |
| `SetFace`, `SetColor` | Applied to `users[self].face` / `.color`, which the compositor draws |
| `DonProp`, `DoffProp`, `RemoveProp` | Applied to the signed-in user's worn-prop list |
| `SetPenColor`, `SetPenSize`, `MovePen`, `PaintLayer` | Reported: the pen is `ScriptHost`'s, and nothing rasterizes a stroke yet |
| `Lock`, `Unlock` | Reported: the frame goes on the wire, and nothing local consults lock state |
| `SetSpotAlarm` | Reported: `ScriptHost` records the alarm and the engine runs it |

### The double defect: seven arms that applied nothing (**closed**)

The table that used to sit here counted *missing arms*, which was too kind a
metric. Seven variants already had an arm that set `*dirty_render = true` and
emitted a note but **mutated no state at all** — so they forced a re-render that
was guaranteed byte-identical. An inert arm is indistinguishable from a working
one inside a `match`, which is how this stayed hidden. All seven now mutate what
the renderer already reads:

| Arm | Now |
|---|---|
| `MoveSpot`, `MoveSpotLocal` (`SETLOC`) | replace `hotspot.loc` |
| `SetPicOffset`, `SetPicOffsetLocal` (`SETPICLOC`) | replace `states[].pic_loc`, the current state's when none is named |
| `SetPicOpacity` | `SessionState.pic_opacity` → `SceneBuilder::set_pic_opacity` → `Sprite.alpha` |
| `HideAvatars`, `ShowAvatars` | `SessionState.avatars_hidden`, which `compose` consults |

All four local overrides reset when a room descriptor arrives, exactly as
`room_dim` does, and `PalaceClient.as` clears its own room overrides the same way.

**The semantics are verified, not guessed.** `SETLOC` and `SETPICLOC` take
**absolute** values, not deltas. `MSG_SPOTMOVE`/`MSG_PICTMOVE` carry a *new
position* with no delta field, ThePalacev0's server assigns it (`spot.loc = pos`),
and OpenPalace carries a commit titled "Making SETLOCLOCAL use absolute positions
instead of relative". Counter-evidence worth knowing: the original IPTSCRAE
manual's prose says `SETLOC` moves a spot "relative to its current position", while
its own summary table says "Moves spotID to x,y" — the summary and the protocol are
right. For `SETPICLOC` the manual's worked example settles it: `-50 -50 100
SETPICLOC` followed by `54 -21 100 SETPICLOC` restores the original offset, which
only works if the value replaces rather than adds. `SETPICOPACITY` is an OpenPalace
extension absent from the original guide, with `alpha = percent / 100`.

**Still open: `SETLOC`/`SETPICLOC` never reach the server.** The non-local variants
should broadcast so the whole room sees the move, but the opcodes (`SPOTMOVE`
`coLs`, `PICTMOVE` `pLoc`) have no verified body layout to port — OpenPalace's own
`moveSpot`/`setPicOffset` are no-op stubs, and every other encoder in `wire.rs` is
pinned byte-for-byte against a working reference sender. Until one is found, a
script's spot move is local to this client only.

### The one fixed earlier: `DIMROOM`

`DIMROOM` was the proof case. `palace-render` had `Scene.dim_level` and
`Canvas::apply_dim` all along, but the builder hardcoded `dim_level = 1.0` and the
runtime's `DimRoom` arm only emitted a note, so a script could dim a room and
nothing happened. Three links were missing, now added:

1. `state.rs`: `SessionState.room_dim: f64`, `1.0` = undimmed, reset to `1.0` when
   a room descriptor arrives (a room you just entered is never dimmed).
2. `runtime.rs` `apply_effect`: `DimRoom` sets
   `room_dim = f64::from((*percent).clamp(0, 100)) / 100.0` and marks the render dirty.
3. `runtime.rs` `compose`: `scene.dim_level = state.room_dim` after building.

The scale is from the reference, not guessed: OpenPalace's
`PalaceCurrentRoom.dimRoom` does `level = clamp(0, 100); dimLevel = level / 100`,
and renders it as `alpha="{1 - dimLevel}"` black — the same maths as `apply_dim`.

### Testing an applied effect, and a harness trap

**Assert what the renderer sees, not what the state says.** Asserting
`state.room_dim` proves nothing, because the defect was precisely that state and
render were not connected. `a_face_or_colour_change_reaches_the_composited_scene`
drives the real `avatar_specs` and `SceneBuilder::build_with` and compares the
drawn image; cutting `.with_face_color(…)` back out of `avatar_specs` fails it.

**Trap: the mock sessions have no `is_self`.** `banner.user_id` comes from
`handshake.user_id()`, and in `tests/mock_runtime.rs` no `UserInfo` ever matches
it, so `is_self` is `None` for the whole run. Every self-targeted effect —
`SETFACE`, `SETCOLOR`, `DONPROP`, `NAKED`, `SETPROPS`, `SETUSERNAME` — therefore
answers "(no change)" over the socket, and cannot be frame-tested through it. That
is a fixture gap rather than a product bug: the same fixture does deliver the room
and the user list. Before writing a socket-level test for a self effect, fix the
fixture so it sends a user-list record carrying the handshake's user id.

**`protocol.md` needs one correction.** Its loose-prop section says the server
"threads `nextOfst` so that `firstLProp` points at the *last* array entry and each
link steps *backwards*", which reads as though traversal order were the reverse of
the prop numbering. It is not. `PalaceClient.as` loads a room by walking
`firstLProp` forward and appending, and then applies a script's index to that same
list *and* forwards it unchanged, so traversal order **is** the prop numbering.
That sentence describes the byte offsets, and should say so.

## The receive path: messages the client could not hear (done 2026-09-16)

The other half of the gap the effect work addressed. Those `Effect` arms apply what *scripts*
ask for; this is what the *server* announces about everyone else. The client decoded 21 of the
75 opcodes in `opcode.rs` and the rest fell to `Message::Unknown`, which only wrote a transcript
line. So in a populated room every other user looked frozen — faces, colours and worn props
never changed, and props people added, moved or removed never appeared. Seven now decode and
apply.

| Message | Opcode | Body | Applied as |
|---|---|---|---|
| USERFACE | `usrF` | `i16 faceNbr` (2 B) | user's face, re-render |
| USERCOLOR | `usrC` | `i16 colorNbr` (2 B) | user's colour, re-render |
| USERPROP | `usrP` | `i32 nbrProps` + `AssetSpec[n]` (4+8n) | worn list replaced wholesale |
| USERDESC | `usrD` | `i16 faceNbr`, `i16 colorNbr`, `i32 nbrProps` + `AssetSpec[n]` (8+8n) | all three at once |
| PROPNEW | `nPrp` | `AssetSpec spec` + `Point pos` (12 B) | appended to the room's loose props |
| PROPMOVE | `mPrp` | `i32 propNum` + `Point pos` (8 B) | that prop's `loc`, absolute |
| PROPDEL | `dPrp` | `i32 propNum` (4 B) | that prop removed; `-1` clears the room |

Wire primitives, both confirmed against the spec: `Point { sint16 v; sint16 h; }` —
**vertical first** — and `AssetSpec { sint32 id; uint32 crc; }` — id first.

**Go here first for any protocol question:**
`$CORPUS/reference/repos/ThePalacev0/ThePalace.Core.Database/Documentation/PalaceProtocolRef.txt`
— the official Communities.com *Palace Server Protocols* (1999), with explicit structs. Point
:193, AssetSpec :212, PROPDEL :1450, PROPMOVE :1464, PROPNEW :1481, USERCOLOR :2009, USERDESC
:2023, USERFACE :2057, USERPROP :2170. It sits beside the original "Mansion" client C source.

Four rules that are easy to get wrong and are now pinned by tests:
- `propNum` is a **0-based insertion-order index** into the room's loose props — not a crc, not
  an offset, not a position. `PROPNEW` carries no index at all: the server appends, so a new
  prop's address is the list length before the add. Two reference *clients* (OpenPalace and the
  ThePalacev0 web client) prepend at index 0, which would desync every later index-based
  MOVE/DEL — we append, matching the server and the spec.
- For the four appearance messages the user id is the frame **`refNum`, not a body field**.
- `USERPROP`/`USERDESC` are **whole-list replacements** with unused slots omitted, never
  zero-padded. The fixed 9-slot array belongs to the `UserRec` in `USERLIST`/`USERNEW`, a
  different message. A declared count is validated against the body length: too short *or*
  trailing bytes is a decode error, not a silent truncation.
- `PROPDEL -1` clears the room; any *other* out-of-range index is ignored with a transcript
  line so a stale index cannot empty the room. `refNum` 0 is never trusted — the ThePalacev0
  server relays `USERCOLOR` with `refNum` 0 while relaying `USERFACE` correctly.

Both paths converge: the effect arms in `runtime.rs` now delegate to the same
`SessionState::add_loose_prop`/`move_loose_prop`/`remove_loose_prop` the wire messages use.

**Test harness gap closed.** `tests/mock_runtime.rs` had no user record carrying the handshake's
id (13), so `is_self` was false for every user and self-targeted activity could not be exercised
over the socket at all. It now emits a self record, and 7 socket tests cover the receive path.

**Verified:** `cargo fmt --all -- --check` clean, `clippy -D warnings` clean on all five crates,
every suite green (palace-client lib 34, mock_runtime 20). Each new test was proven real by
cutting its application arm and confirming the failure, then restoring — e.g. prepending instead
of appending fails `[2222, 1111]` vs `[1111, 2222]`.

**Not yet live-tested.** All of the above is against the mock harness. Nobody has watched a real
second user's face change arrive from Balamb Garden.

### Placeholder art: a decision, not a task
The face art is a **deliberate placeholder** and must not constrain anything. Two facts to keep:
the artwork is 44x44 on a **45-pixel stride** (OpenPalace's `defaultsmileys.png`, 586x720, 44x44
art padded with a blank row and column), and the sheet bakes **16 colour tints** although the
original design tints one face from a 16-entry palette (`SmileyPalette.js`). Our render sheet is
a *resampled* 572x704 copy: 1,467 colours per cell against the source's 200, which is
interpolation, and it inflated the file from 182 KB to 807 KB for a worse result. A pixel-perfect
re-extract (208 clean 44x44 cells) is parked at `$HOME/ProgramFiles/palace-faces-placeholder/` and is
a zero-code-change drop-in.

The real limitation is **not** art quality: it is that the grid (13 faces, 16 colours, 44 px) is
hardcoded in the binary, so different art or a different face count is a code change. Removing
that — describe the grid in a small data file and load the art from a folder like every other
Palace asset — is what would make the placeholder genuinely replaceable. Deliberately not done
yet; it is polish, not a blocker.

**Face order (0-12), confirmed two independent ways:** `closed, smile, tiltdown, talk, winkleft,
normal, winkright, tiltleft, tiltup, tiltright, sad, blotto, angry`. pserver's `FACE_*` enum
(`$CORPUS/reference/repos/pserver/include/connection.hpp:54-70`, `NUM_FACES = 13`) lists
exactly that order, and the sprite sheet's columns match it one-for-one when inspected visually
(closed eyes, smile, flat, open-mouth talk, wink, neutral smile, opposite wink, looking left,
up, right, sad, X-eyes, angry). The canonical names are `TILT*`, not "down/left/up/right".

## Windows portability (first real run, 2026-09-17)

The Windows build had **never been run** — only compiled in CI. The first launch of the
`v0.1.0` portable zip failed immediately, and the failure mode is worth recording because
every check we had was blind to it.

**Symptom:** `io: The filename, directory name, or volume label syntax is incorrect.
(os error 123)`, printed about three times in the first five seconds.

**Cause:** `runtime.rs` built the per-session cache directory from `host:port`:

```rust
let session_root = cfg.cache_root.join(format!("{}:{}", cfg.host, cfg.port));
```

which produced a folder named `localhost:9998`. A colon is legal in a Linux filename and
**illegal in a Windows one** (reserved for drive letters and file streams), so the first
`create_dir_all` inside `AssetWorkspace::new` failed with `ERROR_INVALID_NAME`. The three
repeats were the supervisor retry loop's 1500/3000/6000 ms backoff, not a fixed limit.

**Fixes, all now tested:**
- `host:port` is escaped into a legal single component (`localhost%3A9998`) via
  `palace_asset::escape_name_component`, newly shared with the media-name sanitiser so one
  escaping rule serves both instead of two that can drift. Escaping rather than
  substituting also covers IPv6 hosts, which carry colons of their own.
- `session_cache_dir()` is extracted so the **wiring** is testable, not merely the helper.
  The test asserts no component of the composed path holds a character Windows forbids;
  reverting the wiring to `format!("{}:{}")` fails it with `':' cannot appear in a Windows
  filename, but Normal("localhost:9998") has it`.
- The cache root no longer falls through to `/tmp` (which on Windows means `C:\tmp`). It
  now prefers `XDG_CACHE_HOME`, then `LOCALAPPDATA`, then `HOME/.cache`, then
  `std::env::temp_dir()`. The candidates are passed as arguments so the Windows rung is
  testable from Linux.
- `split_paths` used `split(':')`, which broke Windows drive letters — `C:\props` became
  `C` and `\props`, so the seeds silently vanished. It now uses `std::env::split_paths`,
  which knows the platform separator. `HOME` also falls back to `USERPROFILE`.

**Closed (2026-09-17).** `assets.rs` `write_media` accepted any base name, so a media name Windows
cannot hold — a `:`, a control character, or a reserved device name such as `aux.png` — would fail
there with the same error 123. It now refuses those names via `legal_file_name`, and the caller
skips the file with a logged error rather than failing the session.

The rule is enforced on **every** platform, not only Windows, because the name is what the renderer
later looks the file up by: a name that works on one platform and not another is a bug wherever it
runs. Refusing rather than escaping is deliberate — escaping would change the name the renderer
looks up and so convert a crash into a silently missing image.

**Proven by a wiring test, not a helper test.** Removing the guard while leaving the predicate
intact fails `writing_an_unusable_name_errors_instead_of_writing_it` with
`expected a config error, got Ok("/tmp/.../aux.gif")`. The two tests against the predicate itself
still passed — which is the point: a test on the helper alone would have missed the regression.
This is the same trap as the `session_cache_dir` wiring above, and it has now bitten twice.

**Lesson worth keeping:** a cross-compile check cannot catch this class of bug.
`cargo check --target x86_64-pc-windows-gnu -p palace-app` passed cleanly throughout — the
problem is path *semantics at runtime*, not anything the compiler can see. The Windows
build has to be **launched**, not just compiled. Setup instructions for a fast Windows dev
loop live in `$MEDIA/palace-client-windows-dev-setup.md`.

## Door locks (2026-09-17)

The client could not tell that a door was locked, so clicking one tried to walk through it.
`runtime.rs` carried an admitted TODO for exactly this ("a click does not yet refuse to open a
locked door"). Three opcodes sat in the table with no decoder, falling into `Message::Unknown`:

| Message | Opcode | Body |
|---|---|---|
| DOORLOCK / DOORUNLOCK | `lock` / `unlo` | `RoomID` + `HotspotID` (4 B) |
| SPOTSTATE | `sSta` | `RoomID` + `HotspotID` + `sint16 state` (6 B) |

Both ids are `sint16` (:222-223), bodies at :815 and :1952.

**The load-bearing fact (:1677-1680):** a door's locked state *is* the hotspot's `state` field —
`HS_Unlock = 0`, `HS_Lock = 1` — and doors are hotspot types 1 (door), 2 (shutable), 3
(lockable). So no new model field was needed. The client already tracked `state`; it simply never
heard about changes to it.

Now decoded, applied only to the room we are actually in (a lock aimed at another room is
ignored), and a click on a locked door is refused locally with a transcript note instead of
dispatching `SELECT`. Decoded opcodes: 28 → 31.

`HS_Bolt` (4) is deliberately excluded: a bolt locks the door named by its `dest` and is not
itself a door, so clicking one is never refused.

**`HS_ShutableDoor` (2) is excluded too, and that is a correction to the first version.** `state` is
primarily a *picture selector* — the reference says it "selects which of the pictures associated with
the hotspot should be displayed. Among other things, it encodes whether a door is locked or
unlocked" (:1677) — so a shuttable door's two states are its **closed and open** pictures. Reading
state 1 as "locked" would refuse the very click that closes an open door, breaking the one thing
that type exists for ("a door that can be opened/closed (by clicking)", :1666). Refusal therefore
covers `HS_Door` (1) and `HS_LockableDoor` (3) only.

The failure modes are not symmetric, and that is why narrowing is the safe direction: refusing a
legitimate door breaks it permanently for the user, while failing to refuse a locked one merely lets
the server refuse the move. Guarded by
`only_a_kind_of_door_that_can_be_locked_reads_state_one_as_locked`, which fails when type 2 is
re-admitted — the integration test for refusal uses a type-1 door from the captured corpus and would
*not* have caught this.

### Open questions the door work raised (questions, not answers)

Two hotspot types exist whose *purpose* is a click action the client may be expected to send itself.
We currently only run the hotspot's `ON SELECT` script on a click. That may be sufficient if the
room's own script does the work — but neither of these has been checked against a reference client,
so they are open rather than settled:

- **`HS_Bolt` (4)** — "bolt that locks or unlocks door pointed to by `dest`" (:1668). If a client is
  meant to send `DOORLOCK`/`DOORUNLOCK` for the door at `dest` when the bolt is clicked, we do not,
  and clicking a bolt would do nothing unless the room's script covers it.
- **`HS_ShutableDoor` (2)** — "a door that can be opened/closed (by clicking)" (:1666). Closing
  implies something advances the door's state. Today that is whatever the room's script does; if the
  client is meant to send `SPOTSTATE` itself, we do not.

Both would be settled cheaply by reading how a reference client handles a click on those types.
Neither is known to be broken — they are *unchecked*, which is a different thing, and the difference
is worth keeping.

**Verified by inversion:** flipping the state test so that an *unlocked* door read as locked broke
four tests — the two door tests, plus `a_real_room_hotspot_script_dimroom_darkens_the_frame` and
`entering_a_new_room_resets_the_dim`, because their hotspot click was refused and the script
therefore never ran. Unrelated tests failing is the proof that this check sits on the real click
path rather than in dead code.

**Since closed:** `SPOTNEW`/`SPOTDEL`/`SPOTMOVE`/`PICTMOVE` are now decoded and applied
(commit `6d24495`). The other four of that family turned out to be vestigial — see the section
below before assuming they need implementing.

## Vestigial opcodes: defined but unused (2026-09-17)

Four opcodes from the room-mutation family have **no documented body anywhere** and no server
that uses them:

    opSs  SPOTSETDESC   0x6f705373
    nPct  PICTNEW       0x6e506374
    FPSq  PICTDEL       0x46505371
    sPct  PICTSETDESC   0x73506374

**The proof, so this is not re-investigated:** grepping the protocol reference for
`struct ClientMsg_(pict|spot)` returns exactly five definitions — `pictMove`, `spotDel`,
`spotMove`, `spotNew`, `spotState`. There is no `ClientMsg_pictNew`, `pictDel`, `pictSetDesc` or
`spotSetDesc`. The reason is stated at :258-265: the opcode table's usage legend includes a glyph
meaning *"According to source code, message is defined but unused"*, and the note says such
messages *"will not appear in the individual message descriptions."* These four are precisely the
ones with a table row and no section.

Corroborated across every authority on disk — the original Palace SDK's `m-protocol.h` has no
struct for them, the original server's `s-events.c` has no `case` for them, ThePalacev0 ships
empty `Deserialize` stubs, Taj lists them only in its enum, QPalace annotates each `// ?`, the Go
server has no build/parse function, and the PalaceChat 5 AppImage contains no constants for them
in either byte order.

**Conclusion: do not implement these.** A decoder needs width, field order and string encoding,
and every one of those is unknowable here — `PICTDEL` could be a bare `HotspotID` (mirroring
`SPOTDEL`, :1906) or `RoomID + HotspotID`; `PICTNEW` could be bodyless (mirroring `SPOTNEW`,
:1938) or carry a `PictureRec`. Those are mutually exclusive guesses, and a wrong decoder is
worse than an honest "ignored opcode" line. If a real capture ever shows one, that capture is the
authority and the decoder should be written from its bytes.

## Opcode coverage: what is left, and whether it matters (audited 2026-09-17)

An audit of the **40 opcodes that were still undecoded when it was made** (75 in the table, 35 decoded
at the time; `AUTHENTICATE` has since been decoded, so the split is now 36 decoded / 39 undecoded).
Headline: **nothing still missing breaks using the client on a public server** — with one real
exception, below.

| Class | n | Meaning |
|---|---|---|
| Already handled elsewhere | 7 | no work: `qAst`, `rAst`, `sAst`, `navR`, `xtlk`, `xwis`, `ryit` |
| Vestigial | 14 | no work; named below |
| Unresolved | 1 | `durl` (DISPLAYURL): the spec documents a body, no server anywhere constructs it |
| Real but non-blocking | 18 | ranked below |

**The one hard blocker: authentication — now reported honestly, still not solved.** On a server
configured to require it, the server sends `auth` (AUTHENTICATE) after logon and waits for `autr`
(AUTHRESPONSE). `AUTHENTICATE` is now decoded and the client says so in the transcript instead of
stalling in silence. The exchange itself is still unimplemented, so **an auth-requiring server will
not let this client in**. Completing it needs a credential source plus the reply — a PString of
`user:password` (:750-754).

Worth knowing *why* this bites: our own logon advertises the authenticate flag
(`logon.rs`, `aux_flags = 0x8000_0008`, and a test pins it), so we actively ask the server to
challenge us. The options are to implement the reply, or to stop advertising a capability we do not
have — the latter is one line, but it deviates from the captured reference logon, so it stays a
decision rather than a unilateral change.

Ranked after that, all feedback or optional channels:

| | Opcode | What it would fix |
|---|---|---|
| 2 | `down` SERVERDOWN | a forced disconnect (kick/ban/flood/full/shutdown) shows as a bare socket close with no reason given |
| 3 | `sErr` NAVERROR | a failed room change (full/closed/pinned/password/kicked) is silently ignored |
| 4 | `usrN` USERNAME | live renames are invisible, and there is no way to rename yourself |
| 5 | `draw` DRAW (receive) | other people's paint is dropped at decode; rasterizing strokes stays the separate known gap |
| 6 | `sRom` ROOMSETDESC | room edits made while you are inside are not reflected — and pserver does send this |
| 7 | `blow` BLOWTHRU | the plugin-relay channel |
| 8 | `sFil` / `fnfe` / `qFil` | legacy server-hosted file transfer, superseded by HTTP media |
| 9 | `susr`, `kill`, `gmsg`, `smsg`, `rmsg`, `nRom`, `sInf` | send-side features: become wizard, kick, global shout/page, create room, server info |

**Vestigial (14) — do not implement:** PICTDEL, PICTNEW, PICTSETDESC, SPOTSETDESC, PROPSETDESC,
ASSETNEW, USERENTER, SERVERUP, WMSG, NOOP, TIMYID, RESPORT, TROPSER, INITCONNECTION. Each is absent
from the original server's event switch and from every modern implementation's handlers; most appear
only in the header where they are defined. `PROPSETDESC` is a fifth sibling of the four already
recorded above. Details worth keeping: `NOOP`'s own documentation says "it is not actually used in
the current Unix server"; `ASSETNEW`'s case in the original client is *commented out* ("not really
needed anymore"); `INITCONNECTION` is used only by the classic Mac LocalTalk transport, never on TCP;
`TROPSER` is the legacy HTTP-tunnel banner marker we already handle in `ByteOrder::from_banner`.

**Two caveats about the evidence, so this is not re-derived badly:**

1. **The protocol reference's usage column is corrupted in our copy.** All four legend glyphs and
   every table row read literally `server ( client` — the arrows were lost in a conversion, so that
   column cannot be read at all. Direction had to be reconstructed from Taj's transcription plus the
   server source.
2. **"No §4.3 section means unused" is a useful heuristic but has false positives.** `GMSG`,
   `ASSETREGI` and `ASSETSEND` *do* have sections (indented, easy to miss), and DIYIT has none yet is
   real. No classification above rests on section-presence alone: each was confirmed against a server
   send or receive site.

## Running it

```bash
cd $REPO
bun install
bun run check && bun run build          # frontend
cargo test                              # 621 tests, no network
bun run tauri dev                       # window; auto-connects to localhost:9998

# headless live checks
cargo run -p palace-client --bin live-smoke                                  # connect + render
PALACE_SMOKE_ROOM=817 cargo run -p palace-client --bin live-smoke            # room switch
PALACE_SMOKE_VIEWPORTS=1 cargo run -p palace-client --bin live-smoke         # zoom/1:1/resize
PALACE_DEBUG_FRAMES=1 cargo run -p palace-client --bin live-smoke            # raw frame trace
```

Defaults: `localhost:9998`, user `Guest`. Override with `--host/--port/--user`
or `PALACE_HOST`/`PALACE_PORT`/`PALACE_USER`. `PALACE_SEED_MEDIA` /
`PALACE_SEED_PROPS` add read-only local fallback roots (colon-separated).

## Operational rules that have been earning their keep

- **Stage explicit paths, never `git add -A`, whenever another agent or agent-runner instance may be
  sharing the worktree.** `-A` sweeps a concurrent editor's half-written files into your commit.
  This has already been a near-miss: commit `df09082` used `-A` and was clean only by luck.
  Related: release CI (`.github/workflows/release.yml`) was authored by a *different* instance,
  so commits touching it are not automatically ours.
- **A placeholder must not become a constraint.** The face art is a placeholder; the thing that
  would actually limit us is the hardcoded grid, not the art's quality. Keep the two separate
  when deciding what to work on.

* **NEVER run `--workspace` cargo commands in a fresh worktree.** A new worktree
  has no `target/`, so `cargo test --workspace` / `cargo clippy --workspace
  --all-targets` compiles the **entire Tauri/WebKit dependency tree from scratch —
  5.6 GB, 20–40 minutes** — and it happens again for each cargo invocation
  (clippy uses a different profile). From outside this is indistinguishable from
  a hang, and it caused a *working* agent to be cancelled twice. Instead: scope
  to the crate (`cargo test -p <crate>`), or set
  `CARGO_TARGET_DIR=$REPO/target` to reuse the
  warm 6 GB. A pure crate like `iptscrae` has zero dependencies and builds in
  seconds — it never needed the Tauri stack at all.
* **Delegate one milestone per agent, in its own git worktree.** Never blanket
  `git add -A` during a conflicted merge.
* **ALWAYS re-run the agent's own verification command before merging.**
* **Never read the research dumps** in `~/.local/share/agent-runner/tool-output/`.
* **`$CORPUS/` is read-only reference.** Do not edit it.
* WebKitGTK: the Rust compositor keeps the webview's job to one bitmap, which is
  what makes this milestone possible without WebGL. `WEBKIT_DISABLE_COMPOSITING_MODE=1`
  was **not** needed here.

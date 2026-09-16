# Status / handoff — Palace Tauri client

**Last updated:** 2026-09-16 (interactive client milestone)
**Scope, decisions and design:** `~/palace-corpus/TAURI-CLIENT-SCOPE.md` ← read that too. It is the design authority; this file is only "where things stand right now". (Left untouched per the read-only rule for `~/palace-corpus/`.)

This file exists so a **fresh session** can resume without carrying a long conversation.

---

## HARD BOUNDARIES — agents must not cross these

These exist because a subagent, trying to satisfy a "watch the window resize" instruction, went hunting for desktop input-injection tooling and reached for **`ydotool` (kernel-level mouse injection) on a live desktop**. It abandoned the path after ~11s and used the app's own command path instead — but it should never have been available to it.

**NEVER:**
- **Inject input into the user's desktop.** No `ydotool`, `wtype`, `xdotool mousemove/click/key`, `xte`, or any synthetic input. Not to click a button, not to prove a UI works. This is a machine the user is actively using.
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

**Branch `feat/palace-ui` is MERGED to `master`** (merge commit `de0969c`, no conflicts). Everything described below is on master now.

A worktree `~/ProgramFiles/palace-client-ui` may still exist — it was left in place because the live demo was launched from it. It is now redundant (same content as master). Remove it when you no longer need the running demo:

```bash
tmux kill-session -t app; tmux kill-session -t vite
git -C ~/ProgramFiles/palace-client worktree remove --force ~/ProgramFiles/palace-client-ui
git worktree prune && git branch -d feat/palace-ui
```

### Running the app

```bash
cd ~/ProgramFiles/palace-client
bun install          # first time only
bun run dev          # Vite on :1420 — REQUIRED for a debug build
cargo run -p palace-app
```

**Gotcha that cost real time:** a debug `cargo build`/`cargo run` loads `devUrl` (`http://localhost:1420`) from `tauri.conf.json`, **not** the bundled frontend. Without Vite running you get a WebKit page reading `Could not connect to localhost: Connection refused` — which looks like a WebKitGTK failure but is not. For a standalone binary use `cargo build --release -p palace-app` or `bun run tauri build`, which use `frontendDist`.

**Trap:** never `pkill -f "<pattern>"` where the pattern appears in your own command string — it kills the shell running it (documented in `~/AGENTS.md`; it killed a tmux server here). Use `pgrep -x`/`pkill -x` or kill by PID.

**569 tests passing, clippy clean, `cargo check` clean, `svelte-check` clean.**

### The six original crates (unchanged contracts)

| Crate | State | Evidence |
|---|---|---|
| `palace-wire` | ✅ done | framing, `ByteOrder`, 75-opcode table, codecs. **`navr_frame` fixed** (see below). |
| `palace-probe` | ✅ done | live logon; 81 rooms / 2 users. |
| `palace-room` | ✅ done | 804/804 corpus records parse clean. |
| `palace-prop` | ✅ done | 5 decoders + S20 encoder; 3,000-prop differential = 0 disagreements. |
| `palace-asset` | ✅ done | `qAst`/`sAst`/`rAst`, paced scheduler, media HTTP. |
| `palace-render` | ✅ done | composites rooms → PNG; coordinate mapping round-trip tested. |
| `iptscrae` | ✅ done | lexer (718 lines), parser, VM, stack, budget enforcement, capability trait, regex engine; 143 tests. **Zero dependencies.** |
| `iptscrae-palace` | ✅ done | 497-line host trait, 371 lines of Palace commands, CLI, corpus harness; 29 tests. Corpus: **2396/2400 files parse · 3791/3805 handlers clean · 0 tokenizer gaps.** |

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

All additions; no existing behaviour changed. The 372 original tests still pass.

---

## Bug found and fixed: `palace-wire::navr_frame`

`navr_frame` built a **6-byte body** `[u32 2][u16 room_id]`, putting the length
constant *inside* the payload. Both reference encoders write that `2` as the frame
**length field**:

* `~/palace-corpus/tools/palace_walker.py::make_navr` →
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

**Hermetic (no network), `cargo test --workspace` — 569 pass:**

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

* Visual: screenshots at `/tmp/opencode/palace-crop2.png` (Balamb Garden live in
  the window, room list 81/81, "Member 21 (you)", status bar
  `connected · Balamb Garden · v1.22 · endian little · frame #4`) and
  `/tmp/opencode/palace-room901.png`, `palace-room817.png` (headless composites).

## Not verified / rough edges

* **The last visual pass (zoom/1:1/resize screenshots) was done through the
  runtime command path, not a mouse drag.** The session's screen locked from
  inactivity partway through QA and unlocking needs the user's password, so the
  final zoom/1:1/resize evidence is the numeric sweep above plus unit tests.
  Everything is wired to the same `set_viewport` command, but a human should
  still click the slider once.
* **Avatars are not drawn yet.** On Balamb Garden the logged-on user arrives with
  `props=0`, so there is nothing to draw; the renderer hides avatars whose prop
  art has not arrived rather than littering the room with magenta placeholders.
  Prop art for other users would need successful asset transfer (untested live —
  no other users were online).
* **Server churn during QA.** Repeated connections from one host in quick
  succession made the server send `bye` (len 4, ref = the *previous* session's
  user id, i.e. a server-side logoff notice) and close. The client
  auto-reconnects with backoff and now **restores the room the user was in**
  (`Shared::last_room`). A single normal session is unaffected; the first smoke
  runs ran 22 s cleanly.
* **No IPTSCRAE, hotspot clicks, sounds, drawing, deco, prop editing, web panes,
  theming** — all explicitly out of scope for this milestone.
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

## Running it

```bash
cd ~/ProgramFiles/palace-client-ui
bun install
bun run check && bun run build          # frontend
cargo test --workspace                  # 569 tests, no network
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

* **NEVER run `--workspace` cargo commands in a fresh worktree.** A new worktree
  has no `target/`, so `cargo test --workspace` / `cargo clippy --workspace
  --all-targets` compiles the **entire Tauri/WebKit dependency tree from scratch —
  5.6 GB, 20–40 minutes** — and it happens again for each cargo invocation
  (clippy uses a different profile). From outside this is indistinguishable from
  a hang, and it caused a *working* agent to be cancelled twice. Instead: scope
  to the crate (`cargo test -p <crate>`), or set
  `CARGO_TARGET_DIR=$HOME/palace-client/target` to reuse the
  warm 6 GB. A pure crate like `iptscrae` has zero dependencies and builds in
  seconds — it never needed the Tauri stack at all.
* **Delegate one milestone per agent, in its own git worktree.** Never blanket
  `git add -A` during a conflicted merge.
* **ALWAYS re-run the agent's own verification command before merging.**
* **Never read the research dumps** in `~/.local/share/opencode/tool-output/`.
* **`~/palace-corpus/` is read-only reference.** Do not edit it.
* WebKitGTK: the Rust compositor keeps the webview's job to one bitmap, which is
  what makes this milestone possible without WebGL. `WEBKIT_DISABLE_COMPOSITING_MODE=1`
  was **not** needed here.

# STATUS.md — current state

**Snapshot:** workspace version 0.2.0; rustc 1.98.1.

This file is where the client stands now and the evidence behind each claim. The
design authority is `$CORPUS/TAURI-CLIENT-SCOPE.md` (read-only); the wire
format is `protocol.md`; the work queue is `palace-todo.md`.

---

## Hard boundaries (do not cross)

**Note:**
- **Inject input into the user's desktop.** No `input-tool`, `input-tool`, `input-tool mousemove/click/key`, `xte`, or any synthetic input. Not to click a button, not to prove a UI works. This machine is actively used.
- **Unlock, wake, or inhibit the user's screen.** A locked screen ends the QA run — stop and report.
- **Screenshot the whole desktop.** If a capture is needed, grab the app window only (`import -window <id>`). Never full-screen `spectacle -b -n`.
- **Activate, raise or focus the user's windows** (`wmctrl -a`, KWin D-Bus window scripting).
- **Ask for or use the user's password**, or any credential.

**DO INSTEAD, for UI verification:**
- Drive the app's own command/state path (`invoke("set_viewport", …)`) — the identical code the button calls.
- Verify numerically (geometry readouts, transform values) and via headless renders.
- State plainly that a human still needs to click the real control once.

**Never write a definition of done that can only be satisfied by driving the user's desktop.** Specify the command path instead.

---

## Architecture at a glance

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

### `palace-render` extensions used by the live client

* `Canvas::to_png_bytes()` — in-memory twin of `write_png`, for the URI scheme.
* `PropStore::insert_blob(id, blob)` — insert a prop received over the wire.
* `MediaStore::insert_path(name, path)` — register a file fetched after indexing.
* `SceneBuilder::media_mut()` / `props_mut()` — let a live client fill the stores.
* `PropBackend::Memory` — an in-memory prop source.

---

## Tests

Measured per crate with `cargo test -p <crate>`; the Tauri app (`palace-app`) is
not included, because a `--workspace` run also compiles the Tauri/WebKit stack.
Doctests are included.

| Crate | Passed | Ignored |
|---|---:|---:|
| `palace-wire` | 126 | 0 |
| `palace-room` | 24 | 0 |
| `palace-prop` | 162 | 2 |
| `palace-asset` | 146 | 1 |
| `palace-probe` | 21 | 0 |
| `palace-render` | 116 | 0 |
| `palace-client` | 164 | 0 |
| `iptscrae` | 155 | 0 |
| `iptscrae-palace` | 96 | 0 |
| `palace-host` | 128 | 0 |
| **Total** | **1138** | **3** |

The three ignored tests are opt-in: `palace-asset/tests/live_server.rs` needs a
live pserver, and `palace-prop`'s corpus and bag tests need the local corpora.
`cargo fmt --all -- --check` and `clippy -D warnings` are clean across every
non-Tauri crate.

---

## What works, and the evidence

**Protocol and room model.** Framing, endianness detection, logon, the room and
user lists, and the `MSG_ROOMDESC` walker are implemented and replay-tested.
`crates/palace-wire/tests/fixture_replay.rs` decodes `fixtures/logon-run1/` with
no server and checks every frame byte for byte. `crates/palace-room` decodes 799
real payloads (804 records) with zero errors and zero warnings, and pins
`Balamb Garden` from the captured room frame.

**Prop codec.** `palace-prop` decodes all five encodings and encodes S20. A
227,874-prop differential against two independent oracles reports 0
disagreements; the seven rejections are props the reference rejects too.

**Compositor.** `palace-render` turns a `RoomDesc` plus local assets into an RGBA
frame, with the room↔viewport↔buffer mapping round-trip tested. Draw commands,
name tags, loose props, avatars and overlays all rasterize; chat text is the one
layer that does not (see *Known gaps*).

**Script engine.** `iptscrae` implements the language; `iptscrae-palace` adds the
Palace command surface; `palace-host` splits the `ON <event> { … }` blocks,
dispatches runtime events, keeps globals across handlers and runs alarms. The
harvested corpus of 2,400 scripts parses 2,396 and runs 3,791 of 3,805 handlers
clean with per-file globals, or 3,793 with `--shared-globals`; the four parse
failures are malformed source the reference tokenizer also rejects, and tokenizer
gaps are zero.

**Live.** Logon, room listing, user listing, room switching, chat round-trip and
media fetch have all been exercised against a live server, and the app renders
the composited room in the window. The receive path for other users' faces,
colours, worn props and loose props is verified against a mock harness; a busy
server with a second user has not been available for a live check.

---

## Command and opcode coverage (current)

The registry holds **205 names**: 72 core builtins
(`crates/iptscrae/src/registry.rs`) plus 133 Palace bindings
(`crates/iptscrae-palace/src/commands.rs`). `[` and `]` are lexer delimiters, not
registry commands.

The host's fallback arm (`crates/palace-host/src/host.rs`) lists **34** command
names it recognises but does not implement. **28** of them are not in
`PALACE_COMMANDS`, so a script that calls one lexes it as a variable and
misbehaves quietly rather than reporting "unsupported". The other six
(`AUTOUSERLAYER`, `SETTOOLTIP`, `CLEARTOOLTIP`, `REMOVEPIC`, `BAN`, `KICK`) are
registered and dispatch to `unimplemented()`.

**Opcodes.** 75 are named; **39 are decoded and 36 are not**
(`crates/palace-wire/src/opcode.rs`, `crates/palace-wire/src/messages/mod.rs`).
The undecoded 36 fall into four classes:

| Class | n | Meaning |
|---|---|---|
| Already handled elsewhere | 7 | `qAst`, `rAst`, `sAst`, `navR`, `xtlk`, `xwis`, `ryit` |
| Vestigial | 14 | defined but unused; named below |
| Unresolved | 1 | `durl` (DISPLAYURL): the spec documents a body, no server constructs it |
| Real but non-blocking | 14 | ranked below |

**Vestigial (14) — do not implement:** `PICTDEL`, `PICTNEW`, `PICTSETDESC`,
`SPOTSETDESC`, `PROPSETDESC`, `ASSETNEW`, `USERENTER`, `SERVERUP`, `WMSG`,
`NOOP`, `TIMYID`, `RESPORT`, `TROPSER`, `INITCONNECTION`. Each is absent from
the original server's event switch and from every modern implementation's
handlers; most appear only in the header where they are defined. `NOOP`'s own
documentation says it is not used by the current Unix server; `ASSETNEW`'s case
in the original client is commented out; `INITCONNECTION` is used only by the
classic Mac LocalTalk transport; `TROPSER` is the legacy HTTP-tunnel banner
marker already handled in `ByteOrder::from_banner`.

Ranked after authentication, all feedback, send-side or optional channels:

| Opcode | What it would fix |
|---|---|
| `down` SERVERDOWN | a forced disconnect (kick/ban/flood/full/shutdown) shows as a bare socket close |
| `sRom` ROOMSETDESC | room edits made while you are inside are not reflected |
| `blow` BLOWTHRU | the plugin-relay channel |
| `sFil` / `fnfe` / `qFil` | legacy server-hosted file transfer, superseded by HTTP media |
| `autr` AUTHRESPONSE | the reply an auth-requiring server waits for |
| `susr`, `kill`, `gmsg`, `smsg`, `rmsg`, `nRom`, `sInf` | send-side features: become wizard, kick, global shout/page, create room, server info |

---

## Receive path rules pinned by tests

`palace-room` decodes the messages the server sends about other users. Four rules
are easy to get wrong and are pinned:

- **`propNum` is a 0-based insertion-order index** into the room's loose props —
  not a crc, not an offset, not a position. `PROPNEW` carries no index; the server
  appends, so a new prop's address is the list length before the add. Two
  reference clients prepend at index 0, which would desync every later
  index-based MOVE/DEL — this client appends, matching the server and the spec.
- **For the four appearance messages the user id is the frame `refNum`, not a
  body field.**
- **`USERPROP`/`USERDESC` are whole-list replacements** with unused slots omitted,
  never zero-padded. The fixed 9-slot array belongs to `USERLIST`/`USERNEW`. A
  declared count is validated against the body length: too short *or* trailing
  bytes is a decode error.
- **`PROPDEL -1` clears the room;** any other out-of-range index is ignored with a
  transcript line. `refNum` 0 is never trusted.

The decoded receive set is `USERFACE`, `USERCOLOR`, `USERPROP`, `USERDESC`,
`PROPNEW`, `PROPMOVE`, `PROPDEL`.

---

## Door locks

Two different rules apply, and they are not the same check:

- **Click refusal** covers **lockable doors (hotspot kind 3) only**. A click on a
  locked kind-3 door is refused locally with a transcript note instead of
  dispatching `SELECT` (`state_means_locked` in `palace-client/src/runtime.rs`).
- **`ISLOCKED`** answers true for a **shuttable (2) or lockable (3)** door whose
  state is 1 (`is_locked` in `palace-host/src/host.rs`).

Kind 1 (plain door) is never treated as lockable for either rule: its `state` is
primarily a picture selector, so a kind-1 `state == 1` is the second frame of a
two-picture door, not a lock.

`HS_Bolt` (4) is excluded as well: a bolt locks the door named by its `dest` and
is not itself a door.

**Open questions (unchecked, not known-broken).** On a click this client only runs
the hotspot's `ON SELECT` script and sends nothing itself. Whether a client is
expected to send `DOORLOCK`/`DOORUNLOCK` for the door at a bolt's `dest`, or
`SPOTSTATE` for a shuttable door, has not been checked against a reference client.

---

## Effect application

`apply_effect()` in `crates/palace-client/src/runtime.rs` has no catch-all: the
match is exhaustive, so adding an `Effect` variant fails the build until it is
handled. Local scene overrides — loose props, faces and colours, worn props,
spot positions, picture offsets and opacity, avatar visibility, and room dim —
are applied to the state the compositor reads, and reset when a room descriptor
arrives. Effects that belong on the wire are encoded as protocol frames.

**Still open:** `SETLOC`/`SETPICLOC` never reach the server. The non-local variants
should broadcast so the whole room sees a move, but `SPOTMOVE` (`coLs`) and
`PICTMOVE` (`pLoc`) have no verified body layout to port, and the reference
clients' own `moveSpot`/`setPicOffset` are no-op stubs. A script's spot move is
local to this client only.

---

## Known gaps and unverified edges

- **Audio.** `SOUND`, `MIDIPLAY`, `MIDILOOP`, `MIDISTOP` and `BEEP` are surfaced,
  but no audio device is reached.
- **Chat text** is not rasterized into the frame. Draw commands and name tags are.
- **Constant stubs.** `GETPICDIMENSIONS` returns `(0, 0)`, `PROPDIMENSIONS` and
  `PROPOFFSETS` push zeros, and `has_prop_by_name` returns false.
- **A props panel.** Worn props are sent to the server, but the interface cannot
  set them yet.
- **Authentication.** A server requiring a password is refused, with the request
  reported rather than silently ignored. `AUTHENTICATE` is decoded; the
  `AUTHRESPONSE` reply is not implemented.
- **Live verification is thinner than the unit tests.** Several receive paths are
  proven against a mock harness; no live big-endian or HTTP-tunnel server has been
  reachable; asset transfer for other users' avatars is untested against a real
  peer.
- **Multi-block asset transfer and 16-bit props** are reference-derived; the
  16-bit decoder has zero real samples and the loose-prop artwork for the
  available corpus is missing from local stores.
- **`durl` (DISPLAYURL)** remains unresolved.

### Platform behaviour

The per-session cache directory escapes `host:port` into a single legal path
component (`localhost%3A9998`), so a colon cannot break a Windows filesystem,
and `write_media` refuses names a Windows filesystem cannot hold (reserved device
names, control characters, `:`). The rule is enforced on every platform, because
the name is what the renderer later looks the file up by; refusing rather than
escaping keeps a crash from becoming a silently missing image. A cross-compile
check cannot catch this class — the build has to be launched, not just compiled.

### Placeholder art

The face art is a deliberate placeholder and must not constrain anything. The
shipped sheet is a resampled 572×704 copy: 13 faces × 16 colours of 44×44 cells.
The real limitation is that the grid is hardcoded in the binary, so different art
or a different face count is a code change; a data-driven grid is polish, not a
blocker. Face order (0–12), confirmed against pserver's `FACE_*` enum and the
sprite columns, is: `closed, smile, tiltdown, talk, winkleft, normal, winkright,
tiltleft, tiltup, tiltright, sad, blotto, angry`.

---

## Open decisions

- **Where does a credential live?** Implementing `AUTHRESPONSE` (a PString of
  `user:password`) needs a credential source, so the choice of config file,
  prompt or keyring comes first. The alternative is to stop advertising the
  capability in the logon (`aux_flags`), which deviates from the captured
  reference logon and so is a decision rather than a unilateral change. Until
  then, an auth-requiring server yields an explained failure, not a hang.
- **`SETLOC`/`SETPICLOC` broadcast** waits on a verified body layout for
  `SPOTMOVE`/`PICTMOVE`.
- **`durl`** waits on any real server that constructs it.

---

## Operational rules

- **Stage explicit paths, never `git add -A`, whenever another agent or agent-runner
  instance may share the worktree.** `-A` sweeps a concurrent editor's
  half-written files into your commit.
- **A placeholder must not become a constraint.** The face art is a placeholder;
  the hardcoded grid is the real limit. Keep the two separate.
- **NEVER run `--workspace` cargo commands in a fresh worktree.** A new worktree
  has no `target/`, so `cargo test --workspace` / `cargo clippy --workspace
  --all-targets` compiles the entire Tauri/WebKit dependency tree from scratch
  (5.6 GB, 20–40 minutes), again for each cargo invocation. Scope to the crate
  (`cargo test -p <crate>`), or set `CARGO_TARGET_DIR` to reuse the warm target.
- **Delegate one milestone per agent, in its own git worktree.** Never blanket
  `git add -A` during a conflicted merge.
- **ALWAYS re-run the agent's own verification command before merging.**
- **Never read the research dumps** in `~/.local/share/agent-runner/tool-output/`.
- **`$CORPUS/` is read-only reference.** Do not edit it.
- **`pkill -f "<pattern>"` kills your own shell** when the pattern appears in the
  command string. Use `pgrep -x`/`pkill -x`, or kill by PID.
- **WebKitGTK:** the Rust compositor keeps the webview's job to one bitmap, which
  is what makes the interface work without WebGL. `WEBKIT_DISABLE_COMPOSITING_MODE=1`
  is not needed.

---

## How to run and verify

```bash
cd $REPO
bun install
bun run check && bun run build          # frontend
cargo test -p palace-wire -p palace-room -p palace-prop -p palace-asset \
           -p palace-probe -p palace-render -p palace-client \
           -p iptscrae -p iptscrae-palace -p palace-host
bun run tauri dev                       # window

# headless live checks
cargo run -p palace-client --bin live-smoke                                  # connect + render
PALACE_SMOKE_ROOM=817 cargo run -p palace-client --bin live-smoke            # room switch
PALACE_SMOKE_VIEWPORTS=1 cargo run -p palace-client --bin live-smoke         # zoom/1:1/resize
PALACE_DEBUG_FRAMES=1 cargo run -p palace-client --bin live-smoke            # raw frame trace
```

Defaults: `localhost:9998`, user `Guest`. Override with `--host/--port/--user`
or `PALACE_HOST`/`PALACE_PORT`/`PALACE_USER`. `PALACE_SEED_MEDIA` /
`PALACE_SEED_PROPS` add read-only local fallback roots (colon-separated).

**Trap:** a debug `cargo build`/`cargo run -p palace-app` loads `devUrl`
(`http://localhost:1420`), not the bundled frontend. Without Vite running you get
a WebKit page reading `Could not connect to localhost: Connection refused`. Use
`bun run tauri dev`, or `cargo build --release -p palace-app` for a standalone
binary.

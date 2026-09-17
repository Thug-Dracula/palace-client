# palace-todo.md — what is done, and what is left

A working checklist for the Palace client. Companion files:

| File | Role |
|---|---|
| `README.md` | What The Palace is, and what this project is. |
| `STATUS.md` | The detailed handoff record — evidence, root causes, measurement methods. **This file is the summary; STATUS.md is the authority.** |
| `~/palace-corpus/TAURI-CLIENT-SCOPE.md` | Design authority (scope and decisions). Read-only reference. |
| `protocol.md` | The protocol notes we keep ourselves. |

**Snapshot:** 84 commits on `master` · no git remote (publication is a scrubbed copy, see below) ·
last full measurement in STATUS.md: 621 tests passing, clippy clean.
**Plus uncommitted work** (see §1.9): name tags, click-to-walk, visibility toggles and the avatar
picker — gates green, nothing committed yet.

---

## Part 1 — Work completed

### 1.1 The foundation: eight crates, contracts unchanged

| Crate | State | Evidence |
|---|---|---|
| `palace-wire` | done | framing, `ByteOrder`, 75-opcode table, codecs |
| `palace-probe` | done | live logon: 81 rooms / 2 users |
| `palace-room` | done | 804/804 corpus records parse |
| `palace-prop` | done | 5 decoders + S20 encoder; 227,874-prop differential = 0 disagreements |
| `palace-asset` | done | `qAst`/`sAst`/`rAst`, paced scheduler, media HTTP |
| `palace-render` | done | composites rooms → PNG; coordinate round-trip tested |
| `iptscrae` | done | lexer, parser, VM, budgets, capability trait, regex; zero dependencies |
| `iptscrae-palace` | done | 497-line host trait, 371 lines of Palace commands; corpus 2396/2400 parse, 3791/3805 handlers clean |

### 1.2 The interactive client milestone

- `crates/palace-client/` — headless runtime: connection supervisor, media worker thread, session
  state machine, frame store. Emits `ClientEvent`s, no Tauri types.
- `src-tauri/` (`palace-app`) — Tauri v2 shell; `palace://` URI scheme serves the frame as PNG.
- `src/` — SvelteKit 5 frontend: room viewport, room/user lists, chat, status bar, connect form.
- `crates/palace-client/src/bin/live-smoke.rs` — headless live harness.
- No `Vec<u8>` crosses a Tauri command; the webview receives one bitmap.

### 1.3 Bugs found and fixed

- **`navr_frame` built a malformed body** — the length constant was inside the payload, so the server
  read destination room 2 and navigation silently did nothing. Fixed; wire bytes pinned by test.
  Room switching now works live.
- **`xtlk`/`xwis` had no cipher** — encrypted chat degraded to `Message::Unknown`. Park–Miller
  Lehmer keystream implemented in `crates/palace-client/src/xtlk.rs`, LUT asserted against
  independently computed values.
- **The effect-application gap** — `apply_effect()` ended in a catch-all that logged effects and
  changed nothing. All 16 missing arms written, catch-all removed, match now exhaustive (deleting an
  arm fails the build with `E0004`).
- **Seven arms that applied nothing** — set `*dirty_render = true`, emitted a note, and mutated no
  state, so they forced a byte-identical re-render. An inert arm is indistinguishable from a working
  one inside a `match`, which is how it stayed hidden. All seven now mutate what the renderer reads.
- **`DIMROOM` never reached the room** — `palace-render` had `dim_level` and `apply_dim` all along;
  the builder hardcoded `1.0` and the runtime arm only logged. Three links added.
- **The receive path was mostly deaf** — 7 messages now decode and apply (`USERFACE`, `USERCOLOR`,
  `USERPROP`, `USERDESC`, `PROPNEW`, `PROPMOVE`, `PROPDEL`), so other users' faces, colours and worn
  props change, and loose props appear, move and vanish.
- **Name tags rendered nothing, because no name was ever supplied** — `palace-render` gained a complete,
  tested name-tag rasterizer, but `avatar_specs()` in `runtime.rs` built every `AvatarSpec` without
  `name`, so every tag was `None` and the renderer correctly drew nothing. The renderer was right and
  the wiring was absent — a whole feature silently inert with every test green. Now wired, and locked
  by a test that fails if the assignment is removed. Same class as the "seven arms that applied
  nothing" above: a component can be complete, correct, tested, and still not connected.

### 1.4 Windows portability — the build had never been *run*

The `v0.1.0` portable zip failed immediately with `os error 123`, because the session cache directory
was named `localhost:9998` and a colon is illegal in a Windows filename. Fixed and tested:

- `host:port` escaped to `localhost%3A9998` via a shared `escape_name_component` (also covers IPv6).
- `session_cache_dir()` extracted so the *wiring* is testable, not just the helper.
- Cache root prefers `XDG_CACHE_HOME` → `LOCALAPPDATA` → `HOME/.cache` → `temp_dir()`.
- `split_paths` replaced `split(':')`, which was mangling `C:\props`; `HOME` falls back to `USERPROFILE`.
- `write_media` refuses names Windows cannot hold (reserved device names, control chars, `:`).

**Lesson:** a cross-compile check cannot catch this class. `cargo check --target x86_64-pc-windows-gnu`
passed cleanly throughout — the problem is path *semantics at runtime*. Windows dev loop instructions:
`~/Sync/palace-client-windows-dev-setup.md`.

### 1.5 Door locks

Locked doors were walkable, because the lock state was never heard. `DOORLOCK`/`DOORUNLOCK` (`lock`/`unlo`)
and `SPOTSTATE` (`sSta`) now decode. Key fact: a door's locked state *is* the hotspot `state` field
(`HS_Unlock = 0`, `HS_Lock = 1`), so no new model field was needed. A click on a locked door is
refused with a transcript note instead of dispatching `SELECT`.

**CORRECTED (this session): the refusal used to cover `HS_Door` (1) too, and that was a bug — it broke
every ordinary door.** The spec (`PalaceProtocolRef.txt`, hotspot record) says `state` "selects which of
the pictures associated with the hotspot should be displayed. *Among other things*, it encodes whether a
door is locked or unlocked." So `state` is **primarily a picture index**; a plain `HS_Door` (1) cannot be
locked at all, and its `state == 1` is just the second frame of a two-picture door. Refusing it meant the
click never dispatched, so the door's script (`ME DEST GOTOROOM` on Colosseum) never ran: *"no doors seem
to work."* The rule now applies to `HS_LockableDoor` (3) only. The asymmetry argument below is what should
have caught this — the code erred on the wrong side of it.

### 1.6 Opcodes: what is vestigial, settled permanently

Four room-mutation opcodes (`SPOTSETDESC`, `PICTNEW`, `PICTDEL`, `PICTSETDESC`) have **no documented
body anywhere**. Proof: grepping the spec for `struct ClientMsg_(pict|spot)` returns exactly five
definitions; the protocol's own legend includes a glyph for *"defined but unused"* and says such
messages *"will not appear in the individual message descriptions."* Corroborated across nine
authorities on disk. **Do not implement these** — the width and field order are unknowable, and a
wrong decoder is worse than an honest "ignored opcode" line.

A further 14 opcodes are vestigial (`PROPSETDESC`, `ASSETNEW`, `USERENTER`, `SERVERUP`, `WMSG`, `NOOP`,
`TIMYID`, `RESPORT`, `TROPSER`, `INITCONNECTION`, and the four above).

### 1.7 The opcode-coverage audit

All 40 undecoded opcodes classified: 7 already handled elsewhere, 14 vestigial, 1 unresolved, 18 real
but non-blocking. **Nothing still missing breaks using the client on a public server**, with one
exception.

### 1.8 This session's most recent stretch

| Commit | What |
|---|---|
| `ec21f1e` | `AUTHENTICATE` decoded and reported honestly rather than silently stalling |
| `97fe6b9` | the opcode-coverage audit recorded so it is not repeated |
| `890b325` | two unanswered hotspot-click questions recorded |
| `d4b053d` | only a door that can be locked refuses a click (narrowing correction) |
| `ceaa998` | refuse a media name a Windows filesystem cannot hold |
| `12ec28c` | four room-mutation opcodes recorded as vestigial, not missing |
| `6d24495` | hotspots appearing/moving/vanishing (`SPOTNEW`/`SPOTDEL`/`SPOTMOVE`/`PICTMOVE`) |
| `9306590` | door locks, and refusing to walk through a locked door |
| `23acbd3` | run on Windows, not just compile there |

Verified green: `fmt` clean, `clippy -D warnings` clean across all six protocol/render/client crates,
all suites passing. Each new test was proven real by cutting its application arm and confirming the
failure, then restoring.

### 1.9 Current uncommitted work — name tags, walking, visibility, avatar picker

Four features, all landed in the working tree and gated green (`cargo fmt` clean,
`clippy -D warnings` clean for the touched crates, `palace-client`/`palace-render`/`palace-wire` suites
passing, `bun run check` 0 errors 0 warnings, `bun run build` succeeds). **Not committed.**

| Area | What landed |
|---|---|
| `palace-wire/src/frame.rs` | `user_move_frame`, `user_face_frame`, `user_color_frame` — each byte-pinned in **both** endians against the spec |
| `palace-render/src/nametag.rs` | name-tag raster (438 lines) + embedded OFL font + licence; 8 unit tests |
| `palace-render` | `name_tags_visible` on the scene/builder, drawn at layer 9 (above avatars, below the above-name-tags band) |
| `palace-client` | floor click → `USERMOVE` (clamped like the renderer, applied locally) + `spec.name` wired; `set_visibility`; `set_avatar` |
| `src-tauri` | `set_visibility(names, avatars)`, `set_avatar(face, color)`, and `palace://localhost/faces` serving the sheet |
| `src/` | right-click room menu (Show names / Show avatars / Choose an Avatar…), `AvatarDialog` (13×16 picker), `api.ts` + store wiring |

Two traps worth remembering:

- **`Point::new(v, h)` is `(y, x)`** — `v` (vertical) comes first on the wire, the opposite of what
  `Point::new(x, y)` suggests. Every encoder here depends on getting that right.
- **The name-tag glow is a dilation approximation** of Flash's Gaussian `GlowFilter`, not a
  reproduction. Documented as such in `nametag.rs`; do not "fix" it toward pixel-parity without a
  reference capture.

### 1.10 Found by live testing on Colosseum — four more bugs, all with a common shape

Live testing against Colosseum (room ids into the 73xxx range, scripts far more demanding than the
corpus) exposed four defects. The pattern is worth naming: **every one was a component that was
individually correct, tested, and disconnected** — the same shape as the name-tag bug in §1.3.

| Bug | What it looked like to the user | Root cause |
|---|---|---|
| Room ids >65535 **clamped** instead of truncated | Clicking a team/door hotspot did nothing, silently | `typedef sint16 RoomID`, so `73251` must narrow to `7715`; `clamp(0, 65535)` sent a nonexistent room. Two sites: `palace-host/src/wire.rs`, `palace-client/src/state.rs` |
| `MSG_NAVERROR` (`sErr`) not decoded | A refused room change was **completely silent** | refNum carries the code (`SE_RoomUnknown 1`, `SE_RoomFull 2`, …); now decoded and surfaced as a chat error with a name |
| Script moves (`SETPOS`/`MOVE`) mutated nothing | Doors/teleports did nothing; the note claimed success | `Effect::MoveUserAbs`/`MoveUserRel` set `dirty_render` and logged, but the frame was sent *and* the local avatar was never updated — and the server does not echo your own `uLoc`. Now both use one shared `move_target()` so the sent and applied positions cannot diverge |
| Plain doors refused as "locked" | "No doors seem to work" | `state` is primarily a *picture selector*; treating `state == 1` as locked on `HS_Door` (1) blocked dispatch. Narrowed to `HS_LockableDoor` (3) — see §1.5 |

Two traps worth keeping:

- **A silent failure is worse than a loud one.** The room-id bug was invisible for exactly as long as
  `NAVERROR` was undecoded. Where a server can refuse us, decode the refusal.
- **`state` is overloaded.** Doors use one field for both "which picture" and "locked"; the spec's
  "among other things" is doing a lot of work. Read hotspot fields per type, never globally.

**The room-id conclusion is verified, not assumed** (it was the one load-bearing guess here):
`typedef sint16 RoomID` in the spec (:222), `UserListRec.room_id: i16` (spec §3.18), our own
`RoomListRec` comment ("a 32-bit *field* even though room ids are 16-bit"), and — decisively —
**pserver's own `typedef std::pair<int16_t, Room*> RoomID`** (`include/server.hpp:170`). The id space
is 16-bit in the server, so no room can be `73251`; truncating to `7715` is what addresses the room the
script means. The body is 2 bytes, which small-id navigation working live already proves. pserver's
repo here is a stub (the opcode is defined, no handler), so this rests on the type plus the working
behaviour, not on reading its parser.

Also found (being fixed as this was written): **`SETUSERNAME` was local-only** — the rename never
reached the server, so other players never saw a character change. `usrN` is bidirectional with a
revert-on-failure path, and now has an encoder and decoder.

---

## Part 2 — Work still to do

### 2.1 BLOCKED ON A DECISION — authentication (the only hard blocker)

**Where:** `crates/palace-wire/src/messages/logon.rs`, plus a new `AUTHRESPONSE` encoder and a
credential source.

On a server that requires authentication, the server sends `auth` (`AUTHENTICATE`) after logon and
waits for `autr` (`AUTHRESPONSE`). The request is now **decoded and reported** in the transcript, so
the failure is explained instead of silent — but **the exchange is still unimplemented, so an
auth-requiring server will still refuse this client.**

Two routes, both needing a decision:

- [ ] **(a) Implement the reply.** Needs a credential source. There is **no password anywhere** in
  `ClientConfig` or `Settings` today, so this first requires deciding *where a credential lives*
  (config file? interactive prompt? system keyring?). The reply is a PString of `user:password`
  (`PalaceProtocolRef.txt` :750-754). *Recommended* — every real client advertises this capability
  precisely because it can authenticate.
- [ ] **(b) Stop advertising the capability.** One line: `aux_flags = 0x8000_0008` → `0x0000_0008`,
  plus updating the test that pins it. **Not done unilaterally** because it departs from the captured
  reference logon and could change behaviour on the critical logon path; the research that would have
  settled whether it is safe did not complete.

**Why this bites at all:** our own logon *asks* for the challenge we cannot answer.

**Current state is safe:** an auth-requiring server yields an explained failure, not a hang.

### 2.2 Publish the current work

- [ ] Re-run the scrub-and-push to bring the public copy up to date.

```bash
tools/publish-public.sh git@github.com:example-user/palace-client.git
```

There is **no git remote** on this repo — publication is a scrubbed clone pushed to the public repo.
The published copy was last built when this repo was at 76 commits; it is now at 84, so the published
copy is behind by roughly 8 commits. The Windows build in that release therefore contains none of the
`ec21f1e`…`23acbd3` work. `tools/public-scrub.map` is gitignored and local-only.

### 2.3 Protocol features that are decoded-but-local or missing

- [ ] **`SETLOC`/`SETPICLOC` never reach the server.** The non-local variants should broadcast so the
  whole room sees a move, but `SPOTMOVE` (`coLs`) and `PICTMOVE` (`pLoc`) have **no verified body
  layout** to port. Every other encoder in `wire.rs` is pinned byte-for-byte against a working
  reference sender; OpenPalace's own `moveSpot`/`setPicOffset` are no-op stubs. Until a reference is
  found, a script's spot move is local to this client only.
- [ ] **`durl` (DISPLAYURL) — the one unresolved opcode.** The spec documents a body; no server
  anywhere constructs it.
- [x] **`protocol.md` loose-prop traversal — already correct.** The section now reads "steps *backwards
  through the buffer*. That is a fact about the byte offsets, **not** about the logical order", which is
  exactly the correction this item asked for. Stale entry.
- [x] **`STATUS.md` decoded count — fixed.** It said "40 undecoded opcodes (75 in the table, 35 decoded)"
  while the same section said `AUTHENTICATE` is now decoded. Now states which snapshot the 40 refers to
  and gives the current split (36 decoded / 39 undecoded).

### 2.4 Open questions from the door work (unchecked, not known-broken)

We currently only run a hotspot's `ON SELECT` script on a click, and send nothing ourselves. That may
be sufficient if the room's script does the work:

- [ ] **`HS_Bolt` (4)** — "bolt that locks or unlocks door pointed to by `dest`" (:1668). If the client
  is meant to send `DOORLOCK`/`DOORUNLOCK` for the door at `dest`, we do not, and clicking a bolt does
  nothing unless the room's script covers it.
- [ ] **`HS_ShutableDoor` (2)** — "a door that can be opened/closed (by clicking)" (:1666). Closing
  implies something advances the state. If the client is meant to send `SPOTSTATE`, we do not.

Both settle cheaply by reading how a reference client handles a click on those types.

### 2.5 Feedback channels worth decoding (ranked; none is blocking)

| | Opcode | What it would fix |
|---|---|---|
| 2 | `down` SERVERDOWN | a forced disconnect (kick/ban/flood/full/shutdown) shows as a bare socket close with no reason |
| 3 | `sErr` NAVERROR | a failed room change (full/closed/pinned/password/kicked) is silently ignored |
| 4 | `usrN` USERNAME | ~~live renames are invisible; no way to rename yourself~~ **being fixed** — `usrN` now has an encoder and decoder, so `SETUSERNAME` reaches the server (and the spec's revert-on-failure path applies). See §1.10 |
| 5 | `draw` DRAW (receive) | other people's paint is dropped at decode |
| 6 | `sRom` ROOMSETDESC | room edits made while you are inside are not reflected — pserver does send this |
| 7 | `blow` BLOWTHRU | the plugin-relay channel |
| 8 | `sFil` / `fnfe` / `qFil` | legacy server-hosted file transfer, superseded by HTTP media |
| 9 | `susr`, `kill`, `gmsg`, `smsg`, `rmsg`, `nRom`, `sInf` | send-side: become wizard, kick, global shout/page, create room, server info |

- [ ] Pick these off in order as they annoy you; #2 and #3 are the ones a user would actually notice.

### 2.6 Rendering and presentation gaps

- [x] **Name tags — done** (§1.9): rasterized with an embedded font, positioned at the reference
  formula, drawn above avatars, toggleable. The glow is a documented approximation, not Flex's
  `GlowFilter`.
- [ ] **Everything else in rasterization** — `LINE`/`LINETO` are dispatched and dropped; chat text and
  draw strokes are not rasterized into the frame. Presentation-layer font/line work.
- [ ] **Sound and MIDI playback** — `SOUND`, `MIDIPLAY`, `MIDILOOP`, `MIDISTOP`, `BEEP` are
  dispatched and reported, but nothing plays.
- [ ] **`WEBEMBED`** is not registered; `GOTOURL` is reported rather than opened.
- [ ] **Deco, prop editor UI, theming** — out of scope so far, untouched.

### 2.7 The placeholder-art constraint

- [ ] **Make the face grid data-driven.** The real limitation is *not* art quality: the grid
  (13 faces × 16 colours × 44 px) is hardcoded in the binary, so different art or a different face
  count is a code change. Describing the grid in a small data file and loading art from a folder like
  every other Palace asset is what would make the placeholder genuinely replaceable. Polish, not a
  blocker. **Now worse:** the new avatar picker (§1.9) hardcodes `13`/`16`/`44` a second time in
  `AvatarDialog.svelte`, so a face-count change means editing the renderer, the protocol constant and
  the dialog. Any data-driven fix must cover all three.
- A pixel-perfect re-extract (208 clean 44×44 cells) is parked at
  `~/ProgramFiles/palace-faces-placeholder/` — a zero-code-change drop-in. Current sheet is a
  *resampled* copy: 1,467 colours per cell against the source's 200 (interpolation), 807 KB against
  182 KB, for a worse result.

### 2.8 Known pre-existing gaps (carried over)

- [ ] Multi-block asset transfer — reference-derived only, no real capture exists.
- [ ] 16-bit props — zero real samples.
- [ ] All 155 loose-prop IDs in the local corpus are missing from local stores.
- [ ] `pserver_full.prp` is systematically corrupt.
- [ ] Avatar anchor offset `x−22, y−22`; one reference uses −21 (1 px ambiguity).

### 2.9 Needs a human, not a script

- [ ] **Click the real zoom slider once.** The last visual pass went through the runtime command path,
  not a mouse drag, because the screen locked mid-QA. Everything is wired to the same `set_viewport`
  command and the numeric sweep matches the tested transform — but a human should click it.
- [ ] **Open the new right-click menu and the avatar picker once.** Both are new UI (§1.9). The
  *numeric* core is now verified objectively: the sheet is exactly `572×704 = 13×44 × 16×44` with no
  padding, and the picker's `-(face*44)px -(color*44)px` addresses the same cell the renderer's
  `smiley_cell` reads (`face → x/column`, `color → y/row`, `face.rs:63-64`), so a pick cannot land on
  the wrong cell. What remains unverified is only *appearance* — that the menu and dialog look right,
  sit inside the viewport, and read well. `svelte-check` cannot tell you that, and neither can I
  without driving your desktop, which this project forbids.
- [ ] **Walk somewhere on a live server.** Click-to-walk is tested against the mock harness only; the
  claim that the server does not echo your own `uLoc` back (so the client must apply it locally) is
  from the protocol reference, not from a real observation.
- [ ] **Watch a real second user's face change arrive** from Balamb Garden. The whole receive path is
  verified against the mock harness only; nobody has seen it live. On Balamb Garden the logged-on user
  arrives with `props=0`, so avatar *art transfer* for other users is also still unproven.
- [x] **Re-run `bun install && bun run check`** — done this session: `svelte-check found 0 errors and
  0 warnings`, and `bun run build` succeeds.

### 2.10 Housekeeping

- [ ] Commit `6d24495`'s message contains a `MSG(8->)` typo, left unamended rather than rewriting
  history unasked. Fix only if the history is being rewritten anyway.
- [ ] `feat/palace-ui`, `feat/events`, `feat/iptscrae` branches are kept as a safety net (all
  contained in `master`). Delete once publication makes them redundant.

### 2.11 Colosseum rooms load their logic from server-hosted scripts (`LOADSCRIPT`) — unimplemented

Colosseum rooms call `"big-script.txt" LOADSCRIPT` on entry and define their important functions in
that file, not in the room payload. Verified against room 31000 ("The Colosseum (5v5)"): the room
payload defines `cdead`, but **`bouncedef`, `deader` and `pinchat` have zero definitions in it** — they
come from `big-script.txt`. The same command appears in `Colosseum_Lobby`, `Battle_Prep_bp`,
`Entrance_xlobby`, `Battle_Arena_blackroom` and room 31743.

`LOADSCRIPT` is **not a command in any reference client** (OpenPalace/QPalace/Taj only have an unrelated
internal `loadScripts()` method), so it is a pserver extension. Fetching the file requires the legacy
server-hosted file transfer (`sFil` / `qFil` / `fnfe`), which §2.5 ranks as superseded by HTTP media and
does not implement.

**Consequence:** any room whose behaviour depends on an externally-loaded script will misbehave in ways
that look like script bugs but are missing definitions. Before diagnosing a Colosseum room as broken,
check whether the function it calls is defined in the room payload at all:

```bash
grep -c "bouncedef DEF" ~/palace-corpus/animanic_walk/room_31000.txt   # 0 = it lives in big-script.txt
```

- [ ] Implementing this means the legacy file-transfer path plus loading a fetched script into the VM.
  Scope it deliberately; it is the difference between "Colosseum rooms mostly work" and "they work".

### 2.12 Colosseum rooms fetch their logic over HTTP, after a room lifecycle we never fire

The other half of §2.11, and larger. Counted across `~/palace-corpus/scripts/` and
`~/palace-corpus/animanic_walk/`:

| | Uses | Implemented |
|---|---|---|
| `HTTPGET` in room scripts | **50** | ❌ — our own classifier lists it as an *unregistered* name |
| `ON HTTPRECEIVED` handlers | 7 | ❌ (the event is defined, never fired) |
| `ON ROOMREADY` / `ON ROOMLOAD` | 8 / 8 | ❌ (events defined, never fired) |

Real examples: `"http://api.animanic.de/deviantart/" HTTPGET`, and `"ludo/" HTTPGET` — a **relative**
URL, so it resolves against the server's HTTP origin. We already have that origin:
`state.banner.media_base`, used by the media fetcher.

**The room lifecycle order is now known exactly**, taken from a reference call site
(`~/palace-corpus/reference/repos/sparky/index.js`):

```js
ri(), Si(), C2(), e.setRoom(payload), Bc(payload.spots), w2(), Ze("ENTER", spots), y2()
//                                                          ROOMLOAD   ENTER        ROOMREADY
```

So it is **ROOMLOAD → ENTER → ROOMREADY**. We fire only `ENTER`. Rooms put their fetches in
`ON ROOMREADY {"ludo/" HTTPGET`, which is why they never run for us.

`sparky` also gives the argument semantics, which are easy to get wrong:
- `STATECHANGE` is **spot-scoped** and receives the **previous** state as `lastState`.
- `NAMECHANGE` is room-level with `whoChangeId` + `lastName`; `USERLEAVE`/`USERENTER` are room-level
  with `whoLeaveId`/`whoEnterId`; `SERVERMSG` with `chatStr`.
- `HTTPRECEIVED` is **spot-scoped when the `HTTPGET` came from a spot's handler**, room-level
  otherwise, and carries `httpContents`, `httpHeaders`, `httpContentType`, `httpFilename`, `httpUrl`.
- Events the reference has that our enum lacks entirely: `USERENTER`, `FACECHANGE`, `COLORCHANGE`,
  `USERMOVE`, `PROPCHANGE`, `IDLE`, `LOOSEPROPADDED`/`LOOSEPROPMOVED`/`LOOSEPROPDELETED`.

- [ ] Implement in this order: the room lifecycle events, then `HTTPGET` + `HTTPRECEIVED`/`HTTPERROR`.
  With those, `LOADSCRIPT` (§2.11) is plausibly just "fetch, then execute" on the same mechanism.

### 2.13 Commands Colosseum uses that this client does not act on, ranked

Method: count uppercase tokens across `~/palace-corpus/scripts/` + `~/palace-corpus/animanic_walk/`, then
subtract the command table (`crates/iptscrae-palace/src/commands.rs`), the interpreter's builtin
registry (`crates/iptscrae/src/registry.rs`), **and the explicit recognised-but-unimplemented arm at
`crates/palace-host/src/host.rs:694-704`**. Skipping that third set is what produced the first, wrong
version of this list — count the operand signatures, not just the names.

**The precise status of everything below: the command is recognised and its operands are consumed
correctly (so the stack stays balanced and `IF` still parses), but `unimplemented()` reports it and
nothing happens.** It is not "unknown to the VM".

| Command | Uses | Note |
|---|---|---|
| `ADDPIC` | 115 | needs the client→server picture body, undocumented everywhere (§1.6). May be permanently blocked — do not guess it |
| `SETSPOTSCRIPT` | 82 | needs a spot-script wire body |
| `ADDSPOT` | 81 | as `ADDPIC` |
| `LOADSCRIPT` | 51 | §2.11 |
| `HTTPGET` | 50 | §2.12 |
| `SETTOOLTIP` | 47 | hover text; needs ROLLOVER/ROLLOUT dispatch first |
| `CLEARTOOLTIP` | 41 | as above |

Work order: **room lifecycle + HTTP (§2.12)** first — it gates `HTTPGET` and probably `LOADSCRIPT`;
then the hover pair (ROLLOVER/ROLLOUT + the tooltip commands); then the `ADDPIC`/`ADDSPOT` family, and
accept that it may be permanently blocked.

**Checked and NOT gaps** — recorded so nobody re-derives them as missing, which is what happened here:

- `SGLOBAL` (74) — an alias of the core `GLOBAL`, registered at `commands.rs:215`, with
  `sglobal_is_the_spot_scope_alias_of_global` covering it.
- `STR` (1454) — implemented at `host.rs:690` (int or string to string). Its count is also misleading:
  in these RPG scripts `STR` is overwhelmingly the **strength stat**.
- `HTTP` (75) — an opcode name (`HTTPSERVER`, `opcode.rs:137`), not a command. The bare uses are a variable.
- `IDLE` — an event our VM lacks but Colosseum never uses (0 occurrences).

### 2.14 How the HTTP extension actually works: the response *is* a script

This resolves the open question in §2.11/§2.12 — what a handler does with a fetched body. It does not
read the body, it **executes** it. From `~/palace-corpus/reference/repos/sparky/index.js`:

```js
executeScriptSource(source, spotId, url, source, eventName = "HTTPRECEIVED")
runCachedScripts(event, data)          // CACHESCRIPT'd sources re-run when that event fires
```

The fetch handler branches on content type:

```js
const [type, sub] = splitContentType(contentType);
type === "text" && (sub === "iptscrae" || sub === "ipt") && iptscraeEnabled && body
  && executeScriptSource(body, spotId, url, body)
```

So **a fetched resource whose content type is `text/iptscrae` (or `text/ipt`) is run as IPTSCRAE**. That
is the mechanism behind both `"ludo/" HTTPGET` in `ON ROOMREADY` and `LOADSCRIPT "big-script.txt"`:
the room fetches its own logic and the client executes it. `CACHESCRIPT` caches such a source so it can
be re-run later for a named event, and `HTTPCANCEL` aborts one in flight.

Consequences for the plan:

- §2.11 and §2.12 are **one feature, not two**: `HTTPGET`, `LOADSCRIPT` and the `text/iptscrae` branch
  are the same fetch-and-execute path.
- The media pipeline already fetches files (that is how `notebarw.gif` arrives), so the smallest useful
  step is the **content-type branch**: execute a fetched `text/iptscrae` response instead of storing it
  as media. That alone may be what rooms like 31000 are missing.
- The camelCase properties `httpContents` / `httpHeaders` / `httpContentType` / `httpFilename` /
  `httpUrl` (sparky) are for handlers that want the raw payload rather than executing it. Their
  *accessor spelling inside a script* is still undetermined — no reference spells it in a script, the
  protocol reference has no `HTTPGET` section, and Colosseum's handlers delegate to `preload`, which is
  defined in the very external script we cannot fetch. Determine that before implementing raw-data
  access; do not guess it.
- The concurrency guard matters: sparky rejects a request when too many are in flight
  (`Uc(url, method)` against a cap), so an implementation should bound in-flight requests.

- [ ] Implement as one task: the `text/iptscrae` execute branch on the fetch path, then `HTTPGET`
  (absolute URL, or relative joined to `state.banner.media_base`, scoped to the executing hotspot),
  then `CACHESCRIPT`/`HTTPCANCEL`.

### 2.15 The prop bag

Palace's "prop bag" (OpenPalace: `PropsWindow.mxml` + `model/PropBag.as`) is the user's **own collection
of props**, plus **saved outfits**, with toggle-wear, delete, delete-all and "naked". It is not the same
thing as `palace-prop`, which is the prop *file codec*; the bag is the collection on top of it.

**There is a real bag on this machine.** `~/.local/share/PalaceChat/` is a live Palace client data
directory, and `PropBag.bundle/` holds:

| File | Meaning |
|---|---|
| `PalaceChat.pids` | the bag index — 61504 bytes |
| `PalaceChat.props` | the props — 8939938 bytes |
| `PalaceChat.favs`, `Trash.favs` | favourites and trash |
| `Version`, `macro` | version byte, macros |

**`PalaceChat.pids` format — confirmed, not inferred.** 16-byte big-endian records
`(a: u32, b: u32, offset: u32, size: u32)`. Figures below are from the first measurement (the file
grows while the client runs):

- 61504 / 16 = 3844 with remainder 0; every record satisfies `offset + size <= 8939938` (0 violations);
- offsets are non-decreasing and the blobs **tile `.props` contiguously — every adjacent pair
  satisfies `off[i] + size[i] == off[i+1]`**;
- **`(a, b)` is the prop's identity** — `BagThumbCache/*.png` is named `<a:08X>_<b:08X>.png` and
  **11/11** of those keys are present in `.pids`. The client's own cache is keyed by the same pair,
  which is why `(a, b)` is treated as the key rather than an index position;
- each blob is a **fixed 32-byte metadata prefix** followed by a normal prop at `blob + 0x20`.
  Verified across every record: **3846/3846 blobs have exactly `00 2c 00 2c` at offset 32** (a
  big-endian 44x44 header). The prefix length is therefore constant; its *content* is not zero —
  only **223/3846** are all-zero, the rest carry length-prefixed ASCII names (e.g. `explosion2`,
  `NewProp`, `The Colosseum (1 vs 1)`).

An earlier version of this section called that a "32-byte zero prefix", generalised from blobs 0 and
1. That was wrong: it is a metadata prefix that happens to be empty on some props, and the fixed
length is the part that matters. The reader skips 32 bytes.

**`(a, b)` semantics — measured, not guessed.** `a` is the prop/asset **id** and `b` is the **payload
CRC** (`asset_crc` over `prop[12..]`, the blob minus its 12-byte header):

- `b == payload_crc(prop[12..])` for **3569/3846** records. Spot-check: record with
  `a = 0x3a3ad1f7` has `b = 0xffa0f716`, which is exactly the CRC of its payload — and
  `0x3a3ad1f7` is a real `.prp` record id the crate already documents.
- The exceptions are the synthetic-id records (`a` in the `0x80000000` range with sequential small
  `b`) plus ~66 large props with `0x400`/`0x800` flag bits set (sizes up to ~670 KB, evidently
  animated/multi-frame) whose `b` is not the payload CRC over any slice tested. Undetermined for
  those; do not assume.

**The bag is live data.** It read 3844 records during the first measurement and 3846 minutes later,
while the user's client was running. Read it read-only, and never hard-code a record count.

Also on disk: `~/Pictures/Prop Files/` holds the source rosters (`Palace1.prp`, `AshFile*.prp`,
`idk.prp`) — the same `.prp` container `prop-tool` already parses, ~180k props. The *bag* is the curated
subset; the rosters are everything.

**Wearing a prop is two messages** (`OpenPalace/.../rpc/PalaceClient.as`):

- `ASSET_REGI` = `0x72417374` — uploads the prop's asset bytes, guarded to 44x44 with offsets in
  −44..88 ("web service big prop... ignore request").
- `USER_PROP` = `0x75737250` — **the same opcode as our incoming `USERPROP`**; sets the worn list as
  `(userId, count, (propId, crc=0)*)`, and the reference **caps the list at 9 worn props** while still
  writing the untruncated `count * 8 + 4` size field.

Our wire crate has only the incoming half, so both encoders are new work.

Plan, in dependency order:

- [ ] `palace-prop`: reader for `PropBag.bundle` (`.pids` index + `.props` blobs) + `prop-tool bag`.
- [ ] `palace-wire`: `ASSET_REGI` and `USER_PROP` encoders.
- [ ] A props panel: tile grid of the bag, toggle-wear, delete, save/restore outfit.
- [ ] Wear wiring in the runtime (last — it needs `runtime.rs`, and it is the piece that actually
  changes the session, so it waits until the reader and encoders are proven).

The bag is **live data belonging to a running client** — read it read-only; never write there.

### 2.16 Wearing props: nothing reaches the server — CONFIRMED live

A live run of the smoke harness (`crates/palace-client/src/bin/live-smoke.rs`) against
`media.palace.example.info:9998`:

```text
PALACE_HOST=media.palace.example.info PALACE_USER=SmokeTest PALACE_SMOKE_SECS=20 \
  cargo run -p palace-client --bin live-smoke
```

```text
[room] #901 "Balamb Garden" users=0
[script] ON ENTER: 1 handler(s) fired
        effect: MIDILOOP "garden" x99
        effect: SOUND "garden"
[users] 1
        #637 "SmokeTest" face=5 color=14 props=0 at (210,255)
```

Two things at once: **the room lifecycle fires live** (the point of §2.13), and **our own record
carries `props=0`**.

`HASPROP` reads `HostView::self_props`, built from our own entry in `state.users`
(`runtime.rs:1339`), so `HASPROP` is **always false** and prop-gated room logic is unreachable. The
second half of the cause is that nothing tells the server either — `Effect::SetProps`, `DonProp` and
`DoffProp` mutate local state and return `Vec::new()` without sending a frame. The `USER_PROP` encoder
already exists in `palace-wire` and is simply never called.

Consequence, and the reason it was noticed: the Colosseum menu's **Audience** button is a chain of

```text
{ 14109 GOTOROOM }  cname "Cyan" ==                        IF
{ 7592  GOTOROOM }  cname "Gogo" == 1020771340 HASPROP AND IF
```

so every branch is unreachable — the button does nothing. The **Red/Blue** team buttons work because
they check only `USERNAME` (`USERNAME "Elly" == IFELSE`), which the server does know. That asymmetry
is the signature of this bug: name-gated logic works, prop-gated logic cannot.

- [ ] Send `USER_PROP` when the worn list changes (in progress).
- [ ] Decide whether `ASSET_REGI` (uploading the art) is required for the server to accept a worn
  prop, or whether `USER_PROP` alone suffices.

### 2.17 The Colosseum "you get kicked out" bounce is the room working as designed

The audience rooms carry a capacity guard:

```text
{ "@512 0Too many audience members; space needed for players!"
  LOCALMSG  "wrongo.wav" SOUND  { 31743 GOTOROOM } 80 ALARMEXEC }
NBRROOMUSERS 19 > IF
```

Over 19 users (17 for non-wizards) they announce the message, play `wrongo.wav` and teleport you to
**room 31743, the Colosseum menu**. That is exactly the observed `GOTOROOM 31743` + `SOUND "wrongo.wav"`
and the "boots me out to the menu" symptom: the audience was full. `LOCALMSG` does reach the user —
the runtime renders it as a `System` chat line (`runtime.rs:1612`) — so the ejection is explained in
chat, not silent.

Do not "fix" this by suppressing the teleport.

### 2.18 Media comes from seeded directories, not (only) from fetching

`src-tauri/src/lib.rs` builds the media roots as `seed_media()` plus the session cache:

- `~/palace-corpus/http_harvest` (110 files) and `~/Sync/Colosseum-BGs/all` (280 files) — read-only seeds;
- `~/.cache/palace-client/<host%3Aport>/media/http-cache/media/<hash>/` — per-session fetch cache.

So the harness reports `background "sqoom23.gif" not found ... used flat backdrop` because it sets no
seeds and fetched nothing in its 20s run — an artifact of the harness, **not** a user-facing bug: the
app resolves art from the seeds (and has a populated fetch cache for other servers). Worth re-checking
in the GUI if a room ever looks flat.

### 2.19 Running the live harness

`live-smoke` runs the real runtime against a real server and prints every `ClientEvent`. It is the only
way to see script behaviour live, because the runtime has no logging of its own. Env: `PALACE_HOST`,
`PALACE_PORT` (9998), `PALACE_USER`, `PALACE_SMOKE_SECS`, `PALACE_SMOKE_ROOM`, `PALACE_CLICK_ROOM`.
It logs in as a normal user, so pick a name that makes the test obvious to anyone in the room.

---

## Part 3 — How to verify

```bash
cd ~/ProgramFiles/palace-client

cargo fmt --all -- --check
cargo clippy -p palace-client -p palace-wire -p palace-app \
             -p palace-host -p palace-probe -p palace-render \
             --all-targets -- -D warnings
cargo test -p palace-client -p palace-wire -p palace-render   # scoped; see the --workspace warning below

bun run check && bun run build                 # frontend
cargo run -p palace-client --bin live-smoke    # headless live check
```

**Never run `--workspace` cargo commands in a fresh worktree.** A new worktree has no `target/`, so it
compiles the entire Tauri/WebKit tree from scratch — 5.6 GB, 20–40 minutes — and again for each cargo
invocation. From outside this is indistinguishable from a hang. Scope to the crate, or set
`CARGO_TARGET_DIR` to reuse the warm target directory.

---

## Part 4 — Hard boundaries (do not cross)

Carried from `STATUS.md`, because they exist for a reason: a subagent once reached for `ydotool`
(kernel-level input injection) on this live desktop while trying to satisfy a "watch the window
resize" instruction.

- **Never inject input into the desktop** — no `ydotool`, `wtype`, `xdotool`, `xte`, or synthetic input.
- **Never unlock, wake or inhibit the screen.** A locked screen ends the QA run: stop and report.
- **Never screenshot the whole desktop.** App window only.
- **Never activate or focus the user's windows.**
- **Never ask for or use the user's password or any credential.**

Drive the app's own command path instead (`invoke("set_viewport", …)`), verify numerically or via
headless renders, and say plainly that a human still needs to click the real control once.

**Never write a definition of done that can only be satisfied by driving the user's desktop.**

Also: stage explicit paths, never `git add -A`, while another agent or opencode instance may share the
worktree — `-A` sweeps a concurrent editor's half-written files into your commit. And never
`pkill -f "<pattern>"` where the pattern appears in your own command string; it kills the shell
running it. Use `pgrep -x`/`pkill -x` or kill by PID.

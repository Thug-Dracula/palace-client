# palace-todo.md — live backlog

A working checklist for the Palace client. Completed work is not recorded here;
this file is only what is still open.

| File | Role |
|---|---|
| `README.md` | What The Palace is, and what this project is. |
| `STATUS.md` | Current state, evidence and hard boundaries. **The authority for what is true.** |
| `protocol.md` | The wire specification. |
| `$CORPUS/TAURI-CLIENT-SCOPE.md` | Design authority (scope and decisions). Read-only reference. |

**Snapshot:** the registry holds 72 core names plus 133 Palace commands; 40 of the
75 named opcodes are decoded.

---

## 0. What 1.0 means

The classic client, complete. The two items that were left are resolved or
re-scoped, and nothing else is required for the release line:

- [x] **Worn props are settable.** `set_props` is a Tauri command, registered and
  wrapped end to end. The rendering layer no longer needs new Rust to change
  what the signed-in user wears.
- [x] **A data-driven face grid.** The picker reads `faces`, `colors`, `cell` and
  the row grouping from `palace://faces.json`; the renderer derives that payload
  from the constants it crops with, so the two cannot disagree.

**Decided.** No component calls `set_props` yet, so "settable" is true of the
command and not of the interface. A minimal props control — list the worn props,
remove one — would use the `UserInfo.props` / `is_self` data the interface
already has and needs no Rust; it is deferred to Tier B rather than built for
1.0.

**Deliberately not in 1.0.** A prop bag browser (a tile grid over `PropBag.bundle`
with saved outfits) is PalaceChat 5 work, as is the rest of that tier: Type 1
avatars, the prop editor, extended IPTSCRAE, animated backgrounds, video doors,
embedded web panes, the translator. `$CORPUS/TAURI-CLIENT-SCOPE.md` estimates
the tier at +8–12 person-weeks and calls the web panes the hardest item in it. The
items in §1–§7 below are wanted but do not gate 1.0.

---

## 1. Top of queue

- [x] **Authentication.** A server that requires it sends `auth` (`AUTHENTICATE`)
  after logon and waits for `autr` (`AUTHRESPONSE`). The client now answers with
  the `PString` `user:password` when a credential is configured, and the logon
  advertises the capability only in that case: a plain logon still clears the bit
  (via `ClientProfile`), so a server is never promised a reply this client cannot
  give. `ReferenceProfile` keeps `0x8000_0008` for the walker-oracle tests.
  - **Credential source: environment or command line.** `PALACE_PASSWORD`, or
    `--password`/`--password=`. It is never read from or written to
    `settings.json`, and it renders redacted in `Debug`.

---

## 2. Protocol

- [ ] **`SETLOC`/`SETPICLOC` never reach the server.** The non-local variants
  should broadcast so the whole room sees a move, but `SPOTMOVE` (`coLs`) and
  `PICTMOVE` (`pLoc`) have no verified body layout to port, and the reference
  clients' own `moveSpot`/`setPicOffset` are no-op stubs. Until a reference is
  found, a script's spot move is local to this client only.
- [ ] **`durl` (DISPLAYURL) — the one unresolved opcode.** The spec documents a
  body; no server constructs it.
- [ ] **The unregistered fallback names.** The host's fallback arm lists 32
  commands it recognises but does not implement; 26 of them are still not in
  `PALACE_COMMANDS`, so a script that calls one lexes the name as a variable and
  misbehaves quietly. Registering a name turns that into an explicit report.
  - **Partly sourced.** `HIDESMILEYS` and `LOCKUSERPROPS` are now registered with
    0 operands, because the harvested corpus calls them bare inside a `{ ... }`
    version guard. The rest need pops/pushes from a reference: `sparky/index.js`
    carries a full `GS` command table (it is where `PALACECHAT`, `ENCODEURL` and
    `CONFIRMBOX` came from), but it has no entry for these 26, and neither does
    `reference/docs/iptscrae.txt` nor either AS3 tree. Guessing would corrupt the
    stack (`commands.rs:62`).
- [ ] **Feedback and send-side opcodes**, in the order a user would notice:

  | Opcode | What it would fix |
  |---|---|
  | `sRom` ROOMSETDESC | room edits made while you are inside are not reflected |
  | `blow` BLOWTHRU | the plugin-relay channel |
  | `sFil` / `fnfe` / `qFil` | legacy server-hosted file transfer, superseded by HTTP media |
  | `autr` AUTHRESPONSE | the reply an auth-requiring server waits for (§1) |
  | `susr`, `kill`, `gmsg`, `smsg`, `rmsg`, `nRom`, `sInf` | send-side: become wizard, kick, global shout/page, create room, server info |

---

## 3. Rendering and presentation

- [ ] **The remaining draw commands.** `CIRCLE`, `FILL`, `PAINT` and `TEXT` are
  not implemented, and `DRAW`'s text operands have a layout the references leave
  undetermined.
  - **The four names are reference-absent.** The AS3 `command/` tree implements
    only `LINE`/`LINETO`/`PEN*`/`PAINTCLEAR`/`PAINTUNDO`, and QPalace's IPT table
    has none of the four, so their operand counts and effects cannot be
    reference-derived (`commands.rs:62` forbids guessing; `TEXT` is not
    `DRAWTEXT`). They stay unregistered. The recoverable part of this item is the
    wire `DC_Text` operand layout.
- [ ] **A data-driven face grid.** The grid (13 faces × 16 colours × 44 px) is
  hardcoded in the renderer, the protocol constant and the avatar picker;
  describing it in a data file and loading art from a folder like every other
  asset would make the placeholder genuinely replaceable.
- [ ] **`WEBEMBED`** is not registered; `GOTOURL` is reported rather than opened.
- [ ] **Deco, prop editor UI, theming** — out of scope so far.

---

## 4. Audio

- [ ] **The bundled sound assets are not shipped.** `SOUND` consults a built-in
  table first and otherwise fetches `mediaServer + name + ".mp3"`; the table is
  empty because the reference client's MP3s are not ours to redistribute, so
  every name currently takes the media path. Filling the table is all that is
  left. The engine, its SoundFont path and its fallback tone are in
  `crates/palace-audio`.

---

## 5. Dynamic room content and the arena

The fetched arena interface script runs end to end: `LOADSCRIPT`/`HTTPGET`,
`ADDSPOT`, `ADDPIC`, `SETSPOTOPTIONS`, `SETPICLOCLOCAL`, `SETSPOTSCRIPT`,
`SETTOOLTIP`/`CLEARTOOLTIP` and the `ROLLOVER`/`ROLLOUT`/`MOUSEMOVE` events are
implemented, and the `comma_separator` lexer extension handles the served
dialect. What remains:

- [ ] **No live run.** The script-level chain is proven; nobody has watched the
  arena accept a player. That needs a human at the app.
- [x] **Quoted numeric strings are accepted.** `ADDSPOT`'s polygon operand is read
  by `point_list_arg`, which parses a `Value::Str` as a base-10 integer, so the
  quoted forms the extended dialect accepts (`"10"`) work; a non-numeric string
  is still dropped. `SETPROPS` keeps the strict reader, because the reference
  resolves a string operand there as a prop *name*, not a number.
- [ ] `SETSPOTSCRIPT` matches a literal uppercase `ON`, like the reference
  parser; a lowercase `on` in an existing source is not a handler to either.

---

## 6. Props

- [x] **`set_props` is reachable.** Exposed as a Tauri command
  (`src-tauri/src/commands.rs`), registered, wrapped in `api.ts` and surfaced as a
  store action, so the interface can change worn props without new Rust. Nothing
  calls it yet: the minimal props control is deferred to Tier B (§0).
- [ ] **A prop bag browser is Tier B, not 1.0.** A tile grid over
  `PropBag.bundle` with saved outfits is PalaceChat 5 territory
  (`$CORPUS/TAURI-CLIENT-SCOPE.md`, Tier B). Classic-client prop wearing is
  the 9-prop Type 0 limit, which is already implemented. A browser would also
  need a prop roster the project does not ship — the only full roster is the
  read-only reference `pserver.prp`.
- [ ] **`ASSET_REGI`.** Decide whether uploading a worn prop's art is required
  for the server to accept the prop, or whether `USER_PROP` alone suffices.
- [x] **The prop and picture geometry stubs are implemented.**
  `GETPICDIMENSIONS` resolves a spot and state (a negative state means the spot's
  current one) to its picture's size; `PROPDIMENSIONS` and `PROPOFFSETS` read a
  prop header (`PROPOFFSETS` less the reference's 22). The data reaches the host
  as an immutable `AssetFacts` snapshot carried in `HostView`, built by the
  runtime from decoded media and prop headers and cached on `Shared` — the script
  dispatch path only clones an `Arc`. Anything not yet loaded answers `(0, 0)`.
  `has_prop_by_name` stays false on purpose; the reference's is too.
  - **`GETPICLOC` aligned too.** It now resolves a negative state to the spot's
    current state (shared with `GETPICDIMENSIONS`), where it previously clamped
    to index 0.
- [ ] **`LOADPROPS` records nothing, but now validates.** It stays effect-free by
  design (a cache warm-up), and its operand is now checked as the reference
  checks it: more than 500 ids is refused (*"You may only load up to 500 props at
  a time."*), a non-integer element is refused (*"Only Prop IDs are allowed to be
  specified for LOADPROPS."*), and a non-array operand is a type error. The
  prefetch itself still needs a prop store the host cannot reach — the same
  plumbing the dimension work needs.

---

## 7. Known pre-existing gaps

- [ ] Multi-block asset transfer — reference-derived only, no real capture.
- [ ] 16-bit props — zero real samples.
- [ ] All 155 loose-prop IDs in the local corpus are missing from local stores.
- [ ] `pserver_full.prp` is systematically corrupt.
- [ ] Avatar anchor offset `x−22, y−22`; one reference uses −21 (1 px ambiguity).

---

## 8. Needs a human, not a script

- [ ] **Click the real zoom slider once.** The visual pass goes through the
  runtime command path, not a mouse drag, because the project forbids driving the
  desktop. The numeric sweep matches the tested transform; a human should still
  click it.
- [ ] **Open the right-click menu and the avatar picker once.** Their numeric
  core is verified (the sheet is exactly `572×704 = 13×44 × 16×44`, and the
  picker addresses the same cell the renderer reads); appearance is unverified.
- [ ] **Walk somewhere on a live server.** Click-to-walk is tested against the
  mock harness only.
- [ ] **Watch a real second user's face change arrive.** The receive path is
  verified against the mock harness only; a busy server has not been available.

---

## 9. How to verify

```bash
cd $REPO

cargo fmt --all -- --check
cargo clippy -p palace-client -p palace-wire -p palace-host -p palace-probe \
             -p palace-render -p palace-prop -p palace-asset \
             -p iptscrae -p iptscrae-palace -p palace-room \
             --all-targets -- -D warnings
cargo test -p palace-wire -p palace-room -p palace-prop -p palace-asset \
           -p palace-probe -p palace-render -p palace-client \
           -p iptscrae -p iptscrae-palace -p palace-host

bun run check && bun run build                 # frontend
cargo run -p palace-client --bin live-smoke    # headless live check
```

**Never run `--workspace` cargo commands in a fresh worktree.** A new worktree
has no `target/`, so it compiles the entire Tauri/WebKit tree from scratch
(5.6 GB, 20–40 minutes) and again for each cargo invocation. Scope to the crate,
or set `CARGO_TARGET_DIR` to reuse the warm target directory.

---

## 10. Verifying the interface

See **`STATUS.md` → Verifying the interface**. In short: the interface can be
checked through the app's own command path and headless renders, so a numeric
result is enough to prove a transform. Appearance still needs a human once.

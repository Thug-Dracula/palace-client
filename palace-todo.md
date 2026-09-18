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

## 1. Top of queue

- [ ] **Authentication — blocked on a decision.** A server that requires it sends
  `auth` (`AUTHENTICATE`) after logon and waits for `autr` (`AUTHRESPONSE`). The
  request is decoded and reported; the reply is not implemented, so an
  auth-requiring server refuses this client.
  - **Done — the capability is no longer advertised.** The application sends
    `aux_flags` `0x0000_0008` (via `ClientProfile`); `ReferenceProfile` keeps
    `0x8000_0008` for the walker-oracle tests. A server is no longer told this
    client can answer a challenge it cannot.
  - **Open — implement the reply.** `AUTHRESPONSE` is a PString of
    `user:password`, so it needs a credential source. There is no password field
    anywhere in `ClientConfig` or `Settings`, so this first needs a decision
    about where a credential lives (config file, interactive prompt, system
    keyring).

---

## 2. Protocol

- [ ] **`SETLOC`/`SETPICLOC` never reach the server.** The non-local variants
  should broadcast so the whole room sees a move, but `SPOTMOVE` (`coLs`) and
  `PICTMOVE` (`pLoc`) have no verified body layout to port, and the reference
  clients' own `moveSpot`/`setPicOffset` are no-op stubs. Until a reference is
  found, a script's spot move is local to this client only.
- [ ] **`durl` (DISPLAYURL) — the one unresolved opcode.** The spec documents a
  body; no server constructs it.
- [ ] **Register or remove the unregistered fallback names.** The host's fallback
  arm lists 32 commands it recognises but does not implement; 28 of them are not
  in `PALACE_COMMANDS`, so a script that calls one lexes the name as a variable
  and misbehaves quietly. Registering them changes that silent misbehaviour into
  an explicit report; implementing them is the larger task.
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

- [ ] **Chat text rasterization.** Draw commands and name tags are rasterized;
  chat text is the one layer that is not.
- [ ] **The remaining draw commands.** `CIRCLE`, `FILL`, `PAINT` and `TEXT` are
  not implemented, and `DRAW`'s text operands have a layout the references leave
  undetermined.
- [ ] **A data-driven face grid.** The grid (13 faces × 16 colours × 44 px) is
  hardcoded in the renderer, the protocol constant and the avatar picker;
  describing it in a data file and loading art from a folder like every other
  asset would make the placeholder genuinely replaceable.
- [ ] **`WEBEMBED`** is not registered; `GOTOURL` is reported rather than opened.
- [ ] **Deco, prop editor UI, theming** — out of scope so far.

---

## 4. Audio

- [ ] **`SOUND`** is surfaced but never reaches an audio device. The reference
  plays a bundled name from a built-in map, and otherwise fetches
  `mediaServer + name + ".mp3"`; the media pipeline already fetches and caches
  arbitrary media by name, so the fetch-by-name path is the one that matters.
- [ ] **`MIDIPLAY`/`MIDILOOP`/`MIDISTOP`** need a synthesizer, not just a decoder,
  and are a follow-up to `SOUND`.

---

## 5. Dynamic room content and the arena

The fetched arena interface script runs end to end: `LOADSCRIPT`/`HTTPGET`,
`ADDSPOT`, `ADDPIC`, `SETSPOTOPTIONS`, `SETPICLOCLOCAL`, `SETSPOTSCRIPT`,
`SETTOOLTIP`/`CLEARTOOLTIP` and the `ROLLOVER`/`ROLLOUT`/`MOUSEMOVE` events are
implemented, and the `comma_separator` lexer extension handles the served
dialect. What remains:

- [ ] **No live run.** The script-level chain is proven; nobody has watched the
  arena accept a player. That needs a human at the app.
- [ ] `ADDSPOT` reads its points through `props_arg`, which keeps integers only;
  the reference also accepts quoted numeric strings.
- [ ] `SETSPOTSCRIPT` matches a literal uppercase `ON`, like the reference
  parser; a lowercase `on` in an existing source is not a handler to either.

---

## 6. Props

- [ ] **A props panel.** Worn props are sent to the server, but nothing in the
  interface can set them, so the feature is only half reachable. The bag reader
  in `palace-prop` parses `PropBag.bundle` already; the panel needs a tile grid,
  toggle-wear, delete and saved outfits.
- [ ] **`ASSET_REGI`.** Decide whether uploading a worn prop's art is required
  for the server to accept the prop, or whether `USER_PROP` alone suffices.
- [ ] **The constant prop stubs.** `GETPICDIMENSIONS` returns `(0, 0)`;
  `PROPDIMENSIONS`/`PROPOFFSETS` push zeros and `has_prop_by_name` returns
  false. `GETPICDIMENSIONS` needs a source of picture dimensions (the PNG
  `IHDR`, or the decode already done), a `pict_id → (w, h)` map in `HostView`,
  and the runtime populating it before dispatch. `PROPDIMENSIONS`/`PROPOFFSETS`
  need the equivalent for props.
- [ ] **`LOADPROPS`** is effect-free by design (a prefetch with a 500-id limit).
  The real gaps are the 500-limit error the reference throws (we accept anything
  silently) and the prefetch itself, which needs a prop store the host cannot
  reach yet — the same plumbing the dimension work needs.

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

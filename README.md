# Palace Client

A desktop client for **The Palace**, the graphical chat system that was popular
in the 1990s.

## What The Palace is

The Palace was an online chat service where people met in illustrated rooms
instead of plain text channels. Every room had a background picture, and each
person in it appeared as a small character image called a **prop**. Rooms were
programmable: the server ran short scripts that reacted to what visitors did
(entering, clicking a hotspot, speaking) and to a timer. Those scripts were
written in **IPTSCRAE**, a stack language that looks a lot like Forth.

The original servers are long gone, but a small community still keeps the
software alive. This project is a from-scratch reimplementation of the **client**
side: the program you run to connect to a Palace server, see a room, and take
part in it.

## What this client can do

- **Connect and log on** to a Palace server as a guest, speaking its protocol
  directly. It works whatever byte order the server uses.
- **Show a room** as one assembled picture: the background, the image overlays,
  the loose props and the avatars of the people present, built from the server's
  description of the room plus the room's own artwork.
- **Fetch that artwork**, both the props passed around by the protocol and the
  pictures hosted on the server's web endpoint.
- **List and move between rooms**: see every room and every user on the server,
  and walk to another room by clicking a door.
- **Chat**: send and receive public messages and whispers, including the
  encrypted forms some servers use.
- **Run the room's own scripts.** Rooms are programmable, and this client reads
  and runs their scripts. Events such as entering a room, clicking a hotspot,
  speaking, or a timer firing drive them. The scripts run in a sandbox that
  cannot reach the network, the filesystem or other processes.
- **Run as an ordinary desktop application**, with a window showing the room, a
  room list, a user list, chat and a status bar.

There is also a **command-line probe** (`palace-probe`) that connects with no
window at all and prints what the server said: the handshake, the room list and
the user list. It is useful for checking a server, capturing replayable test
files, and comparing results with the reference Python client.

The server this project was developed and tested against is `localhost:9998`,
known as Balamb Garden. It is only the development server, not a limitation:
the client accepts any host and port, either from the command line
(`--host`/`--port`, `PALACE_HOST` and friends) or from the connect box in the
window, and the protocol is the same everywhere.

## What this client does not do yet

- **No sound or music playback.** A room script's `SOUND`, `MIDIPLAY`,
  `MIDILOOP` and `MIDISTOP` calls are decoded and surfaced to the interface, but
  nothing reaches an audio device.
- **Draw commands, name tags and chat text are not rasterized** into the room
  image. The room's stored draw commands are decoded and kept, but not painted.
- **Avatar art for other users is untested live.** The compositor draws avatars,
  but the development server had no other users online, so the asset transfer
  that would supply their props was never exercised against a real peer.
- **No live big-endian or HTTP-tunnel server was available.** Both paths exist
  and are unit-tested, but no such server was reachable to prove them end to end.
- **Guest logon only.** Authenticated (password) logon has not been attempted.

## The repository

This is a Rust workspace with eleven members: ten crates under `crates/` and the
`src-tauri/` application. The wire protocol, the file formats, the room model,
the renderer and the scripting engine are separate crates, and the desktop app is
Tauri plus SvelteKit on top of them. Everything below is a working client rather
than a skeleton: the GUI, the compositor and the IPTSCRAE engine are all present
and covered by the test suite.

---

## Quick start

```bash
cd $REPO

# Unit + offline integration tests (no network).
cargo test --workspace

# Connect, log on, print the room and user lists.
cargo run -p palace-probe -- --host localhost --port 9998 --user RustProbe

# Same, while writing a replayable fixture corpus.
cargo run -p palace-probe -- --host localhost --port 9998 --user RustProbe \
    --capture fixtures/logon-run1

# Print every known opcode.
cargo run -p palace-probe -- --list-opcodes

# Differential check against the Python oracle (needs the live server).
python3 tools/diff_walker.py
```

`--help` lists every option. `--json` emits a machine-readable summary,
`--verbose` prints every frame in the logon burst.

### Example session

```text
connected to localhost:9998 — server speaks little-endian, assigned user id 16
sending MSG_LOGON (regi) as "RustProbe"
logon burst: 9 frames
  entered room 901 "Balamb Garden" (picture "sqoom23.gif")
requesting MSG_LISTOFALLROOMS (rLst)
requesting MSG_LISTOFALLUSERS (uLst)
sending MSG_LOGOFF (bye )

== session ==
  byte order   : little
  user id      : 16
  server name  : Balamb Garden
  version      : 1.22
  media server : https://media.palace.example.info/palace/media
  logon frames : 9

== room list: 81 rooms ==
     105  users=  0  flags=0x0010  ~Triple Triad A~
     ...
     901  users=  2  flags=0x0114  Balamb Garden
     ...

== user list: 2 users ==
      12  room=   901  flags=0x0000  RustProbe
      10  room=   901  flags=0x0000  Queen Kat

SUMMARY rooms=81 users=2
```

---

## Workspace layout

| Path | What |
|---|---|
| `crates/palace-wire/` | Protocol library: framing, `ByteOrder` plumbing, opcode table, message codecs, fixture format. No I/O, no UI. |
| `crates/palace-room/` | `MSG_ROOMDESC` offset walker: room header, overlays, hotspots, loose props, draw commands. Depends on `palace-wire`. |
| `crates/palace-probe/` | The binary: TCP session, CLI, capture. |
| `crates/palace-prop/` | Prop (sprite) codec: 8/16/20/S20/32-bit decoders, S20 encoder, M&M palette, asset CRC. Validated over 227,874 real props. See its README for the format spec. |
| `crates/palace-asset/` | Asset layer: `qAst`/`sAst`/`rAst` transfer state machines, the paced 20-per-flush request scheduler, and the media HTTP fetch with its `.png→.jpg→original` fallback chain. |
| `crates/palace-render/` | Compositor plus the room↔viewport coordinate mapping. Turns a `RoomDesc` and local assets into an RGBA frame; never panics on missing art. |
| `crates/palace-client/` | Headless client runtime: connection FSM, session state, live asset intake, frame production. Owns the threads and emits `ClientEvent`s; knows nothing about Tauri. |
| `crates/iptscrae/` | The IPTSCRAE language: lexer, parser, virtual machine, resource budgets and the capability trait. Palace-agnostic and dependency-free. See its README for the grammar. |
| `crates/iptscrae-palace/` | The Palace command surface: the `PalaceHost` capability trait and the registry of Palace commands, plus a skeleton host used to run the harvested script corpus. |
| `crates/palace-host/` | The live host: splits `ON <event> { ... }` blocks out of hotspot scripts, dispatches runtime events, keeps globals across handlers, runs alarms, records the effects and encodes them as protocol frames. |
| `src-tauri/` | The Tauri v2 binary (`palace-app`): commands, the `palace://` frame protocol, and the event pump. |
| `src/` | SvelteKit 5 frontend (Vite, adapter-static). Displays one composited frame and draws the chrome around it. |
| `fixtures/logon-run1/` | A real captured session (raw bytes + decoded manifest). |
| `tools/diff_walker.py` | Differential check against `$CORPUS/tools/palace_walker.py`. |
| `tools/oracle_prop.py`, `tools/diff_corpus.py`, `tools/gen_palette.py`, `tools/diff_prerendered.py` | The prop codec's independent oracles, differential runner and palette verifier. |

Dependency policy: `palace-wire` depends only on `serde_json` (for the fixture
manifest). The protocol core is std-only. `palace-room` adds nothing but
`palace-wire`; `palace-probe` adds nothing but `palace-wire`. `palace-prop`
depends on `flate2` (zlib) and `png` (debug output only) and deliberately
**not** on `palace-wire` — props are a self-contained binary format that has
nothing to do with the wire protocol. `iptscrae` is standard-library only
(encoding, randomness and the clock are injectable through its `Host` trait);
`iptscrae-palace` depends on `iptscrae` alone; `palace-host` depends on
`iptscrae`, `iptscrae-palace`, `palace-room` and `palace-wire`.

---

## The protocol

The wire format, the logon exchange, the full layout of the room-description
message and the opcode table live in **[protocol.md](protocol.md)**. This file
stays focused on what the client is and how to use it.

---

## Fixtures

`fixtures/logon-run1/` is a complete real session:

```text
fixtures/logon-run1/
  manifest.json                     # session metadata + decoded form of every frame
  frames/0000-server-tiyr.bin       # raw wire bytes, 12-byte header included
  frames/0001-client-regi.bin
  ...
  frames/0015-client-bye_.bin
```

Raw frame files are exactly what the socket carried — nothing is transformed or
renormalised. `manifest.json` records, per frame, the direction, mnemonic,
opcode value, `refNum`, payload length, file path and the decoded description.

`Fixture::load` re-decodes every frame, re-encodes it and checks the bytes match,
and compares the decoded text against the manifest. A corrupt file therefore
fails loudly.

### Offline replay test

`crates/palace-wire/tests/fixture_replay.rs` loads the fixture **with no server**
and asserts:

- the session is self-consistent and byte-exact (`verify()` / `load()`);
- the first frame is `tiyr` (`MSG_TIYID`) and carries a positive user id;
- the client `regi` frame decodes to a 128-byte `AuxRegistrationRec`;
- the logon burst contains `vers`, `sinf`, `log `, `HTTP`, `room`, `endr`, `nprs`;
- the room list count equals `refNum` and the entry-room family is present;
- the user list count equals `refNum`;
- the room description decodes to `Balamb Garden` with a background picture;
- an unknown opcode in a fixture does not abort replay.

### Regenerating

```bash
cargo run -p palace-probe -- --host localhost --port 9998 --user RustProbe \
    --capture fixtures/logon-run1
```

Re-capture after changing any `Message::describe` text: the manifest stores the
decoded form, and `load` treats a mismatch as corruption.

---

## Tests

```bash
cargo test --workspace     # 621 passed, 0 failed, 2 ignored
```

The suite spans every crate in the workspace, not just the protocol layer:

- **`palace-wire`** unit tests cover framing, opcode packing, every message
  decoder, `Str31`/`PString` handling (including garbage padding) and the fixture
  format — each integer path exercised in **both byte orders**.
- **`palace-wire/tests/endianness.rs`** pins exact bytes for the banner, the
  framing header, the logon body and the list messages in both orders, and
  decodes a whole synthetic session in each. **`fixture_replay.rs`** is the
  offline corpus replay described above, and **`navr_encoding.rs`** pins the
  two-byte `navR` room-goto frame.
- **`palace-room` unit tests and `tests/fixtures.rs`** build a synthetic room with
  every sub-structure (overlay, hotspot with points/states/name/script, two linked
  loose props, a path and a detonate draw command) in both byte orders, then
  corrupt offsets one at a time (out-of-range array, absent offset, negative
  offset, link cycle, packed-stride fallback, oversized draw operand,
  unterminated script) and assert a `RoomWarning` instead of a panic.
- **`palace-room/tests/corpus_replay.rs`** decodes all 799 live payloads
  (804 records) with zero failures and zero warnings, and asserts the four
  concatenated captures split correctly. **`corpus_scripts.rs`** extracts and
  validates 2400/2400 hotspot scripts, and **`logon_room_fixture.rs`** pins the
  Balamb Garden `room` frame from `fixtures/logon-run1/`.
- **`palace-prop`** tests cover round-trips, malformed inputs and the 227,874-prop
  corpus. **`palace-asset`** tests cover the `qAst`/`sAst`/`rAst` state machines,
  the pacing scheduler, the media HTTP fallback chain and adversarial inputs.
- **`palace-render`** tests cover the coordinate mapping round-trip and full
  corpus renders. **`palace-client/tests/fixture_replay.rs`** replays every server
  frame of `fixtures/logon-run1/` through the session state.
- **`iptscrae`** tests run the language conformance and hostile-input suites;
  **`iptscrae-palace`** runs the harvested script corpus and the Palace command
  surface; **`palace-host`** tests script loading, event dispatch and wire effect
  encoding.

Two tests are ignored by default because they need resources this machine may not
have: `palace-asset/tests/live_server.rs` needs a live pserver at
`localhost:9998`, and `palace-prop/tests/corpus.rs` needs the local prop corpus.
Run them with `cargo test -- --ignored` once those are available.

Endianness is not skipped because "the server is little-endian anyway": the
logon packet is asserted byte-for-byte against `palace_walker.py`, the
big-endian twin is asserted to be the byte-swapped form, and a synthetic session
is decoded under both orders with identical results.

---

## Differential check against the Python oracle

```bash
python3 tools/diff_walker.py
```

Runs the probe and `$CORPUS/tools/palace_walker.py` against the same server,
back to back, and compares room id sets and names. `palace_walker.py` is
**executed read-only** and nothing under `$CORPUS/` is modified.

Result recorded on 2026-09-16:

```text
probe  : 81 rooms, 2 users (byte order little, server 'Balamb Garden')
walker : 81 rooms (reported 81)
PASS: 81 room ids and names match exactly
```

The walker has no user-list support, so the user count is validated against the
`uLst` reply itself (2 users, both in room 901) rather than against the walker.

---

## Verified vs assumed

**Verified against live traffic / a reference implementation**

- Framing, the `tiyr`/`ryit`/`pser` banner and endianness detection.
- The full 128-byte `AuxRegistrationRec` (byte-for-byte vs `palace_walker.py`).
- The logon burst message set and its exact fields (see
  [protocol.md](protocol.md#logon-burst-keepalive-exit)).
- `rLst` and `uLst` record layouts and count semantics (including the
  uninitialised `PString` padding, which the protocol reference permits).
- `UserRec` is 124 bytes, confirmed by the live `nprs`.
- `RoomRec` is 40 bytes and its offset fields resolve inside `len_vars`.
- Room list and user list agree with `palace_walker.py`.

**Assumed / not yet exercised**

- **Big-endian servers** are implemented and unit-tested, but no live
  big-endian pserver exists to connect to, so the big-endian path is proven by
  byte fixtures and a synthetic session, not by a real handshake.
- **`pser` (HTTP tunnel)** detection is tested as a byte banner, but the probe
  has not met a tunnel server; it exits with a clear error by construction.
- `MSG_USERSTATUS` (`uSta`) bodies are 44 bytes live. The protocol reference
  describes only the leading `sint16` flag word, so the remaining 42 bytes are
  preserved verbatim in `UserStatus::raw` and **not interpreted**.
- `AuxRegistrationRec.wizPassword` is decoded/encoded as an empty `Str31`; no
  authenticated (non-guest) logon has been attempted.

**Implemented but not yet proven live**

- **Hotspots, pictures, draw commands and loose props inside `room`** are decoded
  by `palace-room`; the raw buffer is still kept as `RoomDescription::var_data`.
  What is missing is painting: draw commands, name tags and chat text are not
  rasterized into the frame.
- **Asset transfer (`qAst`/`sAst`/`rAst`)** is implemented in `palace-asset` and
  drives live media fetching. Multi-block transfer is derived from the reference
  implementations only; no real capture of a multi-block transfer exists.
- **Prop codecs** are implemented in `palace-prop` and validated over the corpus.
  The 16-bit decoder has zero real samples to test against, and `pserver_full.prp`
  is systematically corrupt.
- **Avatars** are composited by `palace-render`, but the development server had no
  other users online, so receiving another user's prop art over the wire is
  untested live.
- **IPTSCRAE** runs live: `iptscrae` and `iptscrae-palace` implement the language
  and command surface, and `palace-host` dispatches room events into it. Sound
  and music effects are decoded and surfaced but not played.

**Still genuinely undetermined**

- `MSG_BLOWTHRU` (`blow`) payload semantics — the live logon burst contained one
  14-byte `blow` on an earlier connection; it is decoded only as a bounded
  payload, never interpreted.

---

## Provenance

Protocol facts come from the official Communities.com *Palace Protocol
Reference* (1999) bundled with `ThePalacev0`, cross-checked against Taj (C#),
QPalace (C++), OpenPalace (AS3), pserver (C++) and the known-good Python client
`palace_walker.py`. Where references disagree, live traffic decides. Full
provenance is in `$CORPUS/reference/MANIFEST.md`.

This project is a non-commercial reimplementation for a small community around
abandoned software; licensing is not treated as a constraint. Sources are
credited for their role in recovering an undocumented protocol. Distributed
binaries, if any, are meant to be GPL-3.0.

No credentials are used or stored: logon is guest-only and the username comes
from `--user`.

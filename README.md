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

The client accepts any host and port, either from the command line
(`--host`/`--port`, `PALACE_HOST` and friends) or from the connect box in the
window, and the protocol is the same everywhere.

## Status

Palace Client is a working desktop client. It logs on as a guest, assembles a
room into one composited picture, fetches the room's artwork, lists and walks
between rooms, sends and receives chat, and runs the room's own IPTSCRAE scripts
in a sandbox with no ambient authority. The protocol layer, the prop codec, the
compositor and the script engine are complete and covered by the test suite.
Audio playback, live verification of the HTTP fetch-and-execute path, and the
tail of the Palace command set are the remaining work.

Current state, evidence and known gaps: **[STATUS.md](STATUS.md)**.
Live work queue: **[palace-todo.md](palace-todo.md)**.

---

## The repository

This is a Rust workspace with eleven members: ten crates under `crates/` and the
`src-tauri/` application. The wire protocol, the file formats, the room model,
the renderer and the scripting engine are separate crates, and the desktop app is
Tauri plus SvelteKit on top of them. Everything is a working client rather than a
skeleton: the GUI, the compositor and the IPTSCRAE engine are all present and
covered by the test suite.

---

## Quick start

```bash
git clone <repository-url> palace-client
cd palace-client

# Headless crates (no Tauri system libraries needed).
cargo test -p palace-wire -p palace-room -p palace-prop -p palace-asset \
           -p palace-probe -p palace-render -p palace-client \
           -p iptscrae -p iptscrae-palace -p palace-host

# The whole workspace, including the Tauri app. Install Tauri's system
# dependencies first (the list is in `.github/workflows/ci.yml`).
cargo test --workspace

# Run the desktop app. `bun run tauri dev` starts Vite and the Tauri shell.
bun install
bun run tauri dev

# Run the headless probe: connect, log on, print the room and user lists.
cargo run -p palace-probe -- --host your.server --port 9998 --user RustProbe
```

`--help` lists every probe option. `--json` emits a machine-readable summary and
`--verbose` prints every frame in the logon burst. Fixture capture and the
differential oracle commands are documented in [protocol.md](protocol.md).

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

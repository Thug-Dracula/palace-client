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
- **Some script effects are recorded but not applied to the room**, so a script
  appears to run while the room stays the same. Six of these change what you
  should see: adding, moving or removing a **loose prop**, and changing your
  **face or colour**. Room dimming (`DIMROOM`) was in the same state and is now
  fixed. See the effect-application note in `STATUS.md` for the full list and the
  pattern for fixing the rest.
- **Avatar art for other users is untested live.** The compositor draws avatars,
  but the development server had no other users online, so the asset transfer
  that would supply their props was never exercised against a real peer.
- **No live big-endian or HTTP-tunnel server was available.** Both paths exist
  and are unit-tested, but no such server was reachable to prove them end to end.
- **Guest logon only.** Authenticated (password) logon has not been attempted.


## Project status

An honest snapshot: what is finished, what is not, and what stands between this
and 1.0. Unit and offline integration tests pass, and line coverage is 90.6%
against an 80% gate — but test coverage is not the same as completeness, and the
gaps below are where the difference shows.

### Finished

- **The protocol layer** — framing, logon, the room-description message, the
  opcode table, and byte order handled whatever the server uses. Cross-checked
  against captured traffic and against three independent reference
  implementations.
- **The prop codec**, validated against roughly 228,000 real props with zero
  disagreements against the reference decoder.
- **A working desktop client** — rooms assemble into one picture, avatars and
  name tags draw, chat sends and receives (including the encrypted forms some
  servers use), doors walk, and the room and user lists are live.
- **The script engine** — the IPTSCRAE language (72 core words: arithmetic,
  logic, strings, maths, stack operations and control flow) plus the Palace
  command host. Scripts run sandboxed, with no network, filesystem or process
  access.
- **Script events** — entering, leaving, selecting, chat, lock, unlock, state
  change, rename, user leave, and the room lifecycle (`ROOMLOAD`, `ENTER`,
  `ROOMREADY`) in the order the reference client fires it.
- **Reading a real prop bag** — the `.bundle` format is decoded, and worn props
  are sent to the server so other clients can see them.

### Not finished

**The command set is the largest gap.** The *language* is in good shape; the
Palace *commands* are not. There are four kinds of gap, in descending order of
how badly they hide:

1. **Commands that run but do nothing.** `SOUND`, `MIDIPLAY`, `MIDILOOP` and
   `MIDISTOP` are surfaced to the interface but never reach an audio device, so
   a room's soundscape is silent while appearing to work. `LOADPROPS` has no
   effect at all. `PAINTUNDO` undoes nothing. `ISLOCKED` always answers false,
   and `GETPICDIMENSIONS` always answers zero.
2. **Commands that are not registered at all.** A room calling one of these does
   not get an "unsupported" report — the name is treated as a variable instead,
   so the room misbehaves quietly. Around forty names are in this state,
   including `ADDPIC` (115 uses in the rooms available to test against),
   `SETSPOTSCRIPT` (82), `ADDSPOT` (81), `LOADSCRIPT` (51), `HTTPGET` (50) and
   the tooltip pair (88 between them).
3. **Commands missing from the dictionary** although the host could dispatch
   them, plus one that is documented but never dispatched.
4. **Two effects that can never be produced.**

Alongside those, these subsystems are missing or incomplete:

- **Audio.** Nothing plays.
- **Drawing.** `LINE` reaches the server but is not rasterized locally;
  `CIRCLE`, `FILL`, `PAINT` and `TEXT` are unimplemented; and the `DRAW` opcode
  that carries other people's strokes is dropped on receipt.
- **Dynamic room content.** `ADDPIC`, `ADDSPOT` and `SETSPOTSCRIPT` — the
  commands rooms use to build their own interfaces.
- **HTTP.** `HTTPGET` and `LOADSCRIPT`. The response to a fetch is itself a
  script, which is how several Colosseum rooms load their logic.
- **A props panel.** Worn props now reach the server, but nothing in the
  interface can *set* them, so the feature is only half reachable.
- **Authentication.** A server requiring a password is refused, with the request
  reported rather than silently ignored.

### Known limitations

- **Placeholder art.** Some visual gaps cannot be closed without real assets.
- **Live verification is thinner than the unit tests.** Several receive paths
  are proven against a mock harness but have not been exercised against a busy
  server.
- **Asset transfer for other users' avatars** is untested against a real peer.
- **Multi-block asset transfer and 16-bit props** are reference-derived, and the
  loose-prop artwork for the available corpus is missing from local stores.
- **One deep opcode (`durl`) remains unresolved.**

### Roadmap to 1.0

1. **Diagnosis tooling first** — a trace log, so live behaviour can be inspected
   rather than inferred from static scripts.
2. **Settle authentication** — implement the reply, which first needs a decision
   about where a credential should live.
3. **Complete the command set**, in order of use: register and implement the
   dead names, then audio, then dynamic room content, then HTTP and
   `LOADSCRIPT`, then drawing and tooltips.
4. **Build the props panel**, so the prop bag is usable from the interface.
5. **Replace the constant-answering stubs** with real answers, and turn any
   remaining silent no-op into an explicit report.
6. **Publish** and retire the remaining safety-net branches.

---
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
cd ~/ProgramFiles/palace-client

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
| `tools/diff_walker.py` | Differential check against `~/palace-corpus/tools/palace_walker.py`. |
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
provenance is in `~/palace-corpus/reference/MANIFEST.md`.

This project is a non-commercial reimplementation for a small community around
abandoned software; licensing is not treated as a constraint. Sources are
credited for their role in recovering an undocumented protocol. Distributed
binaries, if any, are meant to be GPL-3.0.

No credentials are used or stored: logon is guest-only and the username comes
from `--user`.

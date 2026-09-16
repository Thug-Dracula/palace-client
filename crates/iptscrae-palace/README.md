# `iptscrae-palace` — the Palace command surface

The `iptscrae` crate implements the language and deliberately knows nothing about
Palace. This crate adds the layer that does:

* **`PalaceHost`** — the capability trait every Palace command is written
  against. It mirrors OpenPalace's `IPalaceController` (83 methods there); this
  crate's trait declares 111 (`src/traits.rs`): `chat`, `goto_room`, `set_props`,
  `get_spot_state`, the paint and sound operations, and so on. Every method has a
  default, so a headless or cyborg-only host is a few
  lines, and an unsupported operation fails with
  `IptError::CommandUnavailable` rather than silently doing nothing.
* **`PALACE_COMMANDS`** — every Palace command whose stack effect the reference
  implementations document, with its operand and result counts.
* **`PalaceCommands<H>`** — the adapter that turns `PalaceHost` methods into
  registered IPTSCRAE commands. This is the shape the full command set takes:
  one match arm per command, each reading its operands from `args` in push order.
* **`SkeletonHost`** — a no-authority host used to run the harvested corpus.
* **`classify`** — the failure taxonomy the corpus report is built from, and the
  `SourceSpellings` scanner that recovers a symbol's original case (the lexer
  upper-cases it before the VM ever sees it).

## Writing a command

Operands arrive in **push order** and already dereferenced, so `SAYAT`'s
`message x y` presents `args == [message, x, y]`.

```rust
use iptscrae_palace::{harness::SkeletonHost, traits::PalaceHost, PalaceCommands};

struct Client {
    spoke: Vec<String>,
}

impl iptscrae::Host for Client {}

impl PalaceHost for Client {
    fn chat(&mut self, text: &str) -> iptscrae::Result<()> {
        self.spoke.push(text.to_owned());
        Ok(())
    }
}

let mut engine = iptscrae::Engine::new(PalaceCommands::new(Client { spoke: Vec::new() }))
    .with_commands(SkeletonHost::command_set());
engine.run_source("\"hello\" SAY").unwrap();
assert_eq!(engine.host.inner.spoke, ["hello"]);
```

`PalaceCommands::command_pops` reports how many operands a command consumes;
the VM pops exactly that many (dereferencing variables, as the reference's
`popType` does) and pushes whatever the command returns. A command that is
declared but not implemented returns `CommandUnavailable`, so a script that uses
it fails that instruction instead of quietly misusing the stack.

## Scope

This crate supplies the trait, the registry and the adapter. `PalaceCommands`
implements a handful of commands (`SAY`, `CHAT`, `SAYAT`, `PRIVATEMSG`, `SETPOS`,
`GOTOROOM`, `USERID`, `WHOME`, `USERNAME`) as proof that the layers fit together,
and `SkeletonHost` stubs the rest for corpus runs.

The full command surface is no longer stubbed anywhere that matters:
`crates/palace-host`'s `ScriptHost` implements the `PalaceHost` surface over a
capability snapshot and encodes every effect as a protocol frame (see the
Event dispatch section in `STATUS.md`). The remaining work in *this* crate is the
`PalaceCommands` adapter, which the live runtime does not use; `palace-host` wires
the engine directly.

Two deliberate omissions, both recorded in the `iptscrae` README:

* **Extended PalaceChat commands** (175 names: `WEBEMBED`, `SETSPOTSCRIPT`,
  `DRAWTEXT`, …) are *not* registered. This crate has no source for their stack
  effects, and a guessed arity would corrupt the stack. Unregistered, they lex as
  variables — which is what OpenPalace does, since it does not know them either.
* **`SGLOBAL`** *is* registered, as an alias of the core `GLOBAL`. It is not in
  OpenPalace, but the corpus uses `sym SGLOBAL` exactly as `sym GLOBAL`, and `IF`
  only balances if `SGLOBAL` consumes one operand.

## Running the corpus

```bash
cargo run -p iptscrae-palace --bin iptscrae -- eval '2 3 + ITOA'
cargo run -p iptscrae-palace --bin iptscrae -- run path/to/script.txt [--handler SELECT]
cargo run -p iptscrae-palace --bin iptscrae -- corpus ~/palace-corpus/scripts_clean/by_script
cargo run -p iptscrae-palace --bin iptscrae -- corpus ~/palace-corpus/scripts_clean/by_script --shared-globals
```

`corpus` parses and executes every handler, prints the parse/run distribution,
sorts every failure into the milestone's four classes, lists the unregistered
symbols each failing handler named, and reports which Palace commands the corpus
exercises. See the `iptscrae` README for the current numbers and a
classification of every failure.

`--shared-globals` runs the whole corpus through one global store instead of
isolating each file, which is how a real session behaves. It is the evidence for
the report's claim that the residual failures are environmental: sharing globals
rescues 5 handlers (3791 → 3796 of 3805 clean) and leaves the 7 (b) failures
untouched. Both modes are gated by `tests/corpus.rs` when `IPTSCRAE_CORPUS` is
set.

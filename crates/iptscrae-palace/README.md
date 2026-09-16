# `iptscrae-palace` — the Palace command surface

The `iptscrae` crate implements the language and deliberately knows nothing about
Palace. This crate adds the layer that does:

* **`PalaceHost`** — the capability trait every Palace command is written
  against. It mirrors OpenPalace's `IPalaceController` (~90 methods): `chat`,
  `goto_room`, `set_props`, `get_spot_state`, the paint and sound operations, and
  so on. Every method has a default, so a headless or cyborg-only host is a few
  lines, and an unsupported operation fails with
  `IptError::CommandUnavailable` rather than silently doing nothing.
* **`PALACE_COMMANDS`** — every Palace command whose stack effect the reference
  implementations document, with its operand and result counts.
* **`PalaceCommands<H>`** — the adapter that turns `PalaceHost` methods into
  registered IPTSCRAE commands. This is the shape the full command set takes:
  one match arm per command, each reading its operands from `args` in push order.
* **`SkeletonHost`** — a no-authority host used to run the harvested corpus.

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

This milestone implements the trait, the registry and a handful of commands
(`SAY`, `CHAT`, `SAYAT`, `PRIVATEMSG`, `SETPOS`, `GOTOROOM`, `USERID`,
`WHOME`, `USERNAME`) as proof that the layers fit together. The remaining
Palace commands are registered with their documented stack effects and stubbed by
the harness; implementing them is the next milestone.

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
cargo run -p iptscrae-palace --bin iptscrae -- corpus $CORPUS/scripts_clean/by_script
```

`corpus` parses and executes every handler, prints the parse/run/failure
distribution, and lists which Palace commands the corpus exercises. See the
`iptscrae` README for the current numbers and a classification of every failure.

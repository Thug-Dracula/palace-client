//! `iptscrae` — the IPTSCRAE language: lexer, parser, VM and capability trait.
//!
//! IPTSCRAE is the stack language The Palace used for server scripts, room and
//! hotspot scripts, and per-user "cyborg" scripts. It looks like Forth: operands
//! push, commands pop and push, and there are no floats. This crate is a fresh
//! Rust implementation of the language core — no network, no UI, no Palace
//! commands — so it can be tested against a corpus on its own.
//!
//! ## The language in one screen
//!
//! ```
//! use iptscrae::testing::eval_int;
//!
//! // Postfix arithmetic, integer only.
//! assert_eq!(eval_int("2 3 +").unwrap(), 5);
//!
//! // Assignment is value-first: `value name =`.
//! assert_eq!(eval_int("4 x = x 1 + x = x").unwrap(), 5);
//!
//! // Control flow puts the body first. `{body} cond IF`.
//! assert_eq!(eval_int("1 { 7 } 1 IF").unwrap(), 7);
//!
//! // `WHILE` is body first, then the condition `{body} {test} WHILE`.
//! assert_eq!(eval_int("0 n = { n ++ } { n 3 < } WHILE n").unwrap(), 3);
//! ```
//!
//! ## Architecture
//!
//! ```text
//! source ──► lexer::parse_script ──► Script { handlers: {name -> Chunk} }
//!                                        │
//!                                        ▼
//!                            Engine<H: Host>::run_handler ──► Vm ──► Host
//! ```
//!
//! * [`lexer`] turns source into [`value::Chunk`]s. Symbols are resolved
//!   through a [`registry::CommandSet`], so layering a host dictionary changes
//!   what counts as a command.
//! * [`vm`] executes a chunk. It never recurses natively: control flow uses an
//!   explicit frame stack, so nesting cannot overflow the Rust stack.
//! * [`host`] is the capability trait — the only way a script can reach outside
//!   the VM. The default implementation does nothing.
//!
//! ## Untrusted code
//!
//! Scripts arrive from the server, so the VM treats them as hostile:
//!
//! * every loop, frame and allocation is bounded by [`budget::Limits`];
//! * arithmetic wraps and division by zero is `0`, matching the reference;
//! * malformed input is an [`error::IptError`], never a panic;
//! * the default budgets are the PalaceChat-era numbers (1024 stack items,
//!   7500 loop iterations, 256 nesting) — see [`budget`].
//!
//! ## Divergences from the reference implementations
//!
//! The crate README lists every place this implementation deliberately differs
//! from, or is uncertain about, OpenPalace's ActionScript VM and the published
//! language guide. Read it before treating any behaviour as authoritative.
//!
//! [`Host`]: host::Host

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod budget;
pub mod error;
pub mod host;
pub mod lexer;
pub mod regex;
pub mod registry;
pub mod script;
pub mod stack;
pub mod testing;
pub mod value;
pub mod vm;

pub use budget::{Limits, StackDialect};
pub use error::{IptError, Result};
pub use host::{Host, NullHost};
pub use lexer::{parse_body, parse_script};
pub use registry::{Builtin, CommandKind, CommandSet};
pub use script::Script;
pub use stack::Stack;
pub use value::{decode_source, Chunk, Op, Value};
pub use vm::{Engine, Execution, Vm};

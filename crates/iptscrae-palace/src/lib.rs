//! `iptscrae-palace` — the Palace command surface for the `iptscrae` VM.
//!
//! The `iptscrae` crate is deliberately Palace-agnostic: it knows how to lex
//! and execute the language, and nothing about rooms, props or avatars. This
//! crate adds the Palace layer:
//!
//! * [`traits::PalaceHost`] — the ~90-method capability trait that mirrors
//!   OpenPalace's `IPalaceController`. Every Palace command is written against
//!   it, and it is the only door from a script to a Palace client.
//! * [`commands`] — the Palace command registry: every name, its documented
//!   stack effect, and [`commands::PalaceCommands`], the adapter that serves
//!   commands from a [`traits::PalaceHost`].
//! * [`harness::SkeletonHost`] — a no-authority host used to run the harvested
//!   corpus and prove the VM on real scripts.
//!
//! ## Writing a command
//!
//! Operands arrive in push order and already dereferenced, so `SAYAT`'s
//! `message x y` presents `args == [message, x, y]`, the order the guide
//! documents.
//!
//! ```
//! use iptscrae_palace::{harness::SkeletonHost, traits::PalaceHost, PalaceCommands};
//!
//! struct Client {
//!     spoke: Vec<String>,
//! }
//!
//! impl iptscrae::Host for Client {}
//!
//! impl PalaceHost for Client {
//!     fn chat(&mut self, text: &str) -> iptscrae::Result<()> {
//!         self.spoke.push(text.to_owned());
//!         Ok(())
//!     }
//! }
//!
//! let mut engine = iptscrae::Engine::new(PalaceCommands::new(Client { spoke: Vec::new() }))
//!     .with_commands(SkeletonHost::command_set());
//! engine.run_source("\"hello\" SAY").unwrap();
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod classify;
pub mod commands;
pub mod harness;
pub mod traits;

pub use classify::{classify_parse, classify_run, FailureClass, SourceSpellings};
pub use commands::{
    command_spec, register_palace_commands, CommandSpec, PalaceCommands, Push, PALACE_COMMANDS,
};
pub use harness::SkeletonHost;
pub use traits::PalaceHost;

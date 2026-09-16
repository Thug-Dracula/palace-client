//! The capability trait: everything the VM is allowed to ask the host for.
//!
//! This is the security boundary. The VM has no filesystem, no network and no
//! clock of its own; it can only call the methods below. A host that wants
//! scripts to be harmless implements them as constants and no-ops, and every
//! script still tokenises, parses and executes — it just cannot observe or
//! change the outside world.
//!
//! The methods fall into two groups:
//!
//! * **Primitives** the core commands need: randomness, the clock, tracing,
//!   regular expressions and alarm scheduling. `RANDOM`, `TICKS`, `DATETIME`,
//!   `_TRACE`, `GREPSTR`/`GREPSUB` and `ALARMEXEC` are thin wrappers over them.
//! * **`command_pops` / `command`** — the entry point for every
//!   *host-registered* command. All Palace commands (`SAY`, `SETPOS`,
//!   `GOTOROOM`, …) arrive here by name. The host declares how many operands
//!   the command consumes; the VM pops them (dereferencing variables, exactly
//!   as the reference's `popType` does) and passes them in push order. The host
//!   returns the values to push. This keeps the data stack private to the VM
//!   while still giving a command everything it needs.
//!
//! Every method has a deterministic default, so a minimal host is
//! `impl Host for MyHost {}`. The defaults fail closed where a wrong answer
//! would be dangerous (regex and alarms) and are inert everywhere else.

use crate::error::{IptError, Result};
use crate::value::{Chunk, Value};

/// The host services available to a running script.
pub trait Host {
    /// A random integer in `0..bound`.
    ///
    /// `bound <= 0` yields 0, matching `int(random() * n)`. The VM is
    /// deterministic: the host owns the only source of randomness, so a seeded
    /// host makes a whole run reproducible.
    fn random(&mut self, bound: i64) -> i64 {
        let _ = bound;
        0
    }

    /// The host clock in ticks (1/60 s), for `TICKS`.
    fn ticks(&self) -> i64 {
        0
    }

    /// Seconds since the Unix epoch, for `DATETIME`.
    fn datetime(&self) -> i64 {
        0
    }

    /// Write a diagnostic line, for `_TRACE` and `TRACESTACK`.
    fn trace(&mut self, message: &str) {
        let _ = message;
    }

    /// Run `pattern` against `text` for `GREPSTR`.
    ///
    /// `Ok(Some(captures))` means "matched", with `captures[0]` the whole match
    /// and later entries the groups; `Ok(None)` means "no match". The default
    /// refuses, because compiling untrusted patterns is a decision the host
    /// must make deliberately (the reference implementations have no timeout).
    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        let _ = (pattern, text);
        Err(IptError::CommandUnavailable {
            command: "GREPSTR".to_owned(),
        })
    }

    /// Schedule an atomlist to run after `ticks`, for `ALARMEXEC`/`SETALARM`.
    ///
    /// `spot` is the hotspot the alarm belongs to, or 0 for a cyborg script.
    fn schedule_alarm(&mut self, ticks: i64, body: Chunk, spot: i64) -> Result<()> {
        let _ = (ticks, body, spot);
        Err(IptError::CommandUnavailable {
            command: "ALARMEXEC".to_owned(),
        })
    }

    /// Variables the host defines before a handler runs.
    ///
    /// Some names are values the client owns rather than commands the script
    /// calls. The canonical one is `CHATSTR`, the incoming or outgoing chat
    /// text: an `ON OUTCHAT` handler reads and rewrites it. The default defines
    /// nothing, so a minimal host behaves as if those names are ordinary
    /// auto-vivified variables.
    fn initial_variables(&self) -> Vec<(String, Value)> {
        Vec::new()
    }

    /// How many operands a host command consumes.
    ///
    /// Returning the wrong count desynchronises the stack, so a host that does
    /// not implement a name must report 0 and let [`Host::command`] fail.
    fn command_pops(&self, name: &str) -> usize {
        let _ = name;
        0
    }

    /// Service a host command.
    ///
    /// `name` is upper-cased. `args` holds the operands in **push order** —
    /// `args[0]` was pushed first (deepest on the stack), `args.last()` was on
    /// top. Variable operands arrive already dereferenced. Return the values to
    /// push, deepest first; an empty vector pushes nothing.
    ///
    /// [`IptError::CommandUnavailable`] is the right answer for a name the host
    /// does not implement, and is what the default does.
    fn command(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let _ = args;
        Err(IptError::CommandUnavailable {
            command: name.to_owned(),
        })
    }
}

/// A host that does nothing and exposes nothing. Useful as a base and in tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullHost;

impl Host for NullHost {}

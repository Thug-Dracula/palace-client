//! A headless host for running the harvested corpus.
//!
//! The corpus is 2,400 real scripts. Running them proves the VM core on real
//! code, but the scripts' *effects* are Palace operations this milestone does
//! not implement. `SkeletonHost` resolves that: it is a capability
//! implementation with no authority at all. It consumes the operands each
//! Palace command documents and pushes neutral defaults, recording what it saw.
//!
//! That makes "clean run" mean "the VM executed the script's control flow and
//! arithmetic without a fault", which is exactly the property this milestone
//! must demonstrate. It is not a Palace client: no command does anything.

use std::collections::BTreeMap;

use iptscrae::registry::CommandSet;
use iptscrae::value::{Chunk, Value};
use iptscrae::{Engine, Host, Result};

use crate::commands::{command_spec, register_palace_commands, Push};

/// A deterministic host that stubs every Palace command and records usage.
#[derive(Debug, Clone)]
pub struct SkeletonHost {
    rng_state: u64,
    /// How many times each command was invoked, by name.
    pub invocations: BTreeMap<String, u64>,
    /// Trace lines the scripts produced.
    pub trace: Vec<String>,
    /// Value returned by `TICKS`.
    pub tick: i64,
    /// Whether `GREPSTR` should actually match.
    pub grep_enabled: bool,
}

impl Default for SkeletonHost {
    fn default() -> Self {
        Self::seeded(0)
    }
}

impl SkeletonHost {
    /// A host whose `RANDOM` stream is fixed by `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng_state: seed,
            invocations: BTreeMap::new(),
            trace: Vec::new(),
            tick: 0,
            grep_enabled: true,
        }
    }

    /// The command dictionary a corpus run needs: the core engine plus every
    /// Palace name.
    pub fn command_set() -> CommandSet {
        let mut set = CommandSet::core();
        register_palace_commands(&mut set);
        set
    }

    /// An engine wired to a fresh skeleton host.
    pub fn engine() -> Engine<Self> {
        Engine::new(Self::seeded(0)).with_commands(Self::command_set())
    }

    fn next_u64(&mut self) -> u64 {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Commands invoked at least once, most frequent first.
    pub fn usage(&self) -> Vec<(&str, u64)> {
        let mut rows: Vec<(&str, u64)> = self
            .invocations
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        rows
    }
}

impl Host for SkeletonHost {
    fn random(&mut self, bound: i64) -> i64 {
        if bound <= 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as i64
        }
    }

    fn ticks(&self) -> i64 {
        self.tick
    }

    fn datetime(&self) -> i64 {
        1_000_000_000
    }

    fn trace(&mut self, message: &str) {
        self.trace.push(message.to_owned());
    }

    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        if !self.grep_enabled {
            return Ok(None);
        }
        Ok(iptscrae::regex::find(pattern, text))
    }

    fn schedule_alarm(&mut self, _ticks: i64, _body: Chunk, _spot: i64) -> Result<()> {
        Ok(())
    }

    fn initial_variables(&self) -> Vec<(String, Value)> {
        vec![("CHATSTR".to_owned(), Value::str(""))]
    }

    fn command_pops(&self, name: &str) -> usize {
        command_spec(name).map_or(0, |s| usize::from(s.pops))
    }

    fn command(&mut self, name: &str, _args: &[Value]) -> Result<Vec<Value>> {
        *self.invocations.entry(name.to_owned()).or_insert(0) += 1;
        // A Palace host answers the version banner as a Palace client does.
        if name.eq_ignore_ascii_case("IPTVERSION") {
            return Ok(vec![Value::Int(2)]);
        }
        match command_spec(name) {
            Some(spec) => Ok(stub_values(spec)),
            None => Ok(Vec::new()),
        }
    }
}

fn stub_values(spec: &crate::commands::CommandSpec) -> Vec<Value> {
    let mut out = Vec::with_capacity(usize::from(spec.pushes));
    for _ in 0..spec.pushes {
        out.push(match spec.push {
            Push::None => Value::Int(0),
            Push::Int => Value::Int(0),
            Push::Str => Value::str(""),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palace_command_is_recognised_and_stack_balanced() {
        let mut engine = SkeletonHost::engine();
        assert_eq!(
            engine.run_source_resolved("\"hi\" SAY 7").unwrap(),
            vec![Value::Int(7)]
        );
    }

    #[test]
    fn a_stubbed_getter_pushes_a_default() {
        let mut engine = SkeletonHost::engine();
        assert_eq!(
            engine.run_source_resolved("ROOMNAME").unwrap(),
            vec![Value::str("")]
        );
    }

    #[test]
    fn usage_is_recorded() {
        let mut engine = SkeletonHost::engine();
        engine
            .run_source("\"a\" SAY \"b\" SAY \"c\" 1 2 SAYAT")
            .unwrap();
        let host = &engine.host;
        assert_eq!(host.invocations.get("SAY"), Some(&2));
        assert_eq!(host.invocations.get("SAYAT"), Some(&1));
    }
}

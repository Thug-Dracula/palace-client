//! A deterministic host and evaluation helpers.
//!
//! Everything here exists so a test (or the corpus harness) can run a script
//! reproducibly: `TestHost` owns a seeded generator instead of a real one, its
//! clock is a constant, and it records every host interaction so a test can
//! assert what a script asked for. Nothing in this module is required by the
//! VM; a production host implements [`Host`] itself.

use crate::error::{IptError, Result};
use crate::host::Host;
use crate::value::{Chunk, Value};
use crate::vm::Engine;

/// A host that is deterministic, records calls, and exposes no ambient
/// authority.
#[derive(Debug, Clone)]
pub struct TestHost {
    rng_state: u64,
    /// Value returned by `TICKS`.
    pub tick: i64,
    /// Value returned by `DATETIME`.
    pub datetime: i64,
    /// Every `_TRACE`/`TRACESTACK` line, in order.
    pub trace: Vec<String>,
    /// Every host command name, in order.
    pub commands: Vec<String>,
    /// Every `ALARMEXEC`, as `(ticks, spot, body)`.
    pub alarms: Vec<(i64, i64, Chunk)>,
    /// Every `GREPSTR` as `(pattern, text)`.
    pub greps: Vec<(String, String)>,
    /// Whether `GREPSTR` should actually match. When false the host refuses,
    /// which is how a minimal real host would behave.
    pub grep_enabled: bool,
}

impl Default for TestHost {
    fn default() -> Self {
        Self::seeded(0)
    }
}

impl TestHost {
    /// A host whose random stream is fixed by `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng_state: seed,
            tick: 0,
            datetime: 1_000_000_000,
            trace: Vec::new(),
            commands: Vec::new(),
            alarms: Vec::new(),
            greps: Vec::new(),
            grep_enabled: true,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Host for TestHost {
    fn random(&mut self, bound: i64) -> i64 {
        if bound <= 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as i64
    }

    fn ticks(&self) -> i64 {
        self.tick
    }

    fn datetime(&self) -> i64 {
        self.datetime
    }

    fn trace(&mut self, message: &str) {
        self.trace.push(message.to_owned());
    }

    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        if !self.grep_enabled {
            return Err(IptError::CommandUnavailable {
                command: "GREPSTR".to_owned(),
            });
        }
        self.greps.push((pattern.to_owned(), text.to_owned()));
        Ok(crate::regex::find(pattern, text))
    }

    fn schedule_alarm(&mut self, ticks: i64, body: Chunk, spot: i64) -> Result<()> {
        self.alarms.push((ticks, spot, body));
        Ok(())
    }

    fn initial_variables(&self) -> Vec<(String, Value)> {
        vec![("CHATSTR".to_owned(), Value::str(""))]
    }

    fn command(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        self.commands.push(name.to_owned());
        let _ = args;
        Err(IptError::CommandUnavailable {
            command: name.to_owned(),
        })
    }
}

/// Parse and run a bare instruction sequence with a [`TestHost`], resolving any
/// variable references left on the stack.
pub fn eval(source: &str) -> Result<Vec<Value>> {
    Engine::new(TestHost::seeded(1)).run_source_resolved(source)
}

/// Like [`eval`], expecting a single integer on top of the stack.
pub fn eval_int(source: &str) -> Result<i64> {
    match eval(source)?.last() {
        Some(Value::Int(n)) => Ok(*n),
        Some(other) => Err(IptError::TypeMismatch {
            expected: "number",
            found: other.type_name(),
        }),
        None => Err(IptError::StackUnderflow {
            needed: 1,
            available: 0,
        }),
    }
}

/// Like [`eval`], expecting a single string on top of the stack.
pub fn eval_text(source: &str) -> Result<String> {
    match eval(source)?.last() {
        Some(Value::Str(s)) => Ok(s.to_string()),
        Some(other) => Err(IptError::TypeMismatch {
            expected: "string",
            found: other.type_name(),
        }),
        None => Err(IptError::StackUnderflow {
            needed: 1,
            available: 0,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_reproducible() {
        let mut a = TestHost::seeded(42);
        let mut b = TestHost::seeded(42);
        let seq_a: Vec<i64> = (0..8).map(|_| a.random(100)).collect();
        let seq_b: Vec<i64> = (0..8).map(|_| b.random(100)).collect();
        assert_eq!(seq_a, seq_b);
        assert!(seq_a.iter().all(|n| (0..100).contains(n)));
    }

    #[test]
    fn a_non_positive_bound_is_zero() {
        let mut host = TestHost::seeded(1);
        assert_eq!(host.random(0), 0);
        assert_eq!(host.random(-5), 0);
    }

    #[test]
    fn grepsub_uses_the_last_grepstr_capture() {
        assert_eq!(
            eval_text("\"a1b22c\" \"[0-9]+\" GREPSTR POP \"<$0>\" GREPSUB").unwrap(),
            "<1>"
        );
    }
}

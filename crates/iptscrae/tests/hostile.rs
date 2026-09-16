//! Hostile-input suite: the VM is a sandbox boundary, so no script may hang,
//! panic, overflow the native stack or exhaust memory.
//!
//! Two things are asserted here:
//!
//! * **Panic freedom** — random token soup and random bytes are fed to the
//!   lexer, parser and VM. Any panic fails the test.
//! * **Guaranteed termination** — every randomised run returns, and the budgets
//!   that make that true are exercised directly: a `WHILE` that never ends, a
//!   recursion that never bottoms out, a script longer than the step budget, a
//!   deeply nested atomlist, an oversized array and an unbounded string.
//!
//! The generator is a tiny deterministic PRNG so a failure is reproducible from
//! the seed printed in the assertion.

use iptscrae::testing::TestHost;
use iptscrae::{parse_body, parse_script, CommandSet, Engine, IptError, Limits};

/// xorshift64* — deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "{",
    "}",
    "[",
    "]",
    "\"",
    "'",
    "1",
    "0",
    "-3",
    "42",
    "x",
    "y",
    "CHATSTR",
    "DUP",
    "POP",
    "SWAP",
    "OVER",
    "PICK",
    "STACKDEPTH",
    "TOPTYPE",
    "IF",
    "IFELSE",
    "WHILE",
    "FOREACH",
    "EXEC",
    "RETURN",
    "BREAK",
    "EXIT",
    "DEF",
    "=",
    "GLOBAL",
    "ARRAY",
    "GET",
    "PUT",
    "LENGTH",
    "+",
    "-",
    "*",
    "/",
    "%",
    "++",
    "--",
    "+=",
    "-=",
    "&",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "AND",
    "OR",
    "NOT",
    "ATOI",
    "ITOA",
    "STRTOATOM",
    "STRINDEX",
    "SUBSTR",
    "SUBSTRING",
    "LOWERCASE",
    "UPPERCASE",
    "GREPSTR",
    "GREPSUB",
    "SINE",
    "RANDOM",
    "RANDOM",
    "X42",
    "ON",
    "SAY",
    "SETPOS",
    "(",
    ")",
    "@",
    "$1",
    "#",
    ";",
    "\\",
    "\\x41",
    " ",
    "\n",
    "\t",
];

fn tight_limits() -> Limits {
    Limits {
        stack_depth: 32,
        nesting: 8,
        steps: 500,
        while_iterations: 20,
        array_elements: 16,
        string_bytes: 256,
        variables: 32,
        ..Limits::default()
    }
}

fn run_with_limits(source: &str) -> Result<(), IptError> {
    let mut engine = Engine::new(TestHost::seeded(7)).with_limits(tight_limits());
    engine.run_source(source).map(|_| ())
}

#[test]
fn random_token_soup_never_panics_and_always_terminates() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    let mut ok = 0usize;
    let mut err = 0usize;
    for case in 0..5_000 {
        let count = 1 + rng.below(60);
        let mut source = String::new();
        for _ in 0..count {
            source.push_str(TOKENS[rng.below(TOKENS.len())]);
            source.push(' ');
        }
        match run_with_limits(&source) {
            Ok(()) => ok += 1,
            Err(_) => err += 1,
        }
        // Both outcomes are fine; reaching here at all is the assertion.
        assert!(ok + err == case + 1);
    }
    assert!(ok + err == 5_000);
}

#[test]
fn random_bytes_never_panic_in_lexing() {
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let commands = CommandSet::core();
    let limits = Limits::default();
    for _ in 0..5_000 {
        let count = rng.below(80);
        let bytes: Vec<u8> = (0..count).map(|_| (rng.next() & 0xff) as u8).collect();
        let source = iptscrae::decode_source(&bytes);
        let _ = parse_script(&source, &commands, &limits);
        let _ = parse_body(&source, &commands, &limits);
    }
}

#[test]
fn random_byte_soup_never_panics_in_the_vm() {
    let mut rng = Rng(0x0bad_c0de_1234_5678);
    for _ in 0..2_000 {
        let count = rng.below(60);
        let bytes: Vec<u8> = (0..count)
            .map(|_| {
                let b = (rng.next() & 0xff) as u8;
                if b.is_ascii_graphic() || b == b' ' || b == b'\n' {
                    b
                } else {
                    b' '
                }
            })
            .collect();
        let source = iptscrae::decode_source(&bytes);
        let _ = run_with_limits(&source);
    }
}

#[test]
fn an_endless_while_is_stopped_by_the_iteration_cap() {
    let error = run_with_limits("{ 1 } { 1 } WHILE").unwrap_err();
    assert_eq!(error.category(), "budget", "{error}");
}

#[test]
fn a_while_that_hides_in_an_exec_is_stopped_by_the_step_budget() {
    let error = run_with_limits("{ { 1 } { 1 } WHILE } EXEC").unwrap_err();
    assert!(matches!(error.category(), "budget" | "limit",));
}

#[test]
fn unbounded_recursion_hits_the_nesting_cap() {
    let source = "r GLOBAL { r EXEC } r DEF r GLOBAL r EXEC";
    let error = run_with_limits(source).unwrap_err();
    assert_eq!(error.category(), "budget", "{error}");
}

#[test]
fn a_very_long_straight_line_script_hits_the_step_budget() {
    let source = "1 POP ".repeat(5_000);
    let error = run_with_limits(&source).unwrap_err();
    assert_eq!(error.category(), "budget", "{error}");
}

#[test]
fn a_deeply_nested_atomlist_is_rejected_at_parse_time() {
    let depth = 5_000;
    let source = format!("{}{}", "{".repeat(depth), "}".repeat(depth));
    let commands = CommandSet::core();
    let error = parse_body(&source, &commands, &tight_limits()).unwrap_err();
    assert_eq!(error.category(), "budget", "{error}");
}

#[test]
fn a_huge_array_is_refused() {
    let error = run_with_limits("100000 ARRAY").unwrap_err();
    assert_eq!(error.category(), "limit", "{error}");
}

#[test]
fn an_unbounded_string_is_refused() {
    // Each iteration doubles a string; the cap must stop it long before OOM.
    let source = "s GLOBAL \"x\" s = { s s & s = } { s STRLEN 100000 < } WHILE";
    let error = run_with_limits(source).unwrap_err();
    assert!(matches!(error.category(), "limit" | "budget"), "{error}");
}

#[test]
fn a_stack_overflow_is_an_error_not_a_crash() {
    let source = "1 ".repeat(100);
    let error = run_with_limits(&source).unwrap_err();
    assert_eq!(error.category(), "stack", "{error}");
}

#[test]
fn an_index_out_of_range_is_an_error() {
    let error = run_with_limits("[ 1 2 ] 99 GET").unwrap_err();
    assert_eq!(error.category(), "type", "{error}");
}

#[test]
fn a_commented_out_script_is_empty_not_an_error() {
    let commands = CommandSet::core();
    let script = parse_script(
        "; ON ENTER { 1 }\n# nothing here",
        &commands,
        &Limits::default(),
    )
    .expect("parses");
    assert!(script.is_empty());
}

#[test]
fn hostile_input_error_messages_do_not_grow_without_bound() {
    // A 1 MiB string of `{` must fail quickly with a short message.
    let source = "{".repeat(1_000_000);
    let commands = CommandSet::core();
    let error = parse_body(&source, &commands, &tight_limits()).unwrap_err();
    assert!(format!("{error}").len() < 200);
}

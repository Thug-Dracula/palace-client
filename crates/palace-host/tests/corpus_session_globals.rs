//! Locks the cause of the five real-host corpus residuals.
//!
//! The default corpus walk isolates every harvested file (`reset_globals`
//! between files), but Palace Iptscrae globals live for the whole session: the
//! reference keeps one `globalVariableStore` on the manager
//! (`IptManager.as` constructor and `GLOBALCommand.as`), and `GLOBAL` links a
//! name to it. The language guide states the intent directly — a function "must
//! be declared GLOBAL if you want it to be recognized by any event handlers
//! other than the one it's defined in; ... it can be executed in any room in
//! your Palace" (`iptscrae.txt:3483-3486`).
//!
//! Each of the five residuals is a *consumer* handler that reads a global a
//! sibling hotspot (or an earlier room) defines:
//!
//! * `144_hs1`/`144_hs2` read `prar`, defined by `144_hs0`'s `ON ENTER`.
//! * `9211_hs2` reads `hnd`/`vlus`, defined by `9211_hs0`'s `ON ENTER`.
//! * `889_hs0` and `5308_hs4` read `iam`, defined by `893_hs1`'s `ON ENTER`.
//!
//! Every consumer names the word with `GLOBAL` and then `EXEC`s it. An unset
//! variable dereferences to the integer zero (`IptVariable.as` value getter)
//! and `EXEC` of zero pushes nothing (`EXECCommand.as`), so the consumer's
//! `Assign`/`Concat` pops an empty stack. The tests below run the defining and
//! consuming handlers through one engine with shared globals (clean) and run
//! the consumer alone (the reported fault), so the residual is proven to be
//! corpus scope rather than a VM fault. The script files live outside the
//! repository; the test reports a skip when they are absent.

use std::path::{Path, PathBuf};

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::{decode_source, parse_script, CommandSet, Script};
use palace_host::ScriptEngine;

fn corpus_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("IPTSCRAE_CORPUS") {
        return Some(PathBuf::from(dir));
    }
    let fallback = PathBuf::from("$CORPUS/scripts_clean/by_script");
    fallback.is_dir().then_some(fallback)
}

fn limits() -> Limits {
    Limits::default().with_dialect(StackDialect::PalaceChat)
}

fn parse(dir: &Path, name: &str, commands: &CommandSet, limits: Limits) -> Script {
    let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    let source = decode_source(&bytes);
    parse_script(&source, commands, &limits).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

/// Run one handler of `script` on `engine`, collapsing the outcome to a result.
fn run(engine: &mut ScriptEngine, script: &Script, handler: &str) -> Result<(), String> {
    let chunk = script
        .handler(handler)
        .unwrap_or_else(|| panic!("{handler} handler"));
    engine
        .run_chunk(0, chunk)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A fresh live engine and the command table its parse dictionary uses.
fn live_engine() -> (ScriptEngine, CommandSet) {
    let engine = ScriptEngine::new(limits());
    let commands = engine.commands().clone();
    (engine, commands)
}

fn session(dir: &Path, prerequisites: &[(&str, &str)], consumer: &str, handler: &str) {
    let (mut shared, commands) = live_engine();
    for (file, name) in prerequisites {
        let script = parse(dir, file, &commands, limits());
        run(&mut shared, &script, name)
            .unwrap_or_else(|e| panic!("{file} {name} should run clean: {e}"));
    }
    let script = parse(dir, consumer, &commands, limits());
    run(&mut shared, &script, handler).unwrap_or_else(|e| {
        panic!(
            "{consumer} {handler} should run clean once {:?} ran: {e}",
            prerequisites
        )
    });
}

fn isolated_fault(dir: &Path, consumer: &str, handler: &str) {
    let (mut isolated, commands) = live_engine();
    let script = parse(dir, consumer, &commands, limits());
    let fault = run(&mut isolated, &script, handler)
        .expect_err("the isolated walk must still show the reported fault");
    assert!(
        fault.contains("stack underflow"),
        "expected a stack underflow, got {fault}"
    );
}

fn proof(dir: &Path, prerequisites: &[(&str, &str)], consumer: &str, consumer_handler: &str) {
    session(dir, prerequisites, consumer, consumer_handler);
    isolated_fault(dir, consumer, consumer_handler);
}

#[test]
fn prar_is_a_sibling_hotspot_global() {
    let Some(dir) = corpus_dir() else {
        eprintln!("Iptscrae corpus not found; skipping");
        return;
    };
    for consumer in ["144_hs1.txt", "144_hs2.txt"] {
        proof(&dir, &[("144_hs0.txt", "ENTER")], consumer, "SELECT");
    }
}

#[test]
fn hnd_and_vlus_are_sibling_hotspot_globals() {
    let Some(dir) = corpus_dir() else {
        eprintln!("Iptscrae corpus not found; skipping");
        return;
    };
    proof(&dir, &[("9211_hs0.txt", "ENTER")], "9211_hs2.txt", "SELECT");
}

#[test]
fn iam_is_a_cross_room_session_global() {
    let Some(dir) = corpus_dir() else {
        eprintln!("Iptscrae corpus not found; skipping");
        return;
    };
    proof(&dir, &[("893_hs1.txt", "ENTER")], "889_hs0.txt", "ENTER");
    proof(
        &dir,
        &[("893_hs1.txt", "ENTER"), ("889_hs0.txt", "ENTER")],
        "5308_hs4.txt",
        "SELECT",
    );
}

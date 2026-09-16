//! Corpus regression test.
//!
//! The harvested corpus lives outside this repository, so the test is gated on
//! `IPTSCRAE_CORPUS` (a directory of per-hotspot `.txt` scripts). When it is
//! set, every file must parse and every handler must either run clean or fail
//! for a reason the milestone report already characterises; the thresholds
//! below pin the numbers the report quotes so a future change cannot quietly
//! regress them.

use std::path::PathBuf;

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::{parse_script, Engine};
use iptscrae_palace::harness::SkeletonHost;

#[test]
fn the_harvested_corpus_parses_and_runs() {
    let Some(dir) = std::env::var_os("IPTSCRAE_CORPUS").map(PathBuf::from) else {
        eprintln!("IPTSCRAE_CORPUS is not set; skipping the corpus regression test");
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("corpus directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no scripts found in {}", dir.display());

    let limits = Limits::default().with_dialect(StackDialect::PalaceChat);
    let commands = SkeletonHost::command_set();
    let mut engine = Engine::new(SkeletonHost::seeded(0))
        .with_limits(limits)
        .with_commands(commands.clone());

    let mut parsed = 0usize;
    let mut handlers = 0usize;
    let mut clean = 0usize;
    let mut parse_failures = Vec::new();
    let mut run_failures = Vec::new();

    for path in &files {
        let bytes = std::fs::read(path).expect("script is readable");
        let source = iptscrae::decode_source(&bytes);
        let script = match parse_script(&source, &commands, &limits) {
            Ok(script) => script,
            Err(e) => {
                parse_failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        parsed += 1;
        engine.reset_globals();
        for (name, chunk) in script.handlers() {
            handlers += 1;
            if let Err(e) = engine.run_handler(chunk) {
                run_failures.push(format!("{} ON {name}: {e}", path.display()));
            } else {
                clean += 1;
            }
        }
    }

    let parse_rate = parsed as f64 / files.len() as f64;
    let clean_rate = clean as f64 / handlers as f64;
    assert!(
        parse_rate >= 0.99,
        "parse rate {parse_rate:.4} below 99%:\n{}",
        parse_failures.join("\n")
    );
    assert!(
        clean_rate >= 0.99,
        "clean rate {clean_rate:.4} below 99%:\n{}",
        run_failures.join("\n")
    );
    eprintln!(
        "corpus: {} files, {} parsed, {} handlers, {} clean",
        files.len(),
        parsed,
        handlers,
        clean
    );
}

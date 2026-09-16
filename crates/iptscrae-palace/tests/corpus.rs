//! Corpus regression test.
//!
//! The harvested corpus lives outside this repository, so the test is gated on
//! `IPTSCRAE_CORPUS` (a directory of per-hotspot `.txt` scripts). When it is
//! set, every file must parse and every handler must either run clean or fail
//! for a reason the milestone report already characterises; the thresholds
//! below pin the numbers the report quotes so a future change cannot quietly
//! regress them.
//!
//! Two invariants are gated here, not just the rates:
//!
//! * **No failure may be class (a), a tokenizer gap.** That is the one class a
//!   correct core can never produce.
//! * **Sharing globals across the run may not make things worse**, and in fact
//!   fixes the handlers that read a global another script set — which is what the
//!   report claims when it calls those failures environmental.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::{parse_script, Engine};
use iptscrae_palace::classify::{classify_parse, classify_run, FailureClass, SourceSpellings};
use iptscrae_palace::harness::SkeletonHost;

#[derive(Debug, Default)]
struct Tally {
    files: usize,
    parsed: usize,
    handlers: usize,
    clean: usize,
    parse_classes: BTreeMap<FailureClass, usize>,
    run_classes: BTreeMap<FailureClass, usize>,
    parse_failures: Vec<String>,
    run_failures: Vec<String>,
}

impl Tally {
    fn count(map: &BTreeMap<FailureClass, usize>, class: FailureClass) -> usize {
        map.get(&class).copied().unwrap_or(0)
    }

    fn parse_gaps(&self) -> usize {
        Self::count(&self.parse_classes, FailureClass::TokenizerGap)
    }

    fn run_gaps(&self) -> usize {
        Self::count(&self.run_classes, FailureClass::TokenizerGap)
    }
}

fn run_corpus(dir: &Path, shared_globals: bool) -> Tally {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
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

    let mut tally = Tally {
        files: files.len(),
        ..Tally::default()
    };
    for path in &files {
        let bytes = std::fs::read(path).expect("script is readable");
        let source = iptscrae::decode_source(&bytes);
        let script = match parse_script(&source, &commands, &limits) {
            Ok(script) => script,
            Err(e) => {
                *tally.parse_classes.entry(classify_parse(&e)).or_insert(0) += 1;
                tally
                    .parse_failures
                    .push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        tally.parsed += 1;
        if !shared_globals {
            engine.reset_globals();
        }
        let spellings = SourceSpellings::scan(&source);
        for (name, chunk) in script.handlers() {
            tally.handlers += 1;
            match engine.run_handler(chunk) {
                Ok(_) => tally.clean += 1,
                Err(e) => {
                    let unknown = spellings.unknown_in(chunk, &commands);
                    *tally
                        .run_classes
                        .entry(classify_run(&e, &unknown))
                        .or_insert(0) += 1;
                    tally
                        .run_failures
                        .push(format!("{} ON {name}: {e}", path.display()));
                }
            }
        }
    }
    tally
}

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("IPTSCRAE_CORPUS").map(PathBuf::from)
}

#[test]
fn the_harvested_corpus_parses_and_runs() {
    let Some(dir) = corpus_dir() else {
        eprintln!("IPTSCRAE_CORPUS is not set; skipping the corpus regression test");
        return;
    };
    let tally = run_corpus(&dir, false);

    let parse_rate = tally.parsed as f64 / tally.files as f64;
    let clean_rate = tally.clean as f64 / tally.handlers as f64;
    assert_eq!(
        tally.parse_gaps(),
        0,
        "a failure of class (a) means the lexer rejects a character the reference accepts"
    );
    assert_eq!(tally.run_gaps(), 0, "no run failure may be a tokenizer gap");
    assert!(
        parse_rate >= 0.99,
        "parse rate {parse_rate:.4} below 99%:\n{}",
        tally.parse_failures.join("\n")
    );
    assert!(
        clean_rate >= 0.99,
        "clean rate {clean_rate:.4} below 99%:\n{}",
        tally.run_failures.join("\n")
    );
    eprintln!(
        "corpus (isolated globals): {} files, {} parsed, {} handlers, {} clean, classes {:?}/{:?}",
        tally.files,
        tally.parsed,
        tally.handlers,
        tally.clean,
        tally.parse_classes,
        tally.run_classes
    );
}

#[test]
fn sharing_globals_across_the_corpus_never_regresses_and_fixes_the_environmental_set() {
    let Some(dir) = corpus_dir() else {
        eprintln!("IPTSCRAE_CORPUS is not set; skipping the corpus regression test");
        return;
    };
    let isolated = run_corpus(&dir, false);
    let shared = run_corpus(&dir, true);

    assert_eq!(shared.files, isolated.files);
    assert_eq!(
        shared.parsed, isolated.parsed,
        "parsing does not depend on state"
    );
    assert!(
        shared.clean >= isolated.clean,
        "sharing globals made handlers fail that used to pass: {} -> {}",
        isolated.clean,
        shared.clean
    );
    assert!(
        Tally::count(&shared.run_classes, FailureClass::SemanticsOrEnvironment)
            <= Tally::count(&isolated.run_classes, FailureClass::SemanticsOrEnvironment),
        "the environmental class grew when globals were shared"
    );
    let fixed = shared.clean - isolated.clean;
    assert!(
        fixed > 0,
        "no handler was rescued by sharing globals, so the report's environmental claim is unsupported"
    );
    eprintln!(
        "corpus (shared globals): {} handlers, {} clean (+{fixed} vs isolated)",
        shared.handlers, shared.clean
    );
}

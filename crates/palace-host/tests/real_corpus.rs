//! The harvested corpus through the live dispatch, with a floor so a regression
//! in Palace command coverage is caught.
//!
//! The corpus lives outside the repository. When `IPTSCRAE_CORPUS` is set (or
//! the standard harvest directory exists) the whole thing walks through
//! [`palace_host::ScriptEngine`]; otherwise the test reports a skip. The floor
//! is the measured real-host baseline: parse and handler counts are stable
//! because they come from the VM, while `clean` moves as commands land, so only
//! a floor is asserted on it.

#[path = "support/corpus.rs"]
mod corpus;

use std::path::PathBuf;

fn corpus_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("IPTSCRAE_CORPUS") {
        return Some(PathBuf::from(dir));
    }
    let fallback = PathBuf::from("$CORPUS/scripts_clean/by_script");
    fallback.is_dir().then_some(fallback)
}

#[test]
fn the_real_host_corpus_meets_the_measured_floor() {
    let Some(dir) = corpus_dir() else {
        eprintln!("Iptscrae corpus not found; skipping the real-host floor test");
        return;
    };
    let shared_globals =
        std::env::var_os("IPTSCRAE_CORPUS_SHARED_GLOBALS").is_some_and(|value| value == "1");
    let examples = std::env::var("IPTSCRAE_CORPUS_EXAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    let report = corpus::walk(&dir, shared_globals, examples)
        .expect("the harvested corpus is readable and parseable");
    println!(
        "{}",
        corpus::render(&report, &dir.display().to_string(), shared_globals)
    );

    if std::env::var_os("IPTSCRAE_CORPUS_REPORT_ONLY").is_some() {
        return;
    }

    let tokenizer_gaps = report
        .parse_by_class
        .get(&iptscrae_palace::classify::FailureClass::TokenizerGap)
        .copied()
        .unwrap_or(0);
    let run_gaps = report
        .run_by_class
        .get(&iptscrae_palace::classify::FailureClass::TokenizerGap)
        .copied()
        .unwrap_or(0);

    assert_eq!(
        tokenizer_gaps, 0,
        "the lexer rejected a character the reference accepts"
    );
    assert_eq!(run_gaps, 0, "a run failure cannot be a tokenizer gap");
    assert!(report.files >= 2400, "corpus shrank: {}", report.files);
    assert!(
        report.parsed >= 2396,
        "parse regressed: {}/{}",
        report.parsed,
        report.files
    );
    assert!(
        report.handlers >= 3805,
        "handler count regressed: {}",
        report.handlers
    );
    assert!(
        report.clean >= 3750,
        "clean real-host handlers fell to {}/{}",
        report.clean,
        report.handlers
    );
    assert!(
        report.unsupported.is_empty(),
        "the live host reached commands it does not implement: {:?}",
        report.unsupported
    );
}

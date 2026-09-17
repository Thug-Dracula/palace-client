//! Integration tests for the `iptscrae` CLI harness.
//!
//! These invoke the built binary through `std::process::Command` and assert only
//! what a user observes: stdout, stderr and the exit status. The script fixtures
//! are written into a per-test temporary directory and removed on drop, so the
//! tests do not depend on the harvested corpus being installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A self-deleting temporary directory, unique per test within the process.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "iptscrae-cli-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create the temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Write a fixture, creating parent directories as needed.
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(&path, contents).expect("write fixture");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_iptscrae"))
        .args(args)
        .output()
        .expect("spawn the iptscrae binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_contains(haystack: &str, needle: &str, label: &str) {
    assert!(
        haystack.contains(needle),
        "{label}: expected {needle:?} in:\n{haystack}"
    );
}

fn assert_lacks(haystack: &str, needle: &str, label: &str) {
    assert!(
        !haystack.contains(needle),
        "{label}: did not expect {needle:?} in:\n{haystack}"
    );
}

/// The count on the classification row whose label starts with `label`, within
/// the report section that starts with `section`. Scoped by section because the
/// parse and run sections both list all four classes.
fn class_count(text: &str, section: &str, label: &str) -> u64 {
    let body = text
        .split_once(section)
        .unwrap_or_else(|| panic!("no report section {section:?} in:\n{text}"))
        .1;
    let line = body
        .lines()
        .find(|line| line.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no classification row {label:?} in section {section:?}"));
    line.rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("classification row {line:?} has no count"))
}

// ---------------------------------------------------------------------------
// help and dispatch
// ---------------------------------------------------------------------------

#[test]
fn no_arguments_prints_usage_and_succeeds() {
    let output = run(&[]);
    assert!(output.status.success(), "no args should succeed");
    let text = stdout(&output);
    assert_contains(&text, "USAGE:", "usage header");
    assert_contains(&text, "iptscrae run", "run synopsis");
    assert_contains(&text, "iptscrae corpus", "corpus synopsis");
    assert_contains(&text, "DIALECTS:", "dialect legend");
}

#[test]
fn help_spellings_all_print_usage_and_succeed() {
    for spelling in ["help", "--help", "-h"] {
        let output = run(&[spelling]);
        assert!(output.status.success(), "{spelling} should succeed");
        assert_contains(&stdout(&output), "USAGE:", spelling);
    }
}

#[test]
fn an_unknown_subcommand_fails_with_a_message_and_usage() {
    let output = run(&["frobnicate"]);
    assert!(!output.status.success(), "an unknown subcommand must fail");
    assert_contains(
        &stderr(&output),
        "unknown subcommand \"frobnicate\"",
        "error names the subcommand",
    );
    assert_contains(&stdout(&output), "USAGE:", "usage is still printed");
}

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

#[test]
fn eval_prints_the_final_stack() {
    let output = run(&["eval", "2 3 +"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "5");

    let output = run(&["eval", "1 2"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        "1 2",
        "multiple values render in order"
    );

    let output = run(&["eval", "\"hello\""]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        "\"hello\"",
        "strings render debug-style"
    );
}

#[test]
fn eval_marks_an_empty_stack_as_empty() {
    let output = run(&["eval", "1 POP"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "<empty>");
}

#[test]
fn eval_reports_a_runtime_fault_and_fails() {
    let output = run(&["eval", "1 +"]);
    assert!(!output.status.success(), "a stack fault must fail");
    assert_contains(&stderr(&output), "eval: [stack]", "category in the message");
    assert_contains(&stderr(&output), "Add", "the faulting command is named");
    assert!(
        stdout(&output).trim().is_empty(),
        "nothing is printed on the stack"
    );
}

#[test]
fn eval_without_a_source_is_an_error() {
    let output = run(&["eval"]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "eval: missing source", "usage error");
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn script_file(dir: &TempDir) -> PathBuf {
    dir.write(
        "script.txt",
        "ON ENTER {\n  \"hi\" SAY\n  2 3 +\n}\nON SELECT {\n  41 1 +\n}\n",
    )
}

#[test]
fn run_executes_every_handler_and_prints_its_stack() {
    let dir = TempDir::new();
    let path = script_file(&dir);
    let output = run(&["run", path.to_str().unwrap()]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(&text, "ON ENTER: ok, stack = 5", "ENTER result");
    assert_contains(&text, "ON SELECT: ok, stack = 42", "SELECT result");
}

#[test]
fn run_can_select_one_handler_case_insensitively() {
    let dir = TempDir::new();
    let path = script_file(&dir);
    let output = run(&["run", path.to_str().unwrap(), "--handler", "select"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(&text, "ON select: ok, stack = 42", "requested handler runs");
    assert_lacks(&text, "ON ENTER", "other handlers are skipped");
}

#[test]
fn run_with_a_handler_flag_and_no_value_runs_every_handler() {
    let dir = TempDir::new();
    let path = script_file(&dir);
    let output = run(&["run", path.to_str().unwrap(), "--handler"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(&text, "ON ENTER: ok", "ENTER still runs");
    assert_contains(&text, "ON SELECT: ok", "SELECT still runs");
}

#[test]
fn run_reports_a_missing_handler_and_fails() {
    let dir = TempDir::new();
    let path = script_file(&dir);
    let output = run(&["run", path.to_str().unwrap(), "--handler", "NOPE"]);
    assert!(!output.status.success(), "a missing handler must fail");
    assert_contains(
        &stderr(&output),
        "run: no handler named NOPE",
        "named error",
    );
}

#[test]
fn run_prints_the_host_trace_when_asked() {
    let dir = TempDir::new();
    let path = dir.write("trace.txt", "ON ENTER { \"msg\" _TRACE 7 }\n");
    let output = run(&["run", path.to_str().unwrap(), "--trace"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(&text, "ON ENTER: ok, stack = 7", "handler result");
    assert_contains(&text, "--- trace (1 lines) ---", "trace header with count");
    assert_contains(&text, "msg", "the traced line");
}

#[test]
fn run_reports_a_handler_runtime_fault_and_fails() {
    let dir = TempDir::new();
    let path = dir.write("bad.txt", "ON ENTER { 1 + }\n");
    let output = run(&["run", path.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "a runtime fault must fail the run"
    );
    let text = stdout(&output);
    assert_contains(&text, "ON ENTER: FAILED [stack]", "failure line");
    assert_contains(&text, "stack underflow", "underlying fault");
}

#[test]
fn run_reports_a_parse_failure_and_fails() {
    let dir = TempDir::new();
    let path = dir.write("parse.txt", "ON ENTER { \"unterminated\n");
    let output = run(&["run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "run: parse failed [lex]", "category");
    assert_contains(&stderr(&output), "unterminated string", "detail");
}

#[test]
fn run_reports_an_unreadable_file_and_a_missing_file_argument() {
    let output = run(&["run", "/nonexistent/script.txt"]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "run: cannot read", "read error");

    let output = run(&["run"]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "run: missing <file>", "usage error");
}

#[test]
fn run_rejects_a_bad_dialect_and_ignores_unknown_arguments() {
    let dir = TempDir::new();
    let path = script_file(&dir);

    let output = run(&["run", path.to_str().unwrap(), "--dialect", "bogus"]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "run: bad --dialect", "bad dialect");

    let output = run(&["run", path.to_str().unwrap(), "--dialect"]);
    assert!(!output.status.success(), "a --dialect with no value is bad");

    let output = run(&["run", path.to_str().unwrap(), "--wat"]);
    assert!(output.status.success(), "an unknown flag is ignored");
    assert_contains(
        &stderr(&output),
        "run: ignoring unknown argument \"--wat\"",
        "unknown arg warning",
    );
}

#[test]
fn run_honours_seed_and_dialect_flags() {
    let dir = TempDir::new();
    let path = script_file(&dir);
    for flags in [
        vec!["--seed", "5"],
        vec!["--seed", "not-a-number"],
        vec!["--dialect", "windows"],
        vec!["--dialect", "openpalace"],
        vec!["--dialect", "palacechat"],
    ] {
        let mut args = vec!["run", path.to_str().unwrap()];
        args.extend(flags.iter().copied());
        let output = run(&args);
        assert!(output.status.success(), "{flags:?} should run");
    }
}

#[test]
fn the_dialect_flag_changes_the_stack_budget() {
    let dir = TempDir::new();
    let pushes = vec!["1"; 300].join(" ");
    let path = dir.write("deep.txt", &format!("ON ENTER {{ {pushes} }}\n"));

    let narrow = run(&["run", path.to_str().unwrap(), "--dialect", "windows"]);
    assert!(
        !narrow.status.success(),
        "300 pushes must overflow the 256-deep Windows stack"
    );
    assert_contains(
        &stdout(&narrow),
        "stack overflow: depth limit is 256",
        "the Windows budget is the one that applied",
    );

    let wide = run(&["run", path.to_str().unwrap(), "--dialect", "openpalace"]);
    assert!(
        wide.status.success(),
        "the 2048-deep OpenPalace stack holds 300 values"
    );
}

// ---------------------------------------------------------------------------
// corpus
// ---------------------------------------------------------------------------

/// A corpus with one clean file, one run failure, one parse failure and one
/// nested clean file.
fn mixed_corpus(dir: &TempDir) -> PathBuf {
    dir.write("good.txt", "ON ENTER {\n  \"hi\" SAY\n  2 3 +\n}\n");
    dir.write("bad.txt", "ON ENTER { 1 + }\n");
    dir.write("parse.txt", "ON ENTER { \"unterminated\n");
    dir.write("sub/deep.txt", "ON ENTER { 9 9 * }\n");
    dir.path().to_path_buf()
}

#[test]
fn corpus_summarises_every_file_and_recurses_into_subdirectories() {
    let dir = TempDir::new();
    let root = mixed_corpus(&dir);
    let output = run(&["corpus", root.to_str().unwrap()]);
    assert!(output.status.success());
    let text = stdout(&output);

    assert_contains(&text, "IPTSCRAE corpus run", "report header");
    assert_contains(&text, "files          : 4", "nested file is counted");
    assert_contains(&text, "parsed         : 3 (75.0%)", "parse rate");
    assert_contains(&text, "parse failures : 1 (25.0%)", "parse failure rate");
    assert_contains(
        &text,
        "handlers       : 3 in 3 parsed files",
        "handler count",
    );
    assert_contains(
        &text,
        "ran clean      : 2 (66.7% of handlers)",
        "clean handler rate",
    );
    assert_contains(
        &text,
        "files fully ok : 2 (66.7% of parsed files)",
        "clean file rate",
    );
    assert_contains(&text, "Parse failure classification:", "parse classes");
    assert_contains(&text, "Run failure classification:", "run classes");
    assert_eq!(
        class_count(
            &text,
            "Parse failure classification:",
            "(d) malformed source"
        ),
        1,
        "parse class (d)"
    );
    assert_eq!(
        class_count(
            &text,
            "Run failure classification:",
            "(c) semantics / environment"
        ),
        1,
        "run class (c)"
    );
    assert_contains(&text, "Parse failures by message:", "parse messages");
    assert_contains(&text, "Run failures by message:", "run messages");
    assert_contains(&text, "Example parse failures:", "parse examples");
    assert_contains(&text, "Example failures:", "run examples");
    assert_contains(
        &text,
        "Palace commands exercised (1 distinct):",
        "usage header",
    );
    assert_contains(&text, "SAY", "the exercised command is listed");
}

#[test]
fn corpus_reports_unregistered_command_spellings() {
    let dir = TempDir::new();
    dir.write("unimpl.txt", "ON ENTER { UNKNOWNCOMMAND 1 \"a\" & }\n");
    let output = run(&["corpus", dir.path().to_str().unwrap()]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_eq!(
        class_count(
            &text,
            "Run failure classification:",
            "(b) unimplemented command"
        ),
        1,
        "the failure is classified as a missing command"
    );
    assert_contains(
        &text,
        "Unregistered symbols named by failing handlers",
        "the report lists the spelling seen",
    );
    assert_contains(&text, "UNKNOWNCOMMAND", "the source spelling is recovered");
    assert_contains(
        &text,
        "Concat: expected string, found number",
        "fault message",
    );
}

#[test]
fn corpus_handles_a_directory_with_no_parsable_handlers() {
    let dir = TempDir::new();
    dir.write("only.txt", "ON ENTER { \"unterminated\n");
    let output = run(&["corpus", dir.path().to_str().unwrap()]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(
        &text,
        "handlers       : 0 in 0 parsed files",
        "zero handlers",
    );
    assert_contains(
        &text,
        "ran clean      : 0 (0.0% of handlers)",
        "zero percent is not a division by zero",
    );
    assert_contains(
        &text,
        "files fully ok : 0 (0.0% of parsed files)",
        "zero percent over parsed files",
    );
}

#[test]
fn corpus_examples_can_be_suppressed() {
    let dir = TempDir::new();
    dir.write("bad.txt", "ON ENTER { 1 + }\n");
    dir.write("parse.txt", "ON ENTER { \"unterminated\n");
    let root = dir.path().to_str().unwrap();

    let with_examples = run(&["corpus", root]);
    assert_contains(
        &stdout(&with_examples),
        "Example failures:",
        "default keeps examples",
    );

    let without = run(&["corpus", root, "--examples", "0"]);
    let text = stdout(&without);
    assert_lacks(&text, "Example parse failures:", "examples suppressed");
    assert_lacks(&text, "Example failures:", "run examples suppressed");
}

#[test]
fn corpus_honours_dialect_seed_globals_and_rejects_bad_dialect() {
    let dir = TempDir::new();
    let root = mixed_corpus(&dir);
    let root = root.to_str().unwrap();

    let shared = run(&[
        "corpus",
        root,
        "--shared-globals",
        "--seed",
        "3",
        "--dialect",
        "openpalace",
    ]);
    assert!(shared.status.success());
    let text = stdout(&shared);
    assert_contains(
        &text,
        "shared across the whole run (--shared-globals)",
        "globals mode",
    );
    assert_contains(&text, "seed           : 3", "seed echo");
    assert_contains(&text, "OpenPalace (stack 2048)", "dialect echo");

    let isolated = run(&["corpus", root]);
    assert_contains(
        &stdout(&isolated),
        "isolated per script file",
        "default globals mode",
    );

    let bad = run(&["corpus", root, "--dialect", "bogus"]);
    assert!(!bad.status.success());
    assert_contains(&stderr(&bad), "corpus: bad --dialect", "bad dialect");

    let ignored = run(&["corpus", root, "--wat"]);
    assert!(ignored.status.success());
    assert_contains(
        &stderr(&ignored),
        "corpus: ignoring unknown argument \"--wat\"",
        "unknown arg warning",
    );
}

#[test]
#[cfg(unix)]
fn corpus_counts_an_unreadable_file_as_malformed_source() {
    let dir = TempDir::new();
    dir.write("good.txt", "ON ENTER { 1 2 + }\n");
    std::os::unix::fs::symlink(
        dir.path().join("does-not-exist"),
        dir.path().join("gone.txt"),
    )
    .expect("create a broken symlink");

    let output = run(&["corpus", dir.path().to_str().unwrap()]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert_contains(&text, "files          : 2", "the broken link is collected");
    assert_contains(&text, "unreadable:", "the read error is recorded");
    assert_eq!(
        class_count(
            &text,
            "Parse failure classification:",
            "(d) malformed source"
        ),
        1,
        "an unreadable file is malformed source"
    );
}

#[test]
fn corpus_reports_missing_empty_and_unreadable_directories() {
    let missing = run(&["corpus"]);
    assert!(!missing.status.success());
    assert_contains(&stderr(&missing), "corpus: missing <dir>", "usage error");

    let unreadable = run(&["corpus", "/nonexistent/corpus"]);
    assert!(!unreadable.status.success());
    assert_contains(&stderr(&unreadable), "corpus: cannot read", "read error");

    let empty = TempDir::new();
    let output = run(&["corpus", empty.path().to_str().unwrap()]);
    assert!(!output.status.success());
    assert_contains(&stderr(&output), "corpus: no scripts in", "empty directory");
}

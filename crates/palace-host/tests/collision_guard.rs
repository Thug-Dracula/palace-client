//! The corpus-wide command/variable collision guard.
//!
//! Registers a name in the command table means every script that used that
//! spelling as a variable silently changes meaning: the lexer emits a command
//! where the assignment needs a variable reference. This walks the whole
//! harvested corpus and refuses any registered name that a script uses as an
//! assignment target (`GLOBAL`, `=`, `DEF`, `++`, `--`, compound assignment)
//! while spelling it in lower case — the `mousex` bug, generalised.
//!
//! The corpus lives outside the repository, so the test reports a skip when it
//! is absent, exactly like `real_corpus`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iptscrae::budget::{Limits, StackDialect};
use iptscrae_palace::classify::variable_position_collisions;
use palace_host::ScriptEngine;

fn corpus_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("IPTSCRAE_CORPUS") {
        return Some(PathBuf::from(dir));
    }
    let fallback = PathBuf::from("$CORPUS/scripts_clean/by_script");
    fallback.is_dir().then_some(fallback)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "txt") {
            out.push(path);
        }
    }
}

#[test]
fn the_corpus_assignments_to_str_run_clean_through_the_live_host() {
    let mut engine = ScriptEngine::new(Limits::default().with_dialect(StackDialect::PalaceChat));
    engine
        .run_source("str GLOBAL 0 str = 7 str =")
        .expect("str is a variable");
}

#[test]
fn no_command_name_is_used_as_a_variable_target_in_the_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("Iptscrae corpus not found; skipping the collision guard");
        return;
    };
    let engine = ScriptEngine::new(Limits::default().with_dialect(StackDialect::PalaceChat));
    let commands = engine.commands().clone();

    let mut files = Vec::new();
    collect_files(&dir, &mut files);
    files.sort();

    let mut collisions: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = iptscrae::decode_source(&bytes);
        for name in variable_position_collisions(&source, &commands) {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let row = collisions.entry(name).or_default();
            row.0 += 1;
            if row.1.len() < 5 {
                row.1.push(label);
            }
        }
    }

    assert!(
        collisions.is_empty(),
        "registered command names used as variable targets (remove them from the parse dictionary): {collisions:#?}"
    );
}

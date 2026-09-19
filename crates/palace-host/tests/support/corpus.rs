//! Real-host corpus measurement shared by the `real_corpus` test and the
//! `iptscrae` CLI's `corpus --real-host` mode.
//!
//! The skeleton host stubs every Palace command, so a clean run there proves
//! the VM, not Palace semantics. This walks the same harvested scripts through
//! [`ScriptEngine`] — the live dispatch — and counts what the real host cannot
//! do: unregistered names, VM faults, and every command that reached the live
//! host and turned out to be inert.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::parse_script;
use iptscrae_palace::classify::{
    classify_parse, classify_run, faulting_command, FailureClass, SourceSpellings, ALL_CLASSES,
};
use palace_host::ScriptEngine;

/// What one real-host corpus walk measured.
#[derive(Debug, Default)]
pub struct CorpusReport {
    /// Files found in the corpus directory.
    pub files: usize,
    /// Files that parsed.
    pub parsed: usize,
    /// Handlers run across every parsed file.
    pub handlers: u64,
    /// Handlers that ran without a fault.
    pub clean: u64,
    /// Parsed files whose every handler ran clean.
    pub clean_files: u64,
    /// Parse failures by milestone class.
    pub parse_by_class: BTreeMap<FailureClass, u64>,
    /// Run failures by milestone class.
    pub run_by_class: BTreeMap<FailureClass, u64>,
    /// Parse failures by message.
    pub parse_messages: BTreeMap<String, u64>,
    /// Run failures by message.
    pub run_messages: BTreeMap<String, u64>,
    /// VM command named by each run failure (the innermost fault).
    pub faulting_commands: BTreeMap<String, u64>,
    /// Unregistered symbols the failing handlers named, as spelled.
    pub unregistered: BTreeMap<String, u64>,
    /// Commands the live host reached but does not implement, with call counts.
    pub unsupported: BTreeMap<String, u64>,
    /// Example parse failures, capped by the caller's limit.
    pub parse_examples: Vec<String>,
    /// Example run failures, capped by the caller's limit.
    pub run_examples: Vec<String>,
}

/// Walk `dir` through the real host.
///
/// `shared_globals` runs the whole corpus through one global store instead of
/// clearing between files; `examples` caps the sample failures kept for the
/// report.
pub fn walk(dir: &Path, shared_globals: bool, examples: usize) -> Result<CorpusReport, String> {
    let files = collect_files(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    if files.is_empty() {
        return Err(format!("no scripts in {}", dir.display()));
    }

    let limits = Limits::default().with_dialect(StackDialect::PalaceChat);
    let mut engine = ScriptEngine::new(limits);
    let commands = engine.commands().clone();

    let mut report = CorpusReport {
        files: files.len(),
        ..CorpusReport::default()
    };

    for path in &files {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let message = format!("unreadable: {error}");
                *report
                    .parse_by_class
                    .entry(FailureClass::MalformedSource)
                    .or_insert(0) += 1;
                *report.parse_messages.entry(message.clone()).or_insert(0) += 1;
                push_example(&mut report.parse_examples, examples, || {
                    format!("{label}: {message}")
                });
                continue;
            }
        };
        let source = iptscrae::decode_source(&bytes);
        let script = match parse_script(&source, &commands, &limits) {
            Ok(script) => script,
            Err(error) => {
                let class = classify_parse(&error);
                *report.parse_by_class.entry(class).or_insert(0) += 1;
                *report.parse_messages.entry(error.to_string()).or_insert(0) += 1;
                push_example(&mut report.parse_examples, examples, || {
                    format!("{label}: [{} {class:?}] {error}", error.category())
                });
                continue;
            }
        };
        report.parsed += 1;
        if !shared_globals {
            engine.reset_globals();
        }
        let spellings = SourceSpellings::scan(&source);
        let mut file_clean = true;
        for (name, chunk) in script.handlers() {
            report.handlers += 1;
            match engine.run_chunk(0, chunk) {
                Ok(_) => report.clean += 1,
                Err(error) => {
                    file_clean = false;
                    let unknown = spellings.unknown_in(chunk, &commands);
                    let class = classify_run(&error, &unknown);
                    *report.run_by_class.entry(class).or_insert(0) += 1;
                    *report.run_messages.entry(error.to_string()).or_insert(0) += 1;
                    if let Some(command) = faulting_command(&error) {
                        *report
                            .faulting_commands
                            .entry(command.to_owned())
                            .or_insert(0) += 1;
                    }
                    for spelling in &unknown {
                        *report.unregistered.entry(spelling.clone()).or_insert(0) += 1;
                    }
                    push_example(&mut report.run_examples, examples, || {
                        let fault = faulting_command(&error).unwrap_or("-");
                        let names = if unknown.is_empty() {
                            String::new()
                        } else {
                            format!("  unregistered: {}", unknown.join(" "))
                        };
                        format!(
                            "{label} ON {name}: [{} {class:?}] {fault}: {error}{names}",
                            error.category()
                        )
                    });
                }
            }
        }
        if file_clean {
            report.clean_files += 1;
        }
    }

    report.unsupported = engine.host().unsupported.clone();
    Ok(report)
}

/// Render the report in the same shape as the skeleton-host `corpus` output so
/// the two can be diffed.
#[must_use]
pub fn render(report: &CorpusReport, dir: &str, shared_globals: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "IPTSCRAE real-host corpus run");
    let _ = writeln!(out, "  directory      : {dir}");
    let _ = writeln!(
        out,
        "  dialect        : {:?} (stack {})",
        StackDialect::PalaceChat,
        StackDialect::PalaceChat.stack_depth()
    );
    let _ = writeln!(out, "  seed           : 0");
    let _ = writeln!(
        out,
        "  globals        : {}",
        if shared_globals {
            "shared across the whole run"
        } else {
            "isolated per script file"
        }
    );
    let _ = writeln!(
        out,
        "  host           : palace_host::ScriptEngine (live dispatch)"
    );
    let _ = writeln!(out, "  files          : {}", report.files);
    let _ = writeln!(
        out,
        "  parsed         : {} ({:.1}%)",
        report.parsed,
        percent(report.parsed as u64, report.files as u64)
    );
    let parse_total: u64 = report.parse_by_class.values().sum();
    let _ = writeln!(
        out,
        "  parse failures : {parse_total} ({:.1}%)",
        percent(parse_total, report.files as u64)
    );
    let _ = writeln!(
        out,
        "  handlers       : {} in {} parsed files",
        report.handlers, report.parsed
    );
    let _ = writeln!(
        out,
        "  ran clean      : {} ({:.1}% of handlers)",
        report.clean,
        percent(report.clean, report.handlers)
    );
    let _ = writeln!(
        out,
        "  files fully ok : {} ({:.1}% of parsed files)",
        report.clean_files,
        percent(report.clean_files, report.parsed as u64)
    );

    let _ = writeln!(out, "\nParse failure classification:");
    for class in ALL_CLASSES {
        let _ = writeln!(
            out,
            "  {:<30} {}",
            class.label(),
            report.parse_by_class.get(&class).copied().unwrap_or(0)
        );
    }
    let _ = writeln!(out, "Run failure classification:");
    for class in ALL_CLASSES {
        let _ = writeln!(
            out,
            "  {:<30} {}",
            class.label(),
            report.run_by_class.get(&class).copied().unwrap_or(0)
        );
    }

    let _ = writeln!(out, "\nParse failures by message:");
    for (message, count) in by_count(&report.parse_messages, 12) {
        let _ = writeln!(out, "  {count:5}  {message}");
    }
    let _ = writeln!(out, "\nRun failures by message:");
    for (message, count) in by_count(&report.run_messages, 15) {
        let _ = writeln!(out, "  {count:5}  {message}");
    }

    let _ = writeln!(
        out,
        "\nCommands named by run failures (the innermost fault):"
    );
    for (name, count) in by_count(&report.faulting_commands, 20) {
        let _ = writeln!(out, "  {count:5}  {name}");
    }

    let _ = writeln!(
        out,
        "\nUnregistered symbols named by failing handlers (spelling as written):"
    );
    for (name, count) in by_count(&report.unregistered, 20) {
        let _ = writeln!(out, "  {count:5}  {name}");
    }

    let unsupported_total: u64 = report.unsupported.values().sum();
    let _ = writeln!(
        out,
        "\nPalace commands the live host reached but does not implement ({} distinct, {unsupported_total} calls):",
        report.unsupported.len()
    );
    for (name, count) in by_count(&report.unsupported, 40) {
        let _ = writeln!(out, "  {count:5}  {name}");
    }

    if !report.parse_examples.is_empty() {
        let _ = writeln!(out, "\nExample parse failures:");
        for example in &report.parse_examples {
            let _ = writeln!(out, "  {example}");
        }
    }
    if !report.run_examples.is_empty() {
        let _ = writeln!(out, "\nExample run failures:");
        for example in &report.run_examples {
            let _ = writeln!(out, "  {example}");
        }
    }
    out
}

fn collect_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_files(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "txt") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn push_example(examples: &mut Vec<String>, limit: usize, make: impl FnOnce() -> String) {
    if examples.len() < limit {
        examples.push(make());
    }
}

fn by_count(map: &BTreeMap<String, u64>, limit: usize) -> Vec<(&str, u64)> {
    let mut rows: Vec<(&str, u64)> = map
        .iter()
        .map(|(name, count)| (name.as_str(), *count))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    rows.truncate(limit);
    rows
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

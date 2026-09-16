//! Command-line harness for the IPTSCRAE VM.
//!
//! ```text
//! iptscrae run   <file> [--handler NAME] [--dialect D] [--seed N] [--trace]
//! iptscrae eval  "<source>"
//! iptscrae corpus <dir> [--dialect D] [--seed N] [--examples N]
//! ```
//!
//! `run` and `eval` execute a script against the skeleton host and print the
//! final stack. `corpus` walks a directory of harvested scripts, parses and
//! executes every handler, and prints the parse/run/failure distribution that
//! the milestone report is built from. Every failure is sorted into the
//! milestone's four classes — tokenizer gap, unimplemented command, semantics or
//! environment, malformed source — by [`iptscrae_palace::classify`].
//!
//! `--shared-globals` runs the whole corpus through one global store instead of
//! isolating each file, which is how a real session behaves: scripts that read a
//! global another script in the room set can then run.
//!
//! The skeleton host implements no Palace command: it consumes the documented
//! operands and pushes neutral defaults. So `corpus` measures the VM core, not
//! Palace semantics.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::parse_script;
use iptscrae::value::Value;
use iptscrae_palace::classify::{
    self, classify_parse, classify_run, FailureClass, SourceSpellings, ALL_CLASSES,
};
use iptscrae_palace::harness::SkeletonHost;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("eval") => cmd_eval(&args[1..]),
        Some("corpus") => cmd_corpus(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown subcommand {other:?}");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    println!(
        "iptscrae — IPTSCRAE language harness

USAGE:
  iptscrae run    <file> [--handler NAME] [--dialect D] [--seed N] [--trace]
  iptscrae eval   \"<source>\"
  iptscrae corpus <dir>  [--dialect D] [--seed N] [--examples N] [--shared-globals]

DIALECTS: windows (256) | palacechat (1024, default) | openpalace (2048)"
    );
}

fn cmd_run(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("run: missing <file>");
        return ExitCode::FAILURE;
    };
    let mut handler: Option<String> = None;
    let mut dialect = StackDialect::PalaceChat;
    let mut seed = 0u64;
    let mut show_trace = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--handler" => {
                index += 1;
                handler = args.get(index).cloned();
            }
            "--dialect" => {
                index += 1;
                match args.get(index).map(String::as_str).and_then(parse_dialect) {
                    Some(d) => dialect = d,
                    None => {
                        eprintln!("run: bad --dialect");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--seed" => {
                index += 1;
                seed = args.get(index).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--trace" => show_trace = true,
            other => eprintln!("run: ignoring unknown argument {other:?}"),
        }
        index += 1;
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("run: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let source = iptscrae::decode_source(&bytes);
    let limits = Limits::default().with_dialect(dialect);
    let script = match parse_script(&source, &SkeletonHost::command_set(), &limits) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("run: parse failed [{}]: {e}", e.category());
            return ExitCode::FAILURE;
        }
    };

    let mut engine = iptscrae::Engine::new(SkeletonHost::seeded(seed))
        .with_limits(limits)
        .with_commands(SkeletonHost::command_set());

    let names: Vec<String> = match &handler {
        Some(name) => vec![name.clone()],
        None => script.handler_names().map(str::to_owned).collect(),
    };

    let mut failed = false;
    for name in names {
        let Some(chunk) = script.handler(&name) else {
            eprintln!("run: no handler named {name}");
            failed = true;
            continue;
        };
        match engine.run_handler_collect(chunk) {
            Ok(stack) => {
                println!("ON {name}: ok, stack = {}", render(&stack));
            }
            Err(e) => {
                println!("ON {name}: FAILED [{}] {e}", e.category());
                failed = true;
            }
        }
    }

    if show_trace {
        println!("--- trace ({} lines) ---", engine.host.trace.len());
        for line in &engine.host.trace {
            println!("{line}");
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_eval(args: &[String]) -> ExitCode {
    let Some(source) = args.first() else {
        eprintln!("eval: missing source");
        return ExitCode::FAILURE;
    };
    let mut engine = SkeletonHost::engine();
    match engine.run_source_collect(source) {
        Ok(stack) => {
            println!("{}", render(&stack));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("eval: [{}] {e}", e.category());
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct FailureTally {
    messages: BTreeMap<String, u64>,
    examples: Vec<String>,
    classes: BTreeMap<FailureClass, u64>,
    names: BTreeMap<String, u64>,
}

impl FailureTally {
    fn record(
        &mut self,
        class: FailureClass,
        message: String,
        names: &[String],
        example: String,
        keep_examples: usize,
    ) {
        *self.classes.entry(class).or_insert(0) += 1;
        *self.messages.entry(message).or_insert(0) += 1;
        for name in names {
            *self.names.entry(name.clone()).or_insert(0) += 1;
        }
        if self.examples.len() < keep_examples {
            self.examples.push(example);
        }
    }

    fn total(&self) -> u64 {
        self.classes.values().sum()
    }

    fn of(&self, class: FailureClass) -> u64 {
        self.classes.get(&class).copied().unwrap_or(0)
    }

    fn top(&self, n: usize) -> Vec<(&str, u64)> {
        let mut rows: Vec<(&str, u64)> = self
            .messages
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        rows.truncate(n);
        rows
    }

    fn top_names(&self, n: usize) -> Vec<(&str, u64)> {
        let mut rows: Vec<(&str, u64)> = self.names.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        rows.truncate(n);
        rows
    }

    fn report(&self, out: &mut String, heading: &str) {
        let _ = writeln!(out, "\n{heading}");
        for class in ALL_CLASSES {
            let _ = writeln!(out, "  {:<30} {}", class.label(), self.of(class));
        }
    }
}

fn cmd_corpus(args: &[String]) -> ExitCode {
    let Some(dir) = args.first() else {
        eprintln!("corpus: missing <dir>");
        return ExitCode::FAILURE;
    };
    let mut dialect = StackDialect::PalaceChat;
    let mut seed = 0u64;
    let mut examples = 12usize;
    let mut shared_globals = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--dialect" => {
                index += 1;
                match args.get(index).map(String::as_str).and_then(parse_dialect) {
                    Some(d) => dialect = d,
                    None => {
                        eprintln!("corpus: bad --dialect");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--seed" => {
                index += 1;
                seed = args.get(index).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--examples" => {
                index += 1;
                examples = args.get(index).and_then(|s| s.parse().ok()).unwrap_or(12);
            }
            "--shared-globals" => shared_globals = true,
            other => eprintln!("corpus: ignoring unknown argument {other:?}"),
        }
        index += 1;
    }

    let files = match collect_files(Path::new(dir)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("corpus: cannot read {dir}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        eprintln!("corpus: no scripts in {dir}");
        return ExitCode::FAILURE;
    }

    let limits = Limits::default().with_dialect(dialect);
    let commands = SkeletonHost::command_set();
    let mut engine = iptscrae::Engine::new(SkeletonHost::seeded(seed))
        .with_limits(limits)
        .with_commands(commands.clone());

    let mut parsed_files = 0u64;
    let mut clean_files = 0u64;
    let mut handlers_total = 0u64;
    let mut handlers_clean = 0u64;
    let mut parse_failures = FailureTally::default();
    let mut run_failures = FailureTally::default();

    for path in &files {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                let message = format!("unreadable: {e}");
                parse_failures.record(
                    FailureClass::MalformedSource,
                    message.clone(),
                    &[],
                    format!("{label}: {message}"),
                    examples,
                );
                continue;
            }
        };
        let source = iptscrae::decode_source(&bytes);
        let script = match parse_script(&source, &commands, &limits) {
            Ok(s) => s,
            Err(e) => {
                let class = classify_parse(&e);
                parse_failures.record(
                    class,
                    e.to_string(),
                    &[],
                    format!("{label}: [{} {class:?}] {e}", e.category()),
                    examples,
                );
                continue;
            }
        };
        parsed_files += 1;
        if !shared_globals {
            engine.reset_globals();
        }
        let spellings = SourceSpellings::scan(&source);
        let mut file_clean = true;
        for (name, chunk) in script.handlers() {
            handlers_total += 1;
            match engine.run_handler(chunk) {
                Ok(_) => handlers_clean += 1,
                Err(e) => {
                    file_clean = false;
                    let unknown = spellings.unknown_in(chunk, &commands);
                    let class = classify_run(&e, &unknown);
                    let fault = classify::faulting_command(&e).unwrap_or("-");
                    let names = if unknown.is_empty() {
                        String::new()
                    } else {
                        format!("  unregistered: {}", unknown.join(" "))
                    };
                    run_failures.record(
                        class,
                        e.to_string(),
                        &unknown,
                        format!(
                            "{label} ON {name}: [{} {class:?}] {fault}: {e}{names}",
                            e.category()
                        ),
                        examples,
                    );
                }
            }
        }
        if file_clean {
            clean_files += 1;
        }
    }

    let mut report = String::new();
    let _ = writeln!(report, "IPTSCRAE corpus run");
    let _ = writeln!(report, "  directory      : {}", dir);
    let _ = writeln!(
        report,
        "  dialect        : {dialect:?} (stack {})",
        dialect.stack_depth()
    );
    let _ = writeln!(report, "  seed           : {seed}");
    let _ = writeln!(
        report,
        "  globals        : {}",
        if shared_globals {
            "shared across the whole run (--shared-globals)"
        } else {
            "isolated per script file"
        }
    );
    let _ = writeln!(report, "  files          : {}", files.len());
    let _ = writeln!(
        report,
        "  parsed         : {parsed_files} ({:.1}%)",
        percent(parsed_files, files.len() as u64)
    );
    let _ = writeln!(
        report,
        "  parse failures : {} ({:.1}%)",
        parse_failures.total(),
        percent(parse_failures.total(), files.len() as u64)
    );
    let _ = writeln!(
        report,
        "  handlers       : {handlers_total} in {parsed_files} parsed files"
    );
    let _ = writeln!(
        report,
        "  ran clean      : {handlers_clean} ({:.1}% of handlers)",
        percent(handlers_clean, handlers_total)
    );
    let _ = writeln!(
        report,
        "  files fully ok : {clean_files} ({:.1}% of parsed files)",
        percent(clean_files, parsed_files)
    );

    parse_failures.report(&mut report, "Parse failure classification:");
    run_failures.report(&mut report, "Run failure classification:");

    let _ = writeln!(report, "\nParse failures by message:");
    for (message, count) in parse_failures.top(12) {
        let _ = writeln!(report, "  {count:5}  {message}");
    }
    if !parse_failures.examples.is_empty() {
        let _ = writeln!(report, "\nExample parse failures:");
        for example in &parse_failures.examples {
            let _ = writeln!(report, "  {example}");
        }
    }

    let _ = writeln!(report, "\nRun failures by message:");
    for (message, count) in run_failures.top(15) {
        let _ = writeln!(report, "  {count:5}  {message}");
    }
    if !run_failures.names.is_empty() {
        let _ = writeln!(
            report,
            "\nUnregistered symbols named by failing handlers (spelling as written):"
        );
        for (name, count) in run_failures.top_names(20) {
            let _ = writeln!(report, "  {count:5}  {name}");
        }
    }
    if !run_failures.examples.is_empty() {
        let _ = writeln!(report, "\nExample failures:");
        for example in &run_failures.examples {
            let _ = writeln!(report, "  {example}");
        }
    }

    let usage = engine.host.usage();
    let _ = writeln!(
        report,
        "\nPalace commands exercised ({} distinct):",
        usage.len()
    );
    for (name, count) in usage.iter().take(30) {
        let _ = writeln!(report, "  {count:7}  {name}");
    }

    print!("{report}");
    ExitCode::SUCCESS
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn parse_dialect(name: &str) -> Option<StackDialect> {
    match name.to_ascii_lowercase().as_str() {
        "windows" => Some(StackDialect::Windows),
        "palacechat" => Some(StackDialect::PalaceChat),
        "openpalace" => Some(StackDialect::OpenPalace),
        _ => None,
    }
}

fn collect_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "txt") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn render(stack: &[Value]) -> String {
    if stack.is_empty() {
        return "<empty>".to_owned();
    }
    stack
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

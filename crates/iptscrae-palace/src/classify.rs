//! Failure classification for the corpus run.
//!
//! The milestone asks for every corpus failure to be sorted into one of four
//! buckets, because the buckets mean very different things:
//!
//! | Class | Meaning | Healthy? |
//! |---|---|---|
//! | (a) tokenizer gap | our lexer rejects a character the reference accepts | **no** — the core is wrong |
//! | (b) unimplemented command | the handler calls an extension this milestone does not ship | yes, expected |
//! | (c) semantics / environment | the reference agrees with us; the script needed host state we did not provide | yes, expected |
//! | (d) malformed source | the reference tokenizer rejects it too | yes, expected |
//!
//! A large (a) or a large (c) cluster means the VM is wrong; a long (b) tail
//! means the command set is merely incomplete, which is the plan.
//!
//! IPTSCRAE has no reserved words, so "is this symbol an unimplemented command
//! or a variable?" has no definitive answer — an unknown symbol *is* a variable
//! until somebody registers it. The classifier therefore uses the convention the
//! corpus itself follows (commands are upper case, variables lower case) and
//! reports the names it saw so a reader can check the call. The spelling is
//! recovered from the **source**, because the lexer upper-cases symbols before
//! they reach the VM: [`SourceSpellings`] re-scans the text, skipping strings
//! and comments, and records how each symbol was actually written.

use std::collections::BTreeMap;

use iptscrae::registry::CommandSet;
use iptscrae::value::{Chunk, Op};
use iptscrae::IptError;

/// Which of the four milestone failure classes a fault belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailureClass {
    /// (a) A character outside our grammar that the reference tokenizer accepts.
    ///
    /// Should always be zero; it is a bug in the lexer if it is not.
    TokenizerGap,
    /// (b) The handler names an unregistered all-caps symbol — an unimplemented
    /// command extension.
    UnimplementedCommand,
    /// (c) The reference agrees with the VM, but the script needed host state or
    /// behaviour this milestone does not provide.
    SemanticsOrEnvironment,
    /// (d) Source the reference tokenizer rejects as well.
    MalformedSource,
}

impl FailureClass {
    /// A short label for the report.
    pub fn label(self) -> &'static str {
        match self {
            FailureClass::TokenizerGap => "(a) tokenizer gap",
            FailureClass::UnimplementedCommand => "(b) unimplemented command",
            FailureClass::SemanticsOrEnvironment => "(c) semantics / environment",
            FailureClass::MalformedSource => "(d) malformed source",
        }
    }
}

/// Every class, so a report can print zero rows rather than omitting them.
pub const ALL_CLASSES: [FailureClass; 4] = [
    FailureClass::TokenizerGap,
    FailureClass::UnimplementedCommand,
    FailureClass::SemanticsOrEnvironment,
    FailureClass::MalformedSource,
];

/// Classify a *parse* failure.
///
/// [`IptError::UnexpectedToken`] carries the offending character. If that
/// character is one the reference tokenizer accepts, our lexer has a gap (a);
/// if it is not, the script is malformed source (d) — the reference throws on
/// exactly the same character, so no implementation could have run it.
/// Structural faults (unbalanced delimiters, unterminated literals) are (d) for
/// the same reason.
pub fn classify_parse(error: &IptError) -> FailureClass {
    match error {
        IptError::UnexpectedToken { token, .. } => {
            let mut chars = token.chars();
            match chars.next() {
                Some(c) if chars.next().is_none() && reference_accepts(c) => {
                    FailureClass::TokenizerGap
                }
                _ => FailureClass::MalformedSource,
            }
        }
        _ => FailureClass::MalformedSource,
    }
}

/// Whether the reference tokenizer (`IptParser.tokenize`) accepts `c`.
///
/// This is the union of every branch it has: whitespace, the four delimiters,
/// the operator characters, and the symbol alphabet. A character outside it
/// makes the tokenizer throw `"Unexpected character"`, so a source containing it
/// was never a runnable script.
pub fn reference_accepts(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\r' | '\n'
            | '{' | '}' | '[' | ']'
            | '"' | '#' | ';'
            | '!' | '=' | '+' | '-' | '*' | '/' | '%' | '&' | '<' | '>'
            | '_'
            | '0'..='9'
            | 'a'..='z'
            | 'A'..='Z'
    )
}

/// Classify a *run* failure from the fault and the unregistered symbols the file
/// names.
///
/// The two causes leave different fingerprints:
///
/// * An **unimplemented command** is called with operands and never consumes
///   them, so the operands are still there when the next core command runs —
///   the fault is a *type* mismatch, not an empty stack.
/// * **Missing host state** (a global another script was supposed to set, say)
///   denies the script a value it expected. `EXEC` on the resulting `0` is a
///   no-op, so the fault is a *stack underflow*.
///
/// So (b) needs both signals: a symbol spelled like a command, and a fault other
/// than underflow. Everything else is (c).
pub fn classify_run(error: &IptError, unregistered: &[String]) -> FailureClass {
    let names_a_command = unregistered.iter().any(|n| is_command_spelling(n));
    let operands_were_present = !matches!(innermost(error), IptError::StackUnderflow { .. });
    if names_a_command && operands_were_present {
        FailureClass::UnimplementedCommand
    } else {
        FailureClass::SemanticsOrEnvironment
    }
}

/// Strip the `CommandFailed` wrappers the VM adds around a fault.
pub fn innermost(error: &IptError) -> &IptError {
    match error {
        IptError::CommandFailed { source, .. } => innermost(source),
        other => other,
    }
}

/// Whether a symbol is spelled the way the corpus spells commands.
pub fn is_command_spelling(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase()) && !name.chars().any(|c| c.is_ascii_lowercase())
}

/// How each symbol was written in the source, keyed by its upper-cased form.
///
/// Built by scanning the text directly, because [`Op::Var`] only carries the
/// upper-cased name. Strings and comments are skipped so a word inside a chat
/// message does not masquerade as a command.
#[derive(Debug, Clone, Default)]
pub struct SourceSpellings {
    spellings: BTreeMap<String, String>,
}

impl SourceSpellings {
    /// Scan `source`, recording the first spelling seen for each symbol.
    pub fn scan(source: &str) -> Self {
        let mut spellings = BTreeMap::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                chars.next();
                            }
                            '"' | '\0' => break,
                            _ => {}
                        }
                    }
                }
                '#' | ';' => {
                    while let Some(&c) = chars.peek() {
                        if c == '\r' || c == '\n' || c == '\0' {
                            break;
                        }
                        chars.next();
                    }
                }
                c if c.is_ascii_alphanumeric() || c == '_' => {
                    let mut original = String::from(c);
                    while let Some(&c) = chars.peek() {
                        if c.is_ascii_alphanumeric() || c == '_' {
                            original.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    spellings
                        .entry(original.to_ascii_uppercase())
                        .or_insert(original);
                }
                _ => {}
            }
        }
        Self { spellings }
    }

    /// How `upper` was written, falling back to `upper` itself.
    pub fn spelling_of<'a>(&'a self, upper: &'a str) -> &'a str {
        self.spellings
            .get(upper)
            .map(String::as_str)
            .unwrap_or(upper)
    }

    /// The unregistered symbols of `chunk`, spelled as the source spelled them.
    pub fn unknown_in(&self, chunk: &Chunk, commands: &CommandSet) -> Vec<String> {
        unregistered_names(chunk, commands)
            .iter()
            .map(|name| self.spelling_of(name).to_owned())
            .collect()
    }
}

/// Symbols a chunk references that the command set does not register.
///
/// Walks nested atomlists, de-duplicates, and keeps first-seen order so the
/// report names the symbol the script reached first.
pub fn unregistered_names(chunk: &Chunk, commands: &CommandSet) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_unregistered(chunk, commands, &mut out);
    out
}

fn collect_unregistered(chunk: &Chunk, commands: &CommandSet, out: &mut Vec<String>) {
    for op in chunk.ops() {
        match op {
            Op::Var(name) if !commands.contains(name) && !out.iter().any(|n| n == &**name) => {
                out.push(name.to_string());
            }
            Op::Chunk(inner) => collect_unregistered(inner, commands, out),
            _ => {}
        }
    }
}

/// The command a fault is attributed to, if the VM wrapped it.
pub fn faulting_command(error: &IptError) -> Option<&str> {
    match error {
        IptError::CommandFailed { command, .. } => Some(command),
        _ => None,
    }
}

/// Registered command names a source uses as the target of an assignment.
///
/// This is the `mousex` bug generalised. IPTSCRAE has no reserved words: the
/// lexer turns any registered name into a command, so a script that used that
/// name as a variable now gets an instruction in the variable slot `GLOBAL`,
/// `=`, `DEF`, `++`, `--` or a compound assignment needs. `GLOBALCommand` and
/// the assignment builtins read the raw reference from the stack, so a command
/// there faults (or silently corrupts the stack).
///
/// The scan mirrors [`SourceSpellings::scan`] — strings and comments are
/// skipped — and only reports a name when it is *spelled with a lower-case
/// letter* and its upper-cased form is registered. That spelling rule keeps the
/// legitimate idioms out: `value direction DUP GLOBAL` and
/// `{ a } { b } cond IFELSE =` are commands (upper case) producing the
/// reference, not variables.
///
/// A collision inside a string fed to `STRTOATOM` is out of scope: chat text
/// that merely contains a word like `DEF` would be a false positive.
#[must_use]
pub fn variable_position_collisions(source: &str, commands: &CommandSet) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut last_word: Option<String> = None;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' | '\0' => break,
                        _ => {}
                    }
                }
                last_word = None;
            }
            '#' | ';' => {
                while let Some(&c) = chars.peek() {
                    if c == '\r' || c == '\n' || c == '\0' {
                        break;
                    }
                    chars.next();
                }
                last_word = None;
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut word = String::from(c);
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        word.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let is_target_word =
                    word.eq_ignore_ascii_case("GLOBAL") || word.eq_ignore_ascii_case("DEF");
                if is_target_word {
                    record_collision(&mut out, last_word.as_deref(), commands);
                }
                last_word = Some(word);
            }
            c if c.is_ascii_whitespace() => {}
            c => {
                let op = read_operator(c, &mut chars);
                if is_assignment_target(&op) {
                    record_collision(&mut out, last_word.as_deref(), commands);
                }
                last_word = None;
            }
        }
    }
    out
}

fn record_collision(out: &mut Vec<String>, spelling: Option<&str>, commands: &CommandSet) {
    let Some(spelling) = spelling else {
        return;
    };
    if !spelling.chars().any(|c| c.is_ascii_lowercase()) {
        return;
    }
    let upper = spelling.to_ascii_uppercase();
    if commands.contains(&upper) && !out.iter().any(|name| name == &upper) {
        out.push(upper);
    }
}

/// Read the longest operator starting at `c`, consuming the lookahead it uses.
fn read_operator(c: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let next = chars.peek().copied();
    let is_pair = matches!(
        (c, next),
        ('=', Some('='))
            | ('!', Some('='))
            | ('<', Some('='))
            | ('>', Some('='))
            | ('+', Some('='))
            | ('+', Some('+'))
            | ('-', Some('='))
            | ('-', Some('-'))
            | ('*', Some('='))
            | ('/', Some('='))
            | ('%', Some('='))
            | ('<', Some('>'))
    );
    let mut op = String::from(c);
    if is_pair {
        chars.next();
        if let Some(ch) = next {
            op.push(ch);
        }
    }
    op
}

fn is_assignment_target(op: &str) -> bool {
    matches!(op, "=" | "++" | "--" | "+=" | "-=" | "*=" | "/=" | "%=")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iptscrae::lexer::parse_body;
    use iptscrae::Limits;

    fn chunk(source: &str) -> Chunk {
        parse_body(source, &CommandSet::core(), &Limits::default()).expect("parses")
    }

    #[test]
    fn a_foreign_character_is_malformed_source_not_a_tokenizer_gap() {
        for source in ["1 ( 2", "1 ) 2", "@", "$1", "1.5"] {
            let error = parse_body(source, &CommandSet::core(), &Limits::default()).unwrap_err();
            assert_eq!(
                classify_parse(&error),
                FailureClass::MalformedSource,
                "{source:?}"
            );
        }
    }

    #[test]
    fn structural_faults_are_malformed_source() {
        for source in ["\"open", "{ 1", "]", "1 }"] {
            let error = parse_body(source, &CommandSet::core(), &Limits::default()).unwrap_err();
            assert_eq!(
                classify_parse(&error),
                FailureClass::MalformedSource,
                "{source:?}"
            );
        }
    }

    #[test]
    fn the_reference_character_set_is_the_one_our_lexer_uses() {
        for c in "{}[]\"#;!=+-*/%&<>_0aA \t\r\n".chars() {
            assert!(reference_accepts(c), "{c:?} should be accepted");
        }
        for c in "()@$'.\\|,^~?:".chars() {
            assert!(!reference_accepts(c), "{c:?} should be rejected");
        }
    }

    #[test]
    fn unregistered_names_are_collected_from_nested_chunks() {
        let commands = CommandSet::core();
        let names = unregistered_names(&chunk("{ HTTPGET } myVar DUP"), &commands);
        assert!(names.contains(&"HTTPGET".to_owned()));
        assert!(
            names.contains(&"MYVAR".to_owned()),
            "upper-cased: {names:?}"
        );
        assert!(!names.contains(&"DUP".to_owned()));
    }

    #[test]
    fn source_spellings_recover_the_original_case() {
        let spellings = SourceSpellings::scan("prar GLOBAL HTTPGET myVar \"INSIDE A STRING\"");
        assert_eq!(spellings.spelling_of("PRAR"), "prar");
        assert_eq!(spellings.spelling_of("HTTPGET"), "HTTPGET");
        assert_eq!(spellings.spelling_of("MYVAR"), "myVar");
        assert_eq!(
            spellings.spelling_of("INSIDE"),
            "INSIDE",
            "identifiers inside strings are not seen"
        );
    }

    #[test]
    fn comments_are_not_scanned_for_spellings() {
        let spellings = SourceSpellings::scan("1 ; HTTPGET\n2 # GOTOURL");
        assert_eq!(spellings.spelling_of("HTTPGET"), "HTTPGET", "fallback");
        assert_eq!(spellings.spelling_of("GOTOURL"), "GOTOURL");
    }

    #[test]
    fn an_all_caps_unknown_name_with_a_present_operand_is_an_unimplemented_command() {
        let type_fault = IptError::TypeMismatch {
            expected: "atomlist",
            found: "string",
        };
        assert_eq!(
            classify_run(&type_fault, &["HTTPGET".to_owned()]),
            FailureClass::UnimplementedCommand
        );
    }

    #[test]
    fn lower_case_unknown_names_are_environment_not_missing_commands() {
        let type_fault = IptError::TypeMismatch {
            expected: "string",
            found: "number",
        };
        assert_eq!(
            classify_run(&type_fault, &["prar".to_owned(), "cname".to_owned()]),
            FailureClass::SemanticsOrEnvironment
        );
        assert_eq!(
            classify_run(&type_fault, &[]),
            FailureClass::SemanticsOrEnvironment
        );
    }

    #[test]
    fn a_missing_operand_points_at_absent_host_state_not_a_missing_command() {
        let underflow = IptError::StackUnderflow {
            needed: 1,
            available: 0,
        }
        .in_command("Concat");
        assert_eq!(
            classify_run(&underflow, &["HIDESMILEYS".to_owned()]),
            FailureClass::SemanticsOrEnvironment
        );
    }

    #[test]
    fn innermost_unwraps_command_attribution() {
        let error = IptError::StackUnderflow {
            needed: 1,
            available: 0,
        }
        .in_command("Concat");
        assert!(matches!(innermost(&error), IptError::StackUnderflow { .. }));
    }

    #[test]
    fn command_spelling_follows_the_corpus_convention() {
        assert!(is_command_spelling("HTTPGET"));
        assert!(is_command_spelling("SETSPOTSTATELOCAL2"));
        assert!(!is_command_spelling("prar"));
        assert!(!is_command_spelling("MYVar"));
        assert!(!is_command_spelling("_"));
    }

    #[test]
    fn faulting_command_is_unwrapped() {
        let error = IptError::TypeMismatch {
            expected: "string",
            found: "number",
        }
        .in_command("Concat");
        assert_eq!(faulting_command(&error), Some("Concat"));
        assert_eq!(faulting_command(&IptError::Host("x".to_owned())), None);
    }

    fn commands_with(names: &[&str]) -> CommandSet {
        let mut set = CommandSet::core();
        for name in names {
            set.register_host(name);
        }
        set
    }

    #[test]
    fn a_lower_case_spelling_of_a_registered_name_at_an_assignment_target_is_a_collision() {
        let commands = commands_with(&["STR", "MOUSEX"]);
        assert_eq!(
            variable_position_collisions("str GLOBAL 0 str =", &commands),
            vec!["STR"]
        );
        assert_eq!(
            variable_position_collisions("0 mousex GLOBAL =", &commands),
            vec!["MOUSEX"]
        );
        assert_eq!(
            variable_position_collisions("x += 1", &commands_with(&["X"])),
            vec!["X"]
        );
    }

    #[test]
    fn unregistered_lower_case_names_are_variables_not_collisions() {
        let commands = commands_with(&["STR"]);
        assert!(variable_position_collisions("walci GLOBAL 0 walci =", &commands).is_empty());
        assert!(variable_position_collisions("mousex GLOBAL", &commands).is_empty());
        assert!(
            variable_position_collisions("str GLOBAL", &CommandSet::core()).is_empty(),
            "with STR unregistered the name is a variable"
        );
    }

    #[test]
    fn the_legitimate_command_idioms_are_not_collisions() {
        let commands = commands_with(&["STR", "GET", "IFELSE"]);
        assert!(variable_position_collisions("value direction DUP GLOBAL =", &commands).is_empty());
        assert!(variable_position_collisions("[ a b ] i GET =", &commands).is_empty());
        assert!(variable_position_collisions("{ a } { b } c IFELSE =", &commands).is_empty());
    }

    #[test]
    fn comments_strings_and_comparisons_do_not_produce_collisions() {
        let commands = commands_with(&["STR", "DEF"]);
        assert!(variable_position_collisions("; str GLOBAL", &commands).is_empty());
        assert!(variable_position_collisions("\"str GLOBAL\" SAY", &commands).is_empty());
        assert!(variable_position_collisions("str == other", &commands).is_empty());
        assert!(variable_position_collisions("a str <> b", &commands).is_empty());
    }
}

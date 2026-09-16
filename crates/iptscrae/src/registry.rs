//! The command registry — which symbols are commands and which are variables.
//!
//! IPTSCRAE has no reserved words. When the tokenizer reads a symbol it asks
//! this registry: a registered name becomes a command; anything else becomes a
//! variable reference (and auto-vivifies to `0` on first use). That is why a
//! typo never produces "unknown command", only a silently-zero variable.
//!
//! Two layers register names, mirroring the reference implementation's split
//! between a generic engine and a host:
//!
//! 1. [`CommandSet::core`] — the ~72 commands every IPTSCRAE engine ships
//!    (stack, math, strings, control flow, arrays, comparison).
//! 2. [`CommandSet::register_host`] — names whose behaviour belongs to the host
//!    (every Palace command: `SAY`, `SETPOS`, `GOTOROOM`, …).
//!
//! Lookups are case-insensitive because the tokenizer upper-cases symbols
//! first; [`CommandSet::get_uppercase`] takes the pre-upper-cased name so the
//! hot path allocates nothing.

use std::collections::HashMap;
use std::rc::Rc;

/// A built-in, host-independent command.
///
/// Names are the ones the guide documents; several names share one variant
/// (`=`/`DEF`, `!`/`NOT`, `!=`/`<>`) because the reference binds them to the
/// same implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    // ---- stack ------------------------------------------------------------
    /// `DUP` — duplicate the top item.
    Dup,
    /// `POP` — discard the top item.
    Pop,
    /// `SWAP` — exchange the top two items.
    Swap,
    /// `OVER` — copy the second item to the top.
    Over,
    /// `PICK` — copy the `n`-th item to the top.
    Pick,
    /// `STACKDEPTH` — push the current depth.
    StackDepth,
    /// `TOPTYPE` — push the type code of the top item.
    TopType,
    /// `VARTYPE` — like `TOPTYPE`, but dereference a variable first.
    VarType,

    // ---- arithmetic -------------------------------------------------------
    /// `+` — add or (for two strings) concatenate.
    Add,
    /// `-` — subtract.
    Sub,
    /// `*` — multiply.
    Mul,
    /// `/` — integer divide; division by zero yields 0.
    Div,
    /// `%` — integer remainder.
    Mod,
    /// `++` — increment a variable.
    Inc,
    /// `--` — decrement a variable.
    Dec,
    /// `+=` — add to a variable (or append to a string variable).
    AddAssign,
    /// `-=` — subtract from a variable.
    SubAssign,
    /// `*=` — multiply a variable.
    MulAssign,
    /// `/=` — divide a variable.
    DivAssign,
    /// `%=` — take a variable modulo.
    ModAssign,
    /// `RANDOM` — random integer in `0..n`; host-provided.
    Random,
    /// `SINE` — sine of degrees, scaled by 1000.
    Sine,
    /// `COSINE` — cosine of degrees, scaled by 1000.
    Cosine,
    /// `TANGENT` — tangent of degrees, scaled by 1000.
    Tangent,
    /// `ATOI` — string to integer.
    Atoi,
    /// `ITOA` — integer to string.
    Itoa,
    /// `DATETIME` — host clock, seconds since the epoch.
    DateTime,
    /// `TICKS` — host clock, 1/60 s ticks.
    Ticks,
    /// `IPTVERSION` — the language version.
    IptVersion,

    // ---- strings ----------------------------------------------------------
    /// `&` — strict string concatenation.
    Concat,
    /// `&=` — append to a string variable.
    ConcatAssign,
    /// `SUBSTR` — case-insensitive substring *test*, pushes 0/1.
    Substr,
    /// `SUBSTRING` — extract a substring.
    Substring,
    /// `STRINDEX` — offset of a needle in a haystack.
    StrIndex,
    /// `STRLEN` — string length.
    StrLen,
    /// `LOWERCASE` — lower-case a string.
    Lowercase,
    /// `UPPERCASE` — upper-case a string.
    Uppercase,
    /// `STRTOATOM` — compile a string into an executable atomlist.
    StrToAtom,
    /// `GREPSTR` — regex search; host-provided.
    GrepStr,
    /// `GREPSUB` — substitute the captures from the last `GREPSTR`.
    GrepSub,

    // ---- logic and comparison --------------------------------------------
    /// `AND` — logical conjunction.
    And,
    /// `OR` — logical disjunction.
    Or,
    /// `NOT` / `!` — logical negation.
    Not,
    /// `==` — equality; strings compare case-insensitively.
    Eq,
    /// `!=` / `<>` — inequality; strings compare case-sensitively.
    Ne,
    /// `<` — less than.
    Lt,
    /// `<=` — less than or equal.
    Le,
    /// `>` — greater than.
    Gt,
    /// `>=` — greater than or equal.
    Ge,

    // ---- control flow -----------------------------------------------------
    /// `IF` — `{body} cond IF`.
    If,
    /// `IFELSE` — `{true} {false} cond IFELSE`.
    IfElse,
    /// `WHILE` — `{body} {cond} WHILE`.
    While,
    /// `FOREACH` — `{body} [array] FOREACH`.
    ForEach,
    /// `EXEC` — run an atomlist; integer 0 is a silent no-op.
    Exec,
    /// `RETURN` — leave the current atomlist.
    Return,
    /// `BREAK` — leave the enclosing loop.
    Break,
    /// `EXIT` — stop the whole script.
    Exit,
    /// `ALARMEXEC` — schedule an atomlist; host-provided.
    AlarmExec,

    // ---- variables and arrays --------------------------------------------
    /// `=` / `DEF` — assign `value` to `variable`.
    Assign,
    /// `GLOBAL` — promote a variable to the shared store.
    Global,
    /// `ARRAY` — allocate an array of `n` zeros.
    Array,
    /// `GET` — read an array element.
    Get,
    /// `PUT` — write an array element.
    Put,
    /// `LENGTH` — number of elements in an array.
    Length,

    // ---- misc -------------------------------------------------------------
    /// `_TRACE` — write a string to the host trace.
    Trace,
    /// `TRACESTACK` — dump and clear the stack.
    TraceStack,
    /// `_BREAKPOINT` — host debugger hook (a no-op here).
    Breakpoint,
    /// `DELAY` — consumes ticks (a no-op here; scheduling is the host's job).
    Delay,
    /// `BEEP` — a no-op, as in the reference.
    Beep,
}

/// What a registered name maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// A host-independent implementation in this crate.
    Builtin(Builtin),
    /// A command the host must service (all Palace commands).
    Host,
}

/// The `name -> kind` table consulted while lexing.
///
/// Cloneable so a caller can keep a pristine set and hand tuned variants to
/// different engines.
#[derive(Debug, Clone, Default)]
pub struct CommandSet {
    map: HashMap<Rc<str>, CommandKind>,
}

impl CommandSet {
    /// An empty registry: every symbol will lex as a variable.
    pub fn new() -> Self {
        Self::default()
    }

    /// The core command set every IPTSCRAE engine provides.
    pub fn core() -> Self {
        let mut set = Self::new();
        for (name, builtin) in CORE_COMMANDS {
            set.map
                .insert(Rc::from(*name), CommandKind::Builtin(*builtin));
        }
        set
    }

    /// Register a host command, overriding any existing binding.
    pub fn register_host(&mut self, name: &str) {
        self.map
            .insert(Rc::from(name.to_ascii_uppercase()), CommandKind::Host);
    }

    /// Register a host command only if the name is not already taken.
    ///
    /// Used when layering a host dictionary (Palace) on top of [`Self::core`]:
    /// names like `ALARMEXEC` and `GREPSTR` exist in both layers and the core
    /// implementation must win.
    pub fn register_host_if_absent(&mut self, name: &str) -> bool {
        let key = name.to_ascii_uppercase();
        if self.map.contains_key(key.as_str()) {
            false
        } else {
            self.map.insert(Rc::from(key), CommandKind::Host);
            true
        }
    }

    /// Register a builtin under an explicit name.
    pub fn register_builtin(&mut self, name: &str, builtin: Builtin) {
        self.map.insert(
            Rc::from(name.to_ascii_uppercase()),
            CommandKind::Builtin(builtin),
        );
    }

    /// Look up an already upper-cased name.
    pub fn get_uppercase(&self, name: &str) -> Option<CommandKind> {
        self.map.get(name).copied()
    }

    /// Look up a name, upper-casing it first.
    pub fn get(&self, name: &str) -> Option<CommandKind> {
        self.map.get(name.to_ascii_uppercase().as_str()).copied()
    }

    /// Whether a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Number of registered names.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Registered names, in unspecified order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(|k| k.as_ref())
    }
}

/// The 72 commands in the reference engine's default dictionary, with the
/// spellings the guide documents.
pub const CORE_COMMANDS: &[(&str, Builtin)] = &[
    ("DUP", Builtin::Dup),
    ("POP", Builtin::Pop),
    ("SWAP", Builtin::Swap),
    ("OVER", Builtin::Over),
    ("PICK", Builtin::Pick),
    ("STACKDEPTH", Builtin::StackDepth),
    ("TOPTYPE", Builtin::TopType),
    ("VARTYPE", Builtin::VarType),
    ("+", Builtin::Add),
    ("-", Builtin::Sub),
    ("*", Builtin::Mul),
    ("/", Builtin::Div),
    ("%", Builtin::Mod),
    ("++", Builtin::Inc),
    ("--", Builtin::Dec),
    ("+=", Builtin::AddAssign),
    ("-=", Builtin::SubAssign),
    ("*=", Builtin::MulAssign),
    ("/=", Builtin::DivAssign),
    ("%=", Builtin::ModAssign),
    ("RANDOM", Builtin::Random),
    ("SINE", Builtin::Sine),
    ("COSINE", Builtin::Cosine),
    ("TANGENT", Builtin::Tangent),
    ("ATOI", Builtin::Atoi),
    ("ITOA", Builtin::Itoa),
    ("DATETIME", Builtin::DateTime),
    ("TICKS", Builtin::Ticks),
    ("IPTVERSION", Builtin::IptVersion),
    ("&", Builtin::Concat),
    ("&=", Builtin::ConcatAssign),
    ("SUBSTR", Builtin::Substr),
    ("SUBSTRING", Builtin::Substring),
    ("STRINDEX", Builtin::StrIndex),
    ("STRLEN", Builtin::StrLen),
    ("LOWERCASE", Builtin::Lowercase),
    ("UPPERCASE", Builtin::Uppercase),
    ("STRTOATOM", Builtin::StrToAtom),
    ("GREPSTR", Builtin::GrepStr),
    ("GREPSUB", Builtin::GrepSub),
    ("AND", Builtin::And),
    ("OR", Builtin::Or),
    ("NOT", Builtin::Not),
    ("!", Builtin::Not),
    ("==", Builtin::Eq),
    ("!=", Builtin::Ne),
    ("<>", Builtin::Ne),
    ("<", Builtin::Lt),
    ("<=", Builtin::Le),
    (">", Builtin::Gt),
    (">=", Builtin::Ge),
    ("IF", Builtin::If),
    ("IFELSE", Builtin::IfElse),
    ("WHILE", Builtin::While),
    ("FOREACH", Builtin::ForEach),
    ("EXEC", Builtin::Exec),
    ("RETURN", Builtin::Return),
    ("BREAK", Builtin::Break),
    ("EXIT", Builtin::Exit),
    ("ALARMEXEC", Builtin::AlarmExec),
    ("=", Builtin::Assign),
    ("DEF", Builtin::Assign),
    ("GLOBAL", Builtin::Global),
    ("ARRAY", Builtin::Array),
    ("GET", Builtin::Get),
    ("PUT", Builtin::Put),
    ("LENGTH", Builtin::Length),
    ("_TRACE", Builtin::Trace),
    ("TRACESTACK", Builtin::TraceStack),
    ("_BREAKPOINT", Builtin::Breakpoint),
    ("DELAY", Builtin::Delay),
    ("BEEP", Builtin::Beep),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_has_the_seventy_two_documented_names() {
        assert_eq!(CORE_COMMANDS.len(), 72);
        assert_eq!(CommandSet::core().len(), 72, "no duplicate spellings");
    }

    #[test]
    fn lookup_is_case_insensitive_and_symbols_shadow_variables() {
        let set = CommandSet::core();
        assert_eq!(set.get("dup"), Some(CommandKind::Builtin(Builtin::Dup)));
        assert_eq!(set.get("SAY"), None, "SAY is a host command");
        assert!(set.contains("while"));
        assert!(!set.contains("notacommand"));
    }

    #[test]
    fn host_registration_does_not_clobber_core() {
        let mut set = CommandSet::core();
        set.register_host("SAY");
        assert_eq!(set.get("SAY"), Some(CommandKind::Host));
        assert!(
            !set.register_host_if_absent("DUP"),
            "core DUP already present"
        );
        assert!(!set.register_host_if_absent("GREPSTR"), "GREPSTR is core");
        assert_eq!(set.get("DUP"), Some(CommandKind::Builtin(Builtin::Dup)));
        assert_eq!(
            set.get("GREPSTR"),
            Some(CommandKind::Builtin(Builtin::GrepStr))
        );
    }

    #[test]
    fn aliases_share_one_implementation() {
        let set = CommandSet::core();
        assert_eq!(set.get("="), set.get("DEF"));
        assert_eq!(set.get("!"), set.get("NOT"));
        assert_eq!(set.get("!="), set.get("<>"));
    }
}

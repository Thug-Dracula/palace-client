//! Everything that can go wrong lexing, parsing or running an IPTSCRAE script.
//!
//! The library never panics on script-derived data. A malformed script is an
//! [`Err`], and the VM surfaces runtime faults through the same enum so a caller
//! can decide whether a broken handler aborts a whole client or just that event.
//!
//! Error positions are **byte offsets into the source**, so a caller can point a
//! user at the offending character.

use std::fmt;

/// Longest symbol the reference clients accept.
///
/// The guide's "Code Limitations" appendix says symbols "have a maximum length of
/// 31 characters". We accept longer symbols at lex time (so a malformed script is
/// not rejected outright) but the guide is the documented bound; see the README
/// for why this is not enforced.
pub const MAX_SYMBOL_LEN: usize = 31;

/// A recoverable failure while lexing, parsing or executing a script.
///
/// `offset` fields are byte offsets into the UTF-8 (actually Windows-1252-decoded)
/// source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IptError {
    // ---------------------------------------------------------------- lexing
    /// A string literal opened with `"` and never closed.
    UnterminatedString { offset: usize },
    /// An atomlist opened with `{` and never closed.
    UnterminatedAtomList { offset: usize },
    /// A `}` or `]` appeared with no matching opener.
    UnmatchedClose { offset: usize, close: char },
    /// A character appeared that the grammar does not recognise at that point.
    ///
    /// The reference tokenizer throws on any character outside its accepted set,
    /// so a corrupted script lands here rather than being silently skipped.
    UnexpectedToken { offset: usize, token: String },

    // --------------------------------------------------------------- parsing
    /// An `ON NAME` block was not followed by a `{ ... }` atomlist.
    HandlerBodyMissing { offset: usize },
    /// A token stream ended inside an `ON` header.
    UnexpectedEof { offset: usize },

    // --------------------------------------------------------------- runtime
    /// An operator needed more operands than the stack held.
    StackUnderflow { needed: usize, available: usize },
    /// Pushing would exceed the configured stack depth.
    StackOverflow { limit: usize },
    /// Atomlist/`EXEC` nesting exceeded the configured recursion limit.
    NestingTooDeep { limit: usize },
    /// The instruction budget for one activation was exhausted.
    StepBudgetExhausted { limit: u64 },
    /// A `WHILE` loop ran more iterations than allowed.
    WhileLimitExceeded { limit: u64 },
    /// An array was requested with more elements than allowed.
    ArrayTooLarge { requested: usize, limit: usize },
    /// A string grew past the configured cap.
    StringTooLong { len: usize, limit: usize },
    /// The per-activation variable budget was exhausted.
    TooManyVariables { limit: usize },
    /// A value of the wrong kind reached an operator that needs another kind.
    TypeMismatch {
        expected: &'static str,
        found: &'static str,
    },
    /// An argument was outside its permitted domain.
    BadArgument(&'static str),
    /// An array index was outside `0..len`.
    IndexOutOfRange { index: i64, len: usize },
    /// The VM encountered a registered command whose implementation is the
    /// host's responsibility and the host refused it.
    CommandUnavailable { command: String },
    /// A command registered by a host layer reported failure.
    Host(String),
    /// A builtin or host command failed; carries which one.
    CommandFailed {
        /// The command name.
        command: String,
        /// The underlying fault.
        source: Box<IptError>,
    },
}

impl IptError {
    /// Attach the name of the command that was executing.
    ///
    /// Nested attribution is collapsed to the innermost command, so a fault in
    /// `GET` reads "GET: array index 5 out of range" rather than repeating the
    /// whole call chain.
    pub fn in_command(self, command: &str) -> Self {
        match self {
            IptError::CommandFailed { command, source } => {
                IptError::CommandFailed { command, source }
            }
            other => IptError::CommandFailed {
                command: command.to_owned(),
                source: Box::new(other),
            },
        }
    }

    /// A short, stable machine-readable category used by the corpus reporter.
    pub fn category(&self) -> &'static str {
        match self {
            IptError::UnterminatedString { .. }
            | IptError::UnterminatedAtomList { .. }
            | IptError::UnmatchedClose { .. }
            | IptError::UnexpectedToken { .. } => "lex",
            IptError::HandlerBodyMissing { .. } | IptError::UnexpectedEof { .. } => "parse",
            IptError::StackUnderflow { .. } | IptError::StackOverflow { .. } => "stack",
            IptError::NestingTooDeep { .. }
            | IptError::StepBudgetExhausted { .. }
            | IptError::WhileLimitExceeded { .. } => "budget",
            IptError::ArrayTooLarge { .. }
            | IptError::StringTooLong { .. }
            | IptError::TooManyVariables { .. } => "limit",
            IptError::TypeMismatch { .. }
            | IptError::BadArgument(_)
            | IptError::IndexOutOfRange { .. } => "type",
            IptError::CommandUnavailable { .. } | IptError::Host(_) => "host",
            IptError::CommandFailed { source, .. } => source.category(),
        }
    }
}

impl fmt::Display for IptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use IptError::*;
        match self {
            UnterminatedString { offset } => write!(f, "unterminated string at byte {offset}"),
            UnterminatedAtomList { offset } => {
                write!(f, "unterminated atomlist '{{' at byte {offset}")
            }
            UnmatchedClose { offset, close } => {
                write!(f, "unmatched '{close}' at byte {offset}")
            }
            UnexpectedToken { offset, token } => {
                write!(f, "unexpected {token:?} at byte {offset}")
            }
            HandlerBodyMissing { offset } => {
                write!(f, "ON handler at byte {offset} has no '{{ ... }}' body")
            }
            UnexpectedEof { offset } => write!(f, "unexpected end of script at byte {offset}"),
            StackUnderflow { needed, available } => write!(
                f,
                "stack underflow: needed {needed} item(s), {available} available"
            ),
            StackOverflow { limit } => write!(f, "stack overflow: depth limit is {limit}"),
            NestingTooDeep { limit } => {
                write!(f, "atomlist nesting too deep: limit is {limit}")
            }
            StepBudgetExhausted { limit } => {
                write!(f, "step budget exhausted after {limit} instructions")
            }
            WhileLimitExceeded { limit } => {
                write!(f, "WHILE loop exceeded {limit} iterations")
            }
            ArrayTooLarge { requested, limit } => write!(
                f,
                "array of {requested} element(s) exceeds the limit of {limit}"
            ),
            StringTooLong { len, limit } => {
                write!(f, "string of {len} byte(s) exceeds the limit of {limit}")
            }
            TooManyVariables { limit } => write!(f, "too many variables: limit is {limit}"),
            TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            BadArgument(message) => write!(f, "{message}"),
            IndexOutOfRange { index, len } => {
                write!(f, "array index {index} out of range (length {len})")
            }
            CommandUnavailable { command } => {
                write!(f, "command {command} is not available in this host")
            }
            Host(msg) => write!(f, "host command failed: {msg}"),
            CommandFailed { command, source } => write!(f, "{command}: {source}"),
        }
    }
}

impl std::error::Error for IptError {}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, IptError>;

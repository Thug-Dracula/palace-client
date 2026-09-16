//! Resource budgets — the reason a hostile script cannot hang or OOM the client.
//!
//! Scripts are **untrusted remote code**: a room's ON ENTER handler arrives from
//! the server and runs inside the client. Every loop, every recursion and every
//! allocation therefore runs under a limit, and exceeding a limit is an
//! [`IptError`](crate::IptError), never a hang.
//!
//! ## Where the numbers come from
//!
//! The guide's "Code Limitations" appendix gives the original Windows/Mac client
//! limits: 256 stack items, 64 nested atomlists, 256 array elements, 16 KiB
//! GREPSUB results. Later clients raised the stack limit, so the same script can
//! run or overflow depending on who hosts it:
//!
//! | Dialect | Stack items | Source |
//! |---|---|---|
//! | Windows / Mac client | 256 | *Iptscrae Language Guide*, appendix B |
//! | PalaceChat | 1024 | [iptService.js] `MAX_STACK` |
//! | OpenPalace | 2048 | `IptConstants.STACK_DEPTH` |
//!
//! [iptService.js]: https://github.com/OpenPalace
//!
//! [`Limits::default`] uses the **PalaceChat** stack depth (1024) — the smallest
//! common denominator among the interpreters that ran the corpus this crate is
//! tested against — and the iptService budgets for loops and nesting. Use
//! [`Limits::windows`], [`Limits::palacechat`] or [`Limits::openpalace`] to pin a
//! specific dialect; every field is public so a host can mix and match.

/// A named stack-limit dialect from the reference clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackDialect {
    /// Original Windows/Mac client: 256 stack items.
    Windows,
    /// PalaceChat / iptService.js: 1024 stack items.
    PalaceChat,
    /// OpenPalace AS3 VM: 2048 stack items.
    OpenPalace,
}

/// Everything a script can exhaust.
///
/// All fields are inclusive maxima: a stack of exactly `stack_depth` items is
/// legal, `stack_depth + 1` is [`IptError::StackOverflow`](crate::IptError).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum live items on the data stack.
    pub stack_depth: usize,
    /// Maximum atomlist/`EXEC` nesting (the reference's `RECURSION_LIMIT`).
    pub nesting: usize,
    /// Hard cap on instructions executed by one activation of one handler.
    pub steps: u64,
    /// Instructions a cooperative host runs before [`Vm::step_slice`] returns.
    ///
    /// Purely a scheduling knob; it does not change semantics.
    ///
    /// [`Vm::step_slice`]: crate::Vm::step_slice
    pub steps_per_slice: u64,
    /// Maximum iterations of a single `WHILE`.
    pub while_iterations: u64,
    /// Maximum elements in one array.
    pub array_elements: usize,
    /// Maximum bytes in one string value.
    pub string_bytes: usize,
    /// Maximum distinct variables auto-vivified by one activation.
    pub variables: usize,
}

impl StackDialect {
    /// The stack depth this dialect allows.
    pub const fn stack_depth(self) -> usize {
        match self {
            StackDialect::Windows => 256,
            StackDialect::PalaceChat => 1024,
            StackDialect::OpenPalace => 2048,
        }
    }
}

impl Limits {
    /// The original Windows/Mac client limits (guide appendix B).
    pub const fn windows() -> Self {
        Self {
            stack_depth: StackDialect::Windows.stack_depth(),
            nesting: 64,
            ..Self::base()
        }
    }

    /// PalaceChat-era limits: 1024 stack items, 7500 `WHILE` iterations.
    pub const fn palacechat() -> Self {
        Self {
            stack_depth: StackDialect::PalaceChat.stack_depth(),
            ..Self::base()
        }
    }

    /// OpenPalace's VM limits: 2048 stack items, 256 recursion.
    pub const fn openpalace() -> Self {
        Self {
            stack_depth: StackDialect::OpenPalace.stack_depth(),
            nesting: 256,
            ..Self::base()
        }
    }

    /// Fields shared by every dialect. `stack_depth` and `nesting` callers
    /// override these.
    const fn base() -> Self {
        Self {
            stack_depth: StackDialect::PalaceChat.stack_depth(),
            nesting: 256,
            // A hostile `{ 1 } { 1 } WHILE` is caught by `while_iterations`;
            // this catches the sneaky version that hides its loop in an
            // `EXEC`ed atomlist. 1e6 instructions is a few milliseconds.
            steps: 1_000_000,
            steps_per_slice: 10_000,
            while_iterations: 7500,
            array_elements: 256,
            string_bytes: 65_536,
            variables: 4096,
        }
    }

    /// Override the stack depth with a named dialect, keeping everything else.
    pub const fn with_dialect(mut self, dialect: StackDialect) -> Self {
        self.stack_depth = dialect.stack_depth();
        self
    }
}

impl Default for Limits {
    /// PalaceChat-era budgets — see the module docs for why.
    fn default() -> Self {
        Self::palacechat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialects_match_the_reference_clients() {
        assert_eq!(StackDialect::Windows.stack_depth(), 256);
        assert_eq!(StackDialect::PalaceChat.stack_depth(), 1024);
        assert_eq!(StackDialect::OpenPalace.stack_depth(), 2048);
    }

    #[test]
    fn presets_agree_with_their_dialect() {
        assert_eq!(Limits::windows().stack_depth, 256);
        assert_eq!(Limits::windows().nesting, 64);
        assert_eq!(Limits::palacechat().stack_depth, 1024);
        assert_eq!(Limits::openpalace().stack_depth, 2048);
        assert_eq!(Limits::openpalace().nesting, 256);
        // Default is the PalaceChat dialect.
        assert_eq!(Limits::default().stack_depth, 1024);
    }

    #[test]
    fn with_dialect_only_changes_the_stack_depth() {
        let base = Limits::default();
        let swapped = base.with_dialect(StackDialect::OpenPalace);
        assert_eq!(swapped.stack_depth, 2048);
        assert_eq!(swapped.while_iterations, base.while_iterations);
        assert_eq!(swapped.nesting, base.nesting);
    }
}

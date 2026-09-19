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
//! | Dialect | Stack items | Nesting | `WHILE` iterations | Source |
//! |---|---|---|---|---|
//! | Windows / Mac client | 256 | 64 | 7500 | *Iptscrae Language Guide*, appendix B |
//! | PalaceChat | 1024 | 256 | 7500 | [iptService.js] `MAX_STACK`, `gWhileMaxIteration` |
//! | OpenPalace | 2048 | 256 | unbounded | `IptConstants.STACK_DEPTH`, `RECURSION_LIMIT` |
//!
//! [iptService.js]: https://github.com/OpenPalace
//!
//! [`Limits::default`] uses the **PalaceChat** stack depth (1024) — the smallest
//! common denominator among the interpreters that ran the corpus this crate is
//! tested against — and the iptService budgets for loops and nesting. Use
//! [`Limits::windows`], [`Limits::palacechat`] or [`Limits::openpalace`] to pin a
//! specific dialect; every field is public so a host can mix and match.
//!
//! ## Deliberate divergences from OpenPalace
//!
//! OpenPalace's AS3 VM has no `WHILE` cap: a loop runs until its condition is
//! false, and only the executor's step slicing keeps the UI responsive. Our
//! `while_iterations` cap is hardening inherited from PalaceChat
//! (`iptService.js:14`, `gWhileMaxIteration = 7500`), so it is absent from
//! [`Limits::openpalace`] (`while_iterations = u64::MAX`, i.e. unbounded). A
//! runaway loop is still stopped by [`Limits::steps`], which is also not a
//! reference limit — the AS3 VM is unbounded per activation — but is kept so an
//! untrusted script can never hang the client.

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
    ///
    /// The guide caps variables at 64 *per handler*, but the corpus has handlers
    /// that name up to 164 distinct non-command symbols (`5012_hs1.txt`), and the
    /// guide separately calls the global store unbounded — so the two reconcile
    /// as "64 locals plus unbounded globals", which this single counter cannot
    /// express. The default is therefore the corpus-compatible bound rather than
    /// the guide's per-handler number; see the README.
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

    /// The atomlist/`EXEC` nesting this dialect allows.
    pub const fn nesting(self) -> usize {
        match self {
            StackDialect::Windows => 64,
            StackDialect::PalaceChat | StackDialect::OpenPalace => 256,
        }
    }

    /// The `WHILE` iteration cap this dialect allows.
    ///
    /// `u64::MAX` means the reference's unbounded loop; PalaceChat and the
    /// original Windows client cap at 7500.
    pub const fn while_iterations(self) -> u64 {
        match self {
            StackDialect::Windows | StackDialect::PalaceChat => 7_500,
            StackDialect::OpenPalace => u64::MAX,
        }
    }
}

impl Limits {
    /// The original Windows/Mac client limits (guide appendix B).
    pub const fn windows() -> Self {
        Self {
            stack_depth: StackDialect::Windows.stack_depth(),
            nesting: StackDialect::Windows.nesting(),
            while_iterations: StackDialect::Windows.while_iterations(),
            ..Self::base()
        }
    }

    /// PalaceChat-era limits: 1024 stack items, 7500 `WHILE` iterations.
    pub const fn palacechat() -> Self {
        Self {
            stack_depth: StackDialect::PalaceChat.stack_depth(),
            nesting: StackDialect::PalaceChat.nesting(),
            while_iterations: StackDialect::PalaceChat.while_iterations(),
            ..Self::base()
        }
    }

    /// OpenPalace's VM limits: 2048 stack items, 256 recursion, no `WHILE` cap.
    pub const fn openpalace() -> Self {
        Self {
            stack_depth: StackDialect::OpenPalace.stack_depth(),
            nesting: StackDialect::OpenPalace.nesting(),
            while_iterations: StackDialect::OpenPalace.while_iterations(),
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

    /// Select a named dialect, applying all of its limits (stack, nesting and
    /// `WHILE` cap). This is how a host that only knows the dialect name — for
    /// example the `--dialect` flag — reaches [`Self::openpalace`].
    pub const fn with_dialect(mut self, dialect: StackDialect) -> Self {
        self.stack_depth = dialect.stack_depth();
        self.nesting = dialect.nesting();
        self.while_iterations = dialect.while_iterations();
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
        assert_eq!(StackDialect::Windows.nesting(), 64);
        assert_eq!(StackDialect::PalaceChat.nesting(), 256);
        assert_eq!(StackDialect::OpenPalace.nesting(), 256);
        assert_eq!(StackDialect::PalaceChat.while_iterations(), 7_500);
        assert_eq!(StackDialect::OpenPalace.while_iterations(), u64::MAX);
    }

    #[test]
    fn presets_agree_with_their_dialect() {
        assert_eq!(Limits::windows().stack_depth, 256);
        assert_eq!(Limits::windows().nesting, 64);
        assert_eq!(Limits::palacechat().stack_depth, 1024);
        assert_eq!(Limits::palacechat().while_iterations, 7_500);
        assert_eq!(Limits::openpalace().stack_depth, 2048);
        assert_eq!(Limits::openpalace().nesting, 256);
        assert_eq!(
            Limits::openpalace().while_iterations,
            u64::MAX,
            "the reference AS3 VM has no WHILE iteration cap"
        );
        // Default is the PalaceChat dialect.
        assert_eq!(Limits::default().stack_depth, 1024);
    }

    #[test]
    fn with_dialect_selects_the_whole_preset() {
        assert_eq!(
            Limits::default().with_dialect(StackDialect::OpenPalace),
            Limits::openpalace()
        );
        assert_eq!(
            Limits::default().with_dialect(StackDialect::Windows),
            Limits::windows()
        );
        assert_eq!(
            Limits::default().with_dialect(StackDialect::PalaceChat),
            Limits::palacechat()
        );
    }
}

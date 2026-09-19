//! A small regular-expression engine for `GREPSTR`.
//!
//! The VM does not regex: `GREPSTR` delegates to [`Host::grep_match`], because
//! compiling a pattern that arrives from the network is a policy decision the
//! host should make. This module is the engine the bundled test host and the
//! corpus harness use, so the crate ships a working implementation without
//! taking a dependency.
//!
//! [`find`] returns the whole match followed by every capture group, so
//! `GREPSUB` can substitute `$0`…`$n`. A group that did not take part in the
//! match — a skipped optional group — is the empty string, matching the
//! reference's `match()`/`exec()` result array.
//!
//! The dialect is the ECMAScript `RegExp` subset the reference clients rely on:
//! `^ $ .`, classes with ranges and negation, the shorthands `\d \D \w \W \s
//! \S`, the boundaries `\b \B`, `* + ?`, `{m} {m,n} {m,}`, groups, alternation
//! and `\xHH`. A `(?i)` / `(?-i)` group switches case-insensitive matching on
//! and off from that point; the reference `GREPSTR` is case-sensitive and
//! exposes no flag, so this is the crate's spelling for the case control the
//! language otherwise reaches with `LOWERCASE`.
//!
//! The reference implementation also rewrites a leading `^^` to `^\^` (a
//! literal caret at the start of the line), which real corpus scripts depend on;
//! [`find`] applies the same rewrite before parsing.
//!
//! Matching is greedy, leftmost-first and **step-bounded**: a pathological
//! pattern gives up and reports "no match" instead of running away, which
//! matters because patterns are untrusted.

use crate::value::cp1252_char;

/// Backstop on the backtracking stack, so a hostile pattern cannot allocate
/// without bound even before the step budget is spent.
const BACKTRACK_LIMIT: usize = 100_000;

/// Largest `{m,n}` bound the engine will compile; anything larger is treated as
/// literal braces rather than expanded.
const REPEAT_LIMIT: usize = 1024;

/// Match `pattern` against `text`, returning the whole match first and the
/// capture groups after it.
///
/// `Some(captures)` means the pattern matched: `captures[0]` is the whole match
/// and `captures[i]` is group `i` (the empty string when that group did not
/// participate). `None` means there was no match, or the step budget was
/// exhausted.
pub fn find(pattern: &str, text: &str) -> Option<Vec<String>> {
    find_with_limit(pattern, text, 1_000_000)
}

/// [`find`] with an explicit step budget.
pub fn find_with_limit(pattern: &str, text: &str, limit: u32) -> Option<Vec<String>> {
    // Reference fix (GREPSTRCommand.as): legacy scripts write `^^` to mean a
    // literal caret at the start; both OpenPalace and Sparky rewrite it.
    let rewritten;
    let pattern = match pattern.strip_prefix("^^") {
        Some(rest) => {
            rewritten = format!("^\\^{rest}");
            rewritten.as_str()
        }
        None => pattern,
    };

    let chars: Vec<char> = pattern.chars().collect();
    let mut parser = Parser::new(&chars);
    let nodes = parser.parse();
    let anchored = matches!(nodes.first(), Some(Node::Start));
    let program = compile(&nodes, parser.groups, !anchored);

    let text: Vec<char> = text.chars().collect();
    let mut steps = limit;
    let captures = exec(&program, &text, &mut steps)?;
    Some(capture_strings(&captures, program.groups, &text))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shorthand {
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

const SPACES: &[char] = &[
    ' ', '\t', '\n', '\r', '\u{0b}', '\u{0c}', '\u{00a0}', '\u{1680}', '\u{2000}', '\u{2001}',
    '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}',
    '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}', '\u{feff}',
];

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn shorthand_matches(shorthand: Shorthand, c: char) -> bool {
    let raw = match shorthand {
        Shorthand::Digit | Shorthand::NotDigit => c.is_ascii_digit(),
        Shorthand::Word | Shorthand::NotWord => is_word(c),
        Shorthand::Space | Shorthand::NotSpace => SPACES.contains(&c),
    };
    match shorthand {
        Shorthand::NotDigit | Shorthand::NotWord | Shorthand::NotSpace => !raw,
        Shorthand::Digit | Shorthand::Word | Shorthand::Space => raw,
    }
}

fn in_range(c: char, lo: char, hi: char, ignore_case: bool) -> bool {
    let contains = |ch: char| lo <= ch && ch <= hi;
    contains(c)
        || (ignore_case && (contains(c.to_ascii_lowercase()) || contains(c.to_ascii_uppercase())))
}

fn char_eq(a: char, b: char, ignore_case: bool) -> bool {
    if ignore_case {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

fn is_word_boundary(text: &[char], pos: usize) -> bool {
    let before = pos
        .checked_sub(1)
        .and_then(|i| text.get(i))
        .is_some_and(|c| is_word(*c));
    let after = text.get(pos).is_some_and(|c| is_word(*c));
    before != after
}

#[derive(Debug, Clone)]
enum ClassItem {
    Range(char, char),
    Shorthand(Shorthand),
}

#[derive(Debug, Clone)]
struct ClassSpec {
    negated: bool,
    ignore_case: bool,
    items: Vec<ClassItem>,
}

impl ClassSpec {
    fn matches(&self, c: char) -> bool {
        let inside = self.items.iter().any(|item| match item {
            ClassItem::Range(lo, hi) => in_range(c, *lo, *hi, self.ignore_case),
            ClassItem::Shorthand(shorthand) => shorthand_matches(*shorthand, c),
        });
        inside != self.negated
    }
}

#[derive(Debug, Clone)]
enum Node {
    Char { ch: char, ignore_case: bool },
    Any,
    Class(ClassSpec),
    Shorthand(Shorthand),
    Start,
    End,
    WordBoundary,
    NotWordBoundary,
    Group { index: usize, inner: Vec<Node> },
    Star(Box<Node>),
    Plus(Box<Node>),
    Opt(Box<Node>),
    Alt(Vec<Vec<Node>>),
}

#[derive(Debug, Clone, Copy)]
enum ClassEntry {
    Char(char),
    Shorthand(Shorthand),
}

struct Parser<'a> {
    chars: &'a [char],
    index: usize,
    groups: usize,
    ignore_case: bool,
}

impl<'a> Parser<'a> {
    fn new(chars: &'a [char]) -> Self {
        Self {
            chars,
            index: 0,
            groups: 0,
            ignore_case: false,
        }
    }

    fn parse(&mut self) -> Vec<Node> {
        self.alternation()
    }

    fn flag_group(&self) -> Option<(bool, usize)> {
        if self.chars.get(self.index) != Some(&'(') || self.chars.get(self.index + 1) != Some(&'?')
        {
            return None;
        }
        let mut i = self.index + 2;
        let mut enable = true;
        let mut case = None;
        while let Some(&c) = self.chars.get(i) {
            match c {
                ')' => break,
                'i' => case = Some(enable),
                '-' => enable = false,
                _ => return None,
            }
            i += 1;
        }
        if self.chars.get(i) != Some(&')') || i == self.index + 2 {
            return None;
        }
        case.map(|value| (value, i + 1))
    }

    fn alternation(&mut self) -> Vec<Node> {
        let mut branches = vec![self.sequence()];
        while self.chars.get(self.index) == Some(&'|') {
            self.index += 1;
            branches.push(self.sequence());
        }
        if branches.len() == 1 {
            branches.pop().unwrap_or_default()
        } else {
            vec![Node::Alt(branches)]
        }
    }

    fn sequence(&mut self) -> Vec<Node> {
        let mut nodes = Vec::new();
        while let Some(&c) = self.chars.get(self.index) {
            if c == ')' || c == '|' {
                break;
            }
            if c == '(' {
                if let Some((ignore_case, next)) = self.flag_group() {
                    self.ignore_case = ignore_case;
                    self.index = next;
                    continue;
                }
            }
            self.index += 1;
            let atom = match c {
                '^' => Node::Start,
                '$' => Node::End,
                '.' => Node::Any,
                '(' => {
                    self.groups += 1;
                    let index = self.groups;
                    let inner = self.alternation();
                    if self.chars.get(self.index) == Some(&')') {
                        self.index += 1;
                    }
                    Node::Group { index, inner }
                }
                '[' => self.char_class(),
                '\\' => self.escape(),
                other => Node::Char {
                    ch: other,
                    ignore_case: self.ignore_case,
                },
            };
            self.quantify(atom, &mut nodes);
        }
        nodes
    }

    fn quantify(&mut self, atom: Node, out: &mut Vec<Node>) {
        match self.chars.get(self.index) {
            Some('*') => {
                self.index += 1;
                out.push(Node::Star(Box::new(atom)));
            }
            Some('+') => {
                self.index += 1;
                out.push(Node::Plus(Box::new(atom)));
            }
            Some('?') => {
                self.index += 1;
                out.push(Node::Opt(Box::new(atom)));
            }
            Some('{') => match self.repetition(self.index) {
                Some((min, max, next)) => {
                    self.index = next;
                    for _ in 0..min {
                        out.push(atom.clone());
                    }
                    match max {
                        None => out.push(Node::Star(Box::new(atom))),
                        Some(bound) => {
                            for _ in min..bound {
                                out.push(Node::Opt(Box::new(atom.clone())));
                            }
                        }
                    }
                }
                None => out.push(atom),
            },
            _ => out.push(atom),
        }
    }

    /// Parse `{m}`, `{m,}` or `{m,n}` at `start` (which holds `{`).
    ///
    /// Returns `None` when the braces are not a valid counted repetition, so the
    /// caller can treat `{` as a literal, as Annex B of ECMAScript does.
    fn repetition(&self, start: usize) -> Option<(usize, Option<usize>, usize)> {
        let mut i = start + 1;
        let min = self.digits(&mut i)?;
        if self.chars.get(i) == Some(&'}') {
            return (min <= REPEAT_LIMIT).then_some((min, Some(min), i + 1));
        }
        if self.chars.get(i) != Some(&',') {
            return None;
        }
        i += 1;
        if self.chars.get(i) == Some(&'}') {
            return (min <= REPEAT_LIMIT).then_some((min, None, i + 1));
        }
        let max = self.digits(&mut i)?;
        if self.chars.get(i) != Some(&'}') || min > max || max > REPEAT_LIMIT {
            return None;
        }
        Some((min, Some(max), i + 1))
    }

    fn digits(&self, index: &mut usize) -> Option<usize> {
        let start = *index;
        let mut value: usize = 0;
        while let Some(c) = self.chars.get(*index) {
            match c.to_digit(10) {
                Some(digit) => {
                    value = value.saturating_mul(10).saturating_add(digit as usize);
                    *index += 1;
                }
                None => break,
            }
        }
        (*index > start).then_some(value)
    }

    fn escape(&mut self) -> Node {
        match self.chars.get(self.index).copied() {
            Some('d') => {
                self.index += 1;
                Node::Shorthand(Shorthand::Digit)
            }
            Some('D') => {
                self.index += 1;
                Node::Shorthand(Shorthand::NotDigit)
            }
            Some('w') => {
                self.index += 1;
                Node::Shorthand(Shorthand::Word)
            }
            Some('W') => {
                self.index += 1;
                Node::Shorthand(Shorthand::NotWord)
            }
            Some('s') => {
                self.index += 1;
                Node::Shorthand(Shorthand::Space)
            }
            Some('S') => {
                self.index += 1;
                Node::Shorthand(Shorthand::NotSpace)
            }
            Some('b') => {
                self.index += 1;
                Node::WordBoundary
            }
            Some('B') => {
                self.index += 1;
                Node::NotWordBoundary
            }
            Some('x') | Some('X') => {
                self.index += 1;
                self.hex_escape()
            }
            Some(escaped) => {
                self.index += 1;
                Node::Char {
                    ch: escaped,
                    ignore_case: self.ignore_case,
                }
            }
            None => Node::Char {
                ch: '\\',
                ignore_case: self.ignore_case,
            },
        }
    }

    fn hex_escape(&mut self) -> Node {
        let mut digits = String::new();
        for _ in 0..2 {
            match self.chars.get(self.index) {
                Some(hex) if hex.is_ascii_hexdigit() => {
                    digits.push(*hex);
                    self.index += 1;
                }
                _ => break,
            }
        }
        let byte = u8::from_str_radix(&digits, 16).unwrap_or(0);
        Node::Char {
            ch: cp1252_char(byte),
            ignore_case: self.ignore_case,
        }
    }

    fn char_class(&mut self) -> Node {
        let mut negated = false;
        if self.chars.get(self.index) == Some(&'^') {
            negated = true;
            self.index += 1;
        }
        let mut entries: Vec<ClassEntry> = Vec::new();
        while let Some(&c) = self.chars.get(self.index) {
            if c == ']' {
                self.index += 1;
                break;
            }
            self.index += 1;
            if c == '\\' {
                match self.chars.get(self.index).copied() {
                    Some('d') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::Digit));
                    }
                    Some('D') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::NotDigit));
                    }
                    Some('w') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::Word));
                    }
                    Some('W') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::NotWord));
                    }
                    Some('s') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::Space));
                    }
                    Some('S') => {
                        self.index += 1;
                        entries.push(ClassEntry::Shorthand(Shorthand::NotSpace));
                    }
                    Some(escaped) => {
                        self.index += 1;
                        entries.push(ClassEntry::Char(escaped));
                    }
                    None => entries.push(ClassEntry::Char('\\')),
                }
                continue;
            }
            entries.push(ClassEntry::Char(c));
        }

        let mut items = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            match entries[i] {
                ClassEntry::Shorthand(shorthand) => {
                    items.push(ClassItem::Shorthand(shorthand));
                    i += 1;
                }
                ClassEntry::Char(lo) => {
                    if i + 2 < entries.len() {
                        if let (ClassEntry::Char('-'), ClassEntry::Char(hi)) =
                            (entries[i + 1], entries[i + 2])
                        {
                            if lo != '-' {
                                items.push(ClassItem::Range(lo, hi));
                                i += 3;
                                continue;
                            }
                        }
                    }
                    items.push(ClassItem::Range(lo, lo));
                    i += 1;
                }
            }
        }
        Node::Class(ClassSpec {
            negated,
            ignore_case: self.ignore_case,
            items,
        })
    }
}

#[derive(Debug, Clone)]
enum Inst {
    Char(char, bool),
    Any,
    Class(ClassSpec),
    Shorthand(Shorthand),
    WordBoundary(bool),
    Start,
    End,
    Save(usize),
    Split(usize, usize),
    Jmp(usize),
    Match,
}

struct Program {
    insts: Vec<Inst>,
    groups: usize,
}

fn compile(nodes: &[Node], groups: usize, search: bool) -> Program {
    let mut insts = Vec::new();
    insts.push(Inst::Save(0));
    if search {
        // Leftmost search is part of the program: a lazy `.*?` lets the single
        // step budget bound both scanning and matching. The second `Save(0)`
        // overwrites the initial one, so group 0 starts at the real match.
        let split = insts.len();
        insts.push(Inst::Split(0, 0));
        let body = insts.len();
        insts.push(Inst::Any);
        insts.push(Inst::Jmp(split));
        let after = insts.len();
        insts[split] = Inst::Split(after, body);
        insts.push(Inst::Save(0));
    }
    let mut compiler = Compiler { insts };
    compiler.sequence(nodes);
    compiler.insts.push(Inst::Save(1));
    compiler.insts.push(Inst::Match);
    Program {
        insts: compiler.insts,
        groups,
    }
}

struct Compiler {
    insts: Vec<Inst>,
}

impl Compiler {
    fn sequence(&mut self, nodes: &[Node]) {
        for node in nodes {
            self.node(node);
        }
    }

    fn node(&mut self, node: &Node) {
        match node {
            Node::Char { ch, ignore_case } => self.insts.push(Inst::Char(*ch, *ignore_case)),
            Node::Any => self.insts.push(Inst::Any),
            Node::Class(spec) => self.insts.push(Inst::Class(spec.clone())),
            Node::Shorthand(shorthand) => self.insts.push(Inst::Shorthand(*shorthand)),
            Node::Start => self.insts.push(Inst::Start),
            Node::End => self.insts.push(Inst::End),
            Node::WordBoundary => self.insts.push(Inst::WordBoundary(true)),
            Node::NotWordBoundary => self.insts.push(Inst::WordBoundary(false)),
            Node::Group { index, inner } => {
                self.insts.push(Inst::Save(index * 2));
                self.sequence(inner);
                self.insts.push(Inst::Save(index * 2 + 1));
            }
            Node::Star(inner) => {
                let split = self.insts.len();
                self.insts.push(Inst::Split(0, 0));
                let body = self.insts.len();
                self.node(inner);
                self.insts.push(Inst::Jmp(split));
                let after = self.insts.len();
                self.insts[split] = Inst::Split(body, after);
            }
            Node::Plus(inner) => {
                let body = self.insts.len();
                self.node(inner);
                let split = self.insts.len();
                self.insts.push(Inst::Split(0, 0));
                let after = self.insts.len();
                self.insts[split] = Inst::Split(body, after);
            }
            Node::Opt(inner) => {
                let split = self.insts.len();
                self.insts.push(Inst::Split(0, 0));
                let body = self.insts.len();
                self.node(inner);
                let after = self.insts.len();
                self.insts[split] = Inst::Split(body, after);
            }
            Node::Alt(branches) => {
                let last = branches.len().saturating_sub(1);
                let mut jumps = Vec::new();
                for (index, branch) in branches.iter().enumerate() {
                    if index == last {
                        self.sequence(branch);
                    } else {
                        let split = self.insts.len();
                        self.insts.push(Inst::Split(0, 0));
                        let body = self.insts.len();
                        self.sequence(branch);
                        let jump = self.insts.len();
                        self.insts.push(Inst::Jmp(0));
                        jumps.push(jump);
                        let next = self.insts.len();
                        self.insts[split] = Inst::Split(body, next);
                    }
                }
                let end = self.insts.len();
                for jump in jumps {
                    self.insts[jump] = Inst::Jmp(end);
                }
            }
        }
    }
}

struct Thread {
    pc: usize,
    pos: usize,
    caps: Vec<Option<usize>>,
}

fn exec(program: &Program, text: &[char], steps: &mut u32) -> Option<Vec<Option<usize>>> {
    let slots = (program.groups + 1) * 2;
    let mut stack: Vec<Thread> = Vec::new();
    let mut pc = 0usize;
    let mut pos = 0usize;
    let mut caps: Vec<Option<usize>> = vec![None; slots];

    loop {
        if *steps == 0 {
            return None;
        }
        *steps -= 1;
        let inst = program.insts.get(pc)?;
        match inst {
            Inst::Save(slot) => {
                if let Some(cell) = caps.get_mut(*slot) {
                    *cell = Some(pos);
                }
                pc += 1;
                continue;
            }
            Inst::Split(first, second) => {
                if stack.len() >= BACKTRACK_LIMIT {
                    return None;
                }
                stack.push(Thread {
                    pc: *second,
                    pos,
                    caps: caps.clone(),
                });
                pc = *first;
                continue;
            }
            Inst::Jmp(target) => {
                pc = *target;
                continue;
            }
            Inst::Match => return Some(caps),
            Inst::Char(c, ignore_case) => {
                if let Some(&t) = text.get(pos) {
                    if char_eq(t, *c, *ignore_case) {
                        pos += 1;
                        pc += 1;
                        continue;
                    }
                }
            }
            Inst::Any => {
                if pos < text.len() {
                    pos += 1;
                    pc += 1;
                    continue;
                }
            }
            Inst::Class(spec) => {
                if let Some(&t) = text.get(pos) {
                    if spec.matches(t) {
                        pos += 1;
                        pc += 1;
                        continue;
                    }
                }
            }
            Inst::Shorthand(shorthand) => {
                if let Some(&t) = text.get(pos) {
                    if shorthand_matches(*shorthand, t) {
                        pos += 1;
                        pc += 1;
                        continue;
                    }
                }
            }
            Inst::Start => {
                if pos == 0 {
                    pc += 1;
                    continue;
                }
            }
            Inst::End => {
                if pos == text.len() {
                    pc += 1;
                    continue;
                }
            }
            Inst::WordBoundary(wanted) => {
                if is_word_boundary(text, pos) == *wanted {
                    pc += 1;
                    continue;
                }
            }
        }

        let thread = stack.pop()?;
        pc = thread.pc;
        pos = thread.pos;
        caps = thread.caps;
    }
}

fn capture_strings(caps: &[Option<usize>], groups: usize, text: &[char]) -> Vec<String> {
    let mut out = Vec::with_capacity(groups + 1);
    for group in 0..=groups {
        let value = match (
            caps.get(group * 2).copied().flatten(),
            caps.get(group * 2 + 1).copied().flatten(),
        ) {
            (Some(start), Some(end)) if start <= end && end <= text.len() => {
                text[start..end].iter().collect()
            }
            _ => String::new(),
        };
        out.push(value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(pattern: &str, text: &str) -> bool {
        find(pattern, text).is_some()
    }

    fn whole(pattern: &str, text: &str) -> Option<String> {
        find(pattern, text).map(|c| c[0].clone())
    }

    fn groups(pattern: &str, text: &str) -> Option<Vec<String>> {
        find(pattern, text)
    }

    #[test]
    fn literals_and_anchors() {
        assert!(matched("abc", "xabcx"));
        assert!(!matched("^abc$", "xabcx"));
        assert!(matched("^abc$", "abc"));
        assert!(matched("^a", "abc"));
        assert!(!matched("^b", "abc"));
        assert!(matched("c$", "abc"));
    }

    #[test]
    fn dot_and_classes() {
        assert!(matched("a.c", "abc"));
        assert!(!matched("a.c", "ac"));
        assert!(matched("[abc]x", "bx"));
        assert!(!matched("[abc]x", "dx"));
        assert!(matched("[a-z]+", "hello"));
        assert!(matched("[^0-9]", "a"));
        assert!(!matched("^[^0-9]+$", "123"));
    }

    #[test]
    fn quantifiers_are_greedy() {
        assert_eq!(whole("(.*)", "hello"), Some("hello".to_owned()));
        assert_eq!(whole("a+", "aaa"), Some("aaa".to_owned()));
        assert_eq!(whole("ab?c", "ac"), Some("ac".to_owned()));
    }

    #[test]
    fn the_corpus_patterns_from_the_guide() {
        assert!(matched("^(.*) plus (.*)$", "2 plus 3"));
        assert!(matched("(.*)damn(.*)", "oh damn it"));
        assert!(matched("(.*)[lr]([aeiouy][^ .].*)", "flower"));
        assert!(matched("^i like (.*)$", "i like roses"));
        assert!(matched("^gun ([0-3]) ([0-3]) ([0-3])$", "gun 2 1 0"));
        assert!(!matched("^gun ([0-3]) ([0-3]) ([0-3])$", "gun 9 1 0"));
        assert!(matched("^[0-9]$", "7"));
        assert!(!matched("^[0-9]$", "77"));
    }

    #[test]
    fn alternation_and_hex_escapes() {
        assert!(matched("cat|dog", "hotdog"));
        assert!(matched("\\x41", "A"));
        assert!(matched("a\\.b", "a.b"));
        assert!(!matched("a\\.b", "axb"));
    }

    #[test]
    fn a_pathological_pattern_gives_up_instead_of_hanging() {
        let pattern = "(a+)+$";
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab";
        assert!(!matched(pattern, text), "bounded, no match, no hang");
    }

    #[test]
    fn capture_groups_follow_the_whole_match() {
        assert_eq!(
            groups("^(.*) plus (.*)$", "2 plus 3"),
            Some(vec!["2 plus 3".to_owned(), "2".to_owned(), "3".to_owned(),])
        );
    }

    #[test]
    fn a_skipped_optional_group_is_the_empty_string() {
        assert_eq!(
            groups("^(a)?(b)$", "b"),
            Some(vec!["b".to_owned(), String::new(), "b".to_owned()])
        );
        assert_eq!(
            groups("^(a)?(b)$", "ab"),
            Some(vec!["ab".to_owned(), "a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn nested_groups_are_numbered_by_opening_parenthesis() {
        assert_eq!(
            groups("^((a)(b))$", "ab"),
            Some(vec![
                "ab".to_owned(),
                "ab".to_owned(),
                "a".to_owned(),
                "b".to_owned(),
            ])
        );
    }

    #[test]
    fn shorthands_match_ecmascript() {
        assert!(matched(r"^\d+$", "123"));
        assert!(!matched(r"^\d+$", "12a"));
        assert!(matched(r"^\D+$", "abc"));
        assert!(!matched(r"^\D+$", "ab1"));
        assert!(matched(r"^\w+$", "ab_9"));
        assert!(!matched(r"^\w+$", "a-b"));
        assert!(matched(r"^\s+$", " \t"));
        assert!(!matched(r"^\s+$", " x"));
        assert!(matched(r"\bcat\b", "a cat!"));
        assert!(!matched(r"\bcat\b", "scatter"));
        assert!(matched(r"^[\d]+$", "123"));
        assert!(!matched(r"^[\d]+$", "12a"));
    }

    #[test]
    fn repetition_counts_are_greedy_and_bounded() {
        assert!(matched("^a{2,3}$", "aa"));
        assert!(matched("^a{2,3}$", "aaa"));
        assert!(!matched("^a{2,3}$", "a"));
        assert!(!matched("^a{2,3}$", "aaaa"));
        assert!(matched("^a{2}$", "aa"));
        assert!(!matched("^a{2}$", "a"));
        assert!(matched("^a{2,}$", "aaaa"));
        assert!(!matched("^a{2,}$", "a"));
        assert!(matched("^ab{0}c$", "ac"));
    }

    #[test]
    fn a_brace_that_is_not_a_counted_repetition_is_literal() {
        assert!(matched("^a{foo}$", "a{foo}"));
        assert!(matched("^a{,2}$", "a{,2}"));
    }

    #[test]
    fn a_flag_group_controls_case() {
        assert!(!matched("^abc$", "ABC"));
        assert!(matched("(?i)^abc$", "ABC"));
        assert!(matched("(?i)^abc(?-i)DEF$", "abcDEF"));
        assert!(!matched("(?i)^abc(?-i)DEF$", "ABCdef"));
    }

    #[test]
    fn a_leading_double_caret_is_a_literal_caret() {
        assert!(matched("^^", "^hello"));
        assert!(!matched("^^", "hello"));
        assert_eq!(whole("^^(.*)$", "^rosebud"), Some("^rosebud".to_owned()));
    }
}

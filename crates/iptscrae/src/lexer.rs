//! Tokenizer and parser.
//!
//! IPTSCRAE is whitespace-delimited and postfix, so lexing and parsing are the
//! same pass: there is no operator precedence and no expression tree. Atomlists
//! nest, which is the only structural grammar.
//!
//! ## The rules, as implemented
//!
//! * Whitespace is space, tab, CR and LF. **As the `comma_separator` corpus
//!   extension, a `,` also separates tokens exactly like a space**: the
//!   reference `IptParser.as` has no comma branch and would throw "Unexpected
//!   character", but the live server serves bodies that separate values with
//!   commas — `media_custo2.txt`'s arena line is
//!   `[1000,0 537,0 ...] 0,0 ADDSPOT` — and those scripts MUST lex. A comma is
//!   never an operator and never a value, only a separator. A NUL byte ends the
//!   script (the reference client stops at `charCodeAt(0) == 0`).
//! * `#` and `;` begin a comment that runs to end of line, inside or outside an
//!   atomlist. A `}` inside a comment does not close an atomlist.
//! * `{ ... }` is an atomlist and nests. No whitespace is required around the
//!   braces — real corpus scripts contain `{0 0 MOVE` and `{GLOBAL}` — even
//!   though the guide's examples always pad them.
//! * `"..."` is a string and **may span newlines**. `\` protects the next
//!   character (so `\"` is a literal quote); `\xNN` decodes one
//!   Windows-1252 byte. Any other `\c` is just `c`.
//! * `[ ... ]` is an array. The brackets only push/close a mark; the elements
//!   between them are ordinary code.
//! * Multi-character operators are recognised by one character of lookahead:
//!   `!=  ==  ++  +=  --  -=  <>  <=  >=  *=  /=  &=  %=`
//! * A symbol is `[A-Za-z0-9_]+`, upper-cased, and resolved through the
//!   [`CommandSet`]: a registered name becomes a command, anything else a
//!   variable reference. Symbols may not begin with a digit (the number branch
//!   wins).
//! * A number is `-?[0-9]+`. **A `-` immediately before a digit is part of a
//!   negative literal**, so `A-5` is `A` then `-5`, not a subtraction. This is a
//!   real portability trap inherited from the reference tokenizer.
//!
//! Anything else is [`IptError::UnexpectedToken`]. The reference throws on
//! foreign characters rather than skipping them, which is how the handful of
//! corrupted corpus scripts are detected.

use std::rc::Rc;

use crate::budget::Limits;
use crate::error::{IptError, Result};
use crate::registry::{CommandKind, CommandSet};
use crate::script::Script;
use crate::value::{cp1252_char, Chunk, Op};

/// Parse a whole script file into its `ON NAME { ... }` handlers.
///
/// Scanning is deliberately lenient at the top level, exactly like the
/// reference `parseEventHandlers`: anything that is not the literal pattern
/// `ON <whitespace> <name> {` is skipped and ignored. Real harvested scripts
/// contain trailing junk, stray brackets and even wholly commented-out
/// handlers, and the reference client tolerates all of it. Handler bodies are
/// still parsed strictly, so a genuine syntax error inside a handler is an
/// error.
///
/// A file with no `ON` blocks yields an empty [`Script`] rather than an error;
/// use [`Script::is_empty`] to detect that.
pub fn parse_script(source: &str, commands: &CommandSet, limits: &Limits) -> Result<Script> {
    let mut lexer = Lexer::new(source, commands, limits);
    let mut handlers = std::collections::BTreeMap::new();
    loop {
        match lexer.peek() {
            None => break,
            Some(c) if Lexer::separates_tokens(c) => {
                lexer.bump();
            }
            Some('#') | Some(';') => lexer.skip_line_comment(),
            Some('O')
                if lexer.nth(1) == Some('N') && lexer.nth(2).is_some_and(Lexer::is_whitespace) =>
            {
                lexer.bump();
                lexer.bump();
                let name = lexer.read_handler_name();
                if let Some(body) = lexer.read_handler_body()? {
                    if !name.is_empty() {
                        handlers.insert(name, body);
                    }
                }
            }
            Some(_) => {
                lexer.bump();
            }
        }
    }
    Ok(Script::new(handlers))
}

/// Parse a bare instruction sequence with no surrounding braces.
///
/// Used for one-shot input-box commands and by `STRTOATOM`.
pub fn parse_body(source: &str, commands: &CommandSet, limits: &Limits) -> Result<Chunk> {
    let mut lexer = Lexer::new(source, commands, limits);
    lexer.lex_body(1)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    commands: &'a CommandSet,
    limits: &'a Limits,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, commands: &'a CommandSet, limits: &'a Limits) -> Self {
        Self {
            src,
            pos: 0,
            commands,
            limits,
        }
    }

    /// The current character, or `None` at end of input. A NUL terminates, as
    /// in the reference client.
    fn peek(&self) -> Option<char> {
        let c = self.src[self.pos..].chars().next()?;
        if c == '\0' {
            None
        } else {
            Some(c)
        }
    }

    /// The `n`-th character from the cursor (`0` is the current one).
    fn nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n).filter(|c| *c != '\0')
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Real whitespace, as the reference tokenizer defines it.
    fn is_whitespace(c: char) -> bool {
        matches!(c, ' ' | '\t' | '\r' | '\n')
    }

    /// Whether `c` separates tokens.
    ///
    /// This is whitespace **plus the `comma_separator` corpus extension**.
    /// The reference `IptParser.as` has no comma branch, so a comma would be an
    /// "Unexpected character" there; but the live Colosseum server serves
    /// scripts whose bodies use `x,y` separators (`media_custo2.txt`'s arena
    /// line), and the production tokenizer is forced to handle them explicitly.
    /// A comma separates exactly like a space — it is never an operator and
    /// never a value. Only token *separation* uses this predicate; string
    /// literals and comments still consume commas verbatim.
    fn separates_tokens(c: char) -> bool {
        Self::is_whitespace(c) || c == ','
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\r' || c == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if Self::separates_tokens(c) => {
                    self.bump();
                }
                Some('#') | Some(';') => self.skip_line_comment(),
                _ => return,
            }
        }
    }

    /// Read a handler name the way `parseEventHandlers` does: skip whitespace,
    /// comments and any other character until a `[A-Za-z0-9_]` run begins, then
    /// return it with its original casing.
    fn read_handler_name(&mut self) -> String {
        loop {
            match self.peek() {
                None => return String::new(),
                Some(c) if Self::separates_tokens(c) => {
                    self.bump();
                }
                Some('#') | Some(';') => self.skip_line_comment(),
                Some(c) if c.is_ascii_alphanumeric() || c == '_' => return self.read_name(),
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    /// Scan forward for the `{` that opens a handler body.
    ///
    /// Returns `Ok(None)` at end of input, which drops the handler, exactly as
    /// the reference does.
    fn read_handler_body(&mut self) -> Result<Option<Chunk>> {
        loop {
            match self.peek() {
                None => return Ok(None),
                Some('#') | Some(';') => self.skip_line_comment(),
                Some('{') => return self.lex_atomlist(1).map(Some),
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    /// Read `[A-Za-z0-9_]+`, upper-cased. Empty if the cursor is not on a
    /// symbol character.
    fn read_symbol_upper(&mut self) -> Result<String> {
        let offset = self.pos;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                out.push(c.to_ascii_uppercase());
                self.bump();
            } else {
                break;
            }
        }
        if out.is_empty() {
            return Err(IptError::UnexpectedToken {
                offset,
                token: self.peek().map(String::from).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Read a handler name, preserving the source casing.
    fn read_name(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                out.push(c);
                self.bump();
            } else {
                break;
            }
        }
        out
    }

    /// Parse `{ ... }`, including its braces.
    fn lex_atomlist(&mut self, depth: usize) -> Result<Chunk> {
        if depth > self.limits.nesting {
            return Err(IptError::NestingTooDeep {
                limit: self.limits.nesting,
            });
        }
        let offset = self.pos;
        if self.peek() != Some('{') {
            return Err(IptError::HandlerBodyMissing { offset });
        }
        self.bump();
        let inner_start = self.pos;
        self.lex_until(Some('}'), depth, offset, Some(inner_start))
    }

    /// Parse a bare body: instructions to end of input.
    fn lex_body(&mut self, depth: usize) -> Result<Chunk> {
        if depth > self.limits.nesting {
            return Err(IptError::NestingTooDeep {
                limit: self.limits.nesting,
            });
        }
        let offset = self.pos;
        self.lex_until(None, depth, offset, None)
    }

    fn lex_until(
        &mut self,
        terminator: Option<char>,
        depth: usize,
        start: usize,
        inner_start: Option<usize>,
    ) -> Result<Chunk> {
        let mut ops = Vec::new();
        let mut array_depth: i32 = 0;
        loop {
            self.skip_trivia();
            let c = match self.peek() {
                None => {
                    return match terminator {
                        Some('}') => Err(IptError::UnterminatedAtomList { offset: start }),
                        _ => Ok(Chunk::new(ops, start as u32)),
                    };
                }
                Some(c) => c,
            };
            if Some(c) == terminator {
                let close = self.pos;
                self.bump();
                return match inner_start {
                    Some(inner_start) => Ok(Chunk::with_source(
                        ops,
                        start as u32,
                        &self.src[inner_start..close],
                    )),
                    None => Ok(Chunk::new(ops, start as u32)),
                };
            }
            match c {
                '{' => {
                    ops.push(Op::Chunk(self.lex_atomlist(depth + 1)?));
                }
                '"' => {
                    ops.push(Op::Str(self.lex_string()?));
                }
                '[' => {
                    self.bump();
                    array_depth += 1;
                    ops.push(Op::Mark);
                }
                ']' => {
                    array_depth -= 1;
                    if array_depth < 0 {
                        return Err(IptError::UnmatchedClose {
                            offset: self.pos,
                            close: ']',
                        });
                    }
                    self.bump();
                    ops.push(Op::ArrayClose);
                }
                '}' => {
                    return Err(IptError::UnmatchedClose {
                        offset: self.pos,
                        close: '}',
                    });
                }
                _ => ops.push(self.lex_operator()?),
            }
        }
    }

    fn lex_string(&mut self) -> Result<Rc<str>> {
        let start = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(IptError::UnterminatedString { offset: start }),
                Some('"') => {
                    self.bump();
                    return Ok(Rc::from(out.as_str()));
                }
                Some('\\') => {
                    self.bump();
                    match self.peek() {
                        None => return Err(IptError::UnterminatedString { offset: start }),
                        Some('x') => {
                            self.bump();
                            let mut digits = String::new();
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(h) if h.is_ascii_hexdigit() => {
                                        digits.push(h);
                                        self.bump();
                                    }
                                    _ => break,
                                }
                            }
                            let byte = u8::from_str_radix(&digits, 16).unwrap_or(0);
                            out.push(cp1252_char(byte));
                        }
                        Some(c) => {
                            out.push(c);
                            self.bump();
                        }
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn lex_operator(&mut self) -> Result<Op> {
        let offset = self.pos;
        let c = match self.peek() {
            Some(c) => c,
            None => return Err(IptError::UnexpectedEof { offset: self.pos }),
        };
        let next = self.nth(1);
        let two = next.filter(|n| !n.is_ascii_digit());
        match c {
            '!' => {
                if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin("!=", crate::registry::Builtin::Ne))
                } else {
                    self.pos += 1;
                    Ok(self.builtin("!", crate::registry::Builtin::Not))
                }
            }
            '=' => {
                if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin("==", crate::registry::Builtin::Eq))
                } else {
                    self.pos += 1;
                    Ok(self.builtin("=", crate::registry::Builtin::Assign))
                }
            }
            '+' => {
                if next == Some('+') {
                    self.pos += 2;
                    Ok(self.builtin("++", crate::registry::Builtin::Inc))
                } else if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin("+=", crate::registry::Builtin::AddAssign))
                } else {
                    self.pos += 1;
                    Ok(self.builtin("+", crate::registry::Builtin::Add))
                }
            }
            '-' if two.is_some() => {
                if next == Some('-') {
                    self.pos += 2;
                    Ok(self.builtin("--", crate::registry::Builtin::Dec))
                } else if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin("-=", crate::registry::Builtin::SubAssign))
                } else {
                    self.pos += 1;
                    Ok(self.builtin("-", crate::registry::Builtin::Sub))
                }
            }
            '<' => {
                if next == Some('>') {
                    self.pos += 2;
                    Ok(self.builtin("<>", crate::registry::Builtin::Ne))
                } else if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin("<=", crate::registry::Builtin::Le))
                } else {
                    self.pos += 1;
                    Ok(self.builtin("<", crate::registry::Builtin::Lt))
                }
            }
            '>' => {
                if next == Some('=') {
                    self.pos += 2;
                    Ok(self.builtin(">=", crate::registry::Builtin::Ge))
                } else {
                    self.pos += 1;
                    Ok(self.builtin(">", crate::registry::Builtin::Gt))
                }
            }
            '*' | '/' | '&' | '%' => {
                let (name, plain) = match c {
                    '*' => ("*", crate::registry::Builtin::Mul),
                    '/' => ("/", crate::registry::Builtin::Div),
                    '&' => ("&", crate::registry::Builtin::Concat),
                    _ => ("%", crate::registry::Builtin::Mod),
                };
                if next == Some('=') {
                    self.pos += 2;
                    let compound = match c {
                        '*' => ("*=", crate::registry::Builtin::MulAssign),
                        '/' => ("/=", crate::registry::Builtin::DivAssign),
                        '&' => ("&=", crate::registry::Builtin::ConcatAssign),
                        _ => ("%=", crate::registry::Builtin::ModAssign),
                    };
                    Ok(self.builtin(compound.0, compound.1))
                } else {
                    self.pos += 1;
                    Ok(self.builtin(name, plain))
                }
            }
            '-' | '0'..='9' => Ok(Op::Int(self.lex_number())),
            '_' | 'a'..='z' | 'A'..='Z' => Ok(self.lex_symbol()?),
            _ => Err(IptError::UnexpectedToken {
                offset,
                token: c.to_string(),
            }),
        }
    }

    fn builtin(&self, name: &str, fallback: crate::registry::Builtin) -> Op {
        match self.commands.get_uppercase(name) {
            Some(CommandKind::Host) => Op::Host(Rc::from(name)),
            _ => Op::Builtin(fallback),
        }
    }

    fn lex_number(&mut self) -> i64 {
        let negative = self.peek() == Some('-');
        if negative {
            self.bump();
        }
        let mut value: i64 = 0;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                value = value.wrapping_mul(10).wrapping_add((c as u8 - b'0') as i64);
                self.bump();
            } else {
                break;
            }
        }
        if negative {
            value.wrapping_neg()
        } else {
            value
        }
    }

    fn lex_symbol(&mut self) -> Result<Op> {
        let name = self.read_symbol_upper()?;
        Ok(match self.commands.get_uppercase(&name) {
            Some(CommandKind::Builtin(b)) => Op::Builtin(b),
            Some(CommandKind::Host) => Op::Host(Rc::from(name)),
            None => Op::Var(Rc::from(name)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Builtin;

    fn set() -> CommandSet {
        CommandSet::core()
    }

    fn body(src: &str) -> Vec<Op> {
        parse_body(src, &set(), &Limits::default())
            .expect("parses")
            .ops()
            .to_vec()
    }

    #[test]
    fn literals_and_builtins() {
        let ops = body("2 3 +");
        assert!(matches!(ops[0], Op::Int(2)));
        assert!(matches!(ops[1], Op::Int(3)));
        assert!(matches!(ops[2], Op::Builtin(Builtin::Add)));
    }

    #[test]
    fn negative_literals() {
        assert!(matches!(body("-32")[0], Op::Int(-32)));
        assert!(matches!(body("2 -3 +")[1], Op::Int(-3)));
    }

    #[test]
    fn a_dash_before_a_digit_is_never_subtraction() {
        let ops = body("A-5");
        assert!(matches!(&ops[0], Op::Var(n) if &**n == "A"));
        assert!(matches!(ops[1], Op::Int(-5)));
        assert_eq!(ops.len(), 2, "no subtraction operator was produced");
    }

    #[test]
    fn symbols_upper_case_and_unknown_ones_are_variables() {
        let ops = body("myVar say");
        assert!(matches!(&ops[0], Op::Var(n) if &**n == "MYVAR"));
        assert!(matches!(&ops[1], Op::Var(n) if &**n == "SAY"));
    }

    #[test]
    fn host_commands_resolve_through_the_registry() {
        let mut s = set();
        s.register_host("SAY");
        let chunk = parse_body("SAY", &s, &Limits::default()).unwrap();
        assert!(matches!(&chunk.ops()[0], Op::Host(n) if &**n == "SAY"));
    }

    #[test]
    fn comments_are_elided_and_can_hide_braces() {
        let ops = body("1 # } not a close\n2");
        assert_eq!(ops.len(), 2);
        let ops = body("1 ; } also not a close\n2");
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn strings_decode_escapes_and_span_lines() {
        let ops = body("\"a\\\"b\"");
        assert!(matches!(&ops[0], Op::Str(s) if &**s == "a\"b"));

        let ops = body("\"line one\nline two\"");
        assert!(matches!(&ops[0], Op::Str(s) if &**s == "line one\nline two"));

        let ops = body("\"\\x93hi\\x94\"");
        assert!(matches!(&ops[0], Op::Str(s) if &**s == "\u{201c}hi\u{201d}"));

        let ops = body("\"\\x41\"");
        assert!(matches!(&ops[0], Op::Str(s) if &**s == "A"), "\\x41 is 'A'");
    }

    #[test]
    fn atomlists_nest_without_whitespace() {
        let ops = body("{1 2 +}");
        assert!(matches!(ops[0], Op::Chunk(_)));
        let ops = body("{GLOBAL}");
        let Op::Chunk(c) = &ops[0] else {
            panic!("expected a chunk")
        };
        assert!(matches!(c.ops()[0], Op::Builtin(Builtin::Global)));

        let ops = body("{0 0 MOVE}");
        let Op::Chunk(c) = &ops[0] else {
            panic!("expected a chunk")
        };
        assert!(matches!(c.ops()[0], Op::Int(0)));
    }

    #[test]
    fn blocks_record_the_text_between_their_braces() {
        let ops = body("{ 1 2 + }");
        let Op::Chunk(c) = &ops[0] else {
            panic!("expected a chunk")
        };
        assert_eq!(c.source(), Some(" 1 2 + "));
        assert_eq!(
            c.offset(),
            0,
            "the offset still points at the opening brace"
        );

        let ops = body("{glued}");
        let Op::Chunk(c) = &ops[0] else {
            panic!("expected a chunk")
        };
        assert_eq!(c.source(), Some("glued"));
    }

    #[test]
    fn nested_blocks_record_their_own_inner_source() {
        let ops = body("{ outer { inner } tail }");
        let Op::Chunk(outer) = &ops[0] else {
            panic!("expected an outer chunk")
        };
        assert_eq!(outer.source(), Some(" outer { inner } tail "));
        let Op::Chunk(inner) = &outer.ops()[1] else {
            panic!("expected an inner chunk")
        };
        assert_eq!(inner.source(), Some(" inner "));
    }

    #[test]
    fn block_source_keeps_strings_and_comments_verbatim() {
        let ops = body("{ \"a } b\" # not a close\n 1 }");
        let Op::Chunk(c) = &ops[0] else {
            panic!("expected a chunk")
        };
        assert_eq!(c.source(), Some(" \"a } b\" # not a close\n 1 "));
    }

    #[test]
    fn a_bare_body_has_no_source_text() {
        let chunk = parse_body("1 2 +", &set(), &Limits::default()).expect("parses");
        assert_eq!(chunk.source(), None);
    }

    #[test]
    fn arrays_emit_marks() {
        let ops = body("[ 1 2 ]");
        assert!(matches!(ops[0], Op::Mark));
        assert!(matches!(ops[3], Op::ArrayClose));
    }

    /// `comma_separator` corpus extension: a comma separates tokens like a
    /// space. Reference `IptParser.as` has no comma branch.
    #[test]
    fn comma_separator_splits_integers() {
        let ops = body("1,2");
        assert_eq!(ops.len(), 2, "a comma is not an operator and not a value");
        assert!(matches!(ops[0], Op::Int(1)));
        assert!(matches!(ops[1], Op::Int(2)));
    }

    #[test]
    fn comma_separator_inside_an_array() {
        let ops = body("[1000,0 537,0]");
        let ints: Vec<i64> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Int(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(ints, vec![1000, 0, 537, 0], "x,y pairs stay in order");
        assert!(matches!(ops[0], Op::Mark));
        assert!(matches!(ops.last(), Some(Op::ArrayClose)));
    }

    #[test]
    fn comma_separator_lexes_the_real_arena_prefix() {
        let mut s = set();
        s.register_host("ADDSPOT");
        let ops = parse_body(
            "[1000,0 537,0 537,363 933,363 933,396 1000,396] 0,0 ADDSPOT",
            &s,
            &Limits::default(),
        )
        .expect("the media_custo2 arena line lexes")
        .ops()
        .to_vec();

        let ints: Vec<i64> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Int(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(
            ints,
            vec![1000, 0, 537, 0, 537, 363, 933, 363, 933, 396, 1000, 396, 0, 0],
            "six polygon x,y pairs, then the spot's x,y"
        );
        assert!(
            matches!(ops.last(), Some(Op::Host(n)) if &**n == "ADDSPOT"),
            "the trailing command is a single host op, not a comma artifact"
        );
        assert_eq!(
            ops.iter().filter(|op| matches!(op, Op::Host(_))).count(),
            1,
            "a comma never produces a command"
        );
    }

    #[test]
    fn comma_inside_a_string_is_a_literal() {
        let ops = body("\"@934,442 hi\"");
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Op::Str(s) if &**s == "@934,442 hi"));
    }

    #[test]
    fn comma_inside_a_comment_is_ignored() {
        let ops = body("1 ; 2,3 not tokens\n4");
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], Op::Int(1)));
        assert!(matches!(ops[1], Op::Int(4)));

        let ops = body("1 # 2,3 not tokens\n4");
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn multi_char_operators() {
        for (src, want) in [
            ("!=", Builtin::Ne),
            ("<>", Builtin::Ne),
            ("==", Builtin::Eq),
            ("++", Builtin::Inc),
            ("--", Builtin::Dec),
            ("+=", Builtin::AddAssign),
            ("-=", Builtin::SubAssign),
            ("*=", Builtin::MulAssign),
            ("/=", Builtin::DivAssign),
            ("%=", Builtin::ModAssign),
            ("&=", Builtin::ConcatAssign),
            ("<=", Builtin::Le),
            (">=", Builtin::Ge),
        ] {
            let ops = body(src);
            assert!(
                matches!(ops[0], Op::Builtin(b) if b == want),
                "{src} did not lex to {want:?}"
            );
        }
    }

    #[test]
    fn malformed_input_is_an_error_never_a_panic() {
        assert!(matches!(
            parse_body("\"unterminated", &set(), &Limits::default()),
            Err(IptError::UnterminatedString { .. })
        ));
        assert!(matches!(
            parse_body("{ 1 2", &set(), &Limits::default()),
            Err(IptError::UnterminatedAtomList { .. })
        ));
        assert!(matches!(
            parse_body("1 } 2", &set(), &Limits::default()),
            Err(IptError::UnmatchedClose { .. })
        ));
        assert!(matches!(
            parse_body("]", &set(), &Limits::default()),
            Err(IptError::UnmatchedClose { .. })
        ));
        assert!(matches!(
            parse_body("$1", &set(), &Limits::default()),
            Err(IptError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn deep_nesting_is_capped_not_a_stack_overflow() {
        let mut src = String::new();
        let depth = Limits::default().nesting + 8;
        for _ in 0..depth {
            src.push('{');
        }
        for _ in 0..depth {
            src.push('}');
        }
        let limits = Limits {
            nesting: 16,
            ..Limits::default()
        };
        assert!(matches!(
            parse_body(&src, &set(), &limits),
            Err(IptError::NestingTooDeep { .. })
        ));
    }

    #[test]
    fn handler_parsing() {
        let src = "ON ENTER { 1 }\nON SELECT { 2 }\n";
        let script = parse_script(src, &set(), &Limits::default()).unwrap();
        assert_eq!(script.len(), 2);
        assert!(script.handler("ENTER").is_some());
        assert!(script.handler("enter").is_some(), "lookup is forgiving");
        assert!(matches!(
            script.handler("SELECT").unwrap().ops()[0],
            Op::Int(2)
        ));
    }

    #[test]
    fn glued_handler_braces_are_accepted() {
        let src = "ON ENTER{1 2 +}";
        let script = parse_script(src, &set(), &Limits::default()).unwrap();
        assert_eq!(script.len(), 1);
    }

    #[test]
    fn top_level_junk_is_skipped_like_the_reference() {
        let script = parse_script("garbage 1 2 + } more", &set(), &Limits::default()).unwrap();
        assert!(script.is_empty(), "nothing to find");
        assert!(
            parse_script(";ON ENTER {1}", &set(), &Limits::default())
                .unwrap()
                .is_empty(),
            "a commented-out handler is not a handler"
        );
        let script = parse_script("ON ENTER { 1 } 1710", &set(), &Limits::default()).unwrap();
        assert_eq!(script.len(), 1, "trailing junk is ignored");
        assert!(
            parse_script("ON ENTER 1", &set(), &Limits::default())
                .unwrap()
                .is_empty(),
            "an ON with no body is dropped"
        );
    }

    #[test]
    fn errors_inside_a_handler_body_are_still_errors() {
        assert!(matches!(
            parse_script("ON ENTER { \"unterminated }", &set(), &Limits::default()),
            Err(IptError::UnterminatedString { .. })
        ));
        assert!(matches!(
            parse_script("ON ENTER { $1 }", &set(), &Limits::default()),
            Err(IptError::UnexpectedToken { .. })
        ));
    }

    #[test]
    fn nul_terminates_the_script() {
        let ops = body("1\u{0}2");
        assert_eq!(ops.len(), 1);
    }
}

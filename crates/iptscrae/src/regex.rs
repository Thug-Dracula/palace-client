//! A small regular-expression engine for `GREPSTR`.
//!
//! The VM does not regex: `GREPSTR` delegates to [`Host::grep_match`], because
//! compiling a pattern that arrives from the network is a policy decision the
//! host should make. This module is the engine the bundled test host and the
//! corpus harness use, so the crate ships a working implementation without
//! taking a dependency.
//!
//! It supports the subset the reference clients' patterns actually use — `^`,
//! `$`, `.`, character classes with ranges and negation, `*`, `+`, `?`, groups
//! and alternation — and matches greedily, leftmost-first. It is
//! **step-bounded**: a pathological pattern gives up and reports "no match"
//! instead of running away, which matters because patterns are untrusted.
//!
//! Capture groups are parsed but only group 0 (the whole match) is returned, so
//! `GREPSUB` substitutes `$0` and leaves `$1`…`$9` alone. Filling in real
//! capture groups is the host's job; see the crate README.

use crate::value::cp1252_char;

/// Match `pattern` against `text`, returning the whole match as capture 0.
///
/// Returns `None` when there is no match, when the pattern uses syntax this
/// engine does not implement, or when the step budget is exhausted.
pub fn find(pattern: &str, text: &str) -> Option<Vec<String>> {
    find_with_limit(pattern, text, 200_000)
}

/// [`find`] with an explicit step budget.
pub fn find_with_limit(pattern: &str, text: &str, limit: u32) -> Option<Vec<String>> {
    let nodes = parse(pattern);
    if nodes.is_empty() {
        return Some(vec![String::new()]);
    }
    let chars: Vec<char> = text.chars().collect();
    let anchored = matches!(nodes.first(), Some(Node::Start));
    let starts: Vec<usize> = if anchored {
        vec![0]
    } else {
        (0..=chars.len()).collect()
    };
    for start in starts {
        let mut steps = limit;
        let ends = match_seq(&nodes, start, &chars, &mut steps);
        if let Some(end) = ends.iter().copied().max() {
            let whole: String = chars[start..end].iter().collect();
            return Some(vec![whole]);
        }
    }
    None
}

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Start,
    End,
    Group(Vec<Node>),
    Star(Box<Node>),
    Plus(Box<Node>),
    Opt(Box<Node>),
    Alt(Vec<Vec<Node>>),
}

fn parse(pattern: &str) -> Vec<Node> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    parse_alternation(&chars, &mut index)
}

fn parse_alternation(chars: &[char], index: &mut usize) -> Vec<Node> {
    let mut branches = vec![parse_sequence(chars, index)];
    while chars.get(*index) == Some(&'|') {
        *index += 1;
        branches.push(parse_sequence(chars, index));
    }
    if branches.len() == 1 {
        branches.pop().unwrap_or_default()
    } else {
        vec![Node::Alt(branches)]
    }
}

fn parse_sequence(chars: &[char], index: &mut usize) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    while let Some(&c) = chars.get(*index) {
        if c == ')' || c == '|' {
            break;
        }
        *index += 1;
        let atom = match c {
            '^' => Node::Start,
            '$' => Node::End,
            '.' => Node::Any,
            '(' => {
                let inner = parse_alternation(chars, index);
                if chars.get(*index) == Some(&')') {
                    *index += 1;
                }
                Node::Group(inner)
            }
            '[' => parse_class(chars, index),
            '\\' => match chars.get(*index) {
                Some('x') | Some('X') => {
                    *index += 1;
                    let mut digits = String::new();
                    for _ in 0..2 {
                        match chars.get(*index) {
                            Some(h) if h.is_ascii_hexdigit() => {
                                digits.push(*h);
                                *index += 1;
                            }
                            _ => break,
                        }
                    }
                    let byte = u8::from_str_radix(&digits, 16).unwrap_or(0);
                    Node::Char(cp1252_char(byte))
                }
                Some(&escaped) => {
                    *index += 1;
                    Node::Char(escaped)
                }
                None => Node::Char('\\'),
            },
            other => Node::Char(other),
        };
        let quantified = match chars.get(*index) {
            Some('*') => {
                *index += 1;
                Node::Star(Box::new(atom))
            }
            Some('+') => {
                *index += 1;
                Node::Plus(Box::new(atom))
            }
            Some('?') => {
                *index += 1;
                Node::Opt(Box::new(atom))
            }
            _ => atom,
        };
        nodes.push(quantified);
    }
    nodes
}

fn parse_class(chars: &[char], index: &mut usize) -> Node {
    let mut negated = false;
    if chars.get(*index) == Some(&'^') {
        negated = true;
        *index += 1;
    }
    let mut items: Vec<char> = Vec::new();
    let mut closed = false;
    while let Some(&c) = chars.get(*index) {
        if c == ']' {
            *index += 1;
            closed = true;
            break;
        }
        *index += 1;
        if c == '\\' {
            if let Some(&escaped) = chars.get(*index) {
                *index += 1;
                items.push(escaped);
            }
            continue;
        }
        items.push(c);
    }
    let _ = closed;
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if i + 2 < items.len() && items[i + 1] == '-' {
            ranges.push((items[i], items[i + 2]));
            i += 3;
        } else {
            ranges.push((items[i], items[i]));
            i += 1;
        }
    }
    Node::Class { negated, ranges }
}

fn match_seq(nodes: &[Node], start: usize, text: &[char], steps: &mut u32) -> Vec<usize> {
    let mut positions = vec![start];
    for node in nodes {
        let mut next = Vec::new();
        for pos in positions {
            for end in match_node(node, pos, text, steps) {
                if !next.contains(&end) {
                    next.push(end);
                }
            }
        }
        if next.is_empty() {
            return Vec::new();
        }
        positions = next;
    }
    positions
}

fn match_node(node: &Node, pos: usize, text: &[char], steps: &mut u32) -> Vec<usize> {
    if *steps == 0 {
        return Vec::new();
    }
    *steps -= 1;
    match node {
        Node::Char(c) => {
            if text.get(pos) == Some(c) {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Node::Any => {
            if pos < text.len() {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Node::Class { negated, ranges } => match text.get(pos) {
            Some(c) => {
                let inside = ranges.iter().any(|(lo, hi)| lo <= c && c <= hi);
                if inside != *negated {
                    vec![pos + 1]
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        },
        Node::Start => {
            if pos == 0 {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Node::End => {
            if pos == text.len() {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Node::Group(inner) => match_seq(inner, pos, text, steps),
        Node::Alt(branches) => {
            let mut out = Vec::new();
            for branch in branches {
                for end in match_seq(branch, pos, text, steps) {
                    if !out.contains(&end) {
                        out.push(end);
                    }
                }
            }
            out
        }
        Node::Opt(inner) => {
            let mut out = vec![pos];
            for end in match_node(inner, pos, text, steps) {
                if !out.contains(&end) {
                    out.push(end);
                }
            }
            out
        }
        Node::Star(inner) => repeat(inner, pos, text, steps, true),
        Node::Plus(inner) => repeat(inner, pos, text, steps, false),
    }
}

fn repeat(
    inner: &Node,
    pos: usize,
    text: &[char],
    steps: &mut u32,
    allow_zero: bool,
) -> Vec<usize> {
    let mut result: Vec<usize> = if allow_zero { vec![pos] } else { Vec::new() };
    let mut frontier = match_node(inner, pos, text, steps);
    let mut guard = 0;
    while !frontier.is_empty() && guard < 4096 {
        guard += 1;
        let mut next = Vec::new();
        for p in frontier {
            if !result.contains(&p) {
                result.push(p);
            }
            for end in match_node(inner, p, text, steps) {
                if !next.contains(&end) {
                    next.push(end);
                }
            }
        }
        frontier = next;
        if *steps == 0 {
            break;
        }
    }
    result.sort_unstable();
    result.dedup();
    result
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
}

//! Runtime values and the atomlist (subroutine) representation.
//!
//! IPTSCRAE is a stack language, so "value" means "whatever sits on the data
//! stack". The reference VM has seven kinds, and they are distinguishable at
//! runtime through `TOPTYPE`:
//!
//! | Kind | `TOPTYPE` code | Truthiness |
//! |---|---|---|
//! | integer | 1 | false **only** at 0 |
//! | variable reference | 2 | the referenced value's truthiness |
//! | atomlist | 3 | always true |
//! | string | 4 | **always true — even `""`** |
//! | array mark (`[`) | 5 | always true |
//! | array | 6 | always true |
//!
//! Note the two surprises: an empty string is *true*, and variables are pushed
//! as *references* rather than values. A binary operator dereferences its
//! operands, which is why `x 1 +` works, while `=`/`+=`/`GLOBAL` need the raw
//! reference and therefore do not.
//!
//! Sharing follows the reference implementation: `DUP`, `OVER`, `PICK` and
//! assignment copy the *handle*. Atomlists and arrays are `Rc`-shared and
//! variables are looked up by name, so two stack slots can observe the same
//! mutable array.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::registry::Builtin;

/// A parsed, immutable atomlist (`{ ... }`).
///
/// Cloning is cheap: it copies one `Rc`. The reference VM clones atomlists on
/// every encounter in the instruction stream and shares the underlying token
/// vector, which is exactly this.
#[derive(Clone)]
pub struct Chunk(Rc<ChunkData>);

struct ChunkData {
    ops: Vec<Op>,
    offset: u32,
}

impl Chunk {
    /// Build a chunk from already-parsed operations.
    pub fn new(ops: Vec<Op>, offset: u32) -> Self {
        Self(Rc::new(ChunkData { ops, offset }))
    }

    /// The empty atomlist, `{}`.
    pub fn empty() -> Self {
        Self::new(Vec::new(), 0)
    }

    /// The operations, in execution order.
    pub fn ops(&self) -> &[Op] {
        &self.0.ops
    }

    /// Byte offset of the opening `{` (or of the script for a bare body).
    pub fn offset(&self) -> u32 {
        self.0.offset
    }

    /// Whether this chunk has no operations.
    pub fn is_empty(&self) -> bool {
        self.0.ops.is_empty()
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk")
            .field("offset", &self.0.offset)
            .field("ops", &self.0.ops.len())
            .finish()
    }
}

impl PartialEq for Chunk {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || self.0.ops == other.0.ops
    }
}

impl Eq for Chunk {}

/// One parsed instruction or literal.
///
/// The reference VM has no jumps: control flow is expressed by pushing atomlist
/// literals as data and having `IF`/`WHILE`/`EXEC` schedule them. `Op` mirrors
/// that — a chunk is a flat, linear list.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Integer literal.
    Int(i32),
    /// String literal (source bytes already decoded to `char`s).
    Str(Rc<str>),
    /// A symbol that is not a registered command: a variable reference.
    Var(Rc<str>),
    /// An atomlist literal.
    Chunk(Chunk),
    /// `[` — push an array mark.
    Mark,
    /// `]` — collect everything above the nearest mark into an array.
    ArrayClose,
    /// A core command.
    Builtin(Builtin),
    /// A command registered by the host layer (every Palace command).
    Host(Rc<str>),
}

/// A mutable array; arrays are reference values so `PUT` is visible through
/// every handle.
pub type ArrayRef = Rc<RefCell<Vec<Value>>>;

/// A value on the data stack.
#[derive(Clone)]
pub enum Value {
    /// 32-bit signed integer. There are no floats.
    Int(i32),
    /// Immutable string.
    Str(Rc<str>),
    /// Atomlist (subroutine).
    Chunk(Chunk),
    /// Reference-counted mutable array.
    Array(ArrayRef),
    /// A variable reference, carrying the (already upper-cased) name.
    Var(Rc<str>),
    /// The `[` array mark.
    Mark,
}

impl Value {
    /// The `TOPTYPE` code for this value, without dereferencing.
    pub fn type_code(&self) -> i32 {
        match self {
            Value::Int(_) => 1,
            Value::Var(_) => 2,
            Value::Chunk(_) => 3,
            Value::Str(_) => 4,
            Value::Mark => 5,
            Value::Array(_) => 6,
        }
    }

    /// Name used in type-mismatch error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "number",
            Value::Str(_) => "string",
            Value::Chunk(_) => "atomlist",
            Value::Array(_) => "array",
            Value::Var(_) => "variable",
            Value::Mark => "array mark",
        }
    }

    /// Truthiness of an already-dereferenced value.
    ///
    /// Only the integer zero is false. Everything else — including the empty
    /// string — is true.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Int(n) => *n != 0,
            _ => true,
        }
    }

    /// Construct a string value.
    pub fn str(s: impl AsRef<str>) -> Self {
        Value::Str(Rc::from(s.as_ref()))
    }

    /// Construct an array value.
    pub fn array(items: Vec<Value>) -> Self {
        Value::Array(Rc::new(RefCell::new(items)))
    }

    /// The array length, or 0 if the cell is currently borrowed.
    pub fn array_len(&self) -> usize {
        match self {
            Value::Array(a) => a.try_borrow().map(|v| v.len()).unwrap_or(0),
            _ => 0,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Chunk(c) => write!(f, "{c:?}"),
            Value::Array(a) => match a.try_borrow() {
                Ok(items) => write!(f, "Array({})", items.len()),
                Err(_) => write!(f, "Array(<borrowed>)"),
            },
            Value::Var(name) => write!(f, "&{name}"),
            Value::Mark => write!(f, "["),
        }
    }
}

/// Structural equality where it is meaningful.
///
/// Integers and strings compare by value; variables compare by name; atomlists
/// and arrays compare by identity (they are handles, not values). This exists so
/// tests can assert on stack contents without a bespoke comparator.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Var(a), Value::Var(b)) => a == b,
            (Value::Mark, Value::Mark) => true,
            (Value::Chunk(a), Value::Chunk(b)) => Rc::ptr_eq(&a.0, &b.0),
            (Value::Array(a), Value::Array(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for Value {}

/// Decode a Windows-1252 byte to the `char` the reference client produces.
///
/// `\xNN` string escapes are decoded one byte at a time through this map, which
/// is why `\x93` is a left double quote rather than U+0093.
pub fn cp1252_char(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];
    if (0x80..=0x9f).contains(&byte) {
        HIGH[(byte - 0x80) as usize]
    } else {
        byte as char
    }
}

/// Decode arbitrary script bytes into text.
///
/// A UTF-8 BOM is stripped and, if the remainder is valid UTF-8, it is used as
/// is; otherwise the bytes are decoded as Windows-1252, which is what the
/// original clients wrote. Real corpus scripts contain occasional single
/// Latin-1 bytes (an umlaut) that are not valid UTF-8, so the fallback matters.
pub fn decode_source(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    match std::str::from_utf8(body) {
        Ok(s) => s.to_owned(),
        Err(_) => body.iter().copied().map(cp1252_char).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_codes_match_the_reference() {
        assert_eq!(Value::Int(0).type_code(), 1);
        assert_eq!(Value::Var(Rc::from("X")).type_code(), 2);
        assert_eq!(Value::Chunk(Chunk::empty()).type_code(), 3);
        assert_eq!(Value::str("hi").type_code(), 4);
        assert_eq!(Value::Mark.type_code(), 5);
        assert_eq!(Value::array(vec![]).type_code(), 6);
    }

    #[test]
    fn only_integer_zero_is_false() {
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(Value::Int(-1).is_truthy());
        assert!(Value::str("").is_truthy(), "empty string is true");
        assert!(Value::Chunk(Chunk::empty()).is_truthy());
        assert!(Value::array(vec![]).is_truthy());
        assert!(Value::Mark.is_truthy());
    }

    #[test]
    fn cp1252_decodes_the_special_range() {
        assert_eq!(cp1252_char(0x93), '\u{201c}');
        assert_eq!(cp1252_char(0x94), '\u{201d}');
        assert_eq!(cp1252_char(b'A'), 'A');
        assert_eq!(cp1252_char(0xfc), '\u{fc}');
    }

    #[test]
    fn decode_source_falls_back_to_windows_1252() {
        assert_eq!(decode_source(b"abc"), "abc");
        assert_eq!(decode_source(b"\xef\xbb\xbfabc"), "abc");
        assert_eq!(decode_source(&[b'a', 0xfc, b'b']), "a\u{fc}b");
    }
}

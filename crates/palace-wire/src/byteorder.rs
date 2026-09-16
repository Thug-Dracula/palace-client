//! Endianness handling for the Palace wire protocol.
//!
//! Palace has no in-band endianness negotiation. The server's **first** packet
//! is always `MSG_TIYID` ("This Is Your ID"). Because the 32-bit `eventType`
//! field of that packet is the ASCII mnemonic `'tiyr'` written in the server's
//! native byte order, the raw bytes reveal the order:
//!
//! | first 4 bytes on the wire | server writes ints as | we use |
//! |---------------------------|-----------------------|--------|
//! | `ryit`                    | little-endian         | [`ByteOrder::Little`] |
//! | `tiyr`                    | big-endian            | [`ByteOrder::Big`]    |
//! | `pser`                    | (HTTP tunnel)         | unsupported           |
//!
//! Getting this wrong is a silent total failure, so the decision is made once
//! and then threaded through [`Reader`]/[`Writer`]; no call site performs a
//! conditional swap.
//!
//! ## A note on the two mnemonics
//!
//! The *value* `0x7469_7972` spells `tiyr` when its bytes are laid out
//! big-endian. A little-endian server sends the same value as the bytes
//! `72 79 69 74` = `ryit`. In other words: **`ryit` on the wire means the
//! server is little-endian.** The published protocol reference calls the
//! little-endian banner `MSG_DIYIT`, which is why both spellings appear in the
//! literature.

use crate::error::{Result, WireError};

/// The byte order that governs an entire session.
///
/// Once detected from the `MSG_TIYID` banner it is stored here and carried by
/// every [`Reader`] and [`Writer`] for the rest of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    /// The native order of this machine.
    pub const NATIVE: ByteOrder = if cfg!(target_endian = "big") {
        ByteOrder::Big
    } else {
        ByteOrder::Little
    };

    /// Human-readable label, used in fixture manifests and logs.
    pub const fn label(self) -> &'static str {
        match self {
            ByteOrder::Little => "little",
            ByteOrder::Big => "big",
        }
    }

    /// Parse the label produced by [`ByteOrder::label`].
    pub fn from_label(label: &str) -> Option<ByteOrder> {
        match label {
            "little" | "le" | "LE" => Some(ByteOrder::Little),
            "big" | "be" | "BE" => Some(ByteOrder::Big),
            _ => None,
        }
    }

    /// Detect the session byte order from the first four bytes of the
    /// `MSG_TIYID` banner.
    ///
    /// Returns [`WireError::UnsupportedTransport`] when the server advertises
    /// HTTP-tunnel mode (`pser`), and [`WireError::UnknownBanner`] otherwise.
    pub fn from_banner(banner: &[u8; 4]) -> Result<ByteOrder> {
        match banner {
            b"ryit" => Ok(ByteOrder::Little),
            b"tiyr" => Ok(ByteOrder::Big),
            b"pser" => Err(WireError::UnsupportedTransport { banner: *banner }),
            _ => Err(WireError::UnknownBanner { banner: *banner }),
        }
    }

    /// The four banner bytes a server using this order sends.
    pub const fn banner(self) -> &'static [u8; 4] {
        match self {
            ByteOrder::Little => b"ryit",
            ByteOrder::Big => b"tiyr",
        }
    }
}

/// A bounds-checked cursor over a byte slice that reads integers using a fixed
/// [`ByteOrder`].
///
/// Every method returns [`Result`]; there are no panicking paths. This is the
/// only place in the crate where integer endianness is interpreted.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    order: ByteOrder,
}

impl<'a> Reader<'a> {
    /// Create a reader over `buf` that interprets integers as `order`.
    pub fn new(buf: &'a [u8], order: ByteOrder) -> Self {
        Reader { buf, pos: 0, order }
    }

    /// The byte order this reader uses.
    pub fn order(&self) -> ByteOrder {
        self.order
    }

    /// Bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bytes still available.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// The not-yet-consumed slice.
    pub fn remaining_slice(&self) -> &'a [u8] {
        &self.buf[self.pos.min(self.buf.len())..]
    }

    /// True when every byte has been consumed.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(WireError::UnexpectedEof {
                needed: n,
                available: self.remaining(),
            });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Read exactly `n` bytes.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }

    /// Skip `n` bytes (used for padding and reserved fields).
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    /// Read a single unsigned byte.
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Read a little/big-endian `u16`.
    pub fn read_u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(match self.order {
            ByteOrder::Little => u16::from_le_bytes([b[0], b[1]]),
            ByteOrder::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    /// Read a little/big-endian `i16`.
    pub fn read_i16(&mut self) -> Result<i16> {
        Ok(self.read_u16()? as i16)
    }

    /// Read a little/big-endian `u32`.
    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(match self.order {
            ByteOrder::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            ByteOrder::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    /// Read a little/big-endian `i32`.
    pub fn read_i32(&mut self) -> Result<i32> {
        Ok(self.read_u32()? as i32)
    }

    /// Read a fixed 32-byte `Str31`: one length byte followed by the string,
    /// NUL padded. Bytes past `length` are discarded. Decoded as Latin-1.
    pub fn read_str31(&mut self) -> Result<String> {
        let field = self.take(32)?;
        let len = (field[0] as usize).min(31);
        Ok(latin1(&field[1..1 + len]))
    }

    /// Read a `PString`: one length byte followed by that many string bytes.
    /// Decoded as Latin-1.
    pub fn read_pstring(&mut self) -> Result<String> {
        let len = self.read_u8()? as usize;
        let bytes = self.take(len)?;
        Ok(latin1(bytes))
    }

    /// Read a `PString` whose total length (length byte included) is padded up
    /// to a multiple of four. The padding is discarded, and — importantly — its
    /// *contents are not validated*: the live pserver emits uninitialised bytes
    /// there (observed `00 6c 65` after `"Rico"`), and the protocol reference
    /// only promises that padding exists, not that it is zero.
    pub fn read_pstring_aligned(&mut self) -> Result<String> {
        let total = self.read_pstring_len_only()?;
        let name = latin1(self.take(total.name_len)?);
        self.skip(total.pad)?;
        Ok(name)
    }

    /// Read the length byte of an aligned `PString` and compute the padding,
    /// without consuming the string itself.
    ///
    /// Returns `(name_len, pad)` where `name_len + 1 + pad` is a multiple of 4.
    pub fn read_pstring_len_only(&mut self) -> Result<AlignedPString> {
        let name_len = self.read_u8()? as usize;
        let padded_total = round_up_4(name_len + 1);
        let pad = padded_total - (name_len + 1);
        Ok(AlignedPString { name_len, pad })
    }

    /// Read a NUL-terminated string (the terminator is consumed, not kept).
    pub fn read_cstring(&mut self) -> Result<String> {
        let start = self.pos;
        let mut end = start;
        while end < self.buf.len() && self.buf[end] != 0 {
            end += 1;
        }
        if end >= self.buf.len() {
            return Err(WireError::UnexpectedEof {
                needed: (end - start) + 1,
                available: self.remaining(),
            });
        }
        self.pos = end + 1; // consume terminator
        Ok(latin1(&self.buf[start..end]))
    }
}

/// Parts of an aligned `PString`, as produced by
/// [`Reader::read_pstring_len_only`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedPString {
    /// Number of string bytes (excluding the length byte).
    pub name_len: usize,
    /// Number of padding bytes that follow the string.
    pub pad: usize,
}

/// Round `n` up to the next multiple of four.
pub const fn round_up_4(n: usize) -> usize {
    (n + 3) & !3
}

/// Decode bytes as ISO-8859-1 (Latin-1), the charset Palace uses for names and
/// short strings. Every byte maps to the code point of the same value, so this
/// is lossless and never fails.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// A growable buffer that writes integers using a fixed [`ByteOrder`].
#[derive(Debug, Clone)]
pub struct Writer {
    buf: Vec<u8>,
    order: ByteOrder,
}

impl Writer {
    /// Create an empty writer with the given byte order.
    pub fn new(order: ByteOrder) -> Self {
        Writer {
            buf: Vec::new(),
            order,
        }
    }

    /// Create an empty writer with pre-allocated capacity.
    pub fn with_capacity(order: ByteOrder, capacity: usize) -> Self {
        Writer {
            buf: Vec::with_capacity(capacity),
            order,
        }
    }

    /// The byte order this writer uses.
    pub fn order(&self) -> ByteOrder {
        self.order
    }

    /// Number of bytes written so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True when nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Borrow the written bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Take ownership of the written bytes.
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    /// Append raw bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Append a single byte.
    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Append a `u16` in the configured byte order.
    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&match self.order {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        });
    }

    /// Append an `i16` in the configured byte order.
    pub fn write_i16(&mut self, v: i16) {
        self.write_u16(v as u16);
    }

    /// Append a `u32` in the configured byte order.
    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&match self.order {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        });
    }

    /// Append an `i32` in the configured byte order.
    pub fn write_i32(&mut self, v: i32) {
        self.write_u32(v as u32);
    }

    /// Write `s` into a fixed 32-byte `Str31` field: a length byte, then the
    /// bytes (Latin-1), then NUL padding. Bytes beyond 31 are truncated.
    pub fn write_str31(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(31);
        self.buf.push(n as u8);
        self.buf.extend_from_slice(&bytes[..n]);
        self.buf.resize(self.buf.len() + (31 - n), 0);
    }

    /// Write a `PString`: a length byte followed by the bytes (Latin-1).
    /// Panics only if `s` is longer than 255 bytes, which callers avoid by
    /// construction (Palace strings are `Str31`-sized or smaller).
    pub fn write_pstring(&mut self, s: &str) {
        debug_assert!(s.len() <= 255, "PString length must fit in a u8");
        let n = s.len().min(255);
        self.buf.push(n as u8);
        self.buf.extend_from_slice(&s.as_bytes()[..n]);
    }

    /// Write a `PString` whose total length (length byte included) is padded to
    /// a multiple of four. Padding bytes are zero.
    pub fn write_pstring_aligned(&mut self, s: &str) {
        let n = s.len().min(255);
        self.buf.push(n as u8);
        self.buf.extend_from_slice(&s.as_bytes()[..n]);
        let total = n + 1;
        let pad = round_up_4(total) - total;
        self.buf.resize(self.buf.len() + pad, 0);
    }

    /// Write a NUL-terminated string.
    pub fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_detection_matches_reference_implementations() {
        // A little-endian server writes MSG_TIYID (0x74697972) as `ryit`.
        assert_eq!(ByteOrder::from_banner(b"ryit").unwrap(), ByteOrder::Little);
        // A big-endian server writes the same value as `tiyr`.
        assert_eq!(ByteOrder::from_banner(b"tiyr").unwrap(), ByteOrder::Big);
        // `pser` is HTTP-tunnel mode.
        assert!(matches!(
            ByteOrder::from_banner(b"pser"),
            Err(WireError::UnsupportedTransport { .. })
        ));
        assert!(matches!(
            ByteOrder::from_banner(b"nope"),
            Err(WireError::UnknownBanner { .. })
        ));
    }

    #[test]
    fn integers_round_trip_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_u8(0x12);
            w.write_u16(0x3456);
            w.write_i16(-2);
            w.write_u32(0x789a_bcde);
            w.write_i32(-123_456);
            let bytes = w.into_vec();

            let mut r = Reader::new(&bytes, order);
            assert_eq!(r.read_u8().unwrap(), 0x12);
            assert_eq!(r.read_u16().unwrap(), 0x3456);
            assert_eq!(r.read_i16().unwrap(), -2);
            assert_eq!(r.read_u32().unwrap(), 0x789a_bcde);
            assert_eq!(r.read_i32().unwrap(), -123_456);
            assert!(r.is_empty());
        }
    }

    #[test]
    fn str31_and_pstrings() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_str31("Rico");
        w.write_pstring("Balamb");
        w.write_pstring_aligned("Rico");
        let bytes = w.into_vec();
        assert_eq!(bytes.len(), 32 + 7 + 8);

        let mut r = Reader::new(&bytes, ByteOrder::Little);
        assert_eq!(r.read_str31().unwrap(), "Rico");
        assert_eq!(r.read_pstring().unwrap(), "Balamb");
        assert_eq!(r.read_pstring_aligned().unwrap(), "Rico");
        assert!(r.is_empty());
    }

    #[test]
    fn aligned_pstring_tolerates_garbage_padding() {
        // Live pserver emits uninitialised padding after a user name.
        let mut buf = vec![4u8, b'R', b'i', b'c', b'o', 0x00, 0x6c, 0x65];
        buf.push(0); // trailing so alignment math has room
        let mut r = Reader::new(&buf, ByteOrder::Little);
        assert_eq!(r.read_pstring_aligned().unwrap(), "Rico");
        assert_eq!(r.remaining(), 1);
    }

    #[test]
    fn eof_is_an_error_not_a_panic() {
        let mut r = Reader::new(&[1, 2], ByteOrder::Little);
        assert!(matches!(
            r.read_u32(),
            Err(WireError::UnexpectedEof {
                needed: 4,
                available: 2
            })
        ));
    }
}

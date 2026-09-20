//! The per-record half of the model: the raw 12-byte blob header, the pixel
//! encoding it selects, the record identity `(id, crc)` and the record itself.
//!
//! The blob header and the encoding are deliberately kept next to each other:
//! the *encoding* is not stored on disk anywhere, it is a property of the blob
//! header's flag word (spec §3.1), so it must be recomputed from the preserved
//! bytes whenever it is needed. Recomputing it from the codec's own selector in
//! [`crate::header`] — rather than re-deriving the flag rules here — keeps a single
//! source of truth for the 16-bit special case and the format-bit order.

use std::cmp::Ordering;

use crate::error::{PropError, Result};
use crate::header::{self, PropEndian, PropFormat};

use super::model::AssetRec;

/// Bytes in a serialised [`PropHeader`]. Reuses the codec's constant so the two
/// header views cannot disagree about their length.
pub const PROP_HEADER_LEN: usize = header::HEADER_LEN;

/// The 12-byte prop blob header as stored in a `.prp` data region (spec §3.1).
///
/// Six little-endian `s16` words. This is the **on-disk** view only: a roster
/// blob header is always little-endian, so there is no byte-order sniff here.
/// (A loose prop is sniffed per blob; that is [`crate::header::PropHeader`], which
/// also carries the sniffed endianness and is 14 bytes.) Interpreting the flags is
/// delegated to the codec via [`Self::codec_header`].
///
/// The header is part of the blob: [`Self::encode`] reproduces the exact 12 bytes,
/// which is what makes byte-exact rewriting possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct PropHeader {
    /// Width in pixels.
    pub width: i16,
    /// Height in pixels.
    pub height: i16,
    /// Horizontal origin offset (signed; may be negative).
    pub h_offset: i16,
    /// Vertical origin offset (signed; may be negative).
    pub v_offset: i16,
    /// Byte offset of the prop's IptScrae script; 0 in practice.
    pub script_offset: i16,
    /// The prop's flag word. Bit 0x0200 selects S20; `0x0002`/`0x0004` are
    /// HEAD/GHOST. This is the prop's flag word — distinct from
    /// [`AssetRec::flags`], which is the server's runtime state.
    pub flags: u16,
}

impl PropHeader {
    /// Read the six little-endian words at the start of `bytes`.
    ///
    /// Fails only when fewer than 12 bytes are available; no value of `flags` or
    /// of the offsets is invalid at this level.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < PROP_HEADER_LEN {
            return Err(PropError::HeaderTooShort {
                available: bytes.len(),
            });
        }
        let word = |at: usize| -> i16 { i16::from_le_bytes([bytes[at], bytes[at + 1]]) };
        Ok(Self {
            width: word(0),
            height: word(2),
            h_offset: word(4),
            v_offset: word(6),
            script_offset: word(8),
            flags: word(10) as u16,
        })
    }

    /// Serialise back to the exact 12 little-endian bytes stored on disk.
    #[must_use]
    pub fn encode(&self) -> [u8; PROP_HEADER_LEN] {
        let words = [
            self.width,
            self.height,
            self.h_offset,
            self.v_offset,
            self.script_offset,
            self.flags as i16,
        ];
        let mut out = [0u8; PROP_HEADER_LEN];
        for (i, word) in words.iter().enumerate() {
            let bytes = word.to_le_bytes();
            out[i * 2] = bytes[0];
            out[i * 2 + 1] = bytes[1];
        }
        out
    }

    /// The codec's semantic header for these fields, as a little-endian prop.
    ///
    /// Handing the fields to [`crate::header::PropHeader`] means the flag rules
    /// (the 16-bit special case, the format-bit order, HEAD/GHOST) are the
    /// codec's, which are tested against the corpus; this module owns none of them.
    #[must_use]
    pub fn codec_header(self) -> header::PropHeader {
        header::PropHeader {
            width: self.width,
            height: self.height,
            h_offset: self.h_offset,
            v_offset: self.v_offset,
            script_offset: self.script_offset,
            flags: self.flags,
            endian: PropEndian::Little,
        }
    }

    /// The pixel encoding `flags` selects.
    #[must_use]
    pub fn encoding(self) -> PropEncoding {
        PropEncoding::from_format(self.codec_header().format())
    }

    /// `HEAD 0x02`.
    #[must_use]
    pub const fn is_head(self) -> bool {
        self.flags & header::FLAG_HEAD != 0
    }

    /// `GHOST 0x04`.
    #[must_use]
    pub const fn is_ghost(self) -> bool {
        self.flags & header::FLAG_GHOST != 0
    }
}

/// The five pixel encodings a prop blob can carry, selected by its header flags.
///
/// This is the `.prp`-local name for the codec's [`PropFormat`], kept as a
/// separate type so the roster model does not force callers to import codec
/// internals, while the selection itself is always delegated to the codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropEncoding {
    /// Nibble run-length, palette-indexed, uncompressed.
    EightBit,
    /// 5-5-5-1, zlib-compressed.
    SixteenBit,
    /// 6-6-6-2, zlib-compressed.
    TwentyBit,
    /// 5-5-5-5, zlib-compressed.
    S20Bit,
    /// 4-byte RGBA, zlib-compressed.
    ThirtyTwoBit,
}

impl PropEncoding {
    /// The encoding a codec [`PropFormat`] corresponds to.
    #[must_use]
    pub const fn from_format(format: PropFormat) -> Self {
        match format {
            PropFormat::EightBit => PropEncoding::EightBit,
            PropFormat::SixteenBit => PropEncoding::SixteenBit,
            PropFormat::TwentyBit => PropEncoding::TwentyBit,
            PropFormat::S20Bit => PropEncoding::S20Bit,
            PropFormat::ThirtyTwoBit => PropEncoding::ThirtyTwoBit,
        }
    }

    /// Select the encoding for a raw flag word.
    ///
    /// Only `flags` takes part in selection, so a placeholder header is enough to
    /// reach the codec's selector; the order of its tests (16-bit first) is the
    /// part that must not be duplicated here.
    #[must_use]
    pub fn from_flags(flags: u16) -> Self {
        let placeholder = PropHeader {
            flags,
            ..PropHeader::default()
        };
        placeholder.encoding()
    }

    /// A stable lowercase name, matching [`PropFormat::name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            PropEncoding::EightBit => "8-bit",
            PropEncoding::SixteenBit => "16-bit",
            PropEncoding::TwentyBit => "20-bit",
            PropEncoding::S20Bit => "s20-bit",
            PropEncoding::ThirtyTwoBit => "32-bit",
        }
    }

    /// Whether the pixel payload is wrapped in zlib (everything but 8-bit).
    #[must_use]
    pub const fn is_zlib(self) -> bool {
        !matches!(self, PropEncoding::EightBit)
    }
}

/// The identity of a prop record: **`(id, crc)`**.
///
/// The id alone is **not** unique. The server's `FindAssetRecWithCRC` walks a run
/// of records that share an `id` and picks the one whose `crc` matches, so a
/// single id may legally carry several records (a "crc-variant chain", spec §5).
/// Keeping both parts in the key is what makes a record addressable, and the
/// ordering below keeps each run adjacent: sort by signed id, then by crc.
///
/// Ordering is by **signed** `id` because the server binary-searches with a signed
/// compare; comparing the ids as unsigned silently scrambles real rosters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PropKey {
    /// The prop id, compared signed.
    pub id: i32,
    /// The payload CRC, which distinguishes variants of one id.
    pub crc: u32,
}

impl PropKey {
    /// Build a key.
    #[must_use]
    pub const fn new(id: i32, crc: u32) -> Self {
        Self { id, crc }
    }
}

impl Ord for PropKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id
            .cmp(&other.id)
            .then_with(|| self.crc.cmp(&other.crc))
    }
}

impl PartialOrd for PropKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One roster record: the on-disk [`AssetRec`], the raw blob, and what the blob
/// says about itself.
///
/// The raw bytes are kept untouched so the writer (task T8) can reproduce them
/// exactly without re-encoding anything; `header` and `encoding` are derived from
/// those bytes for convenience and are `None` when the blob is shorter than the
/// 12-byte header (the `Fave` sentinel has `data_size == 0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropRecord {
    /// The 32-byte on-disk record, preserved verbatim.
    pub rec: AssetRec,
    /// The blob's raw 12-byte header, when the blob is at least 12 bytes long.
    pub header: Option<PropHeader>,
    /// The encoding selected by the blob header's flags, when a header is present.
    pub encoding: Option<PropEncoding>,
    /// The blob exactly as stored: `data_size` bytes, 12-byte header included.
    pub blob: Vec<u8>,
    /// The name resolved through `name_offset`; `None` when unnamed or unresolved.
    pub name: Option<String>,
}

impl PropRecord {
    /// This record's identity.
    #[must_use]
    pub fn key(&self) -> PropKey {
        PropKey::new(self.rec.id, self.rec.crc)
    }

    /// The prop id (signed).
    #[must_use]
    pub fn id(&self) -> i32 {
        self.rec.id
    }

    /// The payload CRC.
    #[must_use]
    pub fn crc(&self) -> u32 {
        self.rec.crc
    }

    /// The prop's own flag word, from the blob header. Zero for a headerless
    /// blob. This is **not** [`AssetRec::flags`], which is runtime state.
    #[must_use]
    pub fn prop_flags(&self) -> u16 {
        self.header.map_or(0, |h| h.flags)
    }
}

/// Longest name entry the one-byte length prefix can describe.
pub const MAX_NAME_LEN: usize = u8::MAX as usize;

/// Re-encode a decoded name to the bytes the format stores: **latin-1**, one
/// byte per code point. This is the exact inverse of the reader's latin-1 decode,
/// so a parsed name round-trips byte-for-byte. Code points above `0xFF` (which
/// cannot arise from a real file) are written as `?` rather than silently lost.
#[must_use]
pub(super) fn latin1_bytes(name: &str) -> Vec<u8> {
    name.chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect()
}

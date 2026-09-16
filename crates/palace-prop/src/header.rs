//! The 12-byte prop header, its byte order, its flag bits and the format mask.
//!
//! Every prop — avatar, loose prop, deco — is a 12-byte header followed by a
//! pixel payload. The header is six signed 16-bit words:
//!
//! ```text
//! offset  field          type   meaning
//!   0     width          s16    pixels; 44 for every prop in the corpus
//!   2     height         s16    pixels; 44 in every prop in the corpus
//!   4     h_offset       s16    horizontal origin offset, may be negative
//!   6     v_offset       s16    vertical origin offset, may be negative
//!   8     script_offset  s16    byte offset of this prop's IptScrae script, 0 in practice
//!  10     flags          u16    format selector + HEAD/GHOST/RARE/ANIMATE/BOUNCE
//! ```
//!
//! The two most error-prone parts of the format live here, so both are pinned by
//! tests: the endianness sniff and the order in which the format bits are tested.

use crate::error::{PropError, Result, MAX_DIMENSION};

/// Length of the fixed prop header.
pub const HEADER_LEN: usize = 12;

/// Byte order of a prop's header.
///
/// Palace sessions negotiate a byte order once (see `palace-wire`), but props do
/// **not** inherit it: each prop is sniffed on its own. A prop is little-endian
/// when `data[1] == 0`, big-endian otherwise. Because `data[1]` is the high byte
/// of `width` in a little-endian prop and 44 does not fit in a byte, this test is
/// unambiguous for every real prop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropEndian {
    /// `data[1] == 0`.
    Little,
    /// `data[1] != 0`.
    Big,
}

impl PropEndian {
    /// Sniff the byte order from the first two bytes, before any field is read.
    #[must_use]
    pub fn sniff(data: &[u8]) -> Option<Self> {
        data.get(1).map(|b| {
            if *b == 0 {
                PropEndian::Little
            } else {
                PropEndian::Big
            }
        })
    }
}

/// `HEAD 0x02` — the prop is worn on the head.
pub const FLAG_HEAD: u16 = 0x0002;
/// `GHOST 0x04` — the prop draws with a ghosting effect.
pub const FLAG_GHOST: u16 = 0x0004;
/// `RARE 0x08` — the prop is marked rare in the prop bag.
pub const FLAG_RARE: u16 = 0x0008;
/// `ANIMATE 0x10` — the prop animates.
pub const FLAG_ANIMATE: u16 = 0x0010;
/// `BOUNCE 0x20` — also spelled `PALINDROME` in the reference sources.
pub const FLAG_BOUNCE: u16 = 0x0020;

/// `20BIT 0x0040` — 6-6-6-2 20-bit payload.
pub const FLAG_FORMAT_20BIT: u16 = 0x0040;
/// `32BIT 0x0100` — 4-byte RGBA payload.
pub const FLAG_FORMAT_32BIT: u16 = 0x0100;
/// `S20BIT 0x0200` — 5-5-5-5 20-bit payload.
pub const FLAG_FORMAT_S20BIT: u16 = 0x0200;

/// The three bits that select a pixel format.
pub const FORMAT_MASK: u16 = FLAG_FORMAT_20BIT | FLAG_FORMAT_32BIT | FLAG_FORMAT_S20BIT;

/// Mask used for the 16-bit special case. It keeps every "original palace prop
/// flag" bit and discards the format bits, which are all set on a 16-bit prop.
pub const SIXTEEN_BIT_MASK: u16 = 0xFFC1;

/// The value `flags & SIXTEEN_BIT_MASK` takes on a 16-bit prop.
pub const SIXTEEN_BIT_PATTERN: u16 = 0xFF80;

/// Which of the five pixel encodings a prop uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropFormat {
    /// Nibble run-length, palette-indexed, uncompressed.
    EightBit,
    /// 5-5-5-1, zlib-compressed.
    SixteenBit,
    /// 6-6-6-2, zlib-compressed.
    TwentyBit,
    /// 5-5-5-5, zlib-compressed. The only format the reference client emits.
    S20Bit,
    /// 4-byte RGBA, zlib-compressed.
    ThirtyTwoBit,
}

impl PropFormat {
    /// A stable lowercase name, used in error messages and corpus reports.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PropFormat::EightBit => "8-bit",
            PropFormat::SixteenBit => "16-bit",
            PropFormat::TwentyBit => "20-bit",
            PropFormat::S20Bit => "s20-bit",
            PropFormat::ThirtyTwoBit => "32-bit",
        }
    }

    /// Whether the pixel payload is wrapped in zlib.
    ///
    /// The 8-bit payload is raw RLE. This is the single most surprising fact
    /// about the format and it is verified against the whole corpus: see the
    /// crate README.
    #[must_use]
    pub fn is_zlib(self) -> bool {
        !matches!(self, PropFormat::EightBit)
    }

    /// Bytes of decompressed payload consumed per pixel, when uniform.
    ///
    /// `None` for the two 2.5-byte formats, which are handled two pixels at a
    /// time by [`PropFormat::payload_bytes_for`].
    #[must_use]
    pub fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            PropFormat::EightBit => None,
            PropFormat::SixteenBit => Some(2),
            PropFormat::TwentyBit | PropFormat::S20Bit => None,
            PropFormat::ThirtyTwoBit => Some(4),
        }
    }

    /// Decompressed bytes needed to decode `pixels` pixels in this format.
    #[must_use]
    pub fn payload_bytes_for(self, pixels: usize) -> usize {
        match self {
            PropFormat::EightBit => 0,
            PropFormat::SixteenBit => pixels * 2,
            PropFormat::TwentyBit | PropFormat::S20Bit => pixels.div_ceil(2) * 5,
            PropFormat::ThirtyTwoBit => pixels * 4,
        }
    }
}

/// A parsed prop header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropHeader {
    /// Width in pixels.
    pub width: i16,
    /// Height in pixels.
    pub height: i16,
    /// Horizontal origin offset (signed).
    pub h_offset: i16,
    /// Vertical origin offset (signed).
    pub v_offset: i16,
    /// Byte offset of the prop's script, 0 in every prop observed.
    pub script_offset: i16,
    /// The raw flag word.
    pub flags: u16,
    /// The byte order the header was read with.
    pub endian: PropEndian,
}

impl PropHeader {
    /// Parse the 12-byte header at the start of `data`.
    ///
    /// Fails only on a short buffer or on dimensions that cannot describe an
    /// image; a bad format bit pattern is *not* an error, it simply decodes as
    /// 8-bit the way the reference client treats it.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_LEN {
            return Err(PropError::HeaderTooShort {
                available: data.len(),
            });
        }
        let endian = PropEndian::sniff(data).ok_or(PropError::HeaderTooShort {
            available: data.len(),
        })?;
        let word = |offset: usize| -> i16 {
            let lo = data[offset];
            let hi = data[offset + 1];
            match endian {
                PropEndian::Little => i16::from_le_bytes([lo, hi]),
                PropEndian::Big => i16::from_be_bytes([lo, hi]),
            }
        };
        let flags = word(10) as u16;
        let header = PropHeader {
            width: word(0),
            height: word(2),
            h_offset: word(4),
            v_offset: word(6),
            script_offset: word(8),
            flags,
            endian,
        };
        header.validate()?;
        Ok(header)
    }

    /// Reject dimensions that cannot describe a real image.
    pub fn validate(&self) -> Result<()> {
        if self.width <= 0
            || self.height <= 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
        {
            return Err(PropError::ImplausibleDimensions {
                width: self.width,
                height: self.height,
            });
        }
        Ok(())
    }

    /// `width * height` as a `u32`; the caller has already validated the
    /// dimensions, so this cannot overflow in practice, but it is checked anyway.
    pub fn pixel_count(&self) -> Result<u32> {
        u32::try_from(i32::from(self.width) * i32::from(self.height)).map_err(|_| {
            PropError::ImageTooLarge {
                width: self.width,
                height: self.height,
            }
        })
    }

    /// Which pixel encoding this header selects.
    ///
    /// The order of the tests is load-bearing and follows the reference client
    /// exactly:
    ///
    /// 1. `(flags & 0xFFC1) == 0xFF80` — the bizarre 16-bit marker,
    /// 2. `flags & 0x0200` — S20,
    /// 3. `flags & 0x0100` — 32-bit,
    /// 4. `flags & 0x0040` — 20-bit,
    /// 5. otherwise — 8-bit.
    ///
    /// Testing in any other order misclassifies a 16-bit prop, whose flag word
    /// has *all three* format bits set.
    #[must_use]
    pub fn format(&self) -> PropFormat {
        if self.flags & SIXTEEN_BIT_MASK == SIXTEEN_BIT_PATTERN {
            PropFormat::SixteenBit
        } else if self.flags & FLAG_FORMAT_S20BIT != 0 {
            PropFormat::S20Bit
        } else if self.flags & FLAG_FORMAT_32BIT != 0 {
            PropFormat::ThirtyTwoBit
        } else if self.flags & FLAG_FORMAT_20BIT != 0 {
            PropFormat::TwentyBit
        } else {
            PropFormat::EightBit
        }
    }

    /// `HEAD 0x02`.
    #[must_use]
    pub fn is_head(&self) -> bool {
        self.flags & FLAG_HEAD != 0
    }

    /// `GHOST 0x04`.
    #[must_use]
    pub fn is_ghost(&self) -> bool {
        self.flags & FLAG_GHOST != 0
    }

    /// `RARE 0x08`.
    #[must_use]
    pub fn is_rare(&self) -> bool {
        self.flags & FLAG_RARE != 0
    }

    /// `ANIMATE 0x10`.
    #[must_use]
    pub fn is_animate(&self) -> bool {
        self.flags & FLAG_ANIMATE != 0
    }

    /// `BOUNCE 0x20` (also called `PALINDROME`).
    #[must_use]
    pub fn is_bounce(&self) -> bool {
        self.flags & FLAG_BOUNCE != 0
    }

    /// Serialise the header back to 12 bytes in its own byte order.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        let fields = [
            self.width,
            self.height,
            self.h_offset,
            self.v_offset,
            self.script_offset,
            self.flags as i16,
        ];
        for (i, value) in fields.iter().enumerate() {
            let bytes = match self.endian {
                PropEndian::Little => value.to_le_bytes(),
                PropEndian::Big => value.to_be_bytes(),
            };
            out[i * 2] = bytes[0];
            out[i * 2 + 1] = bytes[1];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: i16, height: i16, flags: u16, endian: PropEndian) -> Vec<u8> {
        PropHeader {
            width,
            height,
            h_offset: 0,
            v_offset: 0,
            script_offset: 0,
            flags,
            endian,
        }
        .encode()
        .to_vec()
    }

    #[test]
    fn a_real_8_bit_header_round_trips() {
        // First prop in props_harvested/1001438438_unnamed.bin:
        // 2c 00 2c 00 07 00 07 00 00 00 0a 00
        let raw = [
            0x2c, 0x00, 0x2c, 0x00, 0x07, 0x00, 0x07, 0x00, 0x00, 0x00, 0x0a, 0x00,
        ];
        let h = PropHeader::parse(&raw).unwrap();
        assert_eq!(h.width, 44);
        assert_eq!(h.height, 44);
        assert_eq!(h.h_offset, 7);
        assert_eq!(h.v_offset, 7);
        assert_eq!(h.script_offset, 0);
        assert_eq!(h.flags, 0x000a);
        assert_eq!(h.endian, PropEndian::Little);
        assert_eq!(h.format(), PropFormat::EightBit);
        assert!(h.is_head());
        assert!(h.is_rare());
        assert!(!h.is_ghost());
        assert_eq!(h.encode(), raw);
    }

    #[test]
    fn endianness_is_sniffed_from_the_second_byte_alone() {
        assert_eq!(PropEndian::sniff(&[0x2c, 0x00]), Some(PropEndian::Little));
        assert_eq!(PropEndian::sniff(&[0x00, 0x2c]), Some(PropEndian::Big));
        assert_eq!(PropEndian::sniff(&[0x2c]), None);
    }

    #[test]
    fn a_big_endian_header_decodes_its_offsets_signed() {
        // width 44, height 44, hOffset -44 (0xffd4), vOffset 0, script 0, flags 0x0200
        let raw = [
            0x00, 0x2c, 0x00, 0x2c, 0xff, 0xd4, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00,
        ];
        let h = PropHeader::parse(&raw).unwrap();
        assert_eq!(h.endian, PropEndian::Big);
        assert_eq!((h.width, h.height), (44, 44));
        assert_eq!(h.h_offset, -44);
        assert_eq!(h.format(), PropFormat::S20Bit);
    }

    #[test]
    fn format_selection_order_matters() {
        // 16-bit wins even though all three format bits are set.
        let h = PropHeader::parse(&header(44, 44, 0xff80, PropEndian::Little)).unwrap();
        assert_eq!(h.flags & FORMAT_MASK, 0x0300);
        assert_eq!(h.format(), PropFormat::SixteenBit);

        assert_eq!(
            PropHeader::parse(&header(44, 44, 0x0340, PropEndian::Little))
                .unwrap()
                .format(),
            PropFormat::S20Bit
        );
        assert_eq!(
            PropHeader::parse(&header(44, 44, 0x0100, PropEndian::Little))
                .unwrap()
                .format(),
            PropFormat::ThirtyTwoBit
        );
        assert_eq!(
            PropHeader::parse(&header(44, 44, 0x0042, PropEndian::Little))
                .unwrap()
                .format(),
            PropFormat::TwentyBit
        );
        assert_eq!(
            PropHeader::parse(&header(44, 44, 0x0002, PropEndian::Little))
                .unwrap()
                .format(),
            PropFormat::EightBit
        );
    }

    #[test]
    fn sixteen_bit_pattern_tolerates_every_original_flag_bit() {
        for extra in [0x0000, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020, 0x003e] {
            let flags = 0xff80 | extra;
            let h = PropHeader::parse(&header(44, 44, flags, PropEndian::Little)).unwrap();
            assert_eq!(h.format(), PropFormat::SixteenBit, "flags {flags:#06x}");
        }
    }

    #[test]
    fn short_and_impossible_headers_are_errors_not_panics() {
        assert!(matches!(
            PropHeader::parse(&[0x2c, 0x00, 0x2c, 0x00]),
            Err(PropError::HeaderTooShort { available: 4 })
        ));
        assert!(matches!(
            PropHeader::parse(&header(0, 44, 0, PropEndian::Little)),
            Err(PropError::ImplausibleDimensions { .. })
        ));
        assert!(matches!(
            PropHeader::parse(&header(44, -1, 0, PropEndian::Little)),
            Err(PropError::ImplausibleDimensions { .. })
        ));
        assert!(matches!(
            PropHeader::parse(&header(44, 8000, 0, PropEndian::Little)),
            Err(PropError::ImplausibleDimensions { .. })
        ));
    }

    #[test]
    fn payload_sizes_per_format() {
        assert_eq!(PropFormat::SixteenBit.payload_bytes_for(1936), 3872);
        assert_eq!(PropFormat::TwentyBit.payload_bytes_for(1936), 4840);
        assert_eq!(PropFormat::S20Bit.payload_bytes_for(1936), 4840);
        assert_eq!(PropFormat::ThirtyTwoBit.payload_bytes_for(1936), 7744);
        // Odd pixel counts round up to a whole 5-byte group.
        assert_eq!(PropFormat::S20Bit.payload_bytes_for(3), 10);
    }
}

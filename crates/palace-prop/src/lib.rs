//! `palace-prop` — the Palace prop (sprite) codec.
//!
//! A "prop" is the small sprite that makes up an avatar, a loose prop or a deco
//! object in Palace, the 1990s avatar chat system. The format is undocumented;
//! this crate is a Rust port of the reverse-engineered behaviour in OpenPalace's
//! `PalaceProp.as`, cross-checked against Taj, `prop_decoder.py` and a corpus of
//! ~228,000 real props.
//!
//! ## The format in one paragraph
//!
//! A prop is a 12-byte header of six signed 16-bit words (`width`, `height`,
//! `hOffset`, `vOffset`, `scriptOffset`, `flags`) followed by a pixel payload in
//! one of five encodings. The header's byte order is sniffed per prop: it is
//! little-endian when `data[1] == 0`, big-endian otherwise. The format is selected
//! by flag bits, with a bizarre special case for 16-bit props. The 8-bit payload
//! is raw nibble run-length over a 256-entry palette; the other four are
//! zlib-compressed. The whole thing is read by [`decode`], which never panics on
//! protocol data.
//!
//! ## Quick start
//!
//! ```
//! use palace_prop::{decode, encode_s20_blob, PropFormat};
//!
//! // A 4x1 8-bit prop: width 4, height 1, offsets 0, script 0, flags 0.
//! let mut blob = vec![4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
//! blob.extend_from_slice(&[0x04, 0x01, 0x01, 0x01, 0x01]);
//!
//! let prop = decode(&blob).unwrap();
//! assert_eq!(prop.format(), PropFormat::EightBit);
//! assert_eq!((prop.image.width(), prop.image.height()), (4, 1));
//! assert_eq!(prop.header.is_head(), false);
//!
//! // Round-trip through the S20 encoder the reference client uses.
//! let s20 = encode_s20_blob(&prop.image, 0, 0, 0).unwrap();
//! let again = decode(&s20).unwrap();
//! assert_eq!(again.format(), PropFormat::S20Bit);
//! ```
//!
//! ## Confidence, per format
//!
//! | Format | Decoder confidence | Validated against |
//! |---|---|---|
//! | 8-bit | **High** | 227,358 corpus props; pixel-exact vs `prop_decoder.py` |
//! | S20-bit | **High** | 387 corpus props; encoder/decoder round-trip proof |
//! | 32-bit | **High** (layout) / documented loop-bound change | 2 corpus props |
//! | 20-bit | **Medium-high** | 127 corpus props; self-consistent bit tiling |
//! | 16-bit | **Medium** | no real props exist locally; reference source + fixtures |
//!
//! The full derivation, byte offsets and corpus counts are in the crate README.
//!
//! ## What this crate is not
//!
//! It knows nothing about the `.prp` roster container, the wire protocol or the
//! network. Props are a self-contained binary format, so this crate has no
//! dependency on `palace-wire` — only on `flate2` for the zlib step and `png` for
//! debug output.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod codec;
pub mod crc;
pub mod encode;
pub mod error;
pub mod header;
pub mod image;
pub mod palette;

pub use crc::{asset_crc, payload_crc, ASSET_CRC_MAGIC};
pub use encode::{encode_s20_blob, encode_s20_payload, quantize};
pub use error::{PropError, Result, MAX_DIMENSION, MAX_PIXELS};
pub use header::{
    PropEndian, PropFormat, PropHeader, FLAG_ANIMATE, FLAG_BOUNCE, FLAG_FORMAT_20BIT,
    FLAG_FORMAT_32BIT, FLAG_FORMAT_S20BIT, FLAG_GHOST, FLAG_HEAD, FLAG_RARE, FORMAT_MASK,
    HEADER_LEN, SIXTEEN_BIT_MASK, SIXTEEN_BIT_PATTERN,
};
pub use image::PropImage;

/// A decoded prop: its header plus its pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    /// The parsed 12-byte header.
    pub header: PropHeader,
    /// The decoded image.
    pub image: PropImage,
}

impl Prop {
    /// Which of the five encodings this prop used.
    #[must_use]
    pub fn format(&self) -> PropFormat {
        self.header.format()
    }

    /// Re-encode the pixels as an S20 prop blob, preserving this prop's offsets
    /// and its head/ghost/rare/animate/bounce bits.
    pub fn encode_s20(&self) -> Result<Vec<u8>> {
        encode_s20_blob(
            &self.image,
            self.header.h_offset,
            self.header.v_offset,
            self.header.flags,
        )
    }
}

/// Decode a complete prop blob: the 12-byte header followed by the payload.
///
/// This is the only entry point most callers need. It never panics, whatever the
/// bytes contain: a malformed prop produces an [`PropError`], not a crash.
pub fn decode(data: &[u8]) -> Result<Prop> {
    let header = PropHeader::parse(data)?;
    let payload = data.get(HEADER_LEN..).unwrap_or(&[]);
    let image = codec::decode_payload(&header, payload)?;
    Ok(Prop { header, image })
}

/// Decode only the header, without touching the pixel payload.
///
/// Useful for inventorying a corpus: it is cheap and never fails on a payload.
pub fn decode_header(data: &[u8]) -> Result<PropHeader> {
    PropHeader::parse(data)
}

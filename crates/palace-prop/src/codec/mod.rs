//! The five pixel decoders, plus the shared zlib step.
//!
//! Every decoder takes the payload *after* the 12-byte header and returns a
//! [`PropImage`]. None of them may panic: lengths are checked before every read
//! and every buffer index is derived from a validated dimension.
//!
//! ## What the reference does that we deliberately do not
//!
//! The OpenPalace decoders are translated from 1990s C and VB6 by way of
//! REALBasic, and three of their behaviours are unsafe or silent rather than
//! wrong:
//!
//! * out-of-range reads fall back to ActionScript's `undefined` (0) instead of
//!   erroring, so a truncated 8-bit prop decodes to a partially-filled image;
//! * the 8-bit RLE loop can stall forever on a `0x00` control byte, which is why
//!   it carries a 6000-iteration runaway guard;
//! * the compressed decoders read `width*height` pixels from a buffer they never
//!   length-check.
//!
//! We return [`crate::error::PropError`] where the reference quietly produces a
//! short or blank image. Every such divergence is listed in the crate README
//! together with the corpus evidence.

pub mod eight;
pub mod s20;
pub mod sixteen;
pub mod thirtytwo;
pub mod twenty;

use std::io::Read;

use crate::error::{PropError, Result};
use crate::header::{PropFormat, PropHeader};
use crate::image::PropImage;

/// Extra decompressed bytes tolerated past what a format needs.
///
/// The reference ignores anything after the pixels it wants, so trailing padding
/// must not be an error. Four kilobytes is far more than any real prop carries
/// while still bounding a decompression bomb.
pub(crate) const INFLATE_SLACK: usize = 4096;

/// Decode the payload of a prop whose header has already been parsed.
pub fn decode_payload(header: &PropHeader, payload: &[u8]) -> Result<PropImage> {
    let format = header.format();
    let pixels = header.pixel_count()? as usize;
    if u64::try_from(pixels).unwrap_or(u64::MAX) > crate::error::MAX_PIXELS {
        return Err(PropError::ImageTooLarge {
            width: header.width,
            height: header.height,
        });
    }
    let width = u32::from(header.width.unsigned_abs());
    let height = u32::from(header.height.unsigned_abs());
    match format {
        PropFormat::EightBit => eight::decode(payload, width, height),
        PropFormat::SixteenBit => {
            let data = inflate(payload, format.payload_bytes_for(pixels), format)?;
            sixteen::decode(&data, width, height)
        }
        PropFormat::TwentyBit => {
            let data = inflate(payload, format.payload_bytes_for(pixels), format)?;
            twenty::decode(&data, width, height)
        }
        PropFormat::S20Bit => {
            let data = inflate(payload, format.payload_bytes_for(pixels), format)?;
            s20::decode(&data, width, height)
        }
        PropFormat::ThirtyTwoBit => {
            let data = inflate(payload, format.payload_bytes_for(pixels), format)?;
            thirtytwo::decode(&data, width, height)
        }
    }
}

/// Inflate a zlib-wrapped payload and require at least `needed` bytes.
pub(crate) fn inflate(payload: &[u8], needed: usize, format: PropFormat) -> Result<Vec<u8>> {
    let cap = needed.saturating_add(INFLATE_SLACK);
    let mut out = Vec::with_capacity(needed.min(1 << 20));
    // One byte past the cap: if that byte arrives, the payload really is longer
    // than we are willing to buffer; if it does not, the stream ended on its own.
    let mut decoder = flate2::read::ZlibDecoder::new(payload).take(cap.saturating_add(1) as u64);
    decoder.read_to_end(&mut out).map_err(|e| PropError::Zlib {
        detail: e.to_string(),
    })?;
    if out.len() > cap {
        return Err(PropError::DecompressedTooLarge {
            format: format.name(),
            limit: cap,
        });
    }
    if out.len() < needed {
        return Err(PropError::DecompressedTooShort {
            format: format.name(),
            expected: needed,
            actual: out.len(),
        });
    }
    Ok(out)
}

/// Scale an `n`-bit colour field to 8 bits the way the reference does.
///
/// The reference expresses this as `value * (255 / max)`, with `255 / max` an
/// ActionScript `Number` — i.e. floating point, truncated back to an integer by
/// `uint()`. That is exactly `value * 255 / max` in integer arithmetic, which is
/// what we do here. Note that the divisor must be applied *after* the multiply:
/// Taj's C# port instead folds `255 / 63` and `255 / 31` into integer constants
/// `4` and `8`, which maps a full-scale 31 to 248 and a full-scale 63 to 252.
/// The AS3 behaviour is the one that matches the C++ and VB6 originals, and the
/// S20 encoder in the same file is its exact inverse.
#[inline]
pub(crate) fn scale(value: u32, max: u32) -> u8 {
    ((value * 255) / max) as u8
}

/// Pack an A/R/G/B quad into the internal ARGB word order.
#[inline]
pub(crate) fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    (u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

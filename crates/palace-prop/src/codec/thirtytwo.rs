//! 32-bit: four bytes per pixel, plain RGBA.
//!
//! The simplest of the five once you know it exists: byte order is `R, G, B, A`,
//! row-major, top-down, no bit packing and no palette. Alpha is a full byte, which
//! makes this the only format that can express partial transparency.
//!
//! ## The 1936 loop count
//!
//! Both reference implementations hard-code the loop as `for (X = 0; X <= 1935;
//! X++)` — 1936 pixels, exactly 44×44, the size of the fixed render rectangle.
//! It is not derived from the header at all. We iterate `width * height` instead,
//! which is **byte-identical on every 44×44 prop** (all 180,661 in the corpus) and
//! strictly safer otherwise: the hard-coded 1936 would over-read the payload of a
//! prop whose header declares fewer pixels, and silently truncate one that
//! declares more. A payload that cannot supply `width * height` pixels is an
//! error here rather than a partially-garbage image.
//!
//! Confidence: **high** for the pixel layout, which is trivially self-evident and
//! matches two independent references; the only judgement call is the loop bound
//! documented above.

use crate::error::{PropError, Result};
use crate::image::PropImage;

/// Decode a 32-bit payload into a `width` x `height` image.
pub fn decode(data: &[u8], width: u32, height: u32) -> Result<PropImage> {
    let pixels = (width as usize) * (height as usize);
    let needed = pixels * 4;
    if data.len() < needed {
        return Err(PropError::DecompressedTooShort {
            format: "32-bit",
            expected: needed,
            actual: data.len(),
        });
    }
    PropImage::from_rgba(width, height, data[..needed].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_r_g_b_a_in_order() {
        let image = decode(&[1, 2, 3, 4], 1, 1).unwrap();
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
    }

    #[test]
    fn pixels_are_row_major() {
        let data = [1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0];
        let image = decode(&data, 2, 2).unwrap();
        assert_eq!(image.pixel(0, 0).unwrap()[0], 1);
        assert_eq!(image.pixel(1, 0).unwrap()[0], 2);
        assert_eq!(image.pixel(0, 1).unwrap()[0], 3);
        assert_eq!(image.pixel(1, 1).unwrap()[0], 4);
    }

    #[test]
    fn alpha_is_a_full_byte() {
        let image = decode(&[0, 0, 0, 128], 1, 1).unwrap();
        assert_eq!(image.pixel(0, 0), Some([0, 0, 0, 128]));
    }

    #[test]
    fn a_short_payload_is_an_error() {
        assert!(matches!(
            decode(&[0; 3], 1, 1),
            Err(PropError::DecompressedTooShort { .. })
        ));
    }
}

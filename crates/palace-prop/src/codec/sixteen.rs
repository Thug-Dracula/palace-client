//! 16-bit: two bytes per pixel, RGB555 plus a one-bit alpha.
//!
//! The flag word for a 16-bit prop has *every* format bit set (`flags & 0xFFC1 ==
//! 0xFF80`), so it must be tested before S20/32/20 or it is misclassified. This
//! is the rarest variant by far: the entire local corpus contains no genuine
//! 16-bit prop, so this decoder is validated against the reference source and
//! synthetic fixtures only. See the crate README.
//!
//! Per pixel, the 16-bit big-endian word is laid out:
//!
//! ```text
//! bit  15..11  10..6   5..1   0
//!      red     green   blue   alpha
//! ```
//!
//! i.e. `R = b0 >> 3`, `G = (word >> 6) & 31`, `B = (word >> 1) & 31`,
//! `A = word & 1`. Red is the only field that does not straddle the byte
//! boundary. Colour fields are scaled by `255 / 31`; the single alpha bit is
//! either fully transparent or fully opaque.
//!
//! The word is read **big-endian regardless of the header's endianness**, which
//! is what the reference does. It is harmless here because the corpus is
//! little-endian and contains no 16-bit prop to disagree about.

use crate::codec::{argb, scale};
use crate::error::{PropError, Result};
use crate::image::PropImage;

/// Decode a 16-bit payload into a `width` x `height` image.
pub fn decode(data: &[u8], width: u32, height: u32) -> Result<PropImage> {
    let pixels = (width as usize) * (height as usize);
    let needed = pixels * 2;
    if data.len() < needed {
        return Err(PropError::DecompressedTooShort {
            format: "16-bit",
            expected: needed,
            actual: data.len(),
        });
    }
    let mut words = Vec::with_capacity(pixels);
    for chunk in data[..needed].chunks_exact(2) {
        let word = (u32::from(chunk[0]) << 8) | u32::from(chunk[1]);
        let r = scale((u32::from(chunk[0]) >> 3) & 31, 31);
        let g = scale((word >> 6) & 31, 31);
        let b = scale((word >> 1) & 31, 31);
        let a = ((word & 1) * 255) as u8;
        words.push(argb(a, r, g, b));
    }
    Ok(PropImage::from_argb(width, height, &words))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(data: &[u8]) -> [u8; 4] {
        decode(data, 1, 1).unwrap().pixel(0, 0).unwrap()
    }

    #[test]
    fn full_scale_fields_reach_255() {
        // 0xffff: red 31, green 31, blue 31, alpha 1.
        assert_eq!(one(&[0xff, 0xff]), [255, 255, 255, 255]);
    }

    #[test]
    fn alpha_bit_zero_is_transparent() {
        // 0xfffe: same colour, alpha bit clear.
        assert_eq!(one(&[0xff, 0xfe]), [255, 255, 255, 0]);
    }

    #[test]
    fn red_is_the_high_five_bits_of_the_first_byte() {
        assert_eq!(one(&[0x08, 0x00]), [scale(1, 31), 0, 0, 0]);
    }

    #[test]
    fn green_straddles_the_byte_boundary() {
        assert_eq!(one(&[0x00, 0x40]), [0, scale(1, 31), 0, 0]);
    }

    #[test]
    fn blue_is_bit_one_of_the_second_byte() {
        assert_eq!(one(&[0x00, 0x02]), [0, 0, scale(1, 31), 0]);
    }

    #[test]
    fn a_short_payload_is_an_error() {
        assert!(matches!(
            decode(&[0x00], 1, 1),
            Err(PropError::DecompressedTooShort { .. })
        ));
    }

    #[test]
    fn every_word_decodes_to_one_pixel() {
        for word in [0x0000u16, 0x0001, 0x7bef, 0x8410, 0xfffe, 0xffff] {
            assert!(decode(&word.to_be_bytes(), 1, 1).is_ok());
        }
    }
}

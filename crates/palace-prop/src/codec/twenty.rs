//! 20-bit: 2.5 bytes per pixel, 6-6-6-2, two pixels per five bytes.
//!
//! The 40 bits of a five-byte group are allocated exactly, with no unused bits.
//! Each row below is one byte; the right column is the bit range of the *field*
//! that the byte's bits supply, so the two pixels tile the group with no overlap:
//!
//! ```text
//! byte  bits 7..0
//! b0    [7..2] red0[5..0]      [1..0] green0[5..4]
//! b1    [7..4] green0[3..0]    [3..0] blue0[5..2]
//! b2    [7..6] blue0[1..0]     [5..4] alpha0[1..0]   [3..0] red1[5..2]
//! b3    [7..6] red1[1..0]      [5..0] green1[5..0]
//! b4    [7..2] blue1[5..0]     [1..0] alpha1[1..0]
//! ```
//!
//! Colour fields are scaled by `255 / 63`; the two-bit alpha is scaled by 85, so
//! it can only be 0, 85, 170 or 255.
//!
//! Confidence in this decoder is **medium-high**: the field boundaries are
//! self-consistent (every one of the 40 bits is claimed exactly once, and the two
//! pixels tile the group), the port is faithful to OpenPalace, and it decodes all
//! 127 twenty-bit props in `pserver.prp` to sensible images. What is *not*
//! available is an encoder to invert, so unlike S20 there is no round-trip proof
//! of the bit order.

use crate::codec::{argb, scale};
use crate::error::{PropError, Result};
use crate::image::PropImage;

/// Decode a 20-bit payload into a `width` x `height` image.
pub fn decode(data: &[u8], width: u32, height: u32) -> Result<PropImage> {
    let pixels = (width as usize) * (height as usize);
    let needed = pixels.div_ceil(2) * 5;
    if data.len() < needed {
        return Err(PropError::DecompressedTooShort {
            format: "20-bit",
            expected: needed,
            actual: data.len(),
        });
    }
    let mut words = Vec::with_capacity(pixels);
    for group in data[..needed].chunks_exact(5) {
        let b0 = u32::from(group[0]);
        let b1 = u32::from(group[1]);
        let b2 = u32::from(group[2]);
        let b3 = u32::from(group[3]);
        let b4 = u32::from(group[4]);

        let red0 = scale((b0 >> 2) & 63, 63);
        let green0 = scale((((b0 << 8) | b1) >> 4) & 63, 63);
        let blue_alpha0 = (b1 << 8) | b2;
        let blue0 = scale((blue_alpha0 >> 6) & 63, 63);
        let alpha0 = (((blue_alpha0 >> 4) & 3) * 85) as u8;

        let red_green1 = (b2 << 8) | b3;
        let red1 = scale((red_green1 >> 6) & 63, 63);
        let green1 = scale(red_green1 & 63, 63);
        let blue1 = scale((b4 >> 2) & 63, 63);
        let alpha1 = ((b4 & 3) * 85) as u8;

        words.push(argb(alpha0, red0, green0, blue0));
        if words.len() < pixels {
            words.push(argb(alpha1, red1, green1, blue1));
        }
    }
    words.truncate(pixels);
    Ok(PropImage::from_argb(width, height, &words))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(bytes: [u8; 5]) -> [[u8; 4]; 2] {
        let image = decode(&bytes, 2, 1).unwrap();
        [image.pixel(0, 0).unwrap(), image.pixel(1, 0).unwrap()]
    }

    #[test]
    fn all_ones_is_white_opaque() {
        assert_eq!(group([0xff; 5]), [[255, 255, 255, 255]; 2]);
    }

    #[test]
    fn all_zeroes_is_black_transparent() {
        assert_eq!(group([0x00; 5]), [[0, 0, 0, 0]; 2]);
    }

    #[test]
    fn red_zero_is_the_high_six_bits_of_byte_zero() {
        // b0 = 0b0000_0100 -> red0 = 1
        let [p0, _] = group([0x04, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [scale(1, 63), 0, 0, 0]);
    }

    #[test]
    fn green_zero_straddles_bytes_zero_and_one() {
        // green0 bits 0..3 are byte 1 bits 4..7; bits 4..5 are byte 0 bits 0..1.
        let [p0, _] = group([0x00, 0x10, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, scale(1, 63), 0, 0]);
        let [p0, _] = group([0x01, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, scale(16, 63), 0, 0]);
    }

    #[test]
    fn alpha_zero_uses_the_two_bits_of_byte_two() {
        for (bits, expected) in [(1u8, 85u8), (2, 170), (3, 255)] {
            let [p0, _] = group([0x00, 0x00, bits << 4, 0x00, 0x00]);
            assert_eq!(p0[3], expected);
        }
    }

    #[test]
    fn the_second_pixel_reads_byte_four_for_blue_and_alpha() {
        // b4 = 0b0000_1101 -> blue1 = 3, alpha1 = 1
        let [_, p1] = group([0x00, 0x00, 0x00, 0x00, 0x0d]);
        assert_eq!(p1, [0, 0, scale(3, 63), 85]);
    }

    #[test]
    fn every_group_bits_are_claimed_exactly_once() {
        // Setting bit n of byte m must change exactly one output channel across
        // the two pixels, which proves the layout does not alias.
        for byte in 0..5usize {
            for bit in 0..8u32 {
                let mut bytes = [0u8; 5];
                bytes[byte] = 1 << bit;
                let [p0, p1] = group(bytes);
                let changed = [p0, p1]
                    .iter()
                    .flatten()
                    .filter(|channel| **channel != 0)
                    .count();
                assert!(
                    changed == 1,
                    "byte {byte} bit {bit} touched {changed} channels: {p0:?} {p1:?}"
                );
            }
        }
    }

    #[test]
    fn an_odd_pixel_count_does_not_emit_a_phantom_second_pixel() {
        let image = decode(&[0xff; 5], 1, 1).unwrap();
        assert_eq!(image.pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(image.pixel(1, 0), None);
    }

    #[test]
    fn a_short_payload_is_an_error() {
        assert!(matches!(
            decode(&[0x00; 4], 2, 1),
            Err(PropError::DecompressedTooShort { .. })
        ));
    }
}

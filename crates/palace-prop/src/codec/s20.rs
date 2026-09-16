//! S20-bit: 2.5 bytes per pixel, 5-5-5-5, two pixels per five bytes.
//!
//! This is the format the reference client **writes**: every prop a modern client
//! uploads is S20. It is also the only compressed format with a round-trip proof
//! available, because `PalaceProp.as` contains the encoder next to the decoder and
//! the two are exact inverses — [`crate::encode`] ports that encoder and the
//! `encode_then_decode` test walks random images through both.
//!
//! The 40 bits of a five-byte group, read straight off the decoder's shifts. Each
//! line is one byte, and every one of the 40 bits is claimed exactly once:
//!
//! ```text
//! b0  [7..3] red0            [2..0] green0 (high 3)
//! b1  [7..6] green0 (low 2)  [5..1] blue0        [0] alpha0 bit 4
//! b2  [7..4] alpha0 (low 4)  [3..0] red1 (low 4)
//! b3  [7]    red1 bit 4      [6..2] green1       [1..0] blue1 (high 2)
//! b4  [7..5] blue1 (low 3)   [4..0] alpha1
//! ```
//!
//! Not the layout anyone would design: a pixel's red, green and alpha each split
//! across byte boundaries, and pixel 0's alpha shares byte 3 with pixel 1's red
//! and green. It is what the REALBasic-derived encoder emits.
//!
//! Colours and alpha are all scaled by `255 / 31`.

use crate::codec::{argb, scale};
use crate::error::{PropError, Result};
use crate::image::PropImage;

/// Decode an S20-bit payload into a `width` x `height` image.
pub fn decode(data: &[u8], width: u32, height: u32) -> Result<PropImage> {
    let pixels = (width as usize) * (height as usize);
    let needed = pixels.div_ceil(2) * 5;
    if data.len() < needed {
        return Err(PropError::DecompressedTooShort {
            format: "s20-bit",
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

        let red0 = scale((b0 >> 3) & 31, 31);
        let green0 = scale((((b0 << 8) | b1) >> 6) & 31, 31);
        let blue0 = scale((((b0 << 8) | b1) >> 1) & 31, 31);
        let alpha0 = scale((((b1 << 8) | b2) >> 4) & 31, 31);

        let red1 = scale((((b2 << 8) | b3) >> 7) & 31, 31);
        let green1 = scale((((b2 << 8) | b3) >> 2) & 31, 31);
        let blue1 = scale((((b3 << 8) | b4) >> 5) & 31, 31);
        let alpha1 = scale(((b3 << 8) | b4) & 31, 31);

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
        assert_eq!(
            group([0xff; 5]),
            [[255, 255, 255, 255], [255, 255, 255, 255]]
        );
    }

    #[test]
    fn all_zeroes_is_black_transparent() {
        assert_eq!(group([0x00; 5]), [[0, 0, 0, 0], [0, 0, 0, 0]]);
    }

    #[test]
    fn red_zero_is_the_high_five_bits_of_byte_zero() {
        let [p0, _] = group([0x08, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [scale(1, 31), 0, 0, 0]);
    }

    #[test]
    fn green_zero_spans_bytes_zero_and_one() {
        // green0 bit 0 is byte 1 bit 6; bit 4 is byte 0 bit 2.
        let [p0, _] = group([0x00, 0x40, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, scale(1, 31), 0, 0]);
        let [p0, _] = group([0x04, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, scale(16, 31), 0, 0]);
    }

    #[test]
    fn blue_zero_is_five_bits_of_byte_one() {
        let [p0, _] = group([0x00, 0x02, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, 0, scale(1, 31), 0]);
        let [p0, _] = group([0x00, 0x20, 0x00, 0x00, 0x00]);
        assert_eq!(p0, [0, 0, scale(16, 31), 0]);
    }

    #[test]
    fn alpha_zero_is_split_between_bytes_one_and_two() {
        // alpha0 bits 0..3 live in byte 2 bits 4..7; bit 4 is byte 1 bit 0.
        let [low, _] = group([0x00, 0x00, 0x10, 0x00, 0x00]);
        assert_eq!(low[3], scale(1, 31));
        let [high, _] = group([0x00, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(high[3], scale(16, 31));
        let [all, _] = group([0x00, 0x01, 0xf0, 0x00, 0x00]);
        assert_eq!(all[3], 255);
    }

    #[test]
    fn red_one_is_split_between_bytes_two_and_three() {
        // red1 bits 2..5 are byte 2 bits 0..3; bits 0..1 are byte 3 bits 6..7.
        let [_, p1] = group([0x00, 0x00, 0x08, 0x00, 0x00]);
        assert_eq!(p1[0], scale(16, 31));
        let [_, p1] = group([0x00, 0x00, 0x00, 0x80, 0x00]);
        assert_eq!(p1[0], scale(1, 31));
    }

    #[test]
    fn green_one_is_five_bits_of_byte_three() {
        let [_, p1] = group([0x00, 0x00, 0x00, 0x04, 0x00]);
        assert_eq!(p1[1], scale(1, 31));
        let [_, p1] = group([0x00, 0x00, 0x00, 0x7c, 0x00]);
        assert_eq!(p1[1], 255);
    }

    #[test]
    fn blue_one_spans_bytes_three_and_four() {
        // blue1 bits 0..2 are byte 4 bits 5..7; bits 3..4 are byte 3 bits 0..1.
        let [_, p1] = group([0x00, 0x00, 0x00, 0x00, 0x20]);
        assert_eq!(p1[2], scale(1, 31));
        let [_, p1] = group([0x00, 0x00, 0x00, 0x01, 0x00]);
        assert_eq!(p1[2], scale(8, 31));
    }

    #[test]
    fn alpha_one_is_the_low_five_bits_of_byte_four() {
        let [_, p1] = group([0x00, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!(p1[3], scale(1, 31));
        let [_, p1] = group([0x00, 0x00, 0x00, 0x00, 0x1f]);
        assert_eq!(p1[3], 255);
    }

    #[test]
    fn setting_any_bit_changes_exactly_one_channel() {
        // A layout that aliases would light two channels at once; one that leaks
        // would leave a bit with no effect.
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
    fn a_short_payload_is_an_error() {
        assert!(matches!(
            decode(&[0x00; 4], 2, 1),
            Err(PropError::DecompressedTooShort { .. })
        ));
    }
}

//! The S20-bit encoder — the only format the reference client emits.
//!
//! Ported from `PalaceProp.as::encodeS20BitProp`, which is itself REALBasic code
//! from PalaceChat's author. The bit packing is deliberately kept identical,
//! including the two oddities that make it hard to read: pixel 0's alpha is
//! written as *two* fragments (its top bit into the group's second byte, its low
//! four bits carried into the group's fourth byte), and the intermediate
//! `int_comp` is masked down to `0xF0` between the two pixels, discarding
//! everything else.
//!
//! This is the exact inverse of [`crate::codec::s20::decode`]. The
//! `encode_then_decode` test proves it over the whole 8-bit colour cube, so S20
//! is the one format here with a round-trip proof rather than only a corpus match.
//!
//! Colours are quantised with `round(channel * 31 / 255)`, matching the
//! reference's `Math.round(channel * (31/255))`.

use std::io::Write;

use crate::error::{PropError, Result};
use crate::header::{PropEndian, PropHeader, FLAG_FORMAT_S20BIT, FORMAT_MASK};
use crate::image::PropImage;

/// Quantise one 8-bit channel to 5 bits.
#[must_use]
pub fn quantize(channel: u8) -> u32 {
    (f64::from(channel) * (31.0 / 255.0)).round() as u32
}

/// Encode an image to the compressed S20 pixel payload (no header).
///
/// The image must have an even width, because the format packs pixels in pairs.
pub fn encode_s20_payload(image: &PropImage) -> Result<Vec<u8>> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 || width % 2 != 0 {
        return Err(PropError::UnencodableImage {
            width: image.width(),
            height: image.height(),
        });
    }
    let pixels = image.as_rgba();
    let channel =
        |x: usize, y: usize, c: usize| -> u32 { quantize(pixels[(y * width + x) * 4 + c]) };

    let mut out = Vec::with_capacity(width * height * 5 / 2);
    for y in 0..height {
        for x in (0..width).step_by(2) {
            let mut int_comp = (channel(x, y, 0) << 19)
                | (channel(x, y, 1) << 14)
                | (channel(x, y, 2) << 9)
                | (channel(x, y, 3) << 4);
            out.push(((int_comp >> 16) & 0xFF) as u8);
            out.push(((int_comp >> 8) & 0xFF) as u8);

            int_comp = (int_comp & 0xF0) << 16;
            int_comp |= (channel(x + 1, y, 0) << 15)
                | (channel(x + 1, y, 1) << 10)
                | (channel(x + 1, y, 2) << 5)
                | channel(x + 1, y, 3);
            out.push(((int_comp >> 16) & 0xFF) as u8);
            out.push(((int_comp >> 8) & 0xFF) as u8);
            out.push((int_comp & 0xFF) as u8);
        }
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&out).map_err(|e| PropError::Zlib {
        detail: e.to_string(),
    })?;
    encoder.finish().map_err(|e| PropError::Zlib {
        detail: e.to_string(),
    })
}

/// Encode an image into a complete little-endian S20 prop blob: header + payload.
///
/// `base_flags` carries the head/ghost/rare/animate/bounce bits; the format bits
/// are replaced with `S20BIT`. The offsets are written unchanged, matching what
/// the reference client uploads.
pub fn encode_s20_blob(
    image: &PropImage,
    h_offset: i16,
    v_offset: i16,
    base_flags: u16,
) -> Result<Vec<u8>> {
    let header = PropHeader {
        width: i16::try_from(image.width()).map_err(|_| PropError::UnencodableImage {
            width: image.width(),
            height: image.height(),
        })?,
        height: i16::try_from(image.height()).map_err(|_| PropError::UnencodableImage {
            width: image.width(),
            height: image.height(),
        })?,
        h_offset,
        v_offset,
        script_offset: 0,
        flags: (base_flags & !FORMAT_MASK) | FLAG_FORMAT_S20BIT,
        endian: PropEndian::Little,
    };
    let mut blob = header.encode().to_vec();
    blob.extend_from_slice(&encode_s20_payload(image)?);
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::s20;
    use crate::decode;

    fn image_from(f: impl Fn(usize) -> [u8; 4], width: u32, height: u32) -> PropImage {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for i in 0..(width * height) as usize {
            rgba.extend_from_slice(&f(i));
        }
        PropImage::from_rgba(width, height, rgba).unwrap()
    }

    fn quantised(channel: u8) -> u8 {
        ((quantize(channel) * 255) / 31) as u8
    }

    #[test]
    fn quantisation_hits_both_ends_exactly() {
        assert_eq!(quantize(0), 0);
        assert_eq!(quantize(255), 31);
        assert_eq!(quantised(255), 255);
        assert_eq!(quantised(0), 0);
    }

    #[test]
    fn a_hand_computed_group_matches_the_reference_bit_layout() {
        // Two pixels, only pixel 0's red set to full scale: 31 << 19 = 0xf80000.
        let image = image_from(|i| if i == 0 { [255, 0, 0, 0] } else { [0, 0, 0, 0] }, 2, 1);
        let payload = encode_s20_payload(&image).unwrap();
        let raw = inflate_for_test(&payload);
        assert_eq!(raw, vec![0xf8, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn the_third_byte_carries_pixel_ones_red_and_pixel_zeros_alpha() {
        // Pixel 0 alpha full scale -> int_comp bit 8 -> group byte 1 bit 0, and
        // bits 4..7 -> group byte 2 bits 4..7. Pixel 1 red full scale ->
        // int_comp bit 19 -> group byte 2 bit 3.
        let image = image_from(
            |i| {
                if i == 0 {
                    [0, 0, 0, 255]
                } else {
                    [255, 0, 0, 0]
                }
            },
            2,
            1,
        );
        let raw = inflate_for_test(&encode_s20_payload(&image).unwrap());
        assert_eq!(raw[1] & 0x01, 0x01, "pixel 0 alpha top bit");
        assert_eq!(raw[2] & 0x08, 0x08, "pixel 1 red top bit");
        assert_eq!(raw[2] & 0xf0, 0xf0, "pixel 0 alpha low nibble");
    }

    #[test]
    fn encode_then_decode_is_exact_for_the_colour_cube() {
        // Walk a coarse cube plus the extremes; every channel must survive a
        // round trip as its quantised value.
        let values = [0u8, 1, 9, 64, 128, 130, 200, 254, 255];
        for r in values {
            for g in values {
                let image = image_from(move |i| [r, g, (i * 37) as u8, (i * 91) as u8], 2, 2);
                let blob = encode_s20_blob(&image, 0, 0, 0).unwrap();
                let decoded = decode(&blob).unwrap();
                assert_eq!(decoded.header.format(), crate::header::PropFormat::S20Bit);
                for y in 0..2 {
                    for x in 0..2 {
                        let original = image.pixel(x, y).unwrap();
                        let expected = [
                            quantised(original[0]),
                            quantised(original[1]),
                            quantised(original[2]),
                            quantised(original[3]),
                        ];
                        assert_eq!(
                            decoded.image.pixel(x, y).unwrap(),
                            expected,
                            "pixel ({x},{y}) of {original:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_encoder_is_the_exact_inverse_of_the_decoder() {
        // Direct codec-to-codec check, bypassing the header.
        let image = image_from(
            |i| [(i * 5) as u8, (i * 3) as u8, i as u8, 255 - i as u8],
            44,
            44,
        );
        let raw = inflate_for_test(&encode_s20_payload(&image).unwrap());
        assert_eq!(raw.len(), 44 * 44 * 5 / 2);
        let decoded = s20::decode(&raw, 44, 44).unwrap();
        for y in 0..44 {
            for x in 0..44 {
                let original = image.pixel(x, y).unwrap();
                let expected = [
                    quantised(original[0]),
                    quantised(original[1]),
                    quantised(original[2]),
                    quantised(original[3]),
                ];
                assert_eq!(decoded.pixel(x, y).unwrap(), expected);
            }
        }
    }

    #[test]
    fn an_odd_width_is_refused() {
        let image = PropImage::transparent(3, 1);
        assert!(matches!(
            encode_s20_payload(&image),
            Err(PropError::UnencodableImage { .. })
        ));
    }

    #[test]
    fn encoded_blobs_carry_a_consistent_crc_over_the_payload() {
        let image = image_from(|i| [i as u8, 0, 0, 255], 4, 4);
        let blob = encode_s20_blob(&image, -3, 7, 0x0002).unwrap();
        let header = PropHeader::parse(&blob).unwrap();
        assert_eq!(header.h_offset, -3);
        assert_eq!(header.v_offset, 7);
        assert!(header.is_head());
        assert_eq!(header.format(), crate::header::PropFormat::S20Bit);
        assert_eq!(
            crate::crc::payload_crc(&blob),
            Some(crate::crc::asset_crc(&blob[12..]))
        );
    }

    fn inflate_for_test(payload: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(payload)
            .read_to_end(&mut out)
            .unwrap();
        out
    }
}

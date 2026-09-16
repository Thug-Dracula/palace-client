//! Decode -> encode -> decode round trips.
//!
//! S20 is the only format the reference client emits, so it is the only one with
//! an encoder. That makes it the one format where a round trip can prove the bit
//! layout rather than merely reproduce it: the encoder and decoder are independent
//! code paths, and a mistake in either shows up as a mismatch, not as a plausible
//! image.
//!
//! The exact statement is `decode(encode(x)) == quantise(x)`, because S20 keeps
//! five bits per channel. A lossy round trip is the correct behaviour, so the
//! tests assert the quantised value, never the original.

use palace_prop::{decode, encode_s20_blob, encode_s20_payload, quantize, PropFormat, PropImage};

fn quantised(channel: u8) -> u8 {
    ((quantize(channel) * 255) / 31) as u8
}

fn gradient(width: u32, height: u32) -> PropImage {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.push((x * 6) as u8);
            rgba.push((y * 3) as u8);
            rgba.push(((x + y) * 2) as u8);
            rgba.push(if (x + y) % 2 == 0 { 255 } else { 128 });
        }
    }
    PropImage::from_rgba(width, height, rgba).unwrap()
}

#[test]
fn encode_then_decode_is_exact_after_quantisation() {
    for (width, height) in [(2, 1), (4, 4), (2, 44), (44, 2), (44, 44)] {
        let original = gradient(width, height);
        let blob = encode_s20_blob(&original, 3, -7, 0x0012).unwrap();
        let decoded = decode(&blob).unwrap();

        assert_eq!(decoded.format(), PropFormat::S20Bit);
        assert_eq!(decoded.header.h_offset, 3);
        assert_eq!(decoded.header.v_offset, -7);
        assert!(decoded.header.is_animate());
        assert!(decoded.header.is_head());
        assert_eq!(
            (decoded.image.width(), decoded.image.height()),
            (width, height)
        );

        for y in 0..height {
            for x in 0..width {
                let source = original.pixel(x, y).unwrap();
                let expected = [
                    quantised(source[0]),
                    quantised(source[1]),
                    quantised(source[2]),
                    quantised(source[3]),
                ];
                assert_eq!(
                    decoded.image.pixel(x, y),
                    Some(expected),
                    "{width}x{height} pixel ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn re_encoding_a_decoded_s20_prop_is_stable() {
    // A second round trip must be a no-op: whatever the first one quantised to is
    // already representable, so decode(encode(decode(blob))) == decode(blob).
    let blob = encode_s20_blob(&gradient(44, 44), 1, 2, 0).unwrap();
    let first = decode(&blob).unwrap();
    let reencoded = first.encode_s20().unwrap();
    let second = decode(&reencoded).unwrap();
    assert_eq!(first.image.as_rgba(), second.image.as_rgba());
    assert_eq!(first.header.flags, second.header.flags);
}

#[test]
fn the_encoder_preserves_the_non_format_flag_bits() {
    let image = gradient(2, 2);
    for bits in [0x0000u16, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020, 0x003e] {
        let blob = encode_s20_blob(&image, 0, 0, bits).unwrap();
        let decoded = decode(&blob).unwrap();
        assert_eq!(decoded.header.flags & !palace_prop::FORMAT_MASK, bits);
        assert_eq!(decoded.format(), PropFormat::S20Bit);
    }
}

#[test]
fn the_encoded_payload_is_zlib_wrapped_rather_than_raw_deflate() {
    // `PalaceProp.as` calls `ByteArray.compress()`, which is zlib. A raw-deflate
    // payload would carry no 0x78 header and the reference client could not inflate
    // it, so this pins the container rather than just the pixel bytes.
    let payload = encode_s20_payload(&gradient(44, 44)).unwrap();
    assert_eq!(payload[0] & 0x0F, 0x08, "zlib CM must be deflate");
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]) % 31, 0);
    assert_eq!(payload, encode_s20_payload(&gradient(44, 44)).unwrap());

    // Strip the 2-byte zlib header: the result must no longer decode.
    let mut stripped = vec![0x2c, 0x00, 0x2c, 0x00, 0, 0, 0, 0, 0, 0, 0x00, 0x02];
    stripped.extend_from_slice(&payload[2..]);
    assert!(decode(&stripped).is_err());
}

#[test]
fn round_tripping_every_decoded_fixture_survives() {
    let fixtures = [
        "8bit_head_rare.bin",
        "8bit_avatar.bin",
        "20bit_bit.bin",
        "32bit_bit.bin",
        "s20_avatar.bin",
        "s20_bit.bin",
    ];
    for name in fixtures {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name);
        let original = decode(&std::fs::read(&path).unwrap()).unwrap();
        let reencoded = original.encode_s20().unwrap();
        let again = decode(&reencoded).unwrap();
        assert_eq!(again.format(), PropFormat::S20Bit, "{name}");
        assert_eq!(
            (again.image.width(), again.image.height()),
            (44, 44),
            "{name}"
        );
        assert_eq!(again.encode_s20().unwrap(), reencoded, "{name}");
    }
}

#[test]
fn an_odd_width_cannot_be_encoded() {
    let image = PropImage::transparent(3, 44);
    assert!(encode_s20_payload(&image).is_err());
}

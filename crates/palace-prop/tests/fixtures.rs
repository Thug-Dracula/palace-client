//! Real corpus props, decoded to pinned pixel values.
//!
//! These fixtures are genuine blobs copied out of the local corpus; see
//! `fixtures/README.md` for each one's provenance and SHA-256. They exist so the
//! crate can prove it decodes real data without the 700 MB corpus being present.
//!
//! The expected pixels are regression locks taken from the OpenPalace oracle that
//! `tools/diff_corpus.py` validates the decoder against over 227,000 props, so
//! they are a check on *this* code, not on the format.

use std::path::PathBuf;

use palace_prop::{decode, payload_crc, PropError, PropFormat};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn pixels(name: &str) -> palace_prop::PropImage {
    decode(&fixture(name))
        .unwrap_or_else(|e| panic!("{name} should decode: {e}"))
        .image
}

#[test]
fn eight_bit_head_prop_is_mostly_transparent_with_sparse_colour() {
    let blob = fixture("8bit_head_rare.bin");
    let prop = decode(&blob).unwrap();
    assert_eq!(prop.format(), PropFormat::EightBit);
    assert_eq!((prop.header.width, prop.header.height), (44, 44));
    assert_eq!((prop.header.h_offset, prop.header.v_offset), (7, 7));
    assert!(prop.header.is_head());
    assert!(prop.header.is_rare());
    assert!(!prop.header.is_ghost());

    // The first scanline's RLE is `f0 f0 e0`, three all-skip control bytes, so
    // the whole row is untouched. That is the clearest possible assertion that
    // skipped columns are not written and that the first stream row really is
    // image row 0.
    for x in 0..44 {
        assert_eq!(
            prop.image.pixel(x, 0),
            Some([0, 0, 0, 0]),
            "row 0 column {x} should be untouched"
        );
    }
    let untouched = prop
        .image
        .as_rgba()
        .chunks_exact(4)
        .filter(|p| *p == [0, 0, 0, 0])
        .count();
    assert_eq!(
        untouched,
        1936 - 83,
        "all but 83 pixels should be untouched"
    );
}

#[test]
fn eight_bit_avatar_matches_the_reference_pixels() {
    let image = pixels("8bit_avatar.bin");
    assert_eq!(image.pixel(0, 0), Some([204, 127, 0, 255]));
    assert_eq!(image.pixel(1, 0), Some([102, 95, 85, 255]));
    assert_eq!(image.pixel(0, 1), Some([204, 95, 0, 255]));
    assert_eq!(image.pixel(22, 22), Some([102, 95, 0, 255]));
    assert_eq!(image.pixel(43, 43), Some([0, 0, 0, 0]));
}

#[test]
fn twenty_bit_fixture_matches_the_reference_pixels() {
    let prop = decode(&fixture("20bit_bit.bin")).unwrap();
    assert_eq!(prop.format(), PropFormat::TwentyBit);
    assert_eq!(prop.image.pixel(0, 0), Some([0, 255, 0, 0]));
    assert_eq!(prop.image.pixel(22, 22), Some([113, 113, 113, 255]));
    assert_eq!(prop.image.pixel(43, 43), Some([0, 0, 0, 0]));
}

#[test]
fn s20_fixture_matches_the_reference_pixels() {
    let prop = decode(&fixture("s20_avatar.bin")).unwrap();
    assert_eq!(prop.format(), PropFormat::S20Bit);
    assert_eq!(prop.image.pixel(0, 0), Some([24, 98, 90, 255]));
    assert_eq!(prop.image.pixel(1, 0), Some([24, 90, 90, 255]));
    assert_eq!(prop.image.pixel(22, 22), Some([8, 90, 82, 255]));
    assert_eq!(prop.image.pixel(43, 43), Some([0, 0, 0, 255]));
}

#[test]
fn a_40_byte_s20_prop_is_entirely_transparent() {
    // 4840 bytes of zeroes compress to 28 bytes of payload, which is the smallest
    // S20 prop in the roster and a good check that inflation is not trusted to
    // produce anything but the exact pixel count.
    let blob = fixture("s20_bit.bin");
    assert_eq!(blob.len(), 40);
    let prop = decode(&blob).unwrap();
    assert_eq!(prop.format(), PropFormat::S20Bit);
    assert!(prop.image.as_rgba().iter().all(|b| *b == 0));
}

#[test]
fn thirty_two_bit_fixture_matches_the_reference_pixels() {
    let prop = decode(&fixture("32bit_bit.bin")).unwrap();
    assert_eq!(prop.format(), PropFormat::ThirtyTwoBit);
    assert_eq!(prop.image.pixel(0, 0), Some([255, 255, 255, 0]));
    assert_eq!(prop.image.pixel(22, 22), Some([109, 73, 79, 255]));
}

#[test]
fn the_rejected_prop_is_one_the_reference_also_refuses() {
    // The RLE overruns row 39. The reference's `x < 0` check sets badProp and
    // refuses to render it, so an Err here is the same outcome.
    let error = decode(&fixture("malformed_rle_overflow.bin")).unwrap_err();
    assert!(matches!(
        error,
        PropError::RleRowOverflow { .. } | PropError::RleRunaway { .. }
    ));
}

#[test]
fn a_payload_that_ends_inside_the_final_run_still_decodes() {
    // Eleven of the twelve odd props in `pserver.prp` end one byte into the last
    // row; five of them the reference renders (see `codec::eight`), and this is
    // one. It must decode rather than being "fixed" into an error.
    let prop = decode(&fixture("truncated_final_run.bin")).unwrap();
    assert_eq!(prop.format(), PropFormat::EightBit);
    assert_eq!(prop.image.pixel(22, 22), Some([238, 238, 238, 255]));
}

#[test]
fn the_roster_crc_of_every_fixture_is_reproduced() {
    // `crc` over `blob[12:]` is what the server validates on startup; a codec that
    // decoded correctly but computed a different CRC would still be rejected at
    // load time. The values are the ones stored in the roster records these blobs
    // were copied from.
    let expected: [(&str, u32); 8] = [
        ("20bit_bit.bin", 0xb9e6_4d3a),
        ("32bit_bit.bin", 0x8978_de63),
        ("8bit_avatar.bin", 0x1428_275e),
        ("8bit_head_rare.bin", 0x9596_284b),
        ("malformed_rle_overflow.bin", 0xc518_5c86),
        ("s20_avatar.bin", 0x8024_b78b),
        ("s20_bit.bin", 0xbfd6_46f9),
        ("truncated_final_run.bin", 0xdd4b_b471),
    ];
    for (name, crc) in expected {
        let blob = fixture(name);
        assert_eq!(payload_crc(&blob), Some(crc), "{name}");
    }
}

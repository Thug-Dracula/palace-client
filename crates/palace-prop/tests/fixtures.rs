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

// `.prp` golden-container fixtures. These walk the container with plain integer
// math and hand each embedded blob to the existing public API, so the file must
// not reference the new `.prp` module. Synthetic fixtures are canonical server
// shape (signed-id sorted, contiguous, CRC over `blob[12..]`); the real copy is
// deliberately unordered. See `fixtures/prp/README.md`.

const PROP_TAG: u32 = 0x5072_6F70;
const FAVE_TAG: u32 = 0x4661_7665;

fn prp_fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("prp")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn prp_fixture_exists(name: &str) -> bool {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("prp")
        .join(name)
        .exists()
}

struct PrpRecord {
    id: i32,
    size: u32,
    name_offset: u32,
    crc: u32,
    blob: Vec<u8>,
}

struct Prp {
    n_types: i32,
    n_assets: i32,
    types: Vec<(u32, i32, i32)>,
    records: Vec<PrpRecord>,
    names: Vec<(u32, Vec<u8>)>,
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
}

fn i32_at(buf: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
}

/// Minimal, dependency-free `.prp` container walker for the fixture tests.
///
/// It trusts the declared counts (PRP-FORMAT.md §10) and resolves each blob at
/// `16 + the record's own dataOffset`; it does not reorder or repair anything.
fn parse_prp(buf: &[u8]) -> Prp {
    assert!(buf.len() >= 16, "file is shorter than the 16-byte header");
    let data_offset = u32_at(buf, 0);
    let data_size = u32_at(buf, 4);
    let map_offset = u32_at(buf, 8);
    let map_size = u32_at(buf, 12);
    assert_eq!(data_offset, 16, "data region must start at 16");
    assert_eq!(map_offset, data_offset + data_size);
    assert_eq!(
        buf.len() as u32,
        map_offset + map_size,
        "filesize must equal assetMapOffset + assetMapSize"
    );

    let m = map_offset as usize;
    let n_types = i32_at(buf, m);
    let n_assets = i32_at(buf, m + 4);
    let len_names = i32_at(buf, m + 8);
    let types_off = u32_at(buf, m + 12) as usize;
    let recs_off = u32_at(buf, m + 16) as usize;
    let names_off = u32_at(buf, m + 20) as usize;
    assert_eq!(types_off, 24);
    assert_eq!(recs_off, 24 + n_types as usize * 12);
    assert_eq!(names_off, recs_off + n_assets as usize * 32);
    assert_eq!(names_off + len_names as usize, map_size as usize);

    let mut types = Vec::new();
    for i in 0..n_types.max(0) as usize {
        let t = m + types_off + i * 12;
        types.push((u32_at(buf, t), i32_at(buf, t + 4), i32_at(buf, t + 8)));
    }

    let mut records = Vec::new();
    for i in 0..n_assets.max(0) as usize {
        let r = m + recs_off + i * 32;
        let size = u32_at(buf, r + 12);
        let start = 16 + u32_at(buf, r + 8) as usize;
        let blob = buf
            .get(start..start.saturating_add(size as usize))
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        records.push(PrpRecord {
            id: i32_at(buf, r),
            size,
            name_offset: u32_at(buf, r + 20),
            crc: u32_at(buf, r + 28),
            blob,
        });
    }

    let mut names = Vec::new();
    let nbase = m + names_off;
    let mut p = 0usize;
    while p < len_names.max(0) as usize {
        let len = buf[nbase + p] as usize;
        let bytes = buf[nbase + p + 1..nbase + p + 1 + len].to_vec();
        names.push((p as u32, bytes));
        p += 1 + len;
    }

    Prp {
        n_types,
        n_assets,
        types,
        records,
        names,
    }
}

fn synthetic_prp_names() -> [&'static str; 6] {
    [
        "single_type.prp",
        "prop_fave.prp",
        "dup_id_crc.prp",
        "empty.prp",
        "unnamed_records.prp",
        "long_nonascii_name.prp",
    ]
}

#[test]
fn synthetic_prp_fixtures_are_well_formed_and_crc_clean() {
    for name in synthetic_prp_names() {
        let prp = parse_prp(&prp_fixture(name));
        assert!(prp.n_types >= 1, "{name}: expected at least one type");
        let mut checked = 0;
        for rec in &prp.records {
            if rec.size >= 12 {
                assert_eq!(
                    payload_crc(&rec.blob),
                    Some(rec.crc),
                    "{name}: record id {} has a mismatched CRC",
                    rec.id
                );
                checked += 1;
            }
        }
        if prp.n_assets > 0 {
            assert!(checked > 0, "{name}: expected at least one payload record");
        }
    }
}

#[test]
fn signed_id_order_fixture_pins_high_bit_before_positive() {
    let prp = parse_prp(&prp_fixture("single_type.prp"));
    assert_eq!(prp.n_types, 1);
    assert_eq!(prp.types[0], (PROP_TAG, 2, 0));
    let ids: Vec<i32> = prp.records.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![0x803D_6C40u32 as i32, 0x3A3A_D1F7]);
    assert!(
        ids[0] < ids[1],
        "signed order: the high-bit id sorts before the small positive one"
    );
    assert_eq!(
        decode(&prp.records[0].blob).unwrap().format(),
        PropFormat::TwentyBit
    );
    assert_eq!(
        decode(&prp.records[1].blob).unwrap().format(),
        PropFormat::ThirtyTwoBit
    );
}

#[test]
fn prop_fave_fixture_carries_a_size_zero_sentinel() {
    let prp = parse_prp(&prp_fixture("prop_fave.prp"));
    assert_eq!(prp.n_types, 2);
    assert_eq!(prp.types[0], (PROP_TAG, 1, 0));
    assert_eq!(prp.types[1], (FAVE_TAG, 1, 1));
    let fave = &prp.records[prp.types[1].2 as usize];
    assert_eq!(fave.id, 128);
    assert_eq!(fave.size, 0);
    assert_eq!(fave.name_offset, 0xFFFF_FFFF);
    assert!(fave.blob.is_empty());
}

#[test]
fn duplicate_id_variants_stay_adjacent_with_distinct_crcs() {
    let prp = parse_prp(&prp_fixture("dup_id_crc.prp"));
    let ids: Vec<i32> = prp.records.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![1000, 1000, 1000, 1001]);
    let crcs: Vec<u32> = prp.records[..3].iter().map(|r| r.crc).collect();
    assert_ne!(crcs[0], crcs[1]);
    assert_ne!(crcs[1], crcs[2]);
    assert_ne!(crcs[0], crcs[2]);
    // The same bytes under a different id keep the same CRC: id is not in it.
    assert_eq!(prp.records[0].crc, prp.records[3].crc);
}

#[test]
fn empty_bag_fixture_is_a_valid_zero_record_file() {
    let prp = parse_prp(&prp_fixture("empty.prp"));
    assert_eq!(prp.n_assets, 0);
    assert!(prp.records.is_empty());
    assert!(prp.names.is_empty());
}

#[test]
fn unnamed_records_keep_name_offset_minus_one() {
    let prp = parse_prp(&prp_fixture("unnamed_records.prp"));
    assert_eq!(prp.n_assets, 2);
    for rec in &prp.records {
        assert_eq!(rec.name_offset, 0xFFFF_FFFF);
    }
    assert!(prp.names.is_empty());
}

#[test]
fn long_nonascii_name_is_preserved_verbatim() {
    let prp = parse_prp(&prp_fixture("long_nonascii_name.prp"));
    assert_eq!(prp.records.len(), 1);
    let (off, name) = &prp.names[0];
    assert_eq!(*off, prp.records[0].name_offset);
    assert_eq!(name.len(), 196);
    assert_eq!(name[3], 0xE9, "latin1 e-acute must survive as one byte");
    assert!(
        name.iter().any(|b| *b >= 0x80),
        "the name must contain non-ASCII bytes"
    );
}

#[test]
fn real_palace_hidden_fixture_pins_the_known_collection() {
    // A byte-for-byte copy of `Palace - Hidden.PRP` (SHA-256 a06ad1fc…), the
    // smallest real collection. Its records are NOT sorted and it is not a
    // valid input to a server that binary-searches by id — it is here precisely
    // to pin that a tolerant reader must not require canonical ordering.
    if !prp_fixture_exists("real_palace_hidden.prp") {
        eprintln!("skipping: real fixture not present");
        return;
    }
    let prp = parse_prp(&prp_fixture("real_palace_hidden.prp"));
    assert_eq!(prp.n_types, 2);
    assert_eq!(prp.types[0], (PROP_TAG, 75, 0));
    assert_eq!(prp.types[1], (FAVE_TAG, 1, 75));
    assert_eq!(prp.n_assets, 76);
    assert_eq!(prp.records.len(), 76);

    let named = prp
        .records
        .iter()
        .filter(|r| r.name_offset != 0xFFFF_FFFF)
        .count();
    assert_eq!(named, 56, "56 of the 76 records carry a name");

    // 74 of the 75 Prop records' stored CRC matches blob[12..]; one does not.
    let prop = &prp.records[..75];
    let matched = prop
        .iter()
        .filter(|r| payload_crc(&r.blob) == Some(r.crc))
        .count();
    assert_eq!(matched, 74, "one known Prop record has a non-matching CRC");

    // The Fave record here carries an 8-byte (id, crc) payload, unlike the
    // size-0 sentinel in the synthetic fixture.
    let fave = &prp.records[75];
    assert_eq!(fave.id, 128);
    assert_eq!(fave.size, 8);
    assert_eq!(fave.blob.len(), 8);
}

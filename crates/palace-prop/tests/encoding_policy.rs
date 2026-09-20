//! Integration tests for the encoding-preservation policy and prop identity.
//!
//! These pin the four rules task T6 exists to enforce:
//!
//! (a) a metadata-only edit leaves the blob bytes and the encoding identical;
//! (b) a pixel edit produces an S20 blob whose recomputed CRC validates;
//! (c) an odd width is refused with an error that names the width;
//! (d) the runtime asset defaults are `(0, 0)`.
//!
//! A `PropRecord` is built here the way the reader will build one — header and
//! encoding derived from the blob, CRC over `blob[12:]` — because the `.prp`
//! reader (`Roster::parse`) is a stub in this task's scope.

use palace_prop::encoding::{
    default_asset_fields, encoding_for_pixels, encoding_of, identity, preserves_encoding,
    validate_written, EditKind,
};
use palace_prop::prp::{AssetRec, PropEncoding, PropHeader, PropRecord};
use palace_prop::{asset_crc, encode_s20_blob, PropError, PropImage, ASSET_CRC_MAGIC};

/// The fixed blob header length from spec §3.1.
const HEADER_LEN: usize = 12;

/// Build a record the way the reader will: parse the blob header, derive the
/// encoding from it, and compute the payload CRC over `blob[12:]`.
fn record(blob: Vec<u8>, id: i32, name: Option<&str>) -> PropRecord {
    let header = PropHeader::parse(&blob).ok();
    let crc = asset_crc(blob.get(HEADER_LEN..).unwrap_or(&[]));
    PropRecord {
        rec: AssetRec {
            id,
            data_size: blob.len() as u32,
            crc,
            name_offset: if name.is_some() { 0 } else { -1 },
            ..AssetRec::default()
        },
        header,
        encoding: header.map(|h| h.encoding()),
        blob,
        name: name.map(str::to_owned),
    }
}

/// An RGBA image whose pixels vary with position and `seed`, so two seeds
/// produce genuinely different payloads.
fn rgba(width: u32, height: u32, seed: u8) -> PropImage {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for i in 0..(width * height) as usize {
        let i = i as u8;
        bytes.extend_from_slice(&[
            i.wrapping_mul(3).wrapping_add(seed),
            i.wrapping_mul(5).wrapping_add(seed),
            i.wrapping_mul(7).wrapping_add(seed),
            255,
        ]);
    }
    PropImage::from_rgba(width, height, bytes).expect("rgba buffer has the right length")
}

#[test]
fn metadata_only_edit_preserves_blob_bytes_and_encoding() {
    let blob = encode_s20_blob(&rgba(4, 4, 0), 1, 2, 0x0002).expect("encode S20");
    let original = record(blob, 42, Some("old name"));

    // Metadata-only: rename, retime and move the name offset — never the blob.
    let mut updated = original.clone();
    updated.name = Some("new name".to_owned());
    updated.rec.name_offset = 5;
    updated.rec.last_use_time = 1_789_197_520;

    assert!(
        preserves_encoding(&original, &updated),
        "a metadata-only edit must preserve the encoding"
    );
    assert_eq!(
        original.blob, updated.blob,
        "a metadata-only edit must not touch the blob bytes"
    );
    assert_eq!(encoding_of(&original), Some(PropEncoding::S20Bit));
    assert_eq!(encoding_of(&original), encoding_of(&updated));
}

#[test]
fn pixel_edit_re_encodes_to_s20_with_a_valid_crc() {
    let before = record(
        encode_s20_blob(&rgba(4, 4, 0), 0, 0, 0).expect("encode S20"),
        42,
        None,
    );

    // The documented rule for a pixel edit: S20 (the only encoder), which packs
    // pixels in pairs.
    let edited = rgba(4, 4, 200);
    assert_eq!(encoding_for_pixels(&edited), Ok(PropEncoding::S20Bit));

    let after = record(
        encode_s20_blob(&edited, 0, 0, 0).expect("encode S20"),
        42,
        None,
    );

    // Real pixels changed, so this is not a preserved edit.
    assert_ne!(before.blob, after.blob);
    assert!(!preserves_encoding(&before, &after));

    // The declared CRC is the payload CRC and it validates.
    let declared = identity(&after.blob).crc;
    assert_eq!(declared, asset_crc(&after.blob[HEADER_LEN..]));
    validate_written(&after.blob, declared).expect("written CRC must validate");
    assert!(
        validate_written(&after.blob, declared ^ 0xdead_beef).is_err(),
        "a stale CRC must be refused"
    );

    // And the encoding detected from the written header is S20.
    assert_eq!(encoding_of(&after), Some(PropEncoding::S20Bit));
}

#[test]
fn odd_width_is_rejected_and_the_error_names_it() {
    let image = PropImage::transparent(3, 2);
    let err = encoding_for_pixels(&image).expect_err("an odd width must be refused");
    assert!(
        matches!(
            err,
            PropError::UnencodableImage {
                width: 3,
                height: 2
            }
        ),
        "unexpected error: {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains('3'),
        "error should name the width: {message}"
    );
    assert!(
        message.contains('2'),
        "error should name the height: {message}"
    );

    // A zero dimension is refused for the same reason (no pixels to pack).
    assert!(encoding_for_pixels(&PropImage::transparent(0, 4)).is_err());
}

#[test]
fn default_runtime_asset_fields_are_zero() {
    assert_eq!(default_asset_fields(), (0, 0));
}

#[test]
fn identity_of_a_headerless_blob_is_the_payload_crc_seed() {
    let key = identity(&[1, 2, 3]);
    assert_eq!(key.id, 0, "a bare blob has no asset id");
    assert_eq!(
        key.crc, ASSET_CRC_MAGIC,
        "an empty payload hashes to the seed"
    );
    assert!(
        validate_written(&[1, 2, 3], key.crc).is_err(),
        "a blob with no header cannot be validated"
    );
}

#[test]
fn edit_kind_distinguishes_the_two_edits() {
    assert_ne!(EditKind::MetadataOnly, EditKind::PixelChange);
}

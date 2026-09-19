//! `PropCatalog` against a synthetic bag built from scratch.
//!
//! The fixture never touches the user's live data: it writes a temporary
//! `PropBag.bundle/` in the OS temp directory (`.pids`, `.props` and both
//! `*.favs` files) and points the catalog at it. The known reference entry is
//! reproduced byte for byte — id `976933367`, crc `4288739094`, the 32-byte
//! prefix that carries `"The Colosseum (1 vs 1"` — so the JSON shape and the
//! thumbnail can be asserted exactly.

use std::path::{Path, PathBuf};

use palace_prop::{encode_s20_blob, PropCatalog, PropImage, BAG_PREFIX_LEN};

/// The real 32-byte prefix of id `976933367`, truncated by the fixed prefix
/// length in the full-name tail `"The Colosseum (1 vs 1"`.
const KNOWN_PREFIX: &[u8] = b"\x07NewProp\x00\x00\x16The Colosseum (1 vs 1";

/// A second, nameless prop: an all-zero prefix decodes to no name.
const NAMELESS_ID: u32 = 1234;
const KNOWN_ID: u32 = 976933367;
const KNOWN_CRC: u32 = 4288739094;

fn prop_blob() -> Vec<u8> {
    let image = PropImage::transparent(44, 44);
    encode_s20_blob(&image, 0, 0, 0).expect("encode S20 prop")
}

fn push_record(index: &mut Vec<u8>, id: u32, crc: u32, offset: u32, size: u32) {
    index.extend_from_slice(&id.to_be_bytes());
    index.extend_from_slice(&crc.to_be_bytes());
    index.extend_from_slice(&offset.to_be_bytes());
    index.extend_from_slice(&size.to_be_bytes());
}

/// Build a bundle directory and return its path.
fn write_fixture(dir: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("create bundle dir");

    let prop = prop_blob();
    let known_blob = [KNOWN_PREFIX, prop.as_slice()].concat();
    let nameless_blob = [&[0u8; BAG_PREFIX_LEN][..], prop.as_slice()].concat();
    let duplicate_blob = [&[0xa5u8; BAG_PREFIX_LEN][..], prop.as_slice()].concat();
    let placeholder_blob = [&[0u8; BAG_PREFIX_LEN][..], prop.as_slice()].concat();

    let mut props = Vec::new();
    let mut index = Vec::new();

    // Known prop.
    let known_offset = props.len() as u32;
    props.extend_from_slice(&known_blob);
    push_record(
        &mut index,
        KNOWN_ID,
        KNOWN_CRC,
        known_offset,
        known_blob.len() as u32,
    );

    // Nameless prop.
    let nameless_offset = props.len() as u32;
    props.extend_from_slice(&nameless_blob);
    push_record(
        &mut index,
        NAMELESS_ID,
        5678,
        nameless_offset,
        nameless_blob.len() as u32,
    );

    // A placeholder that must be skipped.
    let placeholder_offset = props.len() as u32;
    props.extend_from_slice(&placeholder_blob);
    push_record(
        &mut index,
        0x8000_0005,
        6,
        placeholder_offset,
        placeholder_blob.len() as u32,
    );

    // A duplicate id that must collapse into the first one.
    let duplicate_offset = props.len() as u32;
    props.extend_from_slice(&duplicate_blob);
    push_record(
        &mut index,
        NAMELESS_ID,
        9999,
        duplicate_offset,
        duplicate_blob.len() as u32,
    );

    std::fs::write(dir.join("PalaceChat.pids"), &index).expect("write .pids");
    std::fs::write(dir.join("PalaceChat.props"), &props).expect("write .props");

    // Favourites: the known id. Trash: the nameless id.
    let mut favs = Vec::new();
    push_record(&mut favs, KNOWN_ID, KNOWN_CRC, 0, 0);
    std::fs::write(dir.join("PalaceChat.favs"), &favs).expect("write PalaceChat.favs");
    let mut trash = Vec::new();
    push_record(&mut trash, NAMELESS_ID, 5678, 0, 0);
    std::fs::write(dir.join("Trash.favs"), &trash).expect("write Trash.favs");

    dir.to_path_buf()
}

fn fixture_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("palace-catalog-{tag}-{}", std::process::id()))
}

fn open_fixture(tag: &str) -> (PropCatalog, PathBuf) {
    let dir = fixture_dir(tag);
    write_fixture(&dir);
    let catalog = PropCatalog::open_dir(&dir).expect("open synthetic bag");
    (catalog, dir)
}

#[test]
fn the_fixture_prefix_is_exactly_the_fixed_prefix_length() {
    assert_eq!(KNOWN_PREFIX.len(), BAG_PREFIX_LEN);
}

#[test]
fn the_catalog_parses_the_fixture_and_skips_placeholders_and_duplicates() {
    let (catalog, dir) = open_fixture("parse");
    assert_eq!(
        catalog.len(),
        2,
        "placeholder and duplicate must be dropped"
    );
    assert!(!catalog.is_empty());

    let known = &catalog.entries()[0];
    assert_eq!(known.id, KNOWN_ID);
    assert_eq!(known.crc, KNOWN_CRC);
    assert_eq!(known.name.as_deref(), Some("The Colosseum (1 vs 1"));
    assert_eq!((known.width, known.height), (44, 44));
    assert_eq!(known.flags, 0x0200);
    assert!(known.favorite, "the known id is in PalaceChat.favs");
    assert!(!known.trash);

    let nameless = &catalog.entries()[1];
    assert_eq!(nameless.id, NAMELESS_ID);
    assert_eq!(nameless.name, None);
    assert!(!nameless.favorite);
    assert!(nameless.trash, "the nameless id is in Trash.favs");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_env_override_selects_the_fixture_bag() {
    let dir = fixture_dir("env");
    write_fixture(&dir);
    std::env::set_var("PALACE_PROP_BAG", &dir);
    let catalog = PropCatalog::open_default().expect("env override opens the fixture");
    assert_eq!(catalog.len(), 2);
    std::env::remove_var("PALACE_PROP_BAG");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_json_payload_has_the_exact_shape_and_omits_unknown_names() {
    let (catalog, dir) = open_fixture("json");
    let expected = concat!(
        "{\"props\":[",
        "{\"id\":976933367,\"crc\":4288739094,\"name\":\"The Colosseum (1 vs 1\",",
        "\"w\":44,\"h\":44,\"flags\":512,\"fav\":true,\"trash\":false},",
        "{\"id\":1234,\"crc\":5678,\"w\":44,\"h\":44,\"flags\":512,\"fav\":false,\"trash\":true}",
        "]}"
    );
    assert_eq!(catalog.catalog_json(), expected);
    assert!(!catalog.catalog_json().contains("null"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn blob_returns_the_prop_without_the_bundle_prefix() {
    let (catalog, dir) = open_fixture("blob");
    let blob = catalog.blob(KNOWN_ID).expect("known id has a blob");
    assert_eq!(blob, prop_blob(), "the 32-byte bundle prefix is stripped");
    assert_eq!(blob.len(), prop_blob().len());
    assert!(catalog.blob(999_999).is_none(), "unknown id");
    let _ = std::fs::remove_dir_all(dir);
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || &bytes[0..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

#[test]
fn a_thumbnail_is_a_44x44_rgba_png() {
    let (catalog, dir) = open_fixture("png");
    let png = catalog
        .thumbnail_png(KNOWN_ID)
        .expect("known id has a thumbnail");
    assert_eq!(png_size(&png), Some((44, 44)));
    assert!(
        catalog.thumbnail_png(999_999).is_none(),
        "unknown id has no thumbnail"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "reads the user's real PropBag.bundle read-only; set PALACE_PROP_BAG or run on a machine with a bag"]
fn the_real_bag_contains_the_known_reference_entry() {
    let catalog = PropCatalog::open_default().expect("open the real bag");
    println!("real catalog entries: {}", catalog.len());

    let known = catalog
        .entries()
        .iter()
        .find(|entry| entry.id == KNOWN_ID)
        .expect("the known reference id is in the real bag");
    println!(
        "known: crc={} name={:?} {}x{} flags={:#06x} fav={} trash={}",
        known.crc, known.name, known.width, known.height, known.flags, known.favorite, known.trash
    );
    assert_eq!(known.crc, KNOWN_CRC);
    assert_eq!(known.name.as_deref(), Some("The Colosseum (1 vs 1"));
    assert_eq!(
        png_size(&catalog.thumbnail_png(KNOWN_ID).unwrap()),
        Some((44, 44))
    );

    let placeholders = catalog
        .entries()
        .iter()
        .filter(|entry| (0x8000_0000..0x8000_0100).contains(&entry.id))
        .count();
    assert_eq!(placeholders, 0, "placeholder ids must be filtered out");
}

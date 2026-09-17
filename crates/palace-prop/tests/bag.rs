//! `PropBag.bundle` reader: synthetic fixtures plus a gated real-bag run.
//!
//! The synthetic tests build a bag from scratch so they never touch the user's
//! live client data. The real-bag test (`prop_bag_real_snapshot`) is `#[ignore]`d
//! and reads a *copy* pointed at by `PALACE_PROP_BAG`:
//!
//! ```text
//! PALACE_PROP_BAG=/tmp/propbag_frozen \
//!   cargo test -p palace-prop --test bag -- --ignored --nocapture
//! ```
//!
//! It reports the record/decode counts and cross-checks every
//! `BagThumbCache/<a>_<b>.png` key against the index. It writes decoded PNGs
//! under `std::env::temp_dir()/prop-bag-verify` (override with
//! `PALACE_PROP_BAG_OUT`), never into the user's directories.

use std::path::{Path, PathBuf};

use palace_prop::{BagEntry, PropBag, BAG_PREFIX_LEN};

/// A valid 4x1 8-bit prop: header `width=4, height=1`, payload `0x04` (skip 0,
/// emit 4) followed by four palette bytes.
fn valid_prop() -> Vec<u8> {
    let mut blob = vec![4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    blob.extend_from_slice(&[0x04, 0x01, 0x01, 0x01, 0x01]);
    blob
}

fn prefixed(prop: &[u8]) -> Vec<u8> {
    let mut blob = vec![0xab; BAG_PREFIX_LEN];
    blob.extend_from_slice(prop);
    blob
}

/// Turn `(a, b, blob)` triples into a contiguous `(index, props)` pair.
fn build_bag(entries: &[(u32, u32, Vec<u8>)]) -> (Vec<u8>, Vec<u8>) {
    let mut index = Vec::new();
    let mut props = Vec::new();
    for (a, b, blob) in entries {
        index.extend_from_slice(&a.to_be_bytes());
        index.extend_from_slice(&b.to_be_bytes());
        index.extend_from_slice(&(props.len() as u32).to_be_bytes());
        index.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        props.extend_from_slice(blob);
    }
    (index, props)
}

fn bag_with(entries: &[(u32, u32, Vec<u8>)]) -> PropBag {
    let (index, props) = build_bag(entries);
    PropBag::parse(&index, &props)
}

fn write_bundle(dir: &Path, pids: &[u8], props: &[u8]) {
    std::fs::create_dir_all(dir).expect("create bundle dir");
    std::fs::write(dir.join("Test.pids"), pids).expect("write .pids");
    std::fs::write(dir.join("Test.props"), props).expect("write .props");
}

// ---------------------------------------------------------------------------
// Record parsing and index quality.
// ---------------------------------------------------------------------------

#[test]
fn records_parse_big_endian_with_their_offsets_and_sizes() {
    let one = prefixed(&valid_prop());
    let bag = bag_with(&[(0xdead_beef, 0x1234_5678, one.clone())]);
    assert_eq!(bag.records(), 1);
    assert_eq!(bag.len(), 1);
    assert_eq!(bag.trailing_bytes(), 0);
    assert_eq!(bag.out_of_bounds(), 0);
    assert_eq!(bag.props_bytes(), one.len());

    let entry = &bag.entries()[0];
    assert_eq!(entry.a(), 0xdead_beef);
    assert_eq!(entry.b(), 0x1234_5678);
    assert_eq!(entry.offset(), 0);
    assert_eq!(entry.size() as usize, one.len());
    assert_eq!(entry.blob(), one.as_slice());
    assert_eq!(entry.prop_bytes(), Some(valid_prop().as_slice()));
}

#[test]
fn a_partial_trailing_record_is_counted_not_parsed() {
    let one = prefixed(&valid_prop());
    let (mut index, props) = build_bag(&[(1, 2, one)]);
    index.extend_from_slice(&[0, 0, 0]); // 3 stray bytes
    let bag = PropBag::parse(&index, &props);
    assert_eq!(bag.records(), 1);
    assert_eq!(bag.len(), 1);
    assert_eq!(bag.trailing_bytes(), 3);
}

#[test]
fn an_out_of_bounds_record_is_dropped_and_counted() {
    let (mut index, props) = build_bag(&[(1, 2, prefixed(&valid_prop()))]);
    // A second record that runs 5 bytes past the end of `.props`.
    index.extend_from_slice(&7u32.to_be_bytes());
    index.extend_from_slice(&8u32.to_be_bytes());
    index.extend_from_slice(&(props.len() as u32).to_be_bytes());
    index.extend_from_slice(&999u32.to_be_bytes());
    let bag = PropBag::parse(&index, &props);
    assert_eq!(bag.records(), 2);
    assert_eq!(bag.len(), 1);
    assert_eq!(bag.out_of_bounds(), 1);
}

// ---------------------------------------------------------------------------
// Contiguity.
// ---------------------------------------------------------------------------

#[test]
fn contiguous_blobs_tile_the_file() {
    let bag = bag_with(&[
        (1, 2, prefixed(&valid_prop())),
        (3, 4, prefixed(&valid_prop())),
        (5, 6, prefixed(&valid_prop())),
    ]);
    assert_eq!(bag.tiled_pairs(), 2);
    assert_eq!(bag.adjacent_pairs(), 2);
    assert!(bag.is_contiguous());
}

#[test]
fn a_gap_breaks_contiguity_without_losing_entries() {
    let first = prefixed(&valid_prop());
    let second = prefixed(&valid_prop());
    let third = prefixed(&valid_prop());
    let mut index = Vec::new();
    let mut props = Vec::new();
    let push = |index: &mut Vec<u8>, props: &Vec<u8>, a: u32, blob: &[u8]| {
        index.extend_from_slice(&a.to_be_bytes());
        index.extend_from_slice(&(a + 1).to_be_bytes());
        index.extend_from_slice(&(props.len() as u32).to_be_bytes());
        index.extend_from_slice(&(blob.len() as u32).to_be_bytes());
    };
    push(&mut index, &props, 1, &first);
    props.extend_from_slice(&first);
    push(&mut index, &props, 3, &second);
    props.extend_from_slice(&second);
    props.extend_from_slice(&[0, 0, 0, 0, 0]); // a five-byte gap
    push(&mut index, &props, 5, &third);
    props.extend_from_slice(&third);

    let bag = PropBag::parse(&index, &props);
    assert_eq!(bag.len(), 3);
    assert_eq!(bag.tiled_pairs(), 1);
    assert_eq!(bag.adjacent_pairs(), 2);
    assert!(!bag.is_contiguous());
}

// ---------------------------------------------------------------------------
// Decoding.
// ---------------------------------------------------------------------------

#[test]
fn a_valid_blob_decodes_after_the_prefix_is_stripped() {
    let entry = bag_with(&[(9, 10, prefixed(&valid_prop()))]).entries()[0].clone();
    let prop = entry.decode().expect("decode prefixed prop");
    assert_eq!((prop.image.width(), prop.image.height()), (4, 1));
    let header = entry.header().expect("parse header");
    assert_eq!((header.width, header.height), (4, 1));
}

#[test]
fn the_prefix_bytes_never_reach_the_decoder() {
    // A prefix that starts with a byte that would sniff as little-endian and a
    // bogus width; the decode must read the prop at offset 32, not these bytes.
    let mut blob = vec![0xffu8; BAG_PREFIX_LEN];
    blob[0] = 0x7f;
    blob[1] = 0x00;
    blob.extend_from_slice(&valid_prop());
    let entry = bag_with(&[(1, 2, blob)]).entries()[0].clone();
    let prop = entry.decode().expect("decode");
    assert_eq!((prop.image.width(), prop.image.height()), (4, 1));
}

#[test]
fn a_truncated_blob_is_an_error_not_a_panic() {
    // Header only, no payload: the 8-bit RLE reader makes no progress and trips
    // the runaway guard. (A payload truncated *inside* its final row is
    // deliberately tolerated by the codec, so it is not a useful failure case.)
    let full = valid_prop();
    let entry = bag_with(&[(1, 2, prefixed(&full[..12]))]).entries()[0].clone();
    assert!(entry.decode().is_err());
}

#[test]
fn a_blob_shorter_than_the_prefix_is_an_error_not_a_panic() {
    let short = vec![0xab; 20];
    let entry = bag_with(&[(1, 2, short)]).entries()[0].clone();
    assert!(entry.prop_bytes().is_none());
    assert!(matches!(
        entry.decode(),
        Err(palace_prop::PropError::BagBlobTooShort { .. })
    ));
    assert!(entry.header().is_err());
}

#[test]
fn find_locates_an_entry_by_its_identity_pair() {
    let bag = bag_with(&[
        (1, 2, prefixed(&valid_prop())),
        (3, 4, prefixed(&valid_prop())),
    ]);
    assert!(bag.find(3, 4).is_some());
    assert_eq!(bag.find(3, 4).map(BagEntry::a), Some(3));
    assert!(bag.find(4, 3).is_none());
    assert!(bag.find(99, 99).is_none());
}

// ---------------------------------------------------------------------------
// Opening from disk.
// ---------------------------------------------------------------------------

#[test]
fn open_dir_discovers_the_pids_props_pair() {
    let dir = std::env::temp_dir().join(format!("palace-bag-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (index, props) = build_bag(&[(42, 43, prefixed(&valid_prop()))]);
    write_bundle(&dir, &index, &props);

    let bag = PropBag::open_dir(&dir).expect("open bundle");
    assert_eq!(bag.len(), 1);
    assert_eq!(bag.find(42, 43).map(BagEntry::b), Some(43));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_dir_rejects_a_directory_without_a_pair() {
    let dir = std::env::temp_dir().join(format!("palace-bag-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    assert!(PropBag::open_dir(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Real bag (gated, read-only).
// ---------------------------------------------------------------------------

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
#[ignore = "reads a real PropBag.bundle copy; set PALACE_PROP_BAG to its directory"]
fn prop_bag_real_snapshot() {
    let spec = std::env::var("PALACE_PROP_BAG").unwrap_or_default();
    if spec.is_empty() {
        panic!("set PALACE_PROP_BAG to a copy of a PropBag.bundle directory");
    }
    let dir = PathBuf::from(&spec);
    let bag = PropBag::open_dir(&dir).expect("open real bag");

    println!(
        "records={} entries={} out_of_bounds={} tiling={}/{} trailing={} props_bytes={}",
        bag.records(),
        bag.len(),
        bag.out_of_bounds(),
        bag.tiled_pairs(),
        bag.adjacent_pairs(),
        bag.trailing_bytes(),
        bag.props_bytes()
    );
    assert_eq!(bag.len(), bag.records(), "every record must be usable");
    assert_eq!(bag.out_of_bounds(), 0, "no out-of-bounds records");
    assert!(bag.is_contiguous(), "blobs must tile .props");

    let (mut decoded, mut rejected) = (0usize, 0usize);
    for entry in bag.entries() {
        match entry.decode() {
            Ok(_) => decoded += 1,
            Err(_) => rejected += 1,
        }
    }
    println!("decoded={decoded} rejected={rejected}");
    assert_eq!(decoded + rejected, bag.len());

    let cache = dir.parent().map(|parent| parent.join("BagThumbCache"));
    let Some(cache) = cache else {
        println!("no BagThumbCache beside the bundle; key cross-check skipped");
        return;
    };
    if !cache.is_dir() {
        println!("{} not present; key cross-check skipped", cache.display());
        return;
    }

    let outdir = std::env::var("PALACE_PROP_BAG_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("prop-bag-verify"));
    let _ = std::fs::create_dir_all(&outdir);

    let mut keys = 0usize;
    let mut found = 0usize;
    let mut rendered = 0usize;
    let mut entries = std::fs::read_dir(&cache)
        .expect("read BagThumbCache")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect::<Vec<_>>();
    entries.sort();
    for thumb in entries {
        let Some(stem) = thumb.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((a_hex, b_hex)) = stem.split_once('_') else {
            continue;
        };
        let (Ok(a), Ok(b)) = (
            u32::from_str_radix(a_hex, 16),
            u32::from_str_radix(b_hex, 16),
        ) else {
            continue;
        };
        keys += 1;
        let Some(entry) = bag.find(a, b) else {
            println!("{stem}: MISSING from .pids");
            continue;
        };
        found += 1;
        let decoded = entry.decode();
        match &decoded {
            Ok(prop) => {
                let path = outdir.join(format!("{a:08x}_{b:08x}.png"));
                let wrote = prop.image.write_png(&path).is_ok();
                let size = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| png_size(&bytes))
                    .map(|(w, h)| format!("{w}x{h}"))
                    .unwrap_or_else(|| "?".to_string());
                println!(
                    "{stem}: found size={} -> {}x{} png={size} written={wrote}",
                    entry.size(),
                    prop.image.width(),
                    prop.image.height()
                );
                if wrote {
                    rendered += 1;
                }
            }
            Err(e) => println!("{stem}: found size={} DECODE FAILED: {e}", entry.size()),
        }
    }
    println!("thumb keys={keys} found={found} rendered={rendered}");
    assert_eq!(found, keys, "every cached key must be in .pids");
    assert!(rendered >= 3, "need at least three decoded thumbnails");
}

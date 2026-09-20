//! `BagCatalog` over synthetic `.prp` collections.
//!
//! Every collection is built in the OS temp directory from synthetic blobs
//! through the crate's own writer and then reparsed, so the catalog sees exactly
//! what a real `My Bag.prp` or shelf file would hold. Nothing here reads the
//! user's bag folder, PalaceChat's data or `$MEDIA/Prop Files/`.
//!
//! The assertions are the task's four: (a) aggregation over a synthetic My Bag
//! and shelf is the union with correct dimensions, flags, names and collection;
//! (b) a thumbnail is pixel-identical to a direct decode and is cached under the
//! `(id, crc)` key; (c) a truncated blob is skipped and counted while the rest
//! still load; (d) the JSON carries no id that is absent from the inputs — the
//! structural cache rule.

use std::collections::HashSet;
use std::path::PathBuf;

use palace_prop::bag_catalog::{thumbnail_cache_name, BagCatalog, BagCollection};
use palace_prop::prp::{AssetRec, PropKey, PropRecord, Roster};
use palace_prop::{
    asset_crc, decode, encode_s20_blob, CatalogEntry, PropImage, FLAG_GHOST, FLAG_HEAD,
};

const MY_BAG: &str = "My Bag";
const SHELF: &str = "Legacy Shelf.prp";

const ID_ALPHA: i32 = 1001;
const ID_BRAVO: i32 = 1002;
/// A high-bit id: negative when read as the signed id a `.prp` stores.
const ID_CHARLIE: i32 = 0x803D_6C40u32 as i32;
const ID_UNNAMED: i32 = 1003;

/// An id that exists in no input collection; the JSON must never mention it.
const CACHE_ONLY_ID: u32 = 4_242_424;

/// The S20 format bit the encoder always sets.
const S20_FLAG: u16 = 0x0200;

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/prp")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// A valid, empty roster to add synthetic props to. (The crate has no public
/// "new roster" constructor, so the synthetic empty fixture seeds it.)
fn empty_roster() -> Roster {
    Roster::parse(&fixture_bytes("empty.prp")).expect("parse the empty fixture")
}

fn test_image(width: u32, height: u32) -> PropImage {
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    if rgba.len() >= 4 {
        rgba[0..4].copy_from_slice(&[255, 0, 0, 255]);
    }
    if rgba.len() >= 8 {
        rgba[4..8].copy_from_slice(&[0, 0, 255, 255]);
    }
    PropImage::from_rgba(width, height, rgba).expect("build the test image")
}

fn s20_blob(width: u32, height: u32, flags: u16) -> Vec<u8> {
    encode_s20_blob(&test_image(width, height), 0, 0, flags).expect("encode an S20 prop")
}

fn alpha_blob() -> Vec<u8> {
    s20_blob(44, 44, FLAG_HEAD)
}

fn bravo_blob() -> Vec<u8> {
    s20_blob(20, 10, FLAG_GHOST)
}

fn charlie_blob() -> Vec<u8> {
    s20_blob(12, 12, 0)
}

fn unnamed_blob() -> Vec<u8> {
    s20_blob(6, 6, 0)
}

fn crc_of(blob: &[u8]) -> u32 {
    asset_crc(&blob[12..])
}

/// A roster holding `(id, name, blob)` records, inserted through the writer path.
fn roster_with(records: &[(i32, Option<&str>, Vec<u8>)]) -> Roster {
    let mut roster = empty_roster();
    for (id, name, blob) in records {
        roster.add_prop(PropRecord {
            rec: AssetRec {
                id: *id,
                ..AssetRec::default()
            },
            header: None,
            encoding: None,
            blob: blob.clone(),
            name: name.map(str::to_string),
        });
    }
    roster
}

/// Serialise and reparse, so the catalog consumes the bytes a file would hold.
fn persisted(roster: &Roster) -> Roster {
    let bytes = roster.write().expect("serialise the roster");
    Roster::parse(&bytes).expect("reparse the roster")
}

fn temp_dir(tag: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("palace-bag-catalog-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the temp dir");
    path
}

fn entry(catalog: &BagCatalog, id: u32) -> &CatalogEntry {
    catalog
        .entries()
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("id {id} is catalogued"))
}

fn json_ids(json: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut rest = json;
    while let Some(at) = rest.find("\"id\":") {
        rest = &rest[at + 5..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(id) = digits.parse::<u32>() {
            ids.push(id);
        }
        rest = &rest[digits.len()..];
    }
    ids
}

/// Dump the payload when the caller asks for an evidence artifact.
fn dump_json_artifact(json: &str) {
    if let Some(path) = std::env::var_os("TASK10_JSON_OUT") {
        std::fs::write(&path, json)
            .unwrap_or_else(|e| panic!("writing {}: {e}", path.to_string_lossy()));
    }
}

#[test]
fn aggregation_over_my_bag_and_a_shelf_is_the_union_with_correct_metadata() {
    let my_bag = persisted(&roster_with(&[
        (ID_ALPHA, Some("Alpha"), alpha_blob()),
        (ID_BRAVO, Some("Bravo"), bravo_blob()),
    ]));
    let shelf = persisted(&roster_with(&[
        (ID_BRAVO, Some("Bravo (shelf copy)"), bravo_blob()),
        (ID_CHARLIE, Some("Charlie"), charlie_blob()),
        (ID_UNNAMED, None, unnamed_blob()),
    ]));
    let catalog = BagCatalog::new([
        BagCollection::new(MY_BAG, my_bag),
        BagCollection::new(SHELF, shelf),
    ]);

    assert_eq!(catalog.len(), 4, "the union has four distinct identities");
    assert_eq!(
        catalog.duplicates(),
        1,
        "Bravo is in both collections and must collapse"
    );
    assert_eq!(catalog.skipped(), 0);
    assert!(
        catalog.entries().iter().all(|e| e.provenance.is_bag()),
        "a catalog from rosters can only hold bag-sourced entries"
    );
    for e in catalog.entries() {
        assert_eq!(
            e.provenance.key(),
            Some(PropKey::new(e.id as i32, e.crc)),
            "provenance carries the record's own identity"
        );
    }

    let alpha = entry(&catalog, ID_ALPHA as u32);
    assert_eq!(alpha.name.as_deref(), Some("Alpha"));
    assert_eq!((alpha.width, alpha.height), (44, 44));
    assert_eq!(alpha.flags, S20_FLAG | FLAG_HEAD);
    assert_eq!(alpha.crc, crc_of(&alpha_blob()));
    assert_eq!(alpha.provenance.collection(), Some(MY_BAG));
    assert!(!alpha.favorite && !alpha.trash);

    let bravos: Vec<&CatalogEntry> = catalog
        .entries()
        .iter()
        .filter(|e| e.id == ID_BRAVO as u32)
        .collect();
    assert_eq!(bravos.len(), 1, "the shared identity appears once");
    assert_eq!(bravos[0].name.as_deref(), Some("Bravo"));
    assert_eq!((bravos[0].width, bravos[0].height), (20, 10));
    assert_eq!(bravos[0].flags, S20_FLAG | FLAG_GHOST);
    assert_eq!(bravos[0].crc, crc_of(&bravo_blob()));
    assert_eq!(
        bravos[0].provenance.collection(),
        Some(MY_BAG),
        "the first collection wins the duplicate"
    );

    let charlie = entry(&catalog, ID_CHARLIE as u32);
    assert_eq!(charlie.name.as_deref(), Some("Charlie"));
    assert_eq!((charlie.width, charlie.height), (12, 12));
    assert_eq!(charlie.flags, S20_FLAG);
    assert_eq!(charlie.provenance.collection(), Some(SHELF));
    assert_eq!(charlie.crc, crc_of(&charlie_blob()));

    let unnamed = entry(&catalog, ID_UNNAMED as u32);
    assert_eq!(unnamed.name, None, "a nameless record stays nameless");
    assert_eq!(unnamed.provenance.collection(), Some(SHELF));
}

#[test]
fn a_thumbnail_is_pixel_identical_to_a_direct_decode_and_is_cached_by_identity() {
    let dir = temp_dir("thumb");
    let roster = persisted(&roster_with(&[(ID_ALPHA, Some("Alpha"), alpha_blob())]));
    let catalog = BagCatalog::new([BagCollection::new(MY_BAG, roster)]).with_cache_dir(&dir);

    let alpha = entry(&catalog, ID_ALPHA as u32);
    let png = catalog
        .thumbnail_png(alpha.id, alpha.crc)
        .expect("the known prop has a thumbnail");
    let direct = decode(&alpha_blob())
        .expect("decode the source blob")
        .image
        .to_png_bytes()
        .expect("encode the direct PNG");
    assert_eq!(png, direct, "pixel-identical to a direct decode");

    let cached = dir.join(thumbnail_cache_name(alpha.id, alpha.crc));
    assert_eq!(
        std::fs::read(&cached).expect("the cache file was written"),
        png,
        "the cache is keyed by the (id, crc) identity pair"
    );

    // The second request must be served from the cache, not the decoder: replace
    // the file with a marker and observe it come back verbatim.
    std::fs::write(&cached, b"cache-hit-marker").expect("overwrite the cache");
    assert_eq!(
        catalog.thumbnail_png(alpha.id, alpha.crc).as_deref(),
        Some(&b"cache-hit-marker"[..]),
        "a cached thumbnail is served without decoding"
    );

    assert!(
        catalog.thumbnail_png(999_999, 1).is_none(),
        "an identity absent from the inputs has no thumbnail"
    );
    assert_eq!(catalog.thumbnail_failures(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_truncated_blob_is_skipped_and_counted_while_the_rest_load() {
    let dir = temp_dir("skip");
    let mut header_only = alpha_blob();
    header_only.truncate(12); // the header parses, the payload is gone
    let short = vec![0u8; 8]; // shorter than a prop header: unusable

    let roster = persisted(&roster_with(&[
        (ID_ALPHA, Some("Alpha"), alpha_blob()),
        (ID_BRAVO, Some("Header Only"), header_only),
        (ID_UNNAMED, Some("Short"), short),
    ]));
    let catalog = BagCatalog::new([BagCollection::new(MY_BAG, roster)]).with_cache_dir(&dir);

    assert_eq!(
        catalog.len(),
        2,
        "the blob below header length is skipped, the other two stay"
    );
    assert_eq!(catalog.skipped(), 1, "and it is counted");
    assert_eq!(catalog.duplicates(), 0);

    let alpha = entry(&catalog, ID_ALPHA as u32);
    assert!(
        catalog.thumbnail_png(alpha.id, alpha.crc).is_some(),
        "the first prop still loads"
    );

    let bravo = entry(&catalog, ID_BRAVO as u32);
    assert!(
        catalog.thumbnail_png(bravo.id, bravo.crc).is_none(),
        "an undecodable payload yields no thumbnail"
    );
    assert_eq!(
        catalog.thumbnail_failures(),
        1,
        "the decode failure is counted, not fatal"
    );
    assert!(
        catalog.thumbnail_png(alpha.id, alpha.crc).is_some(),
        "the rest still load after a failure"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_json_payload_contains_only_ids_from_the_inputs() {
    let my_bag = persisted(&roster_with(&[
        (ID_ALPHA, Some("Alpha"), alpha_blob()),
        (ID_BRAVO, Some("Bravo"), bravo_blob()),
    ]));
    let shelf = persisted(&roster_with(&[(
        ID_CHARLIE,
        Some("Charlie"),
        charlie_blob(),
    )]));
    let catalog = BagCatalog::new([
        BagCollection::new(MY_BAG, my_bag),
        BagCollection::new(SHELF, shelf),
    ]);

    // Trash comes from app-owned storage, so the caller supplies the keys.
    let alpha_key = {
        let alpha = entry(&catalog, ID_ALPHA as u32);
        PropKey::new(alpha.id as i32, alpha.crc)
    };
    let catalog = catalog.with_trash([alpha_key]);

    let json = catalog.catalog_json();
    let expected: HashSet<u32> = [ID_ALPHA as u32, ID_BRAVO as u32, ID_CHARLIE as u32]
        .into_iter()
        .collect();
    let ids: HashSet<u32> = json_ids(&json).into_iter().collect();
    assert_eq!(ids, expected, "every id, and only ids, from the inputs");
    assert!(
        !ids.contains(&CACHE_ONLY_ID),
        "an id that exists only in a cache is never listed"
    );

    assert!(json.contains("\"collection\":\"My Bag\""));
    assert!(json.contains("\"collection\":\"Legacy Shelf.prp\""));
    assert_eq!(json.matches("\"source\":\"bag\"").count(), 3);
    assert_eq!(json.matches("\"trash\":true").count(), 1);
    assert_eq!(json.matches("\"fav\":false").count(), 3);

    dump_json_artifact(&json);
}

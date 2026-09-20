//! Provenance policy: a bag listing shows bag collections only.
//!
//! Everything here is built in memory from synthetic data; no user file is
//! opened and no PalaceChat path is touched. The only file ever written is the
//! evidence JSON, and only when `PALACE_TASK4_JSON` names a path (which is how
//! the task's `task-4-cache-hidden.json` is produced).

use std::path::PathBuf;

use palace_prop::catalog::{bag_listing, entries_json, merge_entries, CatalogEntry, Provenance};

/// The two collections the user owns in this fixture.
const MY_BAG: &str = "My Bag";
const SHELF: &str = "shelf:Legacy Props.PRP";

/// A stand-in for PalaceChat's live bundle: a cache, never a bag.
const CACHE_ORIGIN: &str = "PropBag.bundle:/synthetic/PalaceChat/PropBag.bundle";

/// `(id, crc)` keys listed by My Bag.
const MY_BAG_KEYS: [(u32, u32); 2] = [(7, 0x1111_1111), (8, 0x2222_2222)];
/// `(id, crc)` keys listed by the shelf.
const SHELF_KEYS: [(u32, u32); 2] = [(17, 0x3333_3333), (18, 0x4444_4444)];
/// `(id, crc)` keys that exist only in the cache. The `0x0cac` marker makes a
/// leak unmistakable in the evidence JSON.
const CACHE_ONLY_KEYS: [(u32, u32); 2] = [(0x0cac_0001, 0xaaaa_0001), (0x0cac_0002, 0xaaaa_0002)];
/// The one key deliberately present in both My Bag and the cache.
const COLLIDING_KEY: (u32, u32) = MY_BAG_KEYS[0];

fn entry(id: u32, crc: u32, provenance: Provenance) -> CatalogEntry {
    CatalogEntry {
        id,
        crc,
        name: Some(format!("prop-{id:08x}")),
        width: 44,
        height: 44,
        flags: 0x0200,
        favorite: false,
        trash: false,
        provenance,
    }
}

/// My Bag + one shelf + a synthetic cache, with one deliberate collision.
fn fixture() -> (Vec<CatalogEntry>, Vec<CatalogEntry>) {
    let mut bag = Vec::new();
    for (id, crc) in MY_BAG_KEYS {
        bag.push(entry(id, crc, Provenance::bag_with_id(MY_BAG, id, crc)));
    }
    for (id, crc) in SHELF_KEYS {
        bag.push(entry(id, crc, Provenance::bag_with_id(SHELF, id, crc)));
    }

    let mut cache = Vec::new();
    for (id, crc) in CACHE_ONLY_KEYS {
        cache.push(entry(id, crc, Provenance::cache(CACHE_ORIGIN)));
    }
    cache.push(entry(
        COLLIDING_KEY.0,
        COLLIDING_KEY.1,
        Provenance::cache(CACHE_ORIGIN),
    ));
    (bag, cache)
}

/// The merged render list: bag collections first, cache-only entries after.
fn fixture_render_list() -> Vec<CatalogEntry> {
    let (bag, cache) = fixture();
    merge_entries(bag, cache)
}

fn listed_keys(entries: &[&CatalogEntry]) -> Vec<(u32, u32)> {
    entries.iter().map(|entry| (entry.id, entry.crc)).collect()
}

#[test]
fn a_listing_built_from_my_bag_and_shelves_contains_zero_cache_only_ids() {
    let merged = fixture_render_list();
    let listing = bag_listing(&merged);

    assert_eq!(listing.len(), 4, "two My Bag props plus two shelf props");
    assert!(listing.iter().all(|entry| entry.provenance.is_bag()));

    let listed = listed_keys(&listing);
    for key in MY_BAG_KEYS.into_iter().chain(SHELF_KEYS) {
        assert!(
            listed.contains(&key),
            "bag key {key:?} is missing from the listing"
        );
    }
    for key in CACHE_ONLY_KEYS {
        assert!(
            !listed.contains(&key),
            "cache-only key {key:?} leaked into the bag listing"
        );
    }

    let collections: Vec<Option<&str>> = listing
        .iter()
        .map(|entry| entry.provenance.collection())
        .collect();
    assert!(collections.contains(&Some(MY_BAG)));
    assert!(collections.contains(&Some(SHELF)));

    assert!(
        merged.iter().any(|entry| entry.provenance.is_cache()),
        "the render list still keeps cache entries"
    );
}

#[test]
fn an_identical_id_and_crc_in_both_stores_resolves_to_exactly_one_bag_entry() {
    let merged = fixture_render_list();
    let collided: Vec<&CatalogEntry> = merged
        .iter()
        .filter(|entry| (entry.id, entry.crc) == COLLIDING_KEY)
        .collect();

    assert_eq!(
        collided.len(),
        1,
        "the collided key must appear exactly once in the merged list"
    );
    assert_eq!(
        collided[0].provenance,
        Provenance::bag_with_id(MY_BAG, COLLIDING_KEY.0, COLLIDING_KEY.1),
        "the surviving entry must be the bag one"
    );

    let listed_collisions = bag_listing(&merged)
        .iter()
        .filter(|entry| (entry.id, entry.crc) == COLLIDING_KEY)
        .count();
    assert_eq!(listed_collisions, 1);
}

#[test]
fn a_cache_entry_is_never_promoted_even_when_its_id_looks_like_a_bag_id() {
    let cache = entry(7, 0xdead_beef, Provenance::cache("room render"));
    assert!(
        bag_listing(std::slice::from_ref(&cache)).is_empty(),
        "cache provenance must never satisfy a bag query"
    );
    assert!(cache.provenance.is_cache());
}

#[test]
fn the_bag_listing_json_never_mentions_a_cache_only_id() {
    let merged = fixture_render_list();
    let listing = bag_listing(&merged);
    let json = entries_json(listing);

    for (id, crc) in MY_BAG_KEYS.into_iter().chain(SHELF_KEYS) {
        let needle = format!("\"id\":{id},\"crc\":{crc}");
        assert!(
            json.contains(&needle),
            "bag key {id}/{crc} is missing from the JSON"
        );
    }
    for (id, _) in CACHE_ONLY_KEYS {
        let needle = format!("\"id\":{id}");
        assert!(
            !json.contains(&needle),
            "cache-only id {id} appears in the bag listing JSON"
        );
    }
    assert_eq!(
        json.matches(&format!("\"id\":{}", COLLIDING_KEY.0)).count(),
        1,
        "the collided id must be listed exactly once"
    );
    assert!(json.contains(&format!("\"collection\":\"{MY_BAG}\"")));
    assert!(json.contains(&format!("\"collection\":\"{SHELF}\"")));
    assert!(
        !json.contains(CACHE_ORIGIN),
        "the cache origin must not surface in a bag listing"
    );

    if let Some(path) = std::env::var_os("PALACE_TASK4_JSON") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create evidence directory");
        }
        std::fs::write(&path, format!("{json}\n")).expect("write evidence JSON");
        eprintln!("bag listing JSON written to {}", path.display());
    }
}

//! Paged browsing and the bounded thumbnail cache over a worst-case shelf.
//!
//! The assertions are the task's four:
//!
//! * (a) a 66,885-record shelf pages in correct windows and decodes exactly the
//!   window's entries, never the whole shelf;
//! * (b) the in-memory thumbnail LRU evicts at its entry and byte caps;
//! * (c) an initial listing of the real `Palace.prp` (74 MB) peaks under the
//!   512 MB budget, measured in an isolated child process so the number is the
//!   listing's own;
//! * (d) the first page is served within the 500 ms warm budget, and the number
//!   is printed.
//!
//! The 66k shelf for (a), (b) and (d) is built here in the exact `.prp` layout
//! the reader expects — 16-byte file header, blob region, map header, type
//! table, 32-byte records, empty names blob — so the test exercises the real
//! reader without carrying a 7 MB fixture in the repository. The real-file test
//! (c) copies `Palace.prp` to a shelf under a temporary bag folder and never
//! writes to the original.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use palace_prop::bag_catalog::{BagCatalog, BagCollection};
use palace_prop::catalog_paging::{BAG_LISTING_PEAK_MEMORY_BUDGET_BYTES, FIRST_PAGE_BUDGET_MS};
use palace_prop::prp::Roster;
use palace_prop::{asset_crc, encode_s20_blob, PropImage};

/// Entries per requested window.
const PAGE: usize = 64;

/// The real worst case the task names.
const REAL_RECORDS: usize = 66_885;

/// Set in the measurement child so the test body measures instead of spawning.
const CHILD_ENV: &str = "PALACE_PAGING_MEASURE_CHILD";
/// Path of the `.prp` the measurement child must list.
const FIXTURE_ENV: &str = "PALACE_PAGING_FIXTURE";
/// The test the measure child runs; must match the function name exactly.
const MEASURE_TEST: &str = "real_shelf_initial_listing_peak_memory_stays_under_budget";

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// One valid 8x8 S20 prop blob, shared by every synthetic record.
fn synthetic_blob() -> Vec<u8> {
    encode_s20_blob(&PropImage::transparent(8, 8), 0, 0, 0x0200).expect("encode a synthetic prop")
}

/// A complete `.prp` holding `count` identical props with ids `first_id..`.
fn synthetic_prp(count: usize, first_id: i32) -> Vec<u8> {
    let blob = synthetic_blob();
    let blob_len = blob.len();
    let crc = asset_crc(&blob[12..]);

    let mut data = Vec::with_capacity(count * blob_len);
    for _ in 0..count {
        data.extend_from_slice(&blob);
    }

    let nbr_types = 1usize;
    let nbr_assets = count;
    let names_len = 0usize;
    let types_offset = 24usize;
    let recs_offset = types_offset + 12 * nbr_types;
    let names_offset = recs_offset + 32 * nbr_assets;
    let map_size = names_offset + names_len;
    let data_offset = 16usize;
    let map_offset = data_offset + data.len();

    let mut out = Vec::with_capacity(map_offset + map_size);
    put_u32(&mut out, data_offset as u32);
    put_u32(&mut out, data.len() as u32);
    put_u32(&mut out, map_offset as u32);
    put_u32(&mut out, map_size as u32);
    out.extend_from_slice(&data);

    put_i32(&mut out, nbr_types as i32);
    put_i32(&mut out, nbr_assets as i32);
    put_i32(&mut out, names_len as i32);
    put_u32(&mut out, types_offset as u32);
    put_u32(&mut out, recs_offset as u32);
    put_u32(&mut out, names_offset as u32);

    put_i32(&mut out, 0x5072_6F70u32 as i32); // "Prop"
    put_i32(&mut out, nbr_assets as i32);
    put_i32(&mut out, 0);

    for index in 0..count {
        put_i32(&mut out, first_id + index as i32);
        put_i32(&mut out, 0); // r_handle
        put_u32(&mut out, (index * blob_len) as u32);
        put_u32(&mut out, blob_len as u32);
        put_i32(&mut out, 0); // last_use_time
        put_i32(&mut out, -1); // name_offset
        put_u32(&mut out, 0); // flags
        put_u32(&mut out, crc);
    }
    out
}

fn catalog_from_prp(bytes: &[u8], name: &str) -> BagCatalog {
    let roster = Roster::parse(bytes).expect("parse the synthetic .prp");
    BagCatalog::new([BagCollection::new(name, roster)])
}

#[test]
fn a_66k_shelf_pages_in_windows_and_decodes_only_the_window() {
    let bytes = synthetic_prp(REAL_RECORDS, 1);
    let roster = Roster::parse(&bytes).expect("parse the 66k shelf");
    assert_eq!(roster.records().len(), REAL_RECORDS);
    let catalog = BagCatalog::new([BagCollection::new("Palace", roster)]);

    assert_eq!(catalog.len(), REAL_RECORDS);
    assert_eq!(
        catalog.props_decoded(),
        0,
        "aggregating a listing must not decode a single prop"
    );

    let window = catalog.page(1_000, PAGE);
    assert_eq!(window.len(), PAGE);
    assert_eq!(window[0].id, 1_001, "the window starts at the offset");
    assert_eq!(window[PAGE - 1].id, 1_064, "the window ends at the limit");

    let tail = catalog.page(REAL_RECORDS - 3, PAGE);
    assert_eq!(tail.len(), 3, "a window past the end is clamped");
    assert_eq!(tail[2].id, REAL_RECORDS as u32);

    let middle = catalog.page(50_000, 32);
    for entry in middle {
        assert!(
            catalog.thumbnail_png(entry.id, entry.crc).is_some(),
            "the window's props decode"
        );
    }
    assert_eq!(
        catalog.props_decoded(),
        32,
        "exactly the window size was decoded, not all {REAL_RECORDS}"
    );
    assert!(
        catalog.props_decoded() < catalog.len(),
        "the decoded count must prove the whole shelf was not decoded"
    );

    let json = catalog.catalog_page_json(0, 8);
    assert_eq!(
        json.matches("\"source\":\"bag\"").count(),
        8,
        "the page JSON holds the window and nothing else"
    );
}

#[test]
fn paging_can_be_scoped_to_one_collection() {
    let bag = Roster::parse(&synthetic_prp(10, 1)).expect("parse My Bag");
    let shelf = Roster::parse(&synthetic_prp(6, 100)).expect("parse the shelf");
    let catalog = BagCatalog::new([
        BagCollection::new("My Bag", bag),
        BagCollection::new("Shelf", shelf),
    ]);

    assert_eq!(catalog.len(), 16);
    assert_eq!(catalog.count_in_collection("Shelf"), 6);

    let (window, page) = catalog.page_in_collection("Shelf", 2, 4);
    assert_eq!(window.len(), 4);
    assert_eq!(
        page.total, 6,
        "the page totals the collection, not the listing"
    );
    assert!(window
        .iter()
        .all(|entry| entry.provenance.collection() == Some("Shelf")));

    let json = catalog.catalog_page_json_in_collection("Shelf", 0, 3);
    assert_eq!(json.matches("\"source\":\"bag\"").count(), 3);
    assert!(json.contains("\"collection\":\"Shelf\""));
}

#[test]
fn the_thumbnail_lru_evicts_the_least_recently_used_at_its_cap() {
    let roster = Roster::parse(&synthetic_prp(10, 1)).expect("parse");
    let catalog = BagCatalog::new([BagCollection::new("My Bag", roster)])
        .with_thumbnail_cache(4, 64 * 1024 * 1024);

    let keys: Vec<(u32, u32)> = catalog
        .entries()
        .iter()
        .map(|entry| (entry.id, entry.crc))
        .collect();
    for &(id, crc) in &keys {
        assert!(catalog.thumbnail_png(id, crc).is_some());
    }
    assert_eq!(
        catalog.props_decoded(),
        10,
        "each distinct identity is decoded once"
    );
    assert_eq!(catalog.thumbnail_cache_len(), Some(4), "the cap holds");
    assert_eq!(
        catalog.thumbnail_cache_evictions(),
        Some(6),
        "six least-recently-used thumbnails were evicted"
    );

    let before = catalog.props_decoded();
    let (id, crc) = keys[9];
    assert!(catalog.thumbnail_png(id, crc).is_some());
    assert_eq!(
        catalog.props_decoded(),
        before,
        "a hit in the LRU never touches the decoder"
    );
    assert!(catalog.thumbnail_cache_hits().unwrap_or(0) >= 1);
}

#[test]
fn the_thumbnail_lru_honours_its_byte_cap() {
    let probe = catalog_from_prp(&synthetic_prp(1, 1), "My Bag");
    let first = &probe.entries()[0];
    let png = probe
        .thumbnail_png(first.id, first.crc)
        .expect("probe thumbnail");
    let byte_cap = png.len() * 2;

    let catalog =
        catalog_from_prp(&synthetic_prp(6, 1), "My Bag").with_thumbnail_cache(64, byte_cap);
    for entry in catalog.entries() {
        let _ = catalog.thumbnail_png(entry.id, entry.crc);
    }
    assert!(
        catalog.thumbnail_cache_len().unwrap_or(0) <= 2,
        "the byte cap admits at most two of these PNGs"
    );
    assert!(catalog.thumbnail_cache_bytes().unwrap_or(0) <= byte_cap);
}

#[test]
fn the_first_catalog_page_is_served_within_budget() {
    let catalog = catalog_from_prp(&synthetic_prp(REAL_RECORDS, 1), "Palace");
    let _ = catalog.catalog_page_json(0, PAGE);

    let mut best_us = u128::MAX;
    for _ in 0..50 {
        let start = Instant::now();
        let json = catalog.catalog_page_json(0, PAGE);
        best_us = best_us.min(start.elapsed().as_micros());
        std::hint::black_box(json);
    }
    let millis = best_us as f64 / 1000.0;
    println!("first page ({PAGE} entries, warm): {millis:.3} ms");
    assert!(
        best_us <= FIRST_PAGE_BUDGET_MS * 1000,
        "first page took {millis:.3} ms, over the {FIRST_PAGE_BUDGET_MS} ms budget"
    );
}

/// Read `VmHWM` (peak resident set) in kibibytes on Linux.
fn peak_rss_kb() -> Option<u64> {
    status_value("VmHWM:")
}

/// Read `VmRSS` (current resident set) in kibibytes on Linux.
fn rss_kb() -> Option<u64> {
    status_value("VmRSS:")
}

fn status_value(prefix: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

fn fixture_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(FIXTURE_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join("Pictures/Prop Files/Palace.prp");
    path.is_file().then_some(path)
}

/// The isolated measurement: read one shelf, list it, page it, print the numbers.
fn measure_real_shelf() {
    let path = std::env::var_os(FIXTURE_ENV).expect("the child is given a fixture path");
    let rss_before = rss_kb().unwrap_or(0);

    let bytes = std::fs::read(&path).expect("read the shelf copy");
    let roster = Roster::parse(&bytes).expect("parse the shelf copy");
    let records = roster.records().len();
    drop(bytes);

    let catalog =
        BagCatalog::new([BagCollection::new("Palace", roster)]).with_default_thumbnail_cache();
    let entries = catalog.len();

    let warm = catalog.catalog_page_json(0, PAGE);
    let mut best_us = u128::MAX;
    for _ in 0..25 {
        let start = Instant::now();
        let json = catalog.catalog_page_json(0, PAGE);
        best_us = best_us.min(start.elapsed().as_micros());
        std::hint::black_box(json);
    }

    for entry in catalog.page(0, PAGE) {
        let _ = catalog.thumbnail_png(entry.id, entry.crc);
    }

    let rss_after = rss_kb().unwrap_or(0);
    let peak = peak_rss_kb().unwrap_or(0);
    println!(
        "PAGING_MEASURE records={records} entries={entries} rss_before_kb={rss_before} \
         rss_after_kb={rss_after} peak_rss_kb={peak} first_page_ms={:.3} warm_json_bytes={} \
         thumbnails_cached={}",
        best_us as f64 / 1000.0,
        warm.len(),
        catalog.thumbnail_cache_len().unwrap_or(0),
    );
}

fn measure_value(stdout: &str, key: &str) -> Option<f64> {
    let needle = format!("{key}=");
    stdout
        .lines()
        .find(|line| line.contains("PAGING_MEASURE"))
        .and_then(|line| {
            line.split_whitespace()
                .find_map(|token| token.strip_prefix(&needle))
                .and_then(|value| value.parse().ok())
        })
}

#[test]
fn real_shelf_initial_listing_peak_memory_stays_under_budget() {
    if std::env::var_os(CHILD_ENV).is_some() {
        measure_real_shelf();
        return;
    }

    let Some(fixture) = fixture_path() else {
        eprintln!("skip: no Palace.prp fixture; set {FIXTURE_ENV} to run this test");
        return;
    };

    let root = std::env::temp_dir().join(format!("palace-paging-{}", std::process::id()));
    let shelf = root.join("shelves").join("Palace.prp");
    std::fs::create_dir_all(shelf.parent().expect("shelf parent")).expect("create temp bag folder");
    std::fs::copy(&fixture, &shelf).expect("copy the shelf into the temp bag folder");

    let exe = std::env::current_exe().expect("the test binary path");
    let output = Command::new(exe)
        .args(["--exact", MEASURE_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .env(FIXTURE_ENV, &shelf)
        .output()
        .expect("run the isolated measurement child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "measurement child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let peak_kb = measure_value(&stdout, "peak_rss_kb").expect("peak_rss_kb reported");
    let page_ms = measure_value(&stdout, "first_page_ms").expect("first_page_ms reported");
    let records = measure_value(&stdout, "records").expect("records reported");
    let peak_mb = peak_kb / 1024.0;

    assert!(
        peak_kb * 1024.0 <= BAG_LISTING_PEAK_MEMORY_BUDGET_BYTES as f64,
        "initial listing of {records:.0} records peaked at {peak_mb:.1} MB, over the 512 MB budget"
    );
    assert!(
        page_ms <= FIRST_PAGE_BUDGET_MS as f64,
        "first page took {page_ms:.3} ms, over the {FIRST_PAGE_BUDGET_MS} ms budget"
    );
    println!(
        "PASS real shelf: {records:.0} records, peak {peak_mb:.1} MB, first page {page_ms:.3} ms"
    );

    let _ = std::fs::remove_dir_all(root);
}

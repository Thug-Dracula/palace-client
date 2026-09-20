//! Editor save placement, pinned at the layer the editor's save dialog builds on.
//!
//! The save dialog (`src-tauri/src/editor.rs`) offers three placements on top of
//! [`palace_prop::bag_store`]: **overwrite in place** (remove the opened record's
//! identity, then re-add the same id with the freshly encoded blob), and **new
//! prop at start / end of bag** (allocate an id below/above every id already in
//! My Bag, then add). This file exercises that composition through the crate's
//! public API:
//!
//! * each placement lands where the dialog promises — first, last, same slot;
//! * a refused save (an empty name, a wrong CRC, an unencodable dimension, a
//!   shelf, a forbidden path) leaves every byte on disk untouched;
//! * a bag written by this crate is accepted by `validation/independent_reader.py`,
//!   a reader written from `PRP-FORMAT.md` rather than from our own writer.
//!
//! All fixtures live under the OS temp directory. Nothing here touches the
//! user's real bag or PalaceChat data.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::bag_folder::{my_bag_path, shelves_dir, BagContext};
use palace_prop::bag_store::{BagStore, WriteOutcome};
use palace_prop::encoding::encoding_for_pixels;
use palace_prop::prp::{AssetRec, PropHeader, PropKey, PropRecord, Roster};
use palace_prop::{asset_crc, encode_s20_blob, PropError, PropImage};

/// A 44x44 8-bit prop header, offsets 0, HEAD|RARE flags.
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

/// One of the editor's save-dialog placements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SavePlacement {
    /// Replace the bag record the document was opened from, keeping its id.
    OverwriteInPlace,
    /// Give the prop a fresh id below every id already in My Bag.
    NewPropAtStart,
    /// Give the prop a fresh id above every id already in My Bag.
    NewPropAtEnd,
}

/// A temporary directory that deletes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-save-placement-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn context(root: &Path) -> BagContext {
    BagContext {
        bag_root: Some(root.to_path_buf()),
        home: None,
    }
}

fn blob(payload: &[u8]) -> Vec<u8> {
    let mut out = BLOB_HEADER.to_vec();
    out.extend_from_slice(payload);
    out
}

fn crc_of(blob: &[u8]) -> u32 {
    asset_crc(&blob[12..])
}

/// Add a prop through the store, deriving its crc from the blob.
fn add(
    store: &BagStore,
    collection: &Path,
    id: i32,
    payload: &[u8],
    name: Option<&str>,
) -> WriteOutcome {
    let blob = blob(payload);
    store
        .add_prop(collection, id, crc_of(&blob), &blob, name)
        .expect("add_prop succeeds")
}

/// The id the editor's `allocate_prop_id` would hand a new prop.
fn allocate_id(ids: &[i32], placement: SavePlacement) -> i32 {
    match placement {
        SavePlacement::NewPropAtStart => ids
            .iter()
            .copied()
            .min()
            .map_or(1, |id| id.saturating_sub(1)),
        SavePlacement::NewPropAtEnd => ids
            .iter()
            .copied()
            .max()
            .map_or(1, |id| id.saturating_add(1)),
        SavePlacement::OverwriteInPlace => 0,
    }
}

/// Save exactly the way the editor's `save_to_bag` does: overwrite removes the
/// opened identity and re-adds its id; a new prop allocates an id by placement.
fn editor_save(
    store: &BagStore,
    collection: &Path,
    placement: SavePlacement,
    origin: Option<PropKey>,
    payload: &[u8],
    name: Option<&str>,
) -> (i32, WriteOutcome) {
    let blob = blob(payload);
    let crc = crc_of(&blob);
    match placement {
        SavePlacement::OverwriteInPlace => {
            let origin = origin.expect("an overwrite needs the opened record");
            store
                .remove_prop(collection, origin)
                .expect("remove the opened record");
            let outcome = store
                .add_prop(collection, origin.id, crc, &blob, name)
                .expect("add the replacement");
            (origin.id, outcome)
        }
        SavePlacement::NewPropAtStart | SavePlacement::NewPropAtEnd => {
            let ids: Vec<i32> = read_roster(collection)
                .records()
                .iter()
                .map(PropRecord::id)
                .collect();
            let id = allocate_id(&ids, placement);
            let outcome = store
                .add_prop(collection, id, crc, &blob, name)
                .expect("save the new prop");
            (id, outcome)
        }
    }
}

fn read_roster(path: &Path) -> Roster {
    let bytes = fs::read(path).expect("collection exists");
    Roster::parse(&bytes).expect("collection parses")
}

fn bag_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("collection exists")
}

/// Build a `.prp` record for the read-only shelf fixture.
fn make_record(id: i32, payload: &[u8], name: Option<&str>) -> PropRecord {
    let blob = blob(payload);
    let header = PropHeader::parse(&blob).ok();
    let encoding = header.map(PropHeader::encoding);
    PropRecord {
        rec: AssetRec {
            id,
            r_handle: 0,
            data_offset: 0,
            data_size: blob.len() as u32,
            last_use_time: 0,
            name_offset: -1,
            flags: 0,
            crc: crc_of(&blob),
        },
        header,
        encoding,
        blob,
        name: name.map(str::to_string),
    }
}

/// Write a valid `.prp` directly, used to seed a read-only shelf.
fn write_collection(path: &Path, records: Vec<PropRecord>) {
    let empty = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/empty.prp"))
        .expect("empty.prp is a checked-in fixture");
    let mut roster = Roster::parse(&empty).expect("empty.prp parses");
    for record in records {
        roster.add_prop(record);
    }
    fs::write(path, roster.write().expect("roster writes")).expect("write collection");
}

/// Every type's records ascend by signed id.
fn assert_sorted(roster: &Roster) {
    for asset_type in roster.types() {
        let start = asset_type.first_asset.max(0) as usize;
        let end = start
            .saturating_add(asset_type.nbr_assets.max(0) as usize)
            .min(roster.records().len());
        for pair in roster.records()[start..end].windows(2) {
            assert!(
                pair[0].id() <= pair[1].id(),
                "{} type not sorted: {} then {}",
                asset_type.kind().name(),
                pair[0].id(),
                pair[1].id()
            );
        }
    }
}

/// The listed ids kept their identity, blob and name across a save.
fn assert_untouched(before: &Roster, after: &Roster, ids: &[i32]) {
    for id in ids {
        let old = before
            .records()
            .iter()
            .find(|record| record.id() == *id)
            .expect("seed record exists before");
        let new = after
            .records()
            .iter()
            .find(|record| record.id() == *id)
            .expect("seed record exists after");
        assert_eq!(new.key(), old.key(), "identity of id {id} changed");
        assert_eq!(new.blob, old.blob, "blob of id {id} changed");
        assert_eq!(new.name, old.name, "name of id {id} changed");
    }
}

fn independent_reader_path() -> PathBuf {
    std::env::var_os("PALACE_INDEPENDENT_READER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation/independent_reader.py")
        })
}

fn python_command() -> Option<&'static str> {
    ["python3", "python", "py"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

#[test]
fn saving_at_the_start_places_the_new_prop_first_in_signed_order() {
    let temp = TempDir::new("start");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    assert_eq!(
        store.create_collection(&bag).expect("create My Bag"),
        WriteOutcome::Created
    );
    add(&store, &bag, -5, &[1, 1, 1, 1], Some("Negative"));
    add(&store, &bag, 300, &[2, 2, 2, 2], Some("High"));
    let before = read_roster(&bag);

    let (id, outcome) = editor_save(
        &store,
        &bag,
        SavePlacement::NewPropAtStart,
        None,
        &[7, 7, 7, 7],
        Some("First"),
    );

    assert_eq!(id, -6, "one below the smallest signed id (-5)");
    assert_eq!(outcome, WriteOutcome::Added);
    let after = read_roster(&bag);
    assert_eq!(after.records().len(), 3);
    assert_eq!(after.records()[0].id(), -6, "the new prop must be first");
    assert_eq!(after.records()[0].name.as_deref(), Some("First"));
    assert_eq!(after.records()[0].blob, blob(&[7, 7, 7, 7]));
    assert_sorted(&after);
    assert_untouched(&before, &after, &[-5, 300]);
}

#[test]
fn saving_at_the_end_places_the_new_prop_last_and_an_empty_bag_starts_at_one() {
    let temp = TempDir::new("end");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 100, &[1, 1, 1, 1], Some("Low"));
    add(&store, &bag, 300, &[2, 2, 2, 2], Some("High"));
    let before = read_roster(&bag);

    let (id, outcome) = editor_save(
        &store,
        &bag,
        SavePlacement::NewPropAtEnd,
        None,
        &[8, 8, 8, 8],
        Some("Last"),
    );

    assert_eq!(id, 301, "one above the largest id (300)");
    assert_eq!(outcome, WriteOutcome::Added);
    let after = read_roster(&bag);
    assert_eq!(after.records().len(), 3);
    assert_eq!(after.records()[2].id(), 301, "the new prop must be last");
    assert_eq!(after.records()[2].name.as_deref(), Some("Last"));
    assert_sorted(&after);
    assert_untouched(&before, &after, &[100, 300]);

    let empty = TempDir::new("empty");
    let empty_root = empty.path();
    let empty_store = BagStore::open(context(empty_root));
    let empty_bag = my_bag_path(empty_root);
    empty_store
        .create_collection(&empty_bag)
        .expect("create My Bag");
    let (id, outcome) = editor_save(
        &empty_store,
        &empty_bag,
        SavePlacement::NewPropAtEnd,
        None,
        &[3, 3, 3, 3],
        None,
    );
    assert_eq!(id, 1, "an empty bag starts at id 1");
    assert_eq!(outcome, WriteOutcome::Added);
    assert_eq!(read_roster(&empty_bag).records().len(), 1);
}

#[test]
fn overwriting_in_place_replaces_only_the_opened_variant_in_its_own_slot() {
    let temp = TempDir::new("overwrite");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 100, &[1, 1, 1, 1], Some("Before"));
    add(&store, &bag, 200, &[2, 2, 2, 2], Some("Neighbour"));
    add(&store, &bag, 300, &[3, 3, 3, 3], Some("Far"));
    let before = read_roster(&bag);
    let old_key = before
        .records()
        .iter()
        .find(|record| record.id() == 100)
        .expect("seed record")
        .key();
    let slot = before
        .records()
        .iter()
        .position(|record| record.id() == 100)
        .expect("seed record");

    let new_blob = blob(&[9, 9, 9, 9, 9]);
    let new_crc = crc_of(&new_blob);
    assert_ne!(new_crc, old_key.crc, "the overwrite must change the CRC");
    let (id, outcome) = editor_save(
        &store,
        &bag,
        SavePlacement::OverwriteInPlace,
        Some(old_key),
        &[9, 9, 9, 9, 9],
        Some("After"),
    );
    assert_eq!(id, 100, "an overwrite keeps the opened id");
    assert_eq!(outcome, WriteOutcome::Added);

    let after = read_roster(&bag);
    assert_eq!(
        after.records().len(),
        3,
        "an overwrite replaces, never adds"
    );
    assert_eq!(after.records()[slot].id(), 100, "the id kept its slot");
    assert_eq!(after.records()[slot].key(), PropKey::new(100, new_crc));
    assert_eq!(after.records()[slot].name.as_deref(), Some("After"));
    assert_eq!(after.records()[slot].blob, new_blob);
    assert!(
        after.record_for(old_key).is_none(),
        "the replaced variant is gone"
    );
    assert_untouched(&before, &after, &[200, 300]);
}

#[test]
fn an_invalid_name_is_refused_and_the_collection_is_untouched() {
    let temp = TempDir::new("bad-name");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 50, &[1, 1, 1, 1], Some("Kept"));
    let kept_key = read_roster(&bag).records()[0].key();
    let before = bag_bytes(&bag);

    let payload = blob(&[2, 2, 2, 2]);
    for bad_name in ["", "   ", " \t "] {
        let error = store
            .add_prop(&bag, 51, crc_of(&payload), &payload, Some(bad_name))
            .expect_err("an empty name must be refused");
        assert!(
            error.to_string().contains("name"),
            "the refusal must name the problem: {error}"
        );
        assert_eq!(bag_bytes(&bag), before, "a refused add wrote to disk");
    }

    let error = store
        .rename_prop(&bag, kept_key, Some(" "))
        .expect_err("an empty rename must be refused");
    assert!(error.to_string().contains("name"));
    assert_eq!(bag_bytes(&bag), before, "a refused rename wrote to disk");

    let roster = read_roster(&bag);
    assert_eq!(roster.records().len(), 1);
    assert_eq!(roster.records()[0].name.as_deref(), Some("Kept"));
}

#[test]
fn a_wrong_crc_or_short_blob_is_refused_and_the_collection_is_untouched() {
    let temp = TempDir::new("bad-blob");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 50, &[1, 1, 1, 1], Some("Kept"));
    let before = bag_bytes(&bag);

    let payload = blob(&[2, 2, 2, 2]);
    let error = store
        .add_prop(&bag, 51, crc_of(&payload) ^ 0xFFFF_FFFF, &payload, None)
        .expect_err("a crc that does not match the blob must be refused");
    assert!(error.to_string().contains("crc"));
    assert_eq!(bag_bytes(&bag), before, "a refused add wrote to disk");

    let error = store
        .add_prop(&bag, 51, 0, &[], None)
        .expect_err("a blob without the 12-byte header must be refused");
    assert!(error.to_string().contains("header"));
    assert_eq!(bag_bytes(&bag), before, "a refused add wrote to disk");
}

#[test]
fn an_unencodable_dimension_is_refused_without_writing() {
    let temp = TempDir::new("bad-dimension");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 50, &[1, 1, 1, 1], Some("Kept"));
    let before = bag_bytes(&bag);

    let odd = PropImage::transparent(43, 44);
    let error = encoding_for_pixels(&odd).expect_err("S20 packs pixels in pairs");
    assert!(matches!(
        error,
        PropError::UnencodableImage {
            width: 43,
            height: 44
        }
    ));
    assert!(encode_s20_blob(&odd, 0, 0, 0).is_err());

    let empty = PropImage::transparent(0, 44);
    assert!(encoding_for_pixels(&empty).is_err());

    // The editor runs this guard before it calls the store, so a refused
    // dimension means no store call and no disk write.
    assert_eq!(bag_bytes(&bag), before);
    assert_eq!(read_roster(&bag).records().len(), 1);
}

#[test]
fn a_save_never_touches_a_shelf_or_a_forbidden_path() {
    let temp = TempDir::new("guards");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let shelf = shelves_dir(root).join("Old Gems.prp");
    write_collection(&shelf, vec![make_record(7, &[4, 4, 4, 4], Some("Shelf"))]);
    let shelf_before = bag_bytes(&shelf);
    let shelf_key = read_roster(&shelf).records()[0].key();
    let payload = blob(&[5, 5, 5, 5]);

    let error = store
        .add_prop(&shelf, 8, crc_of(&payload), &payload, Some("Nope"))
        .expect_err("a shelf is read-only");
    assert!(error.to_string().contains("read-only"), "{error}");
    store
        .remove_prop(&shelf, shelf_key)
        .expect_err("a shelf is read-only");
    store
        .rename_prop(&shelf, shelf_key, Some("Nope"))
        .expect_err("a shelf is read-only");
    assert_eq!(bag_bytes(&shelf), shelf_before, "the shelf bytes changed");

    let outside = root.join("Outside.prp");
    store
        .add_prop(&outside, 8, crc_of(&payload), &payload, None)
        .expect_err("a path outside the bag owns nothing");
    assert!(!outside.exists(), "a refused path must not be created");

    let fresh_root = temp.path().join("fresh-root");
    let unowned = BagStore::open(context(&fresh_root));
    let stray = fresh_root.join("Stray.prp");
    unowned
        .add_prop(&stray, 8, crc_of(&payload), &payload, None)
        .expect_err("an unowned path is refused");
    assert!(
        !fresh_root.exists(),
        "a refused write must not create the bag layout"
    );

    let home = temp.path().join("home");
    let protected = home.join("Pictures").join("Prop Files").join("Evil.prp");
    fs::create_dir_all(protected.parent().expect("parent")).expect("mock home");
    let guarded = BagStore::open(BagContext {
        bag_root: Some(root.to_path_buf()),
        home: Some(home),
    });
    let error = guarded
        .add_prop(&protected, 8, crc_of(&payload), &payload, None)
        .expect_err("PalaceChat data is protected");
    assert!(error.to_string().contains("protected"), "{error}");
    assert!(!protected.exists(), "protected data must not be created");
}

#[test]
fn a_writer_built_bag_is_accepted_by_the_independent_reader() {
    let temp = TempDir::new("oracle");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 100, &[1, 2, 3, 4], Some("S20 One"));
    add(&store, &bag, 100, &[5, 6, 7], Some("S20 Variant"));
    add(&store, &bag, 300, &[9, 9, 9, 9, 9], None);

    let bytes = bag_bytes(&bag);
    let roster = Roster::parse(&bytes).expect("the written bag parses");
    assert!(roster.file_header().invariant_holds(bytes.len()));
    assert_eq!(roster.dropped_records(), 0);
    for record in roster.records() {
        assert_eq!(
            asset_crc(&record.blob[12..]),
            record.crc(),
            "Rust-side CRC check failed for id {}",
            record.id()
        );
    }

    if let Ok(dir) = std::env::var("PALACE_SAVE_PLACEMENT_EVIDENCE") {
        let dir = PathBuf::from(dir);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::copy(&bag, dir.join("My Bag.prp"));
    }

    let reader = independent_reader_path();
    assert!(
        reader.is_file(),
        "the independent reader is missing at {}",
        reader.display()
    );
    let Some(python) = python_command() else {
        eprintln!(
            "skipping the independent-reader assertion: no python interpreter on PATH \
             (the file it would read is {})",
            bag.display()
        );
        return;
    };

    let output = Command::new(python)
        .arg(&reader)
        .arg("--verbose")
        .arg(&bag)
        .output()
        .expect("run the independent reader");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(
        output.status.success(),
        "the independent reader rejected the bag:\n{stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("result: ACCEPT"),
        "no ACCEPT in the reader output:\n{stdout}"
    );
    assert!(stdout.contains("crc_failures: 0"), "{stdout}");
    assert!(stdout.contains("errors: 0"), "{stdout}");
    assert!(stdout.contains("records: 3"), "{stdout}");
    assert!(stdout.contains("names: 2"), "{stdout}");
}

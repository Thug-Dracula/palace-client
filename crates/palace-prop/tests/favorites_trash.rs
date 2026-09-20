//! Favourites (the `.prp` `Fave` record) and the trash store, end to end.
//!
//! Every fixture is built under the OS temp directory with a synthetic bag
//! folder, exactly like `tests/outfits.rs` and `tests/bag_layout.rs`. The only
//! checked-in files read are the `.prp` golden fixtures; nothing here touches
//! the user's real bag, PalaceChat data or `$MEDIA/Prop Files/`.
//!
//! The three cases the plan calls for:
//!
//! * `a_favourite_survives_a_round_trip_and_unfavourite_removes_it`
//! * `delete_moves_the_prop_to_trash_and_restore_is_lossless`
//! * `an_empty_trash_is_valid`
//!
//! plus the read-only real-collection case, the no-op byte-exactness case and
//! the corrupt-document case around them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::asset_crc;
use palace_prop::bag_folder::{my_bag_path, BagContext};
use palace_prop::favorites_trash::{
    fave_record, trash_path, Favorites, TrashStore, FAVE_RECORD_ID, FAVE_SPEC_LEN,
};
use palace_prop::prp::{
    AssetRec, AssetType, AssetTypeRec, PropHeader, PropKey, PropRecord, Roster,
};

/// 44x44, offsets 0, flags 0x000a (HEAD|RARE, 8-bit).
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

/// A temporary directory that deletes itself.
struct TempDir(PathBuf);

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-favorites-trash-{tag}-{}-{n}",
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

/// A synthetic bag folder: the bag root and the fake home are the same temp
/// directory, so the write guard sees a normal, writable bag.
struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    context: BagContext,
}

fn fixture(tag: &str) -> Fixture {
    let dir = TempDir::new(tag);
    let root = dir.path().to_path_buf();
    fs::create_dir_all(&root).expect("create bag root");
    let context = BagContext {
        bag_root: Some(root.clone()),
        home: Some(root.clone()),
    };
    Fixture {
        _dir: dir,
        root,
        context,
    }
}

fn fixture_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/prp")
        .join(name)
}

fn blob(payload: &[u8]) -> Vec<u8> {
    let mut out = BLOB_HEADER.to_vec();
    out.extend_from_slice(payload);
    out
}

fn make_record(id: i32, payload: &[u8], name: Option<&str>) -> PropRecord {
    let blob = blob(payload);
    let header = PropHeader::parse(&blob).ok();
    let encoding = header.map(PropHeader::encoding);
    let rec = AssetRec {
        id,
        r_handle: 0,
        data_offset: 0,
        data_size: blob.len() as u32,
        last_use_time: 0,
        name_offset: -1,
        flags: 0,
        crc: asset_crc(&blob[12..]),
    };
    PropRecord {
        rec,
        header,
        encoding,
        blob,
        name: name.map(str::to_string),
    }
}

fn empty_roster() -> Roster {
    let bytes = fs::read(fixture_file("empty.prp")).expect("empty.prp is checked in");
    Roster::parse(&bytes).expect("empty.prp parses")
}

fn write_bag(fixture: &Fixture, roster: &Roster) {
    let bytes = roster.write().expect("the bag roster writes");
    fixture
        .context
        .atomic_write(&my_bag_path(&fixture.root), &bytes)
        .expect("the bag file is replaceable");
}

fn read_bag(fixture: &Fixture) -> Roster {
    let bytes = fs::read(my_bag_path(&fixture.root)).expect("the bag file exists");
    Roster::parse(&bytes).expect("the bag file parses")
}

/// The single type table entry of `kind`, if the roster declares one.
fn type_rec(roster: &Roster, kind: AssetType) -> Option<AssetTypeRec> {
    roster
        .types()
        .iter()
        .copied()
        .find(|entry| entry.kind() == kind)
}

/// Assert the favourites record is owned by the `Fave` type table section.
///
/// This is the check the old suite was missing: it looked at the record's id and
/// payload but never at which section of the type table counted it, so an 8-byte
/// favourites payload sitting in the `Prop` run passed. Here `Prop.nbrAssets`
/// must not have grown for it, `Fave.nbrAssets` must include it, and the record
/// must fall inside the `Fave` run and outside the `Prop` run.
fn assert_fave_section(roster: &Roster, expected_props: i32, expected_faves: i32) {
    let fave_types = roster
        .types()
        .iter()
        .filter(|entry| entry.kind() == AssetType::Fave)
        .count();
    assert_eq!(fave_types, 1, "exactly one Fave type record must exist");

    let prop = type_rec(roster, AssetType::Prop).expect("the Prop type is present");
    let fave = type_rec(roster, AssetType::Fave).expect("the Fave type is present");
    assert_eq!(
        prop.nbr_assets, expected_props,
        "Prop.nbrAssets must exclude the favourites record"
    );
    assert_eq!(
        fave.nbr_assets, expected_faves,
        "the favourites record must be counted under Fave"
    );

    let total: i32 = roster.types().iter().map(|entry| entry.nbr_assets).sum();
    assert_eq!(
        total as usize,
        roster.records().len(),
        "type counts must sum to the record count"
    );

    let index = roster
        .records()
        .iter()
        .position(|record| record.id() == FAVE_RECORD_ID)
        .expect("the favourites record is present");
    let fave_start = fave.first_asset.max(0) as usize;
    let fave_end = fave_start + fave.nbr_assets.max(0) as usize;
    assert!(
        (fave_start..fave_end).contains(&index),
        "the favourites record must sit in the Fave run [{fave_start}..{fave_end}) but is at {index}"
    );
    let prop_start = prop.first_asset.max(0) as usize;
    let prop_end = prop_start + prop.nbr_assets.max(0) as usize;
    assert!(
        !(prop_start..prop_end).contains(&index),
        "the favourites record must not sit in the Prop run [{prop_start}..{prop_end})"
    );
}

#[test]
fn a_favourite_survives_a_round_trip_and_unfavourite_removes_it() {
    let fixture = fixture("fave-roundtrip");
    let mut roster = empty_roster();
    roster.add_prop(make_record(1, &[1, 2, 3, 4], Some("One")));
    roster.add_prop(make_record(2, &[5, 6, 7, 8], Some("Two")));
    roster.add_prop(make_record(3, &[9, 10, 11, 12], Some("Three")));
    let originals: Vec<(PropKey, Vec<u8>)> = roster
        .records()
        .iter()
        .map(|record| (record.key(), record.blob.clone()))
        .collect();
    write_bag(&fixture, &roster);

    // Nothing has been favourited: there is no Fave record at all.
    let roster = read_bag(&fixture);
    assert!(fave_record(&roster).is_none());
    let mut favorites = Favorites::read(&roster);
    assert!(favorites.is_empty());

    assert!(favorites.add(originals[0].0));
    assert!(favorites.add(originals[2].0));
    assert!(
        !favorites.add(originals[0].0),
        "a duplicate favourite is stored once"
    );
    assert!(favorites.contains(originals[2].0));
    assert!(!favorites.contains(originals[1].0));
    assert_eq!(favorites.len(), 2);

    let mut roster = read_bag(&fixture);
    favorites.apply_to(&mut roster).expect("favourites apply");
    write_bag(&fixture, &roster);

    // Write, reparse: still favourite.
    let roster = read_bag(&fixture);
    let favorites = Favorites::read(&roster);
    assert_eq!(favorites.keys(), &[originals[0].0, originals[2].0]);
    assert!(favorites.contains(originals[0].0));
    assert!(!favorites.contains(originals[1].0));

    // The record carrying the list is the Fave record: id 128, name "".
    let record = fave_record(&roster).expect("the favourite record is present");
    assert_eq!(record.id(), FAVE_RECORD_ID);
    assert_eq!(record.name.as_deref(), Some(""));
    assert_eq!(record.blob.len(), 2 * FAVE_SPEC_LEN);
    assert_eq!(record.blob, favorites.encode_payload());
    assert_fave_section(&roster, 3, 1);

    // Every prop record survives with its blob byte for byte.
    for (key, blob) in &originals {
        assert_eq!(
            roster.blob_for(*key),
            Some(blob.as_slice()),
            "prop {key:?} must survive the favourite edit"
        );
    }

    // Unfavourite key 1: only key 3 stays, in a one-spec payload.
    let mut favorites = Favorites::read(&roster);
    assert!(favorites.remove(originals[0].0));
    assert!(
        !favorites.remove(originals[0].0),
        "removing twice is a no-op"
    );
    let mut roster = read_bag(&fixture);
    favorites
        .apply_to(&mut roster)
        .expect("unfavourite applies");
    write_bag(&fixture, &roster);

    let roster = read_bag(&fixture);
    let favorites = Favorites::read(&roster);
    assert_eq!(favorites.keys(), &[originals[2].0]);
    assert!(!favorites.contains(originals[0].0));
    let record = fave_record(&roster).expect("the favourite record stays");
    assert_eq!(record.id(), FAVE_RECORD_ID);
    assert_eq!(record.blob, favorites.encode_payload());
    assert_fave_section(&roster, 3, 1);

    // Unfavourite everything: the empty sentinel record stays.
    let mut favorites = Favorites::read(&roster);
    assert!(favorites.remove(originals[2].0));
    assert!(favorites.is_empty());
    let mut roster = read_bag(&fixture);
    favorites.apply_to(&mut roster).expect("clearing applies");
    write_bag(&fixture, &roster);

    let roster = read_bag(&fixture);
    assert!(Favorites::read(&roster).is_empty());
    let record = fave_record(&roster).expect("the empty sentinel stays");
    assert_eq!(record.id(), FAVE_RECORD_ID);
    assert!(record.blob.is_empty());
    assert_fave_section(&roster, 3, 1);
}

#[test]
fn favourites_stay_in_an_existing_fave_section() {
    let fixture = fixture("fave-section-exists");
    let original = fs::read(fixture_file("prop_fave.prp")).expect("prop_fave.prp is checked in");
    let parsed = Roster::parse(&original).expect("prop_fave.prp parses");
    let props_before = type_rec(&parsed, AssetType::Prop)
        .expect("the fixture has a Prop type")
        .nbr_assets;
    let faves_before = type_rec(&parsed, AssetType::Fave)
        .expect("the fixture has a Fave type")
        .nbr_assets;
    assert_eq!(
        (props_before, faves_before),
        (1, 1),
        "the fixture is one prop plus the Fave sentinel"
    );

    let prop_key = parsed
        .records()
        .iter()
        .find(|record| record.id() != FAVE_RECORD_ID)
        .expect("the fixture has a prop")
        .key();
    fixture
        .context
        .atomic_write(&my_bag_path(&fixture.root), &original)
        .expect("the fixture is copied into the bag");

    let mut favorites = Favorites::new();
    assert!(favorites.add(prop_key));
    let mut roster = read_bag(&fixture);
    favorites.apply_to(&mut roster).expect("favourites apply");
    write_bag(&fixture, &roster);

    if let Ok(dir) = std::env::var("PALACE_FAVE_EVIDENCE_DIR") {
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(
            Path::new(&dir).join("fave-existing-edited.prp"),
            roster.write().expect("the edited roster writes"),
        );
    }

    assert_fave_section(&roster, props_before, faves_before);
    let record = fave_record(&roster).expect("the favourite record is present");
    assert_eq!(record.blob, favorites.encode_payload());

    let reparsed = read_bag(&fixture);
    assert_fave_section(&reparsed, props_before, faves_before);
    assert_eq!(Favorites::read(&reparsed).keys(), &[prop_key]);
}

#[test]
fn favourites_create_a_fave_section_when_none_exists() {
    let fixture = fixture("fave-section-missing");
    let original =
        fs::read(fixture_file("single_type.prp")).expect("single_type.prp is checked in");
    let parsed = Roster::parse(&original).expect("single_type.prp parses");
    let props_before = type_rec(&parsed, AssetType::Prop)
        .expect("the fixture has a Prop type")
        .nbr_assets;
    assert!(
        type_rec(&parsed, AssetType::Fave).is_none(),
        "the fixture declares no Fave section"
    );

    let prop_key = parsed
        .records()
        .first()
        .expect("the fixture has a prop")
        .key();
    fixture
        .context
        .atomic_write(&my_bag_path(&fixture.root), &original)
        .expect("the fixture is copied into the bag");

    let mut favorites = Favorites::new();
    assert!(favorites.add(prop_key));
    let mut roster = read_bag(&fixture);
    favorites.apply_to(&mut roster).expect("favourites apply");
    write_bag(&fixture, &roster);

    assert_fave_section(&roster, props_before, 1);
    let reparsed = read_bag(&fixture);
    assert_fave_section(&reparsed, props_before, 1);
    assert_eq!(Favorites::read(&reparsed).keys(), &[prop_key]);
}

#[test]
fn an_existing_fave_sentinel_is_replaced_by_the_favourite_list() {
    let fixture = fixture("fave-sentinel");
    // A real-shaped collection: one prop plus the size-0 Fave sentinel.
    let original = fs::read(fixture_file("prop_fave.prp")).expect("prop_fave.prp is checked in");
    let roster = Roster::parse(&original).expect("prop_fave.prp parses");
    assert!(fave_record(&roster).is_some(), "the fixture carries it");
    let prop_key = roster
        .records()
        .iter()
        .find(|record| record.id() != FAVE_RECORD_ID)
        .expect("the fixture has a prop")
        .key();
    let prop_blob = roster.blob_for(prop_key).expect("the prop blob").to_vec();

    fixture
        .context
        .atomic_write(&my_bag_path(&fixture.root), &original)
        .expect("the fixture is copied into the bag");

    let mut favorites = Favorites::read(&read_bag(&fixture));
    assert!(favorites.is_empty(), "a size-0 sentinel is an empty list");
    assert!(favorites.add(prop_key));

    let mut guard_roster = read_bag(&fixture);
    let sentinel = fave_record(&guard_roster).expect("sentinel").key();
    assert!(
        TrashStore::open(fixture.context.clone())
            .trash(&mut guard_roster, sentinel)
            .is_err(),
        "the Fave record is not a prop and must never be trashable"
    );

    let mut roster = read_bag(&fixture);
    favorites.apply_to(&mut roster).expect("favourites apply");
    write_bag(&fixture, &roster);

    let roster = read_bag(&fixture);
    assert_eq!(Favorites::read(&roster).keys(), &[prop_key]);
    let record = fave_record(&roster).expect("the favourite record is present");
    assert_eq!(record.id(), FAVE_RECORD_ID);
    assert_eq!(record.blob, favorites.encode_payload());
    assert_eq!(roster.blob_for(prop_key), Some(prop_blob.as_slice()));
}

#[test]
fn a_real_collection_fave_record_reads_without_trusting_its_crc() {
    let bytes =
        fs::read(fixture_file("real_palace_hidden.prp")).expect("the fixture is checked in");
    let roster = Roster::parse(&bytes).expect("the real fixture parses");

    let record = fave_record(&roster).expect("the real collection carries a Fave record");
    assert_eq!(record.id(), FAVE_RECORD_ID);
    assert_eq!(record.blob.len(), FAVE_SPEC_LEN);
    assert_eq!(
        record.crc(),
        0x1f9c_6739,
        "the stored crc is a pinned real value"
    );
    assert_ne!(
        record.crc(),
        asset_crc(&record.blob),
        "the Fave crc is not a payload hash and must not be validated as one"
    );

    let favorites = Favorites::read(&roster);
    assert_eq!(
        favorites.keys(),
        &[PropKey::new(1_675_473_842, 1_347_523_522)]
    );

    // A favourite edit leaves every other record alone, blob for blob. The
    // one real record with a stale CRC gets it repaired by the insert path, so
    // the comparison is on (id, blob), not on the stale key.
    let untouched: Vec<(i32, Vec<u8>)> = roster
        .records()
        .iter()
        .filter(|record| record.id() != FAVE_RECORD_ID)
        .map(|record| (record.id(), record.blob.clone()))
        .collect();
    let total = roster.records().len();
    let mut changed = favorites;
    assert!(changed.add(PropKey::new(7, 8)));
    let mut edited = Roster::parse(&bytes).expect("reparses");
    changed.apply_to(&mut edited).expect("favourites apply");
    let reparsed =
        Roster::parse(&edited.write().expect("writes")).expect("rewritten roster parses");

    assert_eq!(Favorites::read(&reparsed).keys(), changed.keys());
    assert_eq!(reparsed.records().len(), total, "no record is lost");
    for (id, blob) in &untouched {
        assert!(
            reparsed
                .records()
                .iter()
                .any(|record| record.id() == *id && record.blob == *blob),
            "prop id {id} must survive the favourite edit with its blob"
        );
    }
}

#[test]
fn a_no_op_favourites_write_is_byte_identical() {
    let fixture = fixture("fave-noop");
    let mut roster = empty_roster();
    roster.add_prop(make_record(10, &[1, 2, 3, 4], None));
    let key = roster.records()[0].key();
    let mut favorites = Favorites::new();
    assert!(favorites.add(key));
    favorites.apply_to(&mut roster).expect("favourites apply");
    write_bag(&fixture, &roster);

    let before = fs::read(my_bag_path(&fixture.root)).expect("the bag file exists");
    let mut roster = Roster::parse(&before).expect("parses");
    Favorites::read(&roster)
        .apply_to(&mut roster)
        .expect("a no-op apply");
    assert_eq!(
        roster.write().expect("writes"),
        before,
        "a no-op favourites call must not disturb the file"
    );
}

#[test]
fn delete_moves_the_prop_to_trash_and_restore_is_lossless() {
    let fixture = fixture("trash-roundtrip");
    let mut roster = empty_roster();
    roster.add_prop(make_record(42, &[1, 2, 3, 4, 5, 6, 7, 8], Some("FortyTwo")));
    roster.add_prop(make_record(43, &[9, 9, 9, 9], Some("FortyThree")));
    let victim = roster.records().iter().find(|r| r.id() == 42).unwrap();
    let key = victim.key();
    let blob = victim.blob.clone();
    let keeper = roster
        .records()
        .iter()
        .find(|r| r.id() == 43)
        .unwrap()
        .key();
    write_bag(&fixture, &roster);

    let mut trash = TrashStore::open(fixture.context.clone());
    assert!(trash.is_empty());
    assert!(trash.load_error().is_none());
    assert_eq!(trash.path(), Some(trash_path(&fixture.root).as_path()));

    // Delete: gone from the bag, present in the trash.
    let mut roster = read_bag(&fixture);
    assert!(trash.trash(&mut roster, key).expect("trash the prop"));
    assert!(roster.record_for(key).is_none(), "gone in memory");
    write_bag(&fixture, &roster);

    let roster = read_bag(&fixture);
    assert!(roster.record_for(key).is_none(), "gone from the file");
    assert!(roster.record_for(keeper).is_some(), "the other prop stays");

    let reopened = TrashStore::open(fixture.context.clone());
    assert!(reopened.contains(key));
    let entry = reopened
        .entries()
        .iter()
        .find(|entry| entry.key == key)
        .expect("the entry is stored");
    assert_eq!(entry.blob, blob, "the trashed blob is complete");
    assert_eq!(entry.name.as_deref(), Some("FortyTwo"));

    // Restore: identical (id, crc) and byte-identical blob.
    let mut trash = TrashStore::open(fixture.context.clone());
    let mut roster = read_bag(&fixture);
    let restored = trash
        .restore_from_trash(&mut roster, key)
        .expect("restore the prop");
    assert_eq!(restored, key, "the restored identity is identical");
    write_bag(&fixture, &roster);

    let roster = read_bag(&fixture);
    let record = roster.record_for(key).expect("the prop came back");
    assert_eq!(record.blob, blob, "the blob is byte-identical");
    assert_eq!(record.name.as_deref(), Some("FortyTwo"));
    assert!(
        !TrashStore::open(fixture.context.clone()).contains(key),
        "restore removes the trash entry"
    );

    // Purge empties the trash and leaves a valid document behind.
    let mut trash = TrashStore::open(fixture.context.clone());
    let mut roster = read_bag(&fixture);
    assert!(trash.trash(&mut roster, key).expect("trash again"));
    write_bag(&fixture, &roster);
    assert!(!TrashStore::open(fixture.context.clone()).is_empty());

    trash.purge_trash().expect("purge the trash");
    assert!(trash.is_empty());
    let reopened = TrashStore::open(fixture.context.clone());
    assert!(reopened.is_empty());
    assert!(reopened.load_error().is_none());
    assert!(fs::read(trash_path(&fixture.root))
        .expect("the purged document exists")
        .starts_with(b"palace-prop-trash 1\n"));
}

#[test]
fn an_empty_trash_is_valid() {
    let fixture = fixture("empty-trash");
    let store = TrashStore::open(fixture.context.clone());
    assert!(store.is_empty());
    assert!(
        store.load_error().is_none(),
        "a missing trash document is a normal first run"
    );

    let mut store = TrashStore::open(fixture.context.clone());
    store
        .purge_trash()
        .expect("purging an empty trash succeeds");
    assert!(store.is_empty());
    let path = trash_path(&fixture.root);
    assert!(path.exists());
    let reopened = TrashStore::open(fixture.context.clone());
    assert!(reopened.is_empty());
    assert!(reopened.load_error().is_none());

    // An unknown key is a clean error, not a panic and not a write.
    let before = fs::read(&path).expect("the document exists");
    let mut store = TrashStore::open(fixture.context.clone());
    let mut roster = empty_roster();
    assert!(store
        .restore_from_trash(&mut roster, PropKey::new(7, 7))
        .is_err());
    assert_eq!(fs::read(&path).expect("the document is untouched"), before);
}

#[test]
fn a_corrupt_trash_document_loads_empty_with_an_error_and_is_untouched() {
    let fixture = fixture("trash-corrupt");
    let path = trash_path(&fixture.root);
    fs::write(&path, b"palace-prop-trash 1\n1 2 not-hex 00\n").expect("write the corrupt document");
    let before = fs::read(&path).expect("read back");

    let store = TrashStore::open(fixture.context.clone());
    assert!(store.is_empty());
    assert!(store.load_error().is_some());
    assert_eq!(
        fs::read(&path).expect("read back"),
        before,
        "loading never rewrites the document"
    );
}

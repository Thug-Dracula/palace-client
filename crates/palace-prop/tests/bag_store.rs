//! Acceptance tests for the bag store: every mutation survives a reparse, a
//! shelf refuses writes and stays byte-identical, two concurrent writers cannot
//! corrupt the collection, a duplicate add is a no-op, and a removed prop can be
//! restored bit-for-bit.
//!
//! Every fixture is built under the OS temp directory. Nothing here touches the
//! user's real bag or PalaceChat data.
//!
//! The concurrency test runs the same operation from **two OS processes** using
//! the test binary itself: the parent re-invokes `worker_adds_one_prop` (marked
//! `#[ignore]`, so a normal run skips it) with the temp root and a distinct prop
//! id. That exercises the lock **file**, not just the in-process mutex.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use palace_prop::asset_crc;
use palace_prop::bag_folder::{my_bag_path, shelves_dir, BagContext, BAG_FILE_NAME};
use palace_prop::bag_store::{BagStore, WriteOutcome};
use palace_prop::prp::{AssetRec, PropHeader, PropKey, PropRecord, Roster};

/// 44x44 8-bit prop header, offsets 0, HEAD|RARE flags.
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

/// A valid, empty `.prp` (the same bytes as `fixtures/prp/empty.prp`).
const EMPTY_ROSTER: [u8; 52] = [
    0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00,
    0x24, 0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00, 0x70, 0x6f, 0x72, 0x50, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary directory that deletes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("palace-bag-store-{tag}-{}-{n}", std::process::id()));
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

fn key_of(id: i32, blob: &[u8]) -> PropKey {
    PropKey::new(id, crc_of(blob))
}

/// Add a prop, deriving its crc from the blob.
fn add(
    store: &BagStore,
    collection: &Path,
    id: i32,
    data: &[u8],
    name: Option<&str>,
) -> WriteOutcome {
    let blob = blob(data);
    store
        .add_prop(collection, id, crc_of(&blob), &blob, name)
        .expect("add_prop")
}

/// Build a `.prp` record for the shelf-seeding helper.
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

/// Write a valid `.prp` directly (used to seed a read-only shelf).
fn write_collection(path: &Path, records: Vec<PropRecord>) {
    let mut roster = Roster::parse(&EMPTY_ROSTER).expect("empty roster parses");
    for record in records {
        roster.add_prop(record);
    }
    fs::write(path, roster.write().expect("roster writes")).expect("write collection");
}

fn reparse(path: &Path) -> Roster {
    let bytes = fs::read(path).expect("collection exists");
    Roster::parse(&bytes).expect("collection parses")
}

fn count_key(roster: &Roster, key: PropKey) -> usize {
    roster
        .records()
        .iter()
        .filter(|record| record.key() == key)
        .count()
}

#[test]
fn each_mutation_survives_a_reparse() {
    let temp = TempDir::new("mutations");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);

    // create_collection makes a valid, empty My Bag.
    assert_eq!(
        store.create_collection(&bag).expect("create My Bag"),
        WriteOutcome::Created
    );
    assert_eq!(reparse(&bag).records().len(), 0);

    // add
    let alpha = blob(&[1, 2, 3]);
    let alpha_key = key_of(100, &alpha);
    assert_eq!(
        store
            .add_prop(&bag, 100, alpha_key.crc, &alpha, Some("Alpha"))
            .expect("add Alpha"),
        WriteOutcome::Added
    );
    let roster = reparse(&bag);
    let stored = roster.record_for(alpha_key).expect("Alpha is stored");
    assert_eq!(stored.name.as_deref(), Some("Alpha"));
    assert_eq!(stored.blob, alpha, "the blob is stored verbatim");

    // add a second prop, then a legal variant of the first id.
    assert_eq!(
        add(&store, &bag, 200, &[7, 7, 7], Some("Bravo")),
        WriteOutcome::Added
    );
    let variant = blob(&[4, 5, 6]);
    let variant_key = key_of(100, &variant);
    assert_ne!(variant_key.crc, alpha_key.crc);
    assert_eq!(
        store
            .add_prop(&bag, 100, variant_key.crc, &variant, Some("Alpha II"))
            .expect("add variant"),
        WriteOutcome::Added
    );
    let roster = reparse(&bag);
    assert_eq!(count_key(&roster, alpha_key), 1);
    assert_eq!(count_key(&roster, variant_key), 1);
    assert_eq!(
        roster
            .records()
            .iter()
            .filter(|record| record.id() == 100)
            .count(),
        2,
        "same id, different crc is a legal variant"
    );

    // rename
    assert_eq!(
        store
            .rename_prop(&bag, alpha_key, Some("Alpha Renamed"))
            .expect("rename"),
        WriteOutcome::Renamed
    );
    assert_eq!(
        reparse(&bag)
            .record_for(alpha_key)
            .and_then(|record| record.name.clone()),
        Some("Alpha Renamed".to_string())
    );

    // remove
    let bravo_key = PropKey::new(200, crc_of(&blob(&[7, 7, 7])));
    assert_eq!(
        store.remove_prop(&bag, bravo_key).expect("remove Bravo"),
        WriteOutcome::Removed
    );
    assert!(reparse(&bag).record_for(bravo_key).is_none());

    // duplicate from a shelf into My Bag without touching the shelf.
    let shelf = shelves_dir(root).join("Old Gems.prp");
    fs::create_dir_all(shelves_dir(root)).expect("create shelves dir");
    let gem = blob(&[8, 8, 8, 8]);
    let gem_key = key_of(300, &gem);
    write_collection(&shelf, vec![make_record(300, &[8, 8, 8, 8], Some("Gem"))]);
    let shelf_before = fs::read(&shelf).expect("read shelf");
    assert_eq!(
        store
            .duplicate_prop(&shelf, gem_key, &bag, Some("Gem Copy"))
            .expect("duplicate"),
        WriteOutcome::Duplicated
    );
    let roster = reparse(&bag);
    let copied = roster.record_for(gem_key).expect("the gem was copied");
    assert_eq!(copied.name.as_deref(), Some("Gem Copy"));
    assert_eq!(copied.blob, gem);
    assert_eq!(
        fs::read(&shelf).expect("read shelf"),
        shelf_before,
        "duplicating must not modify the shelf"
    );

    // duplicate again is a no-op.
    assert_eq!(
        store
            .duplicate_prop(&shelf, gem_key, &bag, None)
            .expect("duplicate again"),
        WriteOutcome::AlreadyPresent
    );
}

#[test]
fn a_duplicate_add_is_a_no_op_with_already_present() {
    let temp = TempDir::new("duplicate");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let data = blob(&[1, 1, 2, 3]);
    let key = key_of(42, &data);
    assert_eq!(
        store
            .add_prop(&bag, 42, key.crc, &data, Some("Once"))
            .expect("first add"),
        WriteOutcome::Added
    );
    let before = fs::read(&bag).expect("read bag");
    let before_digest = sha256_hex(&before);

    assert_eq!(
        store
            .add_prop(&bag, 42, key.crc, &data, Some("Twice"))
            .expect("second add"),
        WriteOutcome::AlreadyPresent
    );
    let after = fs::read(&bag).expect("read bag");
    assert_eq!(after, before, "a duplicate add must not rewrite the file");
    assert_eq!(sha256_hex(&after), before_digest);
    assert_eq!(count_key(&reparse(&bag), key), 1, "no duplicate record");
}

#[test]
fn a_shelf_write_is_refused_and_the_shelf_is_untouched() {
    let temp = TempDir::new("shelf");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");
    add(&store, &bag, 10, &[1, 2, 3], Some("Mine"));

    let shelf = shelves_dir(root).join("Old Gems.prp");
    fs::create_dir_all(shelves_dir(root)).expect("create shelves dir");
    write_collection(
        &shelf,
        vec![
            make_record(300, &[8, 8, 8], Some("Gem")),
            make_record(301, &[9, 9, 9], Some("Stone")),
        ],
    );
    let before = fs::read(&shelf).expect("read shelf");
    let before_digest = sha256_hex(&before);
    let gem_key = PropKey::new(300, crc_of(&blob(&[8, 8, 8])));
    let shelf_roster = reparse(&shelf);
    assert!(shelf_roster.record_for(gem_key).is_some());

    let attempt = blob(&[5, 5, 5]);
    let attempt_crc = crc_of(&attempt);
    assert!(
        store
            .add_prop(&shelf, 999, attempt_crc, &attempt, None)
            .is_err(),
        "add to a shelf must be refused"
    );
    assert!(
        store.remove_prop(&shelf, gem_key).is_err(),
        "remove from a shelf must be refused"
    );
    assert!(
        store.rename_prop(&shelf, gem_key, Some("Renamed")).is_err(),
        "rename on a shelf must be refused"
    );
    assert!(
        store.move_prop(&shelf, &bag, gem_key).is_err(),
        "a shelf must not be a move source"
    );
    assert!(
        store.move_prop(&bag, &shelf, gem_key).is_err(),
        "a shelf must not be a move destination"
    );
    assert!(
        store
            .duplicate_prop(&bag, PropKey::new(10, 0), &shelf, None)
            .is_err(),
        "a shelf must not be a duplicate destination"
    );
    assert!(
        store.create_collection(&shelf).is_err(),
        "a shelf must not be created through the store"
    );
    assert!(
        store.delete_collection(&shelf).is_err(),
        "a shelf must not be deleted through the store"
    );

    let after = fs::read(&shelf).expect("read shelf");
    assert_eq!(after, before, "the shelf bytes must be unchanged");
    assert_eq!(sha256_hex(&after), before_digest);
}

#[test]
fn a_forbidden_path_is_refused_without_being_created() {
    let temp = TempDir::new("forbidden");
    let root = temp.path();
    let store = BagStore::open(context(root));
    // A Sibling of the bag root, outside it: BagRole::Forbidden.
    let outside = temp
        .path()
        .parent()
        .expect("temp dir has a parent")
        .join(format!(
            "palace-bag-store-outside-{}.prp",
            std::process::id()
        ));
    let _ = fs::remove_file(&outside);

    let data = blob(&[1]);
    assert!(store
        .add_prop(&outside, 1, crc_of(&data), &data, None)
        .is_err());
    assert!(!outside.exists(), "a refused write must not create a file");
}

#[test]
fn two_threads_adding_concurrently_do_not_corrupt() {
    let temp = TempDir::new("threads");
    let root = temp.path();
    let store = Arc::new(BagStore::open(context(root)));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let ids: Vec<i32> = (400..408).collect();
    let mut handles = Vec::new();
    for id in &ids {
        let store = Arc::clone(&store);
        let bag = bag.clone();
        let id = *id;
        handles.push(std::thread::spawn(move || {
            let data = blob(&[id as u8, 0x55, 0x66, 0x77]);
            let crc = asset_crc(&data[12..]);
            store
                .add_prop(&bag, id, crc, &data, Some("threaded"))
                .expect("thread add");
        }));
    }
    for handle in handles {
        handle.join().expect("thread joins");
    }

    let bytes = fs::read(&bag).expect("read bag");
    let roster = Roster::parse(&bytes).expect("bag parses after concurrent adds");
    assert!(roster.file_header().invariant_holds(bytes.len()));
    for id in &ids {
        let data = blob(&[*id as u8, 0x55, 0x66, 0x77]);
        let key = key_of(*id, &data);
        assert_eq!(
            count_key(&roster, key),
            1,
            "id {id} must be present exactly once"
        );
    }
    assert!(
        !lock_file(&bag).exists(),
        "the lock file must be removed after the writers finish"
    );
}

#[test]
fn two_processes_adding_concurrently_do_not_corrupt() {
    let temp = TempDir::new("processes");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let ids = [5001i32, 5002];
    let mut children: Vec<Child> = ids.iter().map(|id| spawn_worker(root, *id)).collect();
    for child in &mut children {
        let status = child.wait().expect("worker waits");
        assert!(status.success(), "worker exited with {status}");
    }

    let bytes = fs::read(&bag).expect("read bag");
    let roster = Roster::parse(&bytes).expect("bag parses after two-process adds");
    assert!(roster.file_header().invariant_holds(bytes.len()));
    for id in &ids {
        let data = blob(&[*id as u8, 0x77]);
        let key = key_of(*id, &data);
        assert_eq!(
            count_key(&roster, key),
            1,
            "id {id} must be present exactly once after two processes"
        );
    }
    assert!(
        !lock_file(&bag).exists(),
        "no lock file is left behind after the processes finish"
    );
}

#[test]
fn a_held_lock_file_refuses_the_second_writer() {
    let temp = TempDir::new("lock");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let lock = lock_file(&bag);
    fs::write(&lock, b"held by someone else").expect("plant a lock file");

    let data = blob(&[3, 1, 4]);
    let err = store
        .add_prop(&bag, 7, crc_of(&data), &data, None)
        .expect_err("a held lock must refuse the write");
    let message = err.to_string();
    assert!(
        message.contains("lock"),
        "the error must name the lock: {message}"
    );

    fs::remove_file(&lock).expect("release the lock");
    assert_eq!(
        store
            .add_prop(&bag, 7, crc_of(&data), &data, None)
            .expect("add"),
        WriteOutcome::Added
    );
    assert!(!lock.exists(), "the store removes its own lock on drop");
}

#[test]
fn remove_then_readd_restores_the_identical_blob() {
    let temp = TempDir::new("restore");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let data = blob(&[2, 7, 1, 8]);
    let key = key_of(88, &data);
    store
        .add_prop(&bag, 88, key.crc, &data, Some("Keep"))
        .expect("add");
    let original_blob = reparse(&bag).record_for(key).expect("stored").blob.clone();

    // The trash module is a stub with no public API, so delete is plain removal.
    assert_eq!(
        store.remove_prop(&bag, key).expect("remove"),
        WriteOutcome::Removed
    );
    assert!(reparse(&bag).record_for(key).is_none());

    // Restore by re-adding the exact same identity and bytes.
    assert_eq!(
        store
            .add_prop(&bag, 88, key.crc, &original_blob, Some("Keep"))
            .expect("restore"),
        WriteOutcome::Added
    );
    let restored = reparse(&bag);
    let restored = restored.record_for(key).expect("restored");
    assert_eq!(restored.blob, original_blob, "restore is bit-identical");
}

#[test]
fn create_and_delete_collection_round_trip() {
    let temp = TempDir::new("collection");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);

    assert_eq!(
        store.create_collection(&bag).expect("create"),
        WriteOutcome::Created
    );
    assert!(bag.is_file());
    let roster = reparse(&bag);
    assert!(roster.is_empty());
    assert!(roster
        .file_header()
        .invariant_holds(fs::read(&bag).expect("read").len()));

    assert_eq!(
        store.create_collection(&bag).expect("create again"),
        WriteOutcome::AlreadyExists
    );
    assert_eq!(
        store.delete_collection(&bag).expect("delete"),
        WriteOutcome::Deleted
    );
    assert!(!bag.exists());
    assert_eq!(
        store.delete_collection(&bag).expect("delete again"),
        WriteOutcome::NotFound
    );
    assert_eq!(
        store.create_collection(&bag).expect("recreate"),
        WriteOutcome::Created
    );
}

#[test]
fn list_collections_lists_my_bag_then_shelves() {
    let temp = TempDir::new("list");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    fs::create_dir_all(shelves_dir(root)).expect("create shelves dir");
    write_collection(
        &shelves_dir(root).join("Old Gems.prp"),
        vec![make_record(1, &[1], None)],
    );
    write_collection(
        &shelves_dir(root).join("Alpha.prp"),
        vec![make_record(2, &[2], None)],
    );

    let collections = store.list_collections().expect("list");
    let names: Vec<&str> = collections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["My Bag", "Alpha", "Old Gems"]);
    assert_eq!(collections[0].path, bag);
    assert!(collections[0].is_writable());
    assert!(collections[0].exists);
    assert!(!collections[1].is_writable());
    assert!(!collections[2].is_writable());
}

#[test]
fn an_untouched_record_is_bit_identical_after_a_mutation() {
    let temp = TempDir::new("untouched");
    let root = temp.path();
    let store = BagStore::open(context(root));
    let bag = my_bag_path(root);
    store.create_collection(&bag).expect("create My Bag");

    let keep = blob(&[4, 2, 4, 2]);
    let keep_key = key_of(1, &keep);
    store
        .add_prop(&bag, 1, keep_key.crc, &keep, Some("Untouched"))
        .expect("add keep");
    add(&store, &bag, 2, &[5, 5], Some("Other"));
    let before = reparse(&bag)
        .record_for(keep_key)
        .expect("keep")
        .blob
        .clone();

    // A mutation elsewhere must not alter the untouched record's blob bytes.
    add(&store, &bag, 3, &[6, 6], Some("Third"));
    let after = reparse(&bag)
        .record_for(keep_key)
        .expect("keep")
        .blob
        .clone();
    assert_eq!(before, after);
}

/// A child of the current test binary, running the ignored worker test.
fn spawn_worker(root: &Path, id: i32) -> Child {
    let exe = std::env::current_exe().expect("test binary path");
    Command::new(exe)
        .arg("--include-ignored")
        .arg("worker_adds_one_prop")
        .arg("--nocapture")
        .env("PALACE_BAG_WORKER_ROOT", root)
        .env("PALACE_BAG_WORKER_ID", id.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn worker process")
}

/// The worker half of the cross-process concurrency test.
///
/// Ignored by default; the parent runs it explicitly in two child processes so
/// the only thing serialising them is the lock file, not the process mutex.
#[test]
#[ignore = "spawned explicitly by two_processes_adding_concurrently_do_not_corrupt"]
fn worker_adds_one_prop() {
    let Ok(root) = std::env::var("PALACE_BAG_WORKER_ROOT") else {
        return;
    };
    let Ok(id) = std::env::var("PALACE_BAG_WORKER_ID") else {
        return;
    };
    let Ok(id) = id.parse::<i32>() else {
        return;
    };

    let root = PathBuf::from(root);
    let bag = my_bag_path(&root);
    let store = BagStore::open(BagContext {
        bag_root: Some(root),
        home: None,
    });
    let data = blob(&[id as u8, 0x77]);
    let crc = asset_crc(&data[12..]);
    store
        .add_prop(&bag, id, crc, &data, Some("worker"))
        .expect("worker add_prop");
}

fn lock_file(target: &Path) -> PathBuf {
    let name = target.file_name().expect("has a file name");
    let mut lock = name.to_os_string();
    lock.push(".lock");
    target.with_file_name(lock)
}

#[test]
fn the_sha256_helper_matches_known_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(BAG_FILE_NAME.as_bytes()),
        sha256_hex(BAG_FILE_NAME.as_bytes())
    );
}

/// SHA-256, implemented here so the shelf-digest assertions need no dependency.
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in schedule.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    state.iter().map(|word| format!("{word:08x}")).collect()
}

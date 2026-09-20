//! Acceptance tests for the gather pipeline: a real prop is copied into My Bag
//! byte-for-byte, a repeat gather is a no-op, a truncated blob is refused with
//! nothing written, and a second CRC variant of an id is a legal second entry.
//!
//! Every fixture tree is built under the OS temp directory. Nothing here touches
//! the user's real bag, a shelf, or PalaceChat data.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::bag_folder::{my_bag_path, BagContext};
use palace_prop::bag_store::BagStore;
use palace_prop::crc::asset_crc;
use palace_prop::gather::{gather, GatherOutcome};
use palace_prop::prp::{PropKey, Roster};

/// Real corpus props, copied out of the local corpus; see `fixtures/README.md`.
const AVATAR: &str = "8bit_avatar.bin";
const HEAD: &str = "8bit_head_rare.bin";

/// The roster CRCs of the two fixtures, pinned by `tests/fixtures.rs`.
const AVATAR_CRC: u32 = 0x1428_275e;
const HEAD_CRC: u32 = 0x9596_284b;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary directory that deletes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("palace-gather-{tag}-{}-{n}", std::process::id()));
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

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// A store over an explicit temp root, never the discovered user bag.
fn store_at(root: &Path) -> BagStore {
    BagStore::open(BagContext {
        bag_root: Some(root.to_path_buf()),
        home: None,
    })
}

fn crc_of(blob: &[u8]) -> u32 {
    asset_crc(&blob[12..])
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
fn a_real_prop_is_gathered_byte_identically_with_its_crc_preserved() {
    let temp = TempDir::new("added");
    let root = temp.path();
    let store = store_at(root);

    let blob = fixture(AVATAR);
    let crc = crc_of(&blob);
    assert_eq!(crc, AVATAR_CRC, "the fixture's roster CRC");
    let id = 1001;

    assert_eq!(
        gather(&store, id, crc, &blob, Some("Avatar")).expect("gather"),
        GatherOutcome::Added
    );

    let bag = my_bag_path(root);
    assert!(bag.is_file(), "gather creates My Bag on first use");
    let roster = reparse(&bag);
    let stored = roster
        .record_for(PropKey::new(id, crc))
        .expect("the gathered prop is stored");
    assert_eq!(stored.blob, blob, "the stored blob must be byte-identical");
    assert_eq!(stored.crc(), crc, "the source CRC must be preserved");
    assert_eq!(asset_crc(&stored.blob[12..]), crc);
    assert_eq!(stored.name.as_deref(), Some("Avatar"));
}

#[test]
fn gathering_the_same_prop_twice_is_already_present_and_changes_nothing() {
    let temp = TempDir::new("duplicate");
    let root = temp.path();
    let store = store_at(root);

    let blob = fixture(AVATAR);
    let crc = crc_of(&blob);
    let key = PropKey::new(1001, crc);

    assert_eq!(
        gather(&store, 1001, crc, &blob, Some("Once")).expect("first gather"),
        GatherOutcome::Added
    );
    let bag = my_bag_path(root);
    let before = fs::read(&bag).expect("read My Bag");

    // A different name must not sneak a second record in, and the store must
    // not even rewrite the file.
    assert_eq!(
        gather(&store, 1001, crc, &blob, Some("Twice")).expect("second gather"),
        GatherOutcome::AlreadyPresent
    );
    let after = fs::read(&bag).expect("read My Bag");
    assert_eq!(after, before, "a repeat gather must not rewrite My Bag");
    assert_eq!(count_key(&reparse(&bag), key), 1, "no duplicate record");
}

#[test]
fn a_truncated_blob_is_rejected_and_nothing_is_written() {
    let temp = TempDir::new("truncated");
    let root = temp.path();
    let store = store_at(root);
    let bag = my_bag_path(root);

    let blob = fixture(AVATAR);
    let truncated = &blob[..8];

    // On a fresh bag: rejected, and My Bag is never created.
    let outcome = gather(&store, 7, 0, truncated, None).expect("gather returns an outcome");
    match &outcome {
        GatherOutcome::Rejected(reason) => {
            assert!(!reason.is_empty(), "a rejection must explain itself");
            assert!(
                reason.contains("decode"),
                "the reason must name the decode failure: {reason}"
            );
        }
        other => panic!("a truncated blob must be rejected, got {other:?}"),
    }
    assert!(
        !bag.exists(),
        "a rejected gather must not create My Bag or any collection"
    );

    // With My Bag already populated: rejected, and the bytes are untouched.
    let real = fixture(AVATAR);
    assert_eq!(
        gather(&store, 7, AVATAR_CRC, &real, None).expect("seed My Bag"),
        GatherOutcome::Added
    );
    let before = fs::read(&bag).expect("read My Bag");
    let outcome = gather(&store, 8, 0, truncated, None).expect("truncated gather");
    assert!(
        matches!(outcome, GatherOutcome::Rejected(_)),
        "got {outcome:?}"
    );
    let after = fs::read(&bag).expect("read My Bag");
    assert_eq!(after, before, "a rejected gather must not rewrite My Bag");
    assert_eq!(reparse(&bag).records().len(), 1);
}

#[test]
fn the_same_id_with_a_different_crc_is_a_second_variant() {
    let temp = TempDir::new("variant");
    let root = temp.path();
    let store = store_at(root);

    let avatar = fixture(AVATAR);
    let head = fixture(HEAD);
    let avatar_crc = crc_of(&avatar);
    let head_crc = crc_of(&head);
    assert_eq!(avatar_crc, AVATAR_CRC);
    assert_eq!(head_crc, HEAD_CRC);
    assert_ne!(avatar_crc, head_crc, "the two fixtures differ in crc");

    let id = 4242;
    assert_eq!(
        gather(&store, id, avatar_crc, &avatar, Some("Avatar")).expect("gather avatar"),
        GatherOutcome::Added
    );
    assert_eq!(
        gather(&store, id, head_crc, &head, Some("Head")).expect("gather head"),
        GatherOutcome::Added
    );

    let bag = my_bag_path(root);
    let roster = reparse(&bag);
    assert_eq!(count_key(&roster, PropKey::new(id, avatar_crc)), 1);
    assert_eq!(count_key(&roster, PropKey::new(id, head_crc)), 1);
    assert_eq!(
        roster
            .records()
            .iter()
            .filter(|record| record.id() == id)
            .count(),
        2,
        "same id, different crc is a legal variant chain"
    );
    assert_eq!(
        roster
            .record_for(PropKey::new(id, avatar_crc))
            .expect("avatar stored")
            .blob,
        avatar
    );
    assert_eq!(
        roster
            .record_for(PropKey::new(id, head_crc))
            .expect("head stored")
            .blob,
        head
    );
}

#[test]
fn a_crc_that_does_not_match_the_blob_is_rejected_without_a_write() {
    let temp = TempDir::new("crc");
    let root = temp.path();
    let store = store_at(root);

    let blob = fixture(AVATAR);
    let wrong = crc_of(&blob) ^ 0x1;
    let outcome = gather(&store, 9, wrong, &blob, None).expect("gather returns an outcome");
    assert!(
        matches!(&outcome, GatherOutcome::Rejected(reason) if reason.contains("crc")),
        "got {outcome:?}"
    );
    assert!(!my_bag_path(root).exists(), "nothing may be written");
}

//! The outfits file: schema round-trip, readable text, and failure behaviour.
//!
//! Every fixture is built under the OS temp directory with a synthetic bag
//! folder, exactly like `tests/bag_layout.rs`. Nothing here ever touches the
//! user's real bag, PalaceChat data, or `$MEDIA/Prop Files/`.
//!
//! The three named cases the plan calls for:
//!
//! * `round_trips_three_outfits_of_nine_refs_in_order`
//! * `the_saved_file_is_readable_text_with_a_version_field`
//! * `a_corrupt_file_loads_empty_with_an_error_and_is_untouched`
//!
//! plus the name-resolution and no-write-on-failure behaviour around them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use palace_prop::bag_folder::{outfits_path, BagContext};
use palace_prop::outfits::{OutfitStore, OUTFITS_VERSION};
use palace_prop::prp::PropKey;

/// A temporary directory that deletes itself.
struct TempDir(PathBuf);

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("palace-outfits-{tag}-{}-{n}", std::process::id()));
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

/// Nine references, including a duplicate id run (same id, different crcs),
/// negative ids and the extremes of both ranges. Order and pairs are what the
/// round-trip test checks, so the sequence is deliberately not sorted.
fn refs(seed: i32) -> Vec<PropKey> {
    vec![
        PropKey::new(seed, 0),
        PropKey::new(seed, 0xdead_beef),
        PropKey::new(-seed, 1),
        PropKey::new(i32::MIN, 2),
        PropKey::new(i32::MAX, 3),
        PropKey::new(0, 4),
        PropKey::new(123_456_789, u32::MAX),
        PropKey::new(seed, 6),
        PropKey::new(-1, 9),
    ]
}

fn pairs(keys: &[PropKey]) -> Vec<(i32, u32)> {
    keys.iter().map(|key| (key.id, key.crc)).collect()
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("modification time")
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn round_trips_three_outfits_of_nine_refs_in_order() {
    let fixture = fixture("roundtrip");
    let mut store = OutfitStore::open(fixture.context.clone());
    assert!(store.is_empty(), "a missing file is an empty store");
    assert!(
        store.load_error().is_none(),
        "a missing file is not an error: {:?}",
        store.load_error()
    );
    assert_eq!(
        store.path(),
        Some(outfits_path(&fixture.root).as_path()),
        "the store reads and writes Outfits.prp in the bag root"
    );

    let outfits = [("First", refs(1)), ("Second", refs(2)), ("Third", refs(3))];
    for (name, keys) in &outfits {
        store.save_worn(name, keys).expect("save outfit");
    }

    // A fresh store must see exactly what was written, in order.
    let reopened = OutfitStore::open(fixture.context.clone());
    assert!(reopened.load_error().is_none());
    assert_eq!(reopened.len(), 3);
    assert_eq!(reopened.names(), vec!["First", "Second", "Third"]);

    for (name, keys) in &outfits {
        let stored = reopened
            .get(name)
            .expect("outfit must survive the round trip");
        assert_eq!(stored.props.len(), 9);
        assert_eq!(pairs(&stored.props), pairs(keys), "{name} pairs changed");
        assert_eq!(
            stored.props, *keys,
            "{name} must preserve the exact (id, crc) order"
        );
    }

    // The duplicate id run is the point of the (id, crc) identity: both
    // variants of `seed` survive, adjacent or not, because the crc keeps them
    // distinguishable.
    let first = reopened.get("First").expect("First");
    let same_id: Vec<u32> = first
        .props
        .iter()
        .filter(|key| key.id == 1)
        .map(|key| key.crc)
        .collect();
    assert_eq!(same_id, vec![0, 0xdead_beef, 6]);
}

#[test]
fn the_saved_file_is_readable_text_with_a_version_field() {
    let fixture = fixture("text");
    let mut store = OutfitStore::open(fixture.context.clone());
    store
        .save_worn("Reading Glasses", &refs(5))
        .expect("save outfit");
    store.save_worn("Empty", &[]).expect("save empty outfit");

    let path = outfits_path(&fixture.root);
    let text = fs::read_to_string(&path).expect("the outfits file is valid UTF-8 text");

    assert!(
        text.contains(&format!("\"version\": {OUTFITS_VERSION}")),
        "the version field must be present and readable: {text}"
    );
    assert!(
        text.contains("\"outfits\""),
        "the outfits array must be named"
    );
    assert!(text.contains("Reading Glasses"));
    assert!(text.starts_with('{'));
    assert!(text.trim_end().ends_with('}'));
    assert!(text.ends_with('\n'), "the document ends with a newline");
    assert!(!text.contains('\0'), "no binary junk in a text file");

    // And the text it wrote loads back to the same thing.
    let reopened = OutfitStore::open(fixture.context.clone());
    assert!(reopened.load_error().is_none());
    assert_eq!(reopened.names(), vec!["Reading Glasses", "Empty"]);
    assert_eq!(
        reopened.get("Reading Glasses").expect("outfit").props,
        refs(5)
    );
    assert!(reopened.get("Empty").expect("outfit").props.is_empty());
}

#[test]
fn a_corrupt_file_loads_empty_with_an_error_and_is_untouched() {
    let fixture = fixture("corrupt");
    let path = outfits_path(&fixture.root);
    let mut store = OutfitStore::open(fixture.context.clone());
    store.save_worn("Intact", &refs(7)).expect("save outfit");
    let good = fs::read(&path).expect("read the good file");

    let mut truncated = good.clone();
    truncated.truncate(good.len() / 2);
    let variants: Vec<(&str, Vec<u8>)> = vec![
        ("truncated", truncated),
        ("garbage", b"{ not json at all".to_vec()),
        ("unterminated", b"{\"version\": 1, \"outfits\": [".to_vec()),
        ("empty", Vec::new()),
        (
            "wrong version",
            format!("{{\"version\": {}, \"outfits\": []}}", OUTFITS_VERSION + 1).into_bytes(),
        ),
        ("wrong shape", b"{\"version\": 1, \"outfits\": {}}".to_vec()),
        ("not utf-8", vec![0xff, 0xfe, 0x00, 0x7b]),
    ];

    for (tag, bytes) in &variants {
        fs::write(&path, bytes).expect("plant a corrupt file");
        let before = fs::read(&path).expect("read before");
        let before_mtime = modified(&path);
        let before_listing = dir_listing(&fixture.root);

        let reopened = OutfitStore::open(fixture.context.clone());
        assert!(
            reopened.is_empty(),
            "{tag}: a corrupt file must load as an empty list"
        );
        assert!(
            reopened.load_error().is_some(),
            "{tag}: the load must report a clear error"
        );
        assert!(reopened.outfits().is_empty());
        assert!(reopened.names().is_empty());
        assert!(
            reopened.get("Intact").is_none(),
            "{tag}: no stale outfit may survive"
        );

        assert_eq!(
            fs::read(&path).expect("read after"),
            before,
            "{tag}: the corrupt file must be left byte-for-byte unchanged"
        );
        assert_eq!(
            modified(&path),
            before_mtime,
            "{tag}: the corrupt file's mtime must not move"
        );
        assert_eq!(
            dir_listing(&fixture.root),
            before_listing,
            "{tag}: no temp file or repair may appear"
        );
    }
}

#[test]
fn outfit_management_resolves_names_deterministically() {
    let fixture = fixture("manage");
    let mut store = OutfitStore::open(fixture.context.clone());
    store.save_worn("Alpha", &refs(1)).expect("save Alpha");
    store.save_worn("Beta", &refs(2)).expect("save Beta");

    // Saving over a name replaces in place, keeping the list order.
    store.save_worn("Alpha", &refs(9)).expect("replace Alpha");
    assert_eq!(store.names(), vec!["Alpha", "Beta"]);
    assert_eq!(store.get("Alpha").expect("Alpha").props, refs(9));

    // Rename: the renamed outfit keeps its position; unknown and taken names
    // are refused with an error rather than guessed at.
    store.rename("Alpha", "Gamma").expect("rename");
    assert_eq!(store.names(), vec!["Gamma", "Beta"]);
    assert!(store.rename("Missing", "Whatever").is_err());
    assert!(store.rename("Gamma", "Beta").is_err(), "name already taken");
    assert_eq!(
        store.names(),
        vec!["Gamma", "Beta"],
        "refusals change nothing"
    );

    // Duplicate: the copy lands directly after its source.
    store.duplicate("Gamma", "Delta").expect("duplicate");
    assert_eq!(store.names(), vec!["Gamma", "Delta", "Beta"]);
    assert_eq!(store.get("Delta").expect("Delta").props, refs(9));
    assert!(store.duplicate("Gamma", "Delta").is_err());

    // Delete, and the deterministic refusal for an unknown name.
    store.delete("Beta").expect("delete");
    assert_eq!(store.names(), vec!["Gamma", "Delta"]);
    assert!(store.delete("Beta").is_err(), "already deleted");

    // An empty or whitespace-only name is refused everywhere.
    assert!(store.save_worn("   ", &refs(3)).is_err());
    assert!(store.rename("Gamma", "\t").is_err());
    assert!(store.duplicate("Gamma", "").is_err());

    // The final state persists.
    let reopened = OutfitStore::open(fixture.context.clone());
    assert!(reopened.load_error().is_none());
    assert_eq!(reopened.names(), vec!["Gamma", "Delta"]);
    assert_eq!(reopened.get("Gamma").expect("Gamma").props, refs(9));

    // Duplicate names already in a hand-edited file: the first wins, later
    // ones are dropped, and the load still succeeds.
    let hand_edited = r#"{
  "version": 1,
  "outfits": [
    { "name": "Twin", "props": [[1, 2]] },
    { "name": "Twin", "props": [[3, 4]] },
    { "name": "Other", "props": [] }
  ]
}"#;
    fs::write(outfits_path(&fixture.root), hand_edited).expect("plant duplicate names");
    let deduped = OutfitStore::open(fixture.context.clone());
    assert!(deduped.load_error().is_none());
    assert_eq!(deduped.dropped_duplicates(), 1);
    assert_eq!(deduped.names(), vec!["Twin", "Other"]);
    assert_eq!(
        deduped.get("Twin").expect("Twin").props,
        vec![PropKey::new(1, 2)]
    );
}

#[test]
fn a_store_without_a_bag_folder_refuses_to_write() {
    let mut store = OutfitStore::open(BagContext::default());
    assert!(store.load_error().is_some(), "no bag folder is reported");
    assert!(store.is_empty());
    assert!(store.path().is_none());

    assert!(store.save_worn("Nope", &refs(1)).is_err());
    assert!(store.save().is_err());
    assert!(
        store.is_empty(),
        "a failed write must not commit the in-memory change"
    );
}

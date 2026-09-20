//! Acceptance tests for the outfit operations over the outfits store.
//!
//! Each case builds its own bag root under the OS temp directory and deletes it
//! afterwards. Nothing here touches the user's real bag or PalaceChat data.
//!
//! The three required behaviours are: a saved outfit applies back as the
//! identical `(id, crc)` list in order, a reference missing from the available
//! set is reported without failing the apply, and rename/delete/duplicate are
//! deterministic across a reload.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::bag_folder::BagContext;
use palace_prop::outfit_ops::{
    apply_outfit, delete_outfit, duplicate_outfit, list_outfits, rename_outfit, save_current_outfit,
};
use palace_prop::outfits::OutfitStore;
use palace_prop::prp::PropKey;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A temporary bag root that deletes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-outfit-ops-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn root(&self) -> &Path {
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

fn key(id: i32, crc: u32) -> PropKey {
    PropKey::new(id, crc)
}

/// A flat list of `(id, crc)` pairs, for terse assertions.
fn pairs(keys: &[PropKey]) -> Vec<(i32, u32)> {
    keys.iter().map(|k| (k.id, k.crc)).collect()
}

#[test]
fn save_then_apply_returns_the_identical_list_in_order() {
    let temp = TempDir::new("round-trip");
    let root = temp.root();

    let worn = [key(100, 7), key(-7, 0), key(100, 42), key(4, u32::MAX)];
    let mut store = OutfitStore::open(context(root));
    // The name carries surrounding whitespace on purpose: the store trims it.
    save_current_outfit(&mut store, "  Party Hat  ", &worn).expect("save");

    // Reload from disk so the assertion is about the persisted file, not memory.
    let mut store = OutfitStore::open(context(root));
    assert!(store.load_error().is_none(), "the saved file must parse");
    assert_eq!(list_outfits(&store), vec!["Party Hat"]);

    let application = apply_outfit(&store, "Party Hat", &worn).expect("apply");
    assert_eq!(application.worn, worn, "the identical list, in order");
    assert!(application.missing.is_empty());
    assert!(application.is_complete());
    assert_eq!(application.total(), 4);

    // Re-saving under the same name replaces in place, keeping one entry.
    save_current_outfit(&mut store, "Party Hat", &worn[..2]).expect("replace");
    assert_eq!(list_outfits(&store), vec!["Party Hat"]);
    let replaced = apply_outfit(&store, "Party Hat", &worn).expect("apply replaced");
    assert_eq!(pairs(&replaced.worn), pairs(&worn[..2]));
}

#[test]
fn a_missing_reference_is_reported_and_the_rest_still_apply() {
    let temp = TempDir::new("missing");
    let root = temp.root();

    let alpha = key(100, 7);
    let bravo = key(200, 9);
    let charlie = key(300, 11);
    let mut store = OutfitStore::open(context(root));
    save_current_outfit(&mut store, "Trio", &[alpha, bravo, charlie]).expect("save");

    // Bravo is not available; the other two are.
    let available = [charlie, alpha];
    let application = apply_outfit(&store, "Trio", &available).expect("apply must not fail");

    assert_eq!(
        pairs(&application.worn),
        pairs(&[alpha, charlie]),
        "the available references still apply, in outfit order"
    );
    assert_eq!(
        pairs(&application.missing),
        pairs(&[bravo]),
        "the unavailable reference is reported exactly"
    );
    assert!(!application.is_complete());
    assert_eq!(application.total(), 3);

    // Nothing at all available: everything is missing, nothing is worn.
    let none = apply_outfit(&store, "Trio", &[]).expect("apply with nothing");
    assert!(none.worn.is_empty());
    assert_eq!(pairs(&none.missing), pairs(&[alpha, bravo, charlie]));
    assert!(!none.is_empty());

    // An unknown name is still an error, unlike a missing reference.
    assert!(
        apply_outfit(&store, "No Such Outfit", &available).is_err(),
        "an unknown outfit name must be refused"
    );
}

#[test]
fn rename_delete_and_duplicate_are_deterministic() {
    let temp = TempDir::new("manage");
    let root = temp.root();

    let props = [key(10, 1), key(20, 2)];
    let mut store = OutfitStore::open(context(root));
    save_current_outfit(&mut store, "Alpha", &props).expect("save Alpha");

    // duplicate inserts the copy directly after its source.
    duplicate_outfit(&mut store, "Alpha", "Bravo").expect("duplicate");
    assert_eq!(list_outfits(&store), vec!["Alpha", "Bravo"]);

    // rename keeps position, and its props are unchanged.
    rename_outfit(&mut store, "Alpha", "Charlie").expect("rename");
    assert_eq!(list_outfits(&store), vec!["Charlie", "Bravo"]);
    let bravo = apply_outfit(&store, "Bravo", &props).expect("apply copy");
    assert_eq!(pairs(&bravo.worn), pairs(&props), "the copy kept its props");

    // delete removes exactly one name.
    delete_outfit(&mut store, "Bravo").expect("delete");
    assert_eq!(list_outfits(&store), vec!["Charlie"]);

    // Refusals are deterministic too.
    assert!(duplicate_outfit(&mut store, "Charlie", "Charlie").is_err());
    assert!(duplicate_outfit(&mut store, "Missing", "Copy").is_err());
    assert!(rename_outfit(&mut store, "Missing", "Other").is_err());
    assert!(
        rename_outfit(&mut store, "Charlie", "Charlie").is_ok(),
        "self-rename is a no-op"
    );
    assert!(delete_outfit(&mut store, "Missing").is_err());

    // The final state survives a reload from disk.
    let store = OutfitStore::open(context(root));
    assert!(store.load_error().is_none());
    assert_eq!(list_outfits(&store), vec!["Charlie"]);
    let charlie = apply_outfit(&store, "Charlie", &props).expect("apply after reload");
    assert_eq!(pairs(&charlie.worn), pairs(&props));
}

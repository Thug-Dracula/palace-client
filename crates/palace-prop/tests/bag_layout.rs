//! Bag folder layout, path roles and the write guard, against synthetic trees.
//!
//! Every fixture is built under the OS temp directory: a fake home, a fake
//! protected directory for each of the four PalaceChat/Prop Files subtrees, and
//! a bag folder holding `My Bag.prp` plus one shelf. Nothing here ever touches
//! the user's real bag or PalaceChat data.
//!
//! The interrupted-write test uses the crate's documented test seam
//! (`WriteFault::BeforeRename`) to stop an atomic write in the same window a
//! power loss or `SIGKILL` would, so the original file can be hashed before and
//! after and the temp file checked for.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use palace_prop::bag_folder::{
    my_bag_path, outfits_path, shelves_dir, BagContext, BagRole, WriteFault, BAG_FILE_NAME,
    SHELVES_DIR_NAME,
};

/// The four subtrees that must never be written, relative to the fake home.
const PROTECTED_RELATIVE: [&str; 4] = [
    ".local/share/PalaceChat",
    ".config/PalaceChat 5",
    ".cache/PalaceChat",
    "Pictures/Prop Files",
];

const PROTECTED_FILE: &str = "Do Not Touch.prp";
const SHELF_FILE: &str = "Old Gems.prp";

const BAG_ORIGINAL: &[u8] = b"synthetic My Bag contents\n";
const SHELF_ORIGINAL: &[u8] = b"synthetic shelf contents\n";
const PROTECTED_ORIGINAL: &[u8] = b"synthetic PalaceChat data\n";

/// A temporary directory that deletes itself. Each test builds its own
/// synthetic home here and never touches the user's real files.
struct TempDir(PathBuf);

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-bag-layout-{tag}-{}-{n}",
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

/// A synthetic home with a bag folder and one file under every protected path.
struct Fixture {
    /// Kept alive so the whole tree is deleted when the test ends.
    _home: TempDir,
    context: BagContext,
    root: PathBuf,
}

fn fixture(tag: &str) -> Fixture {
    let home = TempDir::new(tag);
    let home_path = home.path().to_path_buf();
    let root = home_path.join(".local/share/org.palace.client/props");

    fs::create_dir_all(shelves_dir(&root)).expect("create bag layout");
    fs::write(my_bag_path(&root), BAG_ORIGINAL).expect("write My Bag.prp");
    fs::write(shelves_dir(&root).join(SHELF_FILE), SHELF_ORIGINAL).expect("write shelf");

    for relative in PROTECTED_RELATIVE {
        let dir = home_path.join(relative);
        fs::create_dir_all(&dir).expect("create protected dir");
        fs::write(dir.join(PROTECTED_FILE), PROTECTED_ORIGINAL).expect("write protected file");
    }

    Fixture {
        context: BagContext {
            bag_root: Some(root.clone()),
            home: Some(home_path),
        },
        root,
        _home: home,
    }
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

fn protected_path(home: &Path, relative: &str) -> PathBuf {
    home.join(relative).join(PROTECTED_FILE)
}

#[test]
fn every_protected_path_is_refused_and_left_untouched() {
    let fixture = fixture("guard");
    let home = fixture.context.home.clone().expect("fixture home");

    for relative in PROTECTED_RELATIVE {
        let path = protected_path(&home, relative);
        assert_eq!(
            fixture.context.classify(&path),
            BagRole::Forbidden,
            "{} must classify as Forbidden",
            path.display()
        );

        let before_bytes = fs::read(&path).expect("read before");
        let before_mtime = modified(&path);
        let parent = path.parent().expect("protected parent").to_path_buf();
        let before_listing = dir_listing(&parent);

        assert!(
            fixture.context.open_for_write(&path).is_err(),
            "opening {} for write must be refused",
            path.display()
        );
        assert!(
            fixture
                .context
                .atomic_write(&path, b"owned by the bag")
                .is_err(),
            "atomic write to {} must be refused",
            path.display()
        );

        assert_eq!(
            fs::read(&path).expect("read after"),
            before_bytes,
            "{} content changed",
            path.display()
        );
        assert_eq!(
            modified(&path),
            before_mtime,
            "{} mtime changed",
            path.display()
        );
        assert_eq!(
            dir_listing(&parent),
            before_listing,
            "{} directory gained or lost entries",
            path.display()
        );
    }
}

#[test]
fn a_shelf_collection_is_read_only() {
    let fixture = fixture("shelf");
    let shelf = shelves_dir(&fixture.root).join(SHELF_FILE);
    assert_eq!(fixture.context.classify(&shelf), BagRole::Shelf);

    let before_bytes = fs::read(&shelf).expect("read before");
    let before_mtime = modified(&shelf);
    let before_listing = dir_listing(&shelves_dir(&fixture.root));

    assert!(fixture.context.open_for_write(&shelf).is_err());
    assert!(fixture.context.atomic_write(&shelf, b"rewritten").is_err());

    assert_eq!(fs::read(&shelf).expect("read after"), before_bytes);
    assert_eq!(modified(&shelf), before_mtime);
    assert_eq!(dir_listing(&shelves_dir(&fixture.root)), before_listing);
}

#[test]
fn paths_outside_the_bag_folder_are_forbidden() {
    let fixture = fixture("outside");
    let home = fixture.context.home.clone().expect("fixture home");
    for path in [
        fixture.root.clone(),
        fixture.root.join("Loose.prp"),
        home.join("Documents/Some.prp"),
        PathBuf::from("/tmp/loose.prp"),
    ] {
        assert_eq!(
            fixture.context.classify(&path),
            BagRole::Forbidden,
            "{} must be Forbidden",
            path.display()
        );
        assert!(fixture.context.open_for_write(&path).is_err());
        assert!(fixture.context.atomic_write(&path, b"nope").is_err());
    }
}

#[test]
fn an_atomic_write_replaces_the_file_and_leaves_no_temp_behind() {
    let fixture = fixture("replace");
    let bag = my_bag_path(&fixture.root);
    let before_listing = dir_listing(&fixture.root);

    fixture
        .context
        .atomic_write(&bag, b"first replacement")
        .expect("first atomic write");
    assert_eq!(fs::read(&bag).expect("read first"), b"first replacement");

    fixture
        .context
        .atomic_write(&bag, b"second replacement")
        .expect("second atomic write");
    assert_eq!(fs::read(&bag).expect("read second"), b"second replacement");

    // A direct handle works too, and only for the writable file.
    let mut handle = fixture
        .context
        .open_for_write(&bag)
        .expect("My Bag.prp is writable");
    handle
        .write_all(b"written through the handle")
        .expect("write");
    drop(handle);
    assert_eq!(
        fs::read(&bag).expect("read handle"),
        b"written through the handle"
    );

    assert_eq!(
        dir_listing(&fixture.root),
        before_listing,
        "no temp file may survive"
    );
}

#[test]
fn an_interrupted_atomic_write_leaves_the_original_byte_identical() {
    let fixture = fixture("interrupted");
    let bag = my_bag_path(&fixture.root);

    let original = fs::read(&bag).expect("read original");
    let original_digest = sha256_hex(&original);
    let before_mtime = modified(&bag);
    let before_listing = dir_listing(&fixture.root);

    let outcome =
        fixture
            .context
            .atomic_write_faulted(&bag, b"never lands", WriteFault::BeforeRename);
    assert!(
        outcome.is_err(),
        "the injected fault must surface as an error"
    );

    let after = fs::read(&bag).expect("read after");
    assert_eq!(after, original, "the original bytes must be untouched");
    let after_digest = sha256_hex(&after);
    println!("sha256 before: {original_digest}");
    println!("sha256 after:  {after_digest}");
    assert_eq!(
        after_digest, original_digest,
        "sha256 before and after must match"
    );
    assert_eq!(
        modified(&bag),
        before_mtime,
        "the original mtime must not move"
    );
    assert_eq!(
        dir_listing(&fixture.root),
        before_listing,
        "the temp file must be cleaned up"
    );
}

#[test]
fn an_interrupted_first_write_creates_nothing() {
    let fixture = fixture("interrupted-new");
    let bag = my_bag_path(&fixture.root);
    fs::remove_file(&bag).expect("remove My Bag.prp");

    let before_listing = dir_listing(&fixture.root);
    let outcome =
        fixture
            .context
            .atomic_write_faulted(&bag, b"never lands", WriteFault::BeforeRename);
    assert!(outcome.is_err());

    assert!(!bag.exists(), "a failed first write must not leave a file");
    assert_eq!(
        dir_listing(&fixture.root),
        before_listing,
        "a failed first write must not leave a temp file"
    );
}

#[test]
fn a_lexical_detour_still_reaches_the_writable_file() {
    let fixture = fixture("detour");
    let detour = shelves_dir(&fixture.root).join("..").join(BAG_FILE_NAME);
    assert_eq!(fixture.context.classify(&detour), BagRole::MyBag);

    fixture
        .context
        .atomic_write(&detour, b"through the detour")
        .expect("a normalised detour is still My Bag.prp");
    assert_eq!(
        fs::read(my_bag_path(&fixture.root)).expect("read bag"),
        b"through the detour"
    );
    assert_eq!(
        dir_listing(&shelves_dir(&fixture.root)),
        vec![SHELF_FILE.to_string()],
        "nothing may be created inside shelves/"
    );
}

#[test]
fn the_outfits_file_is_writable_through_the_same_guard() {
    let fixture = fixture("outfits");
    let outfits = outfits_path(&fixture.root);
    assert_eq!(fixture.context.classify(&outfits), BagRole::MyBag);
    fixture
        .context
        .atomic_write(&outfits, b"outfits")
        .expect("the outfits file is writable");
    assert_eq!(fs::read(&outfits).expect("read outfits"), b"outfits");
}

#[test]
fn ensure_layout_creates_only_inside_the_root_and_refuses_protected_roots() {
    let home = TempDir::new("layout");
    let home_path = home.path().to_path_buf();
    let root = home_path.join(".local/share/org.palace.client/props");
    let context = BagContext {
        bag_root: Some(root.clone()),
        home: Some(home_path.clone()),
    };

    let created = context.ensure_layout().expect("create the layout");
    assert_eq!(created, root);
    assert!(root.is_dir());
    assert!(shelves_dir(&root).is_dir());
    assert!(
        !my_bag_path(&root).exists(),
        "an empty file is not a valid .prp, so the layout must not invent one"
    );
    assert!(
        !home_path.join(".local/share/PalaceChat").exists(),
        "nothing outside the root may be created"
    );

    let protected_root = home_path.join(".local/share/PalaceChat/props");
    let refusing = BagContext {
        bag_root: Some(protected_root.clone()),
        home: Some(home_path),
    };
    assert!(refusing.ensure_layout().is_err());
    assert!(!protected_root.exists(), "protected roots stay untouched");
}

#[cfg(unix)]
#[test]
fn a_symlinked_bag_file_cannot_escape_to_protected_data() {
    use std::os::unix::fs::symlink;

    let fixture = fixture("symlink");
    let bag = my_bag_path(&fixture.root);
    fs::remove_file(&bag).expect("remove real My Bag.prp");
    let home = fixture.context.home.clone().expect("fixture home");
    let protected = protected_path(&home, ".local/share/PalaceChat");
    symlink(&protected, &bag).expect("point My Bag.prp at protected data");

    // The literal path looks like My Bag.prp; the resolved path does not.
    assert_eq!(fixture.context.classify(&bag), BagRole::MyBag);
    let before_bytes = fs::read(&protected).expect("read protected before");
    let before_mtime = modified(&protected);

    assert!(fixture.context.open_for_write(&bag).is_err());
    assert!(fixture.context.atomic_write(&bag, b"owned").is_err());

    assert_eq!(
        fs::read(&protected).expect("read protected after"),
        before_bytes
    );
    assert_eq!(modified(&protected), before_mtime);
    assert!(
        fs::symlink_metadata(&bag)
            .expect("symlink metadata")
            .file_type()
            .is_symlink(),
        "the symlink itself must be left alone"
    );
}

#[cfg(unix)]
#[test]
fn ensure_layout_will_not_create_through_a_symlink_into_protected_data() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new("layout-link");
    let home_path = home.path().to_path_buf();
    let protected = home_path.join(".local/share/PalaceChat");
    fs::create_dir_all(&protected).expect("create protected dir");
    let link = home_path.join("props");
    symlink(&protected, &link).expect("link the bag root at protected data");

    let context = BagContext {
        bag_root: Some(link),
        home: Some(home_path),
    };
    assert!(context.ensure_layout().is_err());
    assert!(
        !protected.join(SHELVES_DIR_NAME).exists(),
        "no directory may be created through the link"
    );
}

#[test]
fn a_bag_root_inside_protected_data_is_refused() {
    let home = TempDir::new("trapped-root");
    let home_path = home.path().to_path_buf();
    let trapped = home_path.join(".local/share/PalaceChat/props");
    fs::create_dir_all(&trapped).expect("create trapped root");
    let context = BagContext {
        bag_root: Some(trapped.clone()),
        home: Some(home_path),
    };

    let bag = my_bag_path(&trapped);
    assert_eq!(context.classify(&bag), BagRole::Forbidden);
    assert!(context.open_for_write(&bag).is_err());
    assert!(context.atomic_write(&bag, b"nope").is_err());
    assert!(!bag.exists());
}

#[test]
fn the_environment_free_functions_are_generous_refusers() {
    // These read the host environment; an arbitrary temp file can never be the
    // host's writable bag, so all three must refuse it.
    let dir = TempDir::new("env");
    let loose = dir.path().join("loose.prp");
    fs::write(&loose, b"loose").expect("write loose file");

    assert_eq!(
        palace_prop::bag_folder::classify(&loose),
        BagRole::Forbidden
    );
    assert!(palace_prop::bag_folder::open_for_write(&loose).is_err());
    assert!(palace_prop::bag_folder::atomic_write(&loose, b"x").is_err());
    assert_eq!(fs::read(&loose).expect("read loose"), b"loose");
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
        sha256_hex(BAG_ORIGINAL),
        sha256_hex(BAG_ORIGINAL),
        "equal inputs hash equal"
    );
}

/// SHA-256, implemented here so the interruption test can hash the original
/// without adding a dependency to the crate.
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

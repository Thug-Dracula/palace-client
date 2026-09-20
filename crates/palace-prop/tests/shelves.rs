//! Shelf discovery and read-only aggregation.
//!
//! Every test builds its own synthetic `.prp` shelves under a temporary
//! directory and never touches the user's bag, the user's `shelves/` or any
//! PalaceChat data. The containers are assembled here from the documented
//! layout, not copied from real collections.
//!
//! Three behaviours are pinned:
//!
//! 1. three shelves aggregate to the sum of their entries, each tagged with the
//!    right per-shelf [`Provenance::Bag`];
//! 2. a truncated shelf is reported and skipped while the valid shelves still
//!    load;
//! 3. discovery never writes: every shelf file's SHA-256 and mtime are identical
//!    before and after, and no stray file appears beside it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use palace_prop::asset_crc;
use palace_prop::provenance::Provenance;
use palace_prop::prp::{AssetType, PropKey};
use palace_prop::shelves::{ShelfSet, ShelfStatus};

// ---------------------------------------------------------------------------
// Temporary directory
// ---------------------------------------------------------------------------

/// A temporary directory that deletes itself on drop.
struct TempDir(PathBuf);

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("palace-shelves-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("shelves")).expect("create temp shelves dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn shelves(&self) -> PathBuf {
        self.0.join("shelves")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Synthetic `.prp` construction
// ---------------------------------------------------------------------------

/// The payload every synthetic record carries. Keeping it constant means every
/// record's CRC is the same, which makes the expected provenance key explicit.
const PAYLOAD: [u8; 4] = [1, 2, 3, 4];

/// A 12-byte blob header: 44x44, offsets 0, script 0, flags 0x000a (HEAD|RARE,
/// which selects the 8-bit encoding). No pixels are decoded by these tests.
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

/// One synthetic record: its id and, optionally, a name.
struct Spec {
    id: i32,
    name: Option<&'static str>,
}

fn spec(id: i32, name: Option<&'static str>) -> Spec {
    Spec { id, name }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_blob(out: &mut Vec<u8>) {
    out.extend_from_slice(&BLOB_HEADER);
    out.extend_from_slice(&PAYLOAD);
}

/// Build a minimal, valid `.prp` roster: one `Prop` type with one small
/// 8-bit record per [`Spec`], laid out exactly as `tests/prp_read.rs` does.
fn synth_prp(specs: &[Spec]) -> Vec<u8> {
    let blob_len = (BLOB_HEADER.len() + PAYLOAD.len()) as u32;
    let data_size = blob_len * specs.len() as u32;

    let mut names: Vec<u8> = Vec::new();
    let mut name_offsets: Vec<i32> = Vec::new();
    for s in specs {
        match s.name {
            Some(name) => {
                name_offsets.push(names.len() as i32);
                names.push(name.len() as u8);
                names.extend_from_slice(name.as_bytes());
            }
            None => name_offsets.push(-1),
        }
    }

    let n = specs.len() as u32;
    let recs_off = 24 + 12; // map header + one 12-byte type record
    let names_off = recs_off + n * 32;
    let map_size = names_off + names.len() as u32;

    let mut out = Vec::new();
    push_u32(&mut out, 16); // data_offset
    push_u32(&mut out, data_size); // data_size
    push_u32(&mut out, 16 + data_size); // asset_map_offset
    push_u32(&mut out, map_size); // asset_map_size

    for _ in specs {
        push_blob(&mut out);
    }

    push_i32(&mut out, 1); // nbr_types
    push_i32(&mut out, n as i32); // nbr_assets
    push_i32(&mut out, names.len() as i32); // len_names
    push_u32(&mut out, 24); // types_offset
    push_u32(&mut out, recs_off); // recs_offset
    push_u32(&mut out, names_off); // names_offset

    push_i32(&mut out, AssetType::PROP as i32); // type record: "Prop"
    push_i32(&mut out, n as i32);
    push_i32(&mut out, 0);

    let crc = asset_crc(&PAYLOAD);
    let mut data_offset = 0u32;
    for (index, s) in specs.iter().enumerate() {
        push_i32(&mut out, s.id);
        push_i32(&mut out, 0); // r_handle
        push_u32(&mut out, data_offset);
        push_u32(&mut out, blob_len);
        push_i32(&mut out, 0); // last_use_time
        push_i32(&mut out, name_offsets[index]);
        push_u32(&mut out, 0); // runtime flags
        push_u32(&mut out, crc);
        data_offset += blob_len;
    }

    out.extend_from_slice(&names);
    out
}

fn write_shelf(dir: &Path, file_name: &str, bytes: &[u8]) {
    fs::write(dir.join(file_name), bytes).expect("write synthetic shelf");
}

// ---------------------------------------------------------------------------
// SHA-256 (dependency-free, so the read-only test proves bytes unchanged)
// ---------------------------------------------------------------------------

mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut state: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];

        let mut msg = data.to_vec();
        let bit_len = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());

        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in w.iter_mut().take(16).enumerate() {
                *word = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
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

            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
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

        let mut out = [0u8; 32];
        for (i, word) in state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

fn dir_entry_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sha256_helper_matches_the_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn three_shelves_aggregate_to_the_sum_of_their_entries_with_per_shelf_provenance() {
    let root = TempDir::new("aggregate");
    let shelves = root.shelves();

    write_shelf(
        &shelves,
        "Alpha.prp",
        &synth_prp(&[spec(101, Some("One")), spec(102, None)]),
    );
    write_shelf(
        &shelves,
        "Beta.prp",
        &synth_prp(&[spec(201, None), spec(202, Some("Two")), spec(203, None)]),
    );
    write_shelf(
        &shelves,
        "Gamma.prp",
        &synth_prp(&[
            spec(301, None),
            spec(302, None),
            spec(303, None),
            spec(304, Some("Four")),
        ]),
    );

    let set = ShelfSet::discover(root.path());

    assert_eq!(set.len(), 3, "all three shelves are discovered");
    assert_eq!(set.entries().len(), 2 + 3 + 4, "the aggregate is the sum");
    assert_eq!(set.clone().into_entries().len(), 9);

    let names: Vec<&str> = set.shelves().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta", "Gamma"], "deterministic order");

    let expected_crc = asset_crc(&PAYLOAD);
    for shelf in set.shelves() {
        assert!(shelf.is_healthy(), "{} should be healthy", shelf.name);
        assert_eq!(
            shelf.health.records,
            shelf.entries.len(),
            "{}: every record has a header here",
            shelf.name
        );
        assert_eq!(shelf.health.dropped, 0);
        assert_eq!(shelf.health.undecodable, 0);
        for entry in &shelf.entries {
            assert_eq!(
                entry.provenance,
                Provenance::bag(
                    shelf.name.as_str(),
                    PropKey::new(entry.id as i32, entry.crc)
                ),
                "{} entry {} must carry this shelf's provenance",
                shelf.name,
                entry.id
            );
            assert_eq!(entry.provenance.collection(), Some(shelf.name.as_str()));
        }
    }

    // Spot-check one full entry against the synthetic bytes.
    let alpha = set
        .shelves()
        .iter()
        .find(|s| s.name == "Alpha")
        .expect("Alpha exists");
    assert_eq!(alpha.health.named, 1, "one of Alpha's two records is named");
    let first = &alpha.entries[0];
    assert_eq!(first.id, 101);
    assert_eq!(first.crc, expected_crc);
    assert_eq!(first.name.as_deref(), Some("One"));
    assert_eq!((first.width, first.height), (44, 44));
    assert_eq!(first.flags, 0x000a);
    assert_eq!(
        first.provenance.key(),
        Some(PropKey::new(101, expected_crc))
    );

    let beta = set
        .shelves()
        .iter()
        .find(|s| s.name == "Beta")
        .expect("Beta exists");
    assert_eq!(beta.health.records, 3);
    assert_eq!(beta.health.named, 1);

    let gamma = set
        .shelves()
        .iter()
        .find(|s| s.name == "Gamma")
        .expect("Gamma exists");
    assert_eq!(gamma.health.records, 4);
    assert_eq!(gamma.health.named, 1);
}

#[test]
fn a_truncated_shelf_is_reported_and_skipped_while_the_valid_ones_load() {
    let root = TempDir::new("truncated");
    let shelves = root.shelves();

    let mut corrupt = synth_prp(&[spec(1, None), spec(2, Some("Two")), spec(3, None)]);
    let half = corrupt.len() / 2;
    corrupt.truncate(half); // the declared map now runs past EOF

    write_shelf(
        &shelves,
        "Alpha.prp",
        &synth_prp(&[spec(11, None), spec(12, None)]),
    );
    write_shelf(&shelves, "Beta.prp", &synth_prp(&[spec(21, None)]));
    write_shelf(&shelves, "Gamma.prp", &corrupt);

    let set = ShelfSet::discover(root.path());

    assert_eq!(set.len(), 3, "the broken shelf is still reported");
    assert_eq!(set.entries().len(), 3, "the two valid shelves load fully");
    assert_eq!(set.failed_shelves().count(), 1);

    let gamma = set
        .shelves()
        .iter()
        .find(|s| s.name == "Gamma")
        .expect("Gamma exists");
    assert!(gamma.error.is_some(), "the corruption is reported");
    assert!(!gamma.is_readable());
    assert_eq!(gamma.health.status, ShelfStatus::Unreadable);
    assert!(gamma.entries.is_empty(), "no entries from a corrupt shelf");
    assert!(
        gamma
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Gamma.prp")),
        "the reason names the file: {:?}",
        gamma.error
    );

    for name in ["Alpha", "Beta"] {
        let shelf = set
            .shelves()
            .iter()
            .find(|s| s.name == name)
            .expect("valid shelf exists");
        assert!(shelf.is_readable(), "{name} must still load");
        assert_eq!(shelf.health.status, ShelfStatus::Healthy);
    }

    // The aggregate contains only the valid shelves' entries, and all keep
    // their own provenance.
    for entry in set.entries() {
        assert!(matches!(
            entry.provenance.collection(),
            Some("Alpha") | Some("Beta")
        ));
    }
}

#[test]
fn discovery_never_writes_to_a_shelf_file() {
    let root = TempDir::new("readonly");
    let shelves = root.shelves();
    let path = shelves.join("Only.prp");

    write_shelf(
        &shelves,
        "Only.prp",
        &synth_prp(&[spec(7, Some("Seven")), spec(8, None)]),
    );

    let before_hash = sha256_hex(&fs::read(&path).expect("read before"));
    let before_mtime = mtime(&path);
    let before_dir = dir_entry_names(&shelves);

    let set = ShelfSet::discover(root.path());

    // Reading through discovery must not have changed a single byte or the
    // modification time, and must not have left a temp/lock file behind.
    assert_eq!(set.entries().len(), 2, "discovery still loaded the shelf");
    assert_eq!(
        sha256_hex(&fs::read(&path).expect("read after")),
        before_hash,
        "the shelf's bytes must be untouched"
    );
    assert_eq!(
        mtime(&path),
        before_mtime,
        "the shelf's mtime must be untouched"
    );
    assert_eq!(
        dir_entry_names(&shelves),
        before_dir,
        "no new file may appear beside a shelf"
    );
    assert_eq!(before_dir, vec!["Only.prp".to_string()]);

    // Adding the same explicit path is idempotent and also read-only.
    let mut set = ShelfSet::discover(root.path());
    let explicit = set.add_shelf(&path).path.clone();
    assert_eq!(explicit, path);
    assert_eq!(set.len(), 1, "a duplicate path is not loaded twice");
    assert_eq!(
        sha256_hex(&fs::read(&path).expect("read after add")),
        before_hash
    );
    assert_eq!(mtime(&path), before_mtime);
    assert_eq!(dir_entry_names(&shelves), before_dir);
}

#[test]
fn a_shelf_can_be_added_by_an_explicit_path_outside_the_bag() {
    let root = TempDir::new("explicit");
    // A file that is not inside any `shelves/` directory, to model the user
    // pointing at a collection elsewhere.
    let standalone = root.path().join("Standalone.prp");
    fs::write(&standalone, synth_prp(&[spec(42, Some("Answer"))])).expect("write standalone");

    let mut set = ShelfSet::new();
    assert!(set.is_empty());
    let shelf = set.add_shelf(&standalone);
    assert_eq!(shelf.name, "Standalone");
    assert!(shelf.is_readable());
    assert_eq!(shelf.entries.len(), 1);
    assert_eq!(shelf.entries[0].provenance.collection(), Some("Standalone"));
    assert_eq!(shelf.entries[0].name.as_deref(), Some("Answer"));
}

//! The `.prp` writer: literal byte-exact round-trips, and the explicit
//! `canonicalise`/insert/remove operations that are allowed to change the layout.
//!
//! The byte-exact half runs over every golden fixture in `fixtures/prp/` and,
//! when their `/tmp` copies are present, all 15 real collections. A no-op
//! parse→write must reproduce each file exactly — the user's real collections are
//! unordered and some have gaps between blobs, so the writer must preserve them,
//! not normalise them.
//!
//! `rebuild_prp.py` is deliberately not used: T5 proved it is not a re-serialiser.

use std::path::{Path, PathBuf};

use palace_prop::asset_crc;
use palace_prop::prp::{AssetRec, PropHeader, PropKey, PropRecord, Roster};

/// 44x44, offsets 0, flags 0x000a (HEAD|RARE, 8-bit).
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

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

fn fixture_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "prp"))
        .collect();
    files.sort();
    files
}

fn real_files() -> Vec<PathBuf> {
    for candidate in ["/tmp/work/prp-oracle/originals", "/tmp/prp"] {
        let dir = Path::new(candidate);
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("prp"))
            })
            .collect();
        if !files.is_empty() {
            files.sort();
            return files;
        }
    }
    Vec::new()
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    let shared = a.len().min(b.len());
    for index in 0..shared {
        if a[index] != b[index] {
            return Some(index);
        }
    }
    (a.len() != b.len()).then_some(shared)
}

#[test]
fn a_no_op_write_is_byte_identical() {
    let fixtures = fixture_files();
    let reals = real_files();
    assert!(
        fixtures.len() >= 7,
        "golden fixtures missing from fixtures/prp ({} found)",
        fixtures.len()
    );
    if !reals.is_empty() {
        assert_eq!(reals.len(), 15, "expected 15 real collections under /tmp");
    }

    let mut failures = Vec::new();
    for path in fixtures.iter().chain(reals.iter()) {
        let original =
            std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let roster =
            Roster::parse(&original).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let written = roster
            .write()
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));

        match first_diff(&original, &written) {
            None => {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                println!("IDENTICAL {name:<28} {:>9} bytes", original.len());
                if let Ok(dir) = std::env::var("PALACE_PRP_ROUNDTRIP_DIR") {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(Path::new(&dir).join(&name), &written);
                }
            }
            Some(offset) => failures.push(format!(
                "{}: first diff at {offset:#x} ({} vs {} bytes)",
                path.display(),
                original.len(),
                written.len()
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "no-op write was not byte-identical for {} file(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn variant_indices(roster: &Roster, id: i32) -> Vec<usize> {
    roster
        .records()
        .iter()
        .enumerate()
        .filter(|(_, record)| record.id() == id)
        .map(|(index, _)| index)
        .collect()
}

fn assert_contiguous(indices: &[usize]) {
    for pair in indices.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "variant chain is not contiguous: {indices:?}"
        );
    }
}

fn assert_sorted_per_type(roster: &Roster) {
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

#[test]
fn canonicalise_sorts_and_validates_every_crc() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/real_palace_hidden.prp");
    let original = std::fs::read(&path).expect("real_palace_hidden.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("the real fixture parses");

    let mut ids_before: Vec<i32> = roster.records().iter().map(|record| record.id()).collect();
    ids_before.sort_unstable();
    assert!(!ids_before.is_empty());

    roster.canonicalise();
    let written = roster.write().expect("canonical roster writes");
    assert!(
        roster.file_header().invariant_holds(written.len()),
        "canonical output must satisfy the size invariant"
    );

    let reparsed = Roster::parse(&written).expect("canonical output parses back");
    assert_eq!(reparsed.dropped_records(), 0);
    assert_eq!(reparsed.records().len(), ids_before.len());
    assert_sorted_per_type(&reparsed);

    let mut ids_after: Vec<i32> = reparsed
        .records()
        .iter()
        .map(|record| record.id())
        .collect();
    ids_after.sort_unstable();
    assert_eq!(
        ids_after, ids_before,
        "canonicalise must not drop or add ids"
    );

    let mut checked = 0usize;
    for record in reparsed.records() {
        if record.blob.len() >= 12 {
            assert_eq!(
                asset_crc(&record.blob[12..]),
                record.crc(),
                "recomputed CRC wrong for id {}",
                record.id()
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 70,
        "expected the real fixture's payloads ({checked})"
    );
}

#[test]
fn insert_and_remove_keep_the_variant_chain_adjacent() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/dup_id_crc.prp");
    let original = std::fs::read(&path).expect("dup_id_crc.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("dup_id_crc.prp parses");

    let before = variant_indices(&roster, 1000);
    assert_eq!(before.len(), 3, "fixture has three id-1000 variants");
    assert_contiguous(&before);
    let total_before = roster.records().len();

    // Insert a fourth variant of the same id with a different payload/CRC.
    roster.add_prop(make_record(1000, &[9, 9, 9, 9], Some("NewVariant")));
    assert_eq!(roster.records().len(), total_before + 1);
    let after_add = variant_indices(&roster, 1000);
    assert_eq!(after_add.len(), 4);
    assert_contiguous(&after_add);

    let written = roster.write().expect("roster with an insertion writes");
    let reparsed = Roster::parse(&written).expect("inserted roster parses back");
    assert!(reparsed.file_header().invariant_holds(written.len()));
    assert_eq!(variant_indices(&reparsed, 1000).len(), 4);
    assert_contiguous(&variant_indices(&reparsed, 1000));
    assert_eq!(
        reparsed
            .records()
            .iter()
            .find(|record| record.name.as_deref() == Some("NewVariant"))
            .map(PropRecord::id),
        Some(1000)
    );

    // Remove one variant: the remaining chain stays adjacent and the invariant holds.
    let victim = reparsed
        .records()
        .iter()
        .find(|record| record.id() == 1000)
        .expect("an id-1000 variant exists")
        .key();
    let mut edited = reparsed;
    let removed = edited.remove_prop(victim).expect("the variant is removed");
    assert_eq!(removed.id(), 1000);
    assert_eq!(variant_indices(&edited, 1000).len(), 3);
    assert_contiguous(&variant_indices(&edited, 1000));

    let rewritten = edited.write().expect("roster with a removal writes");
    let reparsed_again = Roster::parse(&rewritten).expect("edited roster parses back");
    assert!(reparsed_again
        .file_header()
        .invariant_holds(rewritten.len()));
    assert!(variant_indices(&reparsed_again, 1000).len() == 3);
    assert_contiguous(&variant_indices(&reparsed_again, 1000));
}

#[test]
fn an_inserted_id_lands_in_signed_order() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/single_type.prp");
    let original = std::fs::read(&path).expect("single_type.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("single_type.prp parses");
    let before = roster.records().len();

    roster.add_prop(make_record(7, &[1, 2, 3, 4], None));
    roster.add_prop(make_record(-5, &[5, 6, 7, 8], None));
    assert_eq!(roster.records().len(), before + 2);

    let written = roster.write().expect("roster writes");
    let reparsed = Roster::parse(&written).expect("roster parses back");
    assert_sorted_per_type(&reparsed);
    assert_eq!(variant_indices(&reparsed, 7).len(), 1);
    assert_eq!(variant_indices(&reparsed, -5).len(), 1);

    // The negative id must sort before the positive one, as signed lookup expects.
    let negative = reparsed
        .records()
        .iter()
        .position(|record| record.id() == -5)
        .expect("id -5 present");
    let positive = reparsed
        .records()
        .iter()
        .position(|record| record.id() == 7)
        .expect("id 7 present");
    assert!(negative < positive);
}

#[test]
fn empty_roster_round_trips() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/empty.prp");
    let original = std::fs::read(&path).expect("empty.prp is a checked-in fixture");
    let roster = Roster::parse(&original).expect("empty.prp parses");
    let written = roster.write().expect("empty roster writes");
    assert_eq!(written, original);
    assert!(roster.file_header().invariant_holds(written.len()));
}

#[test]
fn a_removed_then_rewritten_roster_keeps_its_identity() {
    // A round trip through parse→canonicalise→write→parse must preserve the set
    // of `(id)` values and stay parseable, even for a file with duplicate ids.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/dup_id_crc.prp");
    let original = std::fs::read(&path).expect("dup_id_crc.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("parses");
    let key = roster
        .records()
        .iter()
        .find(|record| record.id() == 1000)
        .expect("id 1000 exists")
        .key();

    let mut ids: Vec<i32> = roster.records().iter().map(PropRecord::id).collect();
    ids.sort_unstable();

    roster.remove_prop(key);
    roster.canonicalise();
    let written = roster.write().expect("writes");
    assert!(roster.file_header().invariant_holds(written.len()));
    let reparsed = Roster::parse(&written).expect("parses back");
    assert_eq!(reparsed.records().len(), ids.len() - 1);
    assert!(reparsed.record_for(key).is_none());

    let mut remaining: Vec<i32> = reparsed.records().iter().map(PropRecord::id).collect();
    remaining.sort_unstable();
    let expected: Vec<i32> = {
        let mut v = ids.clone();
        v.remove(v.iter().position(|id| *id == key.id).expect("id present"));
        v
    };
    assert_eq!(remaining, expected);
}

#[test]
fn writing_twice_is_stable() {
    // `write` is a pure function of the model: serialising its own output again
    // must be a fixed point.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/prop_fave.prp");
    let original = std::fs::read(&path).expect("prop_fave.prp is a checked-in fixture");
    let first = Roster::parse(&original)
        .expect("parses")
        .write()
        .expect("writes");
    let second = Roster::parse(&first)
        .expect("reparses")
        .write()
        .expect("rewrites");
    assert_eq!(first, second);
    assert_eq!(first, original);
}

#[test]
fn a_duplicate_key_survives() {
    // Duplicate `(id, crc)` pairs are legal; adding one next to an existing equal
    // key must not merge or drop either.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/dup_id_crc.prp");
    let original = std::fs::read(&path).expect("dup_id_crc.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("parses");
    let existing = roster
        .records()
        .iter()
        .find(|record| record.id() == 1000)
        .expect("id 1000 exists")
        .clone();
    let key = existing.key();
    let before = roster
        .records()
        .iter()
        .filter(|record| record.key() == key)
        .count();

    roster.add_prop(existing);
    assert_eq!(
        roster
            .records()
            .iter()
            .filter(|record| record.key() == key)
            .count(),
        before + 1
    );

    let written = roster.write().expect("writes");
    let reparsed = Roster::parse(&written).expect("parses back");
    assert_eq!(
        reparsed
            .records()
            .iter()
            .filter(|record| record.key() == key)
            .count(),
        before + 1
    );
}

#[test]
fn remove_is_a_no_op_for_an_unknown_key() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/unnamed_records.prp");
    let original = std::fs::read(&path).expect("unnamed_records.prp is a checked-in fixture");
    let mut roster = Roster::parse(&original).expect("parses");
    assert!(roster.remove_prop(PropKey::new(0x7fff_ffff, 0)).is_none());
    assert_eq!(
        Roster::parse(&roster.write().expect("writes"))
            .expect("parses back")
            .records()
            .len(),
        roster.records().len()
    );
}

//! Non-destructive CRC repair: the audit, the repaired copy, and the proof that
//! the source is never touched.
//!
//! The checked-in fixture `fixtures/prp/real_palace_hidden.prp` is a copy of the
//! user's "Palace - Hidden.PRP" collection and carries exactly one stale CRC:
//! record 0 (`ALLBLACK`, id 1675473842). It is the end-to-end case — the repaired
//! copy must pass `validation/independent_reader.py`, which validates `Prop`
//! records only, exactly as the server's `ValidateProps` does.
//!
//! The real-corpus test reads `/tmp/work/prp-oracle/originals/` when it is
//! present and skips otherwise; it never writes into that directory.

use std::fs;
use std::mem::offset_of;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::asset_crc;
use palace_prop::crc_repair;
use palace_prop::prp::{AssetRec, AssetType, Roster};

const ALLBLACK_ID: i32 = 1675473842;
const ALLBLACK_STORED: u32 = 0x5051_93c2;
const ALLBLACK_COMPUTED: u32 = 0x3048_93ad;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-crc-repair-{tag}-{}-{n}",
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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/prp")
        .join(name)
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

fn real_collection(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from("/tmp/work/prp-oracle/originals").join(name);
    path.is_file().then_some(path)
}

fn assert_reader_accepts(path: &Path, expected_records: usize, expected_unvalidated: usize) {
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
            path.display()
        );
        return;
    };
    let output = Command::new(python)
        .arg(&reader)
        .arg("--verbose")
        .arg(path)
        .output()
        .expect("run the independent reader");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the independent reader rejected {}:\n{stdout}\nstderr: {stderr}",
        path.display()
    );
    assert!(stdout.contains("result: ACCEPT"), "{stdout}");
    assert!(stdout.contains("crc_failures: 0"), "{stdout}");
    assert!(
        stdout.contains(&format!("crc_unvalidated: {expected_unvalidated}")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("records: {expected_records}")),
        "{stdout}"
    );
}

#[test]
fn audit_finds_the_prop_mismatch_and_leaves_fave_unvalidated() {
    let bytes = fs::read(fixture("real_palace_hidden.prp")).expect("the fixture reads");
    let audit = crc_repair::audit_bytes(&bytes).expect("the fixture parses");

    assert_eq!(audit.checked, 75, "all 75 Prop records must be hashed");
    assert_eq!(audit.unvalidated, 1, "the Fave sentinel is not a payload");
    assert_eq!(audit.repaired_count(), 1);
    assert!(!audit.is_clean());

    let mismatch = &audit.mismatches[0];
    assert_eq!(mismatch.index, 0);
    assert_eq!(mismatch.key.id, ALLBLACK_ID);
    assert_eq!(mismatch.name.as_deref(), Some("ALLBLACK"));
    assert_eq!(mismatch.stored_crc, ALLBLACK_STORED);
    assert_eq!(mismatch.computed_crc, ALLBLACK_COMPUTED);
    assert_eq!(mismatch.data_size, 2080);
    assert_eq!(mismatch.prop_flags, 0);
}

#[test]
fn the_repaired_copy_passes_the_independent_reader() {
    let temp = TempDir::new("reader");
    let source = temp.path().join("source.prp");
    let destination = temp.path().join("repaired.prp");
    fs::copy(fixture("real_palace_hidden.prp"), &source).expect("stage the source");

    let report = crc_repair::repair_file(&source, &destination).expect("repair");
    assert_eq!(report.audit.repaired_count(), 1);
    assert_eq!(
        report.bytes_written,
        fs::metadata(&destination).expect("stat").len() as usize
    );

    let repaired = fs::read(&destination).expect("read the repaired copy");
    let audit = crc_repair::audit_bytes(&repaired).expect("the repaired copy parses");
    assert!(audit.is_clean(), "repair must leave no mismatch: {audit:?}");
    assert_eq!(audit.checked, 75);
    assert_eq!(audit.unvalidated, 1);

    let roster = Roster::parse(&repaired).expect("the repaired copy parses");
    for record in roster.records() {
        if record.blob.len() >= 12 {
            assert_eq!(
                asset_crc(&record.blob[12..]),
                record.crc(),
                "id {} still has a stale CRC",
                record.id()
            );
        }
    }

    assert_reader_accepts(&destination, 76, 1);
}

#[test]
fn the_repaired_copy_differs_only_in_the_crc_field() {
    let source = fs::read(fixture("real_palace_hidden.prp")).expect("the fixture reads");
    let repaired = crc_repair::repair_bytes(&source).expect("repair").bytes;

    assert_eq!(
        source.len(),
        repaired.len(),
        "repair must not resize the file"
    );
    let differing: Vec<usize> = source
        .iter()
        .zip(repaired.iter())
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, _)| index)
        .collect();

    let roster = Roster::parse(&source).expect("the fixture parses");
    let crc_at = roster.file_header().asset_map_offset as usize
        + roster.map_header().recs_offset as usize
        + offset_of!(AssetRec, crc);
    assert!(
        differing.iter().all(|at| (crc_at..crc_at + 4).contains(at)),
        "only the stale record's CRC field may change, got {differing:?} (CRC at {crc_at})"
    );
    assert!(!differing.is_empty(), "the stale CRC must actually change");

    let before = Roster::parse(&source).expect("source parses");
    let after = Roster::parse(&repaired).expect("copy parses");
    assert_eq!(before.records().len(), after.records().len());
    for index in 1..before.records().len() {
        assert_eq!(
            before.records()[index],
            after.records()[index],
            "record {index} must be untouched"
        );
    }
    let mut expected = before.records()[0].rec;
    expected.crc = ALLBLACK_COMPUTED;
    assert_eq!(after.records()[0].rec, expected);
    assert_eq!(after.records()[0].blob, before.records()[0].blob);
    assert_eq!(after.records()[0].name, before.records()[0].name);
}

#[test]
fn non_prop_records_are_left_untouched() {
    let source = fs::read(fixture("real_palace_hidden.prp")).expect("the fixture reads");
    let repaired = crc_repair::repair_bytes(&source).expect("repair").bytes;
    let before = Roster::parse(&source).expect("source parses");
    let after = Roster::parse(&repaired).expect("copy parses");

    let fave_index = 75;
    let fave_type = before
        .types()
        .iter()
        .find(|asset_type| asset_type.kind() == AssetType::Fave)
        .expect("the fixture has a Fave type");
    assert!(
        fave_index >= fave_type.first_asset.max(0) as usize
            && fave_index < (fave_type.first_asset + fave_type.nbr_assets).max(0) as usize,
        "record {fave_index} must be the Fave sentinel"
    );

    assert_eq!(before.records()[fave_index], after.records()[fave_index]);
    assert_eq!(before.types(), after.types());
    assert_eq!(before.names(), after.names());
}

#[test]
fn a_clean_roster_repairs_to_a_byte_identical_copy() {
    let source = fs::read(fixture("prop_fave.prp")).expect("the fixture reads");
    let audit = crc_repair::audit_bytes(&source).expect("the fixture parses");
    assert!(audit.is_clean());

    let repaired = crc_repair::repair_bytes(&source).expect("repair");
    assert_eq!(repaired.bytes, source);
}

#[test]
fn repair_refuses_to_write_over_the_source() {
    let temp = TempDir::new("in-place");
    let source = temp.path().join("source.prp");
    fs::copy(fixture("real_palace_hidden.prp"), &source).expect("stage the source");
    let before = fs::read(&source).expect("read the source");

    let error = crc_repair::repair_file(&source, &source).expect_err("in-place repair is refused");
    assert!(error.to_string().contains("refusing"), "{error}");
    assert_eq!(before, fs::read(&source).expect("read again"));

    #[cfg(unix)]
    {
        let link = temp.path().join("link.prp");
        std::os::unix::fs::symlink(&source, &link).expect("symlink the source");
        let error = crc_repair::repair_file(&source, &link)
            .expect_err("a symlink to the source is refused");
        assert!(error.to_string().contains("refusing"), "{error}");
        assert_eq!(before, fs::read(&source).expect("read again"));
    }
}

#[test]
fn repair_refuses_a_file_with_trailing_bytes() {
    let mut source = fs::read(fixture("real_palace_hidden.prp")).expect("the fixture reads");
    source.extend_from_slice(b"trailing junk");
    let error = crc_repair::repair_bytes(&source).expect_err("trailing bytes are refused");
    assert!(error.to_string().contains("filesize"), "{error}");

    let temp = TempDir::new("trailing");
    let staged = temp.path().join("staged.prp");
    let destination = temp.path().join("copy.prp");
    fs::write(&staged, &source).expect("stage");
    let error = crc_repair::repair_file(&staged, &destination).expect_err("refused");
    assert!(error.to_string().contains("filesize"), "{error}");
    assert!(!destination.exists(), "no copy may be produced");
    assert_eq!(fs::read(&staged).expect("read"), source);
}

#[test]
fn the_reader_skips_non_prop_records_and_flags_prop_records() {
    let reader = independent_reader_path();
    assert!(
        reader.is_file(),
        "the independent reader is missing at {}",
        reader.display()
    );
    let Some(python) = python_command() else {
        eprintln!("skipping: no python interpreter on PATH");
        return;
    };

    let source = fs::read(fixture("prop_fave.prp")).expect("the fixture reads");
    let roster = Roster::parse(&source).expect("the fixture parses");
    let index_of = |kind: AssetType| {
        roster
            .types()
            .iter()
            .find(|asset_type| asset_type.kind() == kind)
            .map(|asset_type| asset_type.first_asset.max(0) as usize)
    };
    let prop_index = index_of(AssetType::Prop).expect("the fixture has a Prop record");
    let fave_index = index_of(AssetType::Fave).expect("the fixture has a Fave record");

    let temp = TempDir::new("reader-prop-only");

    let mut fave_bad = source.clone();
    patch_crc(&mut fave_bad, &roster, fave_index, 0xdead_beef);
    let fave_path = temp.path().join("fave-bad.prp");
    fs::write(&fave_path, &fave_bad).expect("write");
    let output = Command::new(python)
        .arg(&reader)
        .arg("--verbose")
        .arg(&fave_path)
        .output()
        .expect("run the reader");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a wrong CRC on a Fave record must not fail the audit:\n{stdout}"
    );
    assert!(stdout.contains("result: ACCEPT"), "{stdout}");
    assert!(stdout.contains("CRC not validated (type Fave)"), "{stdout}");
    assert!(stdout.contains("crc_unvalidated: 1"), "{stdout}");

    let mut prop_bad = source.clone();
    patch_crc(&mut prop_bad, &roster, prop_index, 0xdead_beef);
    let prop_path = temp.path().join("prop-bad.prp");
    fs::write(&prop_path, &prop_bad).expect("write");
    let output = Command::new(python)
        .arg(&reader)
        .arg("--verbose")
        .arg(&prop_path)
        .output()
        .expect("run the reader");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a wrong CRC on a Prop record must fail the audit:\n{stdout}"
    );
    assert!(stdout.contains("result: REJECT"), "{stdout}");
    assert!(stdout.contains("CRC MISMATCH"), "{stdout}");
}

fn patch_crc(bytes: &mut [u8], roster: &Roster, index: usize, crc: u32) {
    let at = roster.file_header().asset_map_offset as usize
        + roster.map_header().recs_offset as usize
        + index * std::mem::size_of::<AssetRec>()
        + offset_of!(AssetRec, crc);
    bytes[at..at + 4].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn repairing_a_real_collection_never_touches_the_source() {
    let Some(source) = real_collection("Palace - Hidden.PRP") else {
        eprintln!("skipping: /tmp/work/prp-oracle/originals is not present");
        return;
    };
    let temp = TempDir::new("real");
    let destination = temp.path().join("Palace - Hidden.repaired.prp");
    let before = fs::read(&source).expect("read the real source");

    let report = crc_repair::repair_file(&source, &destination).expect("repair the real file");
    assert_eq!(report.audit.repaired_count(), 1);

    let after = fs::read(&source).expect("read the real source again");
    assert_eq!(
        before, after,
        "the source must be byte-identical (hence sha256-identical) after repair"
    );
    assert_ne!(
        fs::read(&destination).expect("read the copy"),
        before,
        "the copy must actually carry the repaired CRC"
    );
    assert_reader_accepts(&destination, 76, 1);

    if let Ok(dir) = std::env::var("PALACE_CRC_REPAIR_EVIDENCE") {
        let dir = PathBuf::from(dir);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::copy(&destination, dir.join("Palace - Hidden.repaired.prp"));
    }
}

#[test]
fn repairing_a_real_palace_roster_never_touches_the_source() {
    let Some(source) = real_collection("Palace.prp") else {
        eprintln!("skipping: /tmp/work/prp-oracle/originals is not present");
        return;
    };
    let temp = TempDir::new("real-palace");
    let destination = temp.path().join("Palace.repaired.prp");
    let before = fs::read(&source).expect("read the real source");

    let report = crc_repair::repair_file(&source, &destination).expect("repair the real file");
    assert_eq!(report.audit.repaired_count(), 1);
    let mismatch = &report.audit.mismatches[0];
    assert_eq!(mismatch.key.id, 969004551);
    assert_eq!(mismatch.name.as_deref(), Some("b504"));
    assert_eq!(mismatch.stored_crc, 0xfd25_0a2d);
    assert_eq!(mismatch.computed_crc, 0xc518_5c86);

    let after = fs::read(&source).expect("read the real source again");
    assert_eq!(
        before, after,
        "the source must be byte-identical (hence sha256-identical) after repair"
    );
    let repaired = fs::read(&destination).expect("read the copy");
    let audit = crc_repair::audit_bytes(&repaired).expect("the repaired copy parses");
    assert!(audit.is_clean(), "repair must leave no mismatch");
    assert_eq!(audit.unvalidated, 3, "three Fave records stay unvalidated");

    if let Ok(dir) = std::env::var("PALACE_CRC_REPAIR_EVIDENCE") {
        let dir = PathBuf::from(dir);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::copy(&destination, dir.join("Palace.repaired.prp"));
    }
}

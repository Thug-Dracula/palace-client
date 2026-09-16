//! Whole-corpus validation, gated behind `PALACE_PROP_CORPUS`.
//!
//! ```text
//! PALACE_PROP_CORPUS="$CORPUS/props_harvested:$CORPUS/pserver.prp" \
//!   cargo test -p palace-prop --test corpus -- --ignored --nocapture
//! ```
//!
//! Entries are colon-separated. A directory is walked for `*.bin` prop blobs; a
//! `.prp` roster is read with the container layout from `$CORPUS/PRP-FORMAT.md`;
//! anything else is treated as a single prop blob.
//!
//! The test prints the same table `prop-tool inventory` does, so the numbers in the
//! crate README can be reproduced from `cargo test` alone. It asserts only that
//! every blob was accounted for — the exact per-format counts are the *result* of
//! the run, not a precondition, so the test keeps working as the corpus grows.
//! Set `PALACE_PROP_EXPECT_FAILURES` to additionally pin the rejected-prop count.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use palace_prop::{decode, decode_header, PropFormat};

#[test]
#[ignore = "needs the local corpus; see the module docs for how to run it"]
fn whole_local_corpus_decodes() {
    let spec = std::env::var("PALACE_PROP_CORPUS").unwrap_or_default();
    if spec.is_empty() {
        panic!("set PALACE_PROP_CORPUS to a colon-separated list of directories and .prp files");
    }

    let mut decoded: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut endian: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut total = 0u64;

    for entry in spec.split(':').filter(|entry| !entry.is_empty()) {
        let path = PathBuf::from(entry);
        let blobs = load(&path);
        println!("{}: {} blobs", path.display(), blobs.len());
        for (label, bytes) in blobs {
            total += 1;
            let header = match decode_header(&bytes) {
                Ok(header) => header,
                Err(e) => {
                    failures.push((label, e.to_string()));
                    continue;
                }
            };
            let format = header.format().name();
            match decode(&bytes) {
                Ok(prop) => {
                    *decoded.entry(format).or_default() += 1;
                    let order = match prop.header.endian {
                        palace_prop::PropEndian::Little => "little",
                        palace_prop::PropEndian::Big => "big",
                    };
                    *endian.entry(order).or_default() += 1;
                }
                Err(e) => failures.push((label, e.to_string())),
            }
        }
    }

    println!("\nformat        count");
    for format in [
        PropFormat::EightBit,
        PropFormat::SixteenBit,
        PropFormat::TwentyBit,
        PropFormat::S20Bit,
        PropFormat::ThirtyTwoBit,
    ] {
        println!(
            "{:12}  {}",
            format.name(),
            decoded.get(format.name()).copied().unwrap_or(0)
        );
    }
    println!("endian        {endian:?}");
    println!("rejected      {}", failures.len());
    for (label, reason) in failures.iter().take(40) {
        println!("  FAIL {label}: {reason}");
    }

    let success = decoded.values().sum::<u64>();
    assert_eq!(
        success + failures.len() as u64,
        total,
        "every blob must be either decoded or reported"
    );
    assert!(success > 0, "the corpus decoded nothing at all");

    if let Ok(expected) = std::env::var("PALACE_PROP_EXPECT_FAILURES") {
        let expected: usize = expected
            .parse()
            .expect("PALACE_PROP_EXPECT_FAILURES is a count");
        assert_eq!(
            failures.len(),
            expected,
            "rejected-prop count changed: {failures:#?}"
        );
    }
}

fn load(path: &Path) -> Vec<(String, Vec<u8>)> {
    if path.is_dir() {
        let mut out = Vec::new();
        walk(path, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        return out;
    }
    if path.extension().is_some_and(|e| e == "prp") {
        if let Some(records) = roster(path) {
            return records;
        }
    }
    match std::fs::read(path) {
        Ok(bytes) => vec![(path.display().to_string(), bytes)],
        Err(e) => {
            println!("skipping {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "bin") {
            if let Ok(bytes) = std::fs::read(&path) {
                out.push((path.display().to_string(), bytes));
            }
        }
    }
}

/// Read a `.prp` roster: 16-byte file header, data region, then 32-byte records.
fn roster(path: &Path) -> Option<Vec<(String, Vec<u8>)>> {
    let buf = std::fs::read(path).ok()?;
    let u32_at = |at: usize| -> Option<u32> {
        buf.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let data_offset = u32_at(0)? as usize;
    let map_offset = u32_at(8)? as usize;
    let n_assets = u32_at(map_offset + 4)? as usize;
    let recs_offset = u32_at(map_offset + 16)? as usize;
    let mut out = Vec::new();
    for i in 0..n_assets {
        let at = map_offset + recs_offset + i * 32;
        let Some(record) = buf.get(at..at + 32) else {
            break;
        };
        let id = u32::from_le_bytes(record[0..4].try_into().ok()?);
        let data_rel = u32::from_le_bytes(record[8..12].try_into().ok()?) as usize;
        let size = u32::from_le_bytes(record[12..16].try_into().ok()?) as usize;
        if size < 12 {
            continue;
        }
        if let Some(blob) = buf.get(data_offset + data_rel..data_offset + data_rel + size) {
            out.push((format!("{}#{id}", path.display()), blob.to_vec()));
        }
    }
    Some(out)
}

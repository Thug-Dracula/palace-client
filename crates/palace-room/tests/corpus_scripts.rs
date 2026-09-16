//! Differential: `palace-room`'s hotspot script extraction vs the harvested corpus.
//!
//! `$CORPUS/scripts_clean/by_script/<payload>_hs<idx>.txt` holds 2,400 scripts
//! produced by an independent Python extractor
//! (`$CORPUS/tools/extract_scripts.py`) from the 799 payloads in
//! `$CORPUS/payloads_all/`. This test re-extracts them through `palace-room`
//! and asserts the bytes match, so the Rust parser cannot silently drift from the
//! reference corpus that the IPTSCRAE dispatch work depends on.
//!
//! Hotspot indices are numbered **globally across a payload's records**, matching
//! the Python extractor's output (four payloads concatenate several room records).
//!
//! Skipped when the corpus is absent, so it never blocks a machine without the
//! reference data. Point `PALACE_SCRIPTS_CORPUS` at the `colosseum` root to use a
//! different corpus.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use palace_room::decode_stream;
use palace_wire::ByteOrder;

fn latin1_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn corpus() -> Option<(PathBuf, PathBuf)> {
    let root = std::env::var("PALACE_SCRIPTS_CORPUS")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join("colosseum"))
        })?;
    let by = root.join("scripts_clean").join("by_script");
    let payloads = root.join("payloads_all");
    (by.is_dir() && payloads.is_dir()).then_some((payloads, by))
}

#[test]
fn rust_extraction_matches_the_harvested_corpus() {
    let Some((payloads, by)) = corpus() else {
        eprintln!("scripts corpus absent; skipping");
        return;
    };

    // Every harvested script, keyed "<payload-stem>_hs<idx>".
    let mut harvested: BTreeSet<String> = BTreeSet::new();
    for entry in fs::read_dir(&by).expect("by_script must be readable") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("txt") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                harvested.insert(stem.to_string());
            }
        }
    }
    assert!(
        !harvested.is_empty(),
        "no harvested scripts found in {by:?} - corpus mapping is wrong"
    );

    let mut payloads_seen = 0usize;
    let mut compared = 0usize;
    let mut matched = 0usize;
    let mut encoding_only = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    let mut consumed: BTreeSet<String> = BTreeSet::new();

    let mut files: Vec<PathBuf> = fs::read_dir(&payloads)
        .expect("payload dir must be readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bin"))
        .collect();
    files.sort();

    for path in files {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let bytes = fs::read(&path).expect("payload must be readable");
        let records = decode_stream(&bytes, ByteOrder::Little);
        if records.is_empty() {
            continue;
        }
        payloads_seen += 1;

        let mut idx = 0usize;
        for record in records {
            let Ok(desc) = record else { continue };
            for spot in &desc.hotspots {
                let key = format!("{stem}_hs{idx}");
                idx += 1;
                if !harvested.contains(&key) {
                    continue;
                }
                consumed.insert(key.clone());
                compared += 1;

                let harvested_bytes = fs::read(by.join(format!("{key}.txt"))).unwrap_or_default();
                let harvested_text = latin1_text(&harvested_bytes);
                match spot.script.as_deref() {
                    Some(text) if *text == harvested_text => {
                        matched += 1;
                        if text.as_bytes() != harvested_bytes.as_slice() {
                            encoding_only += 1;
                        }
                    }
                    Some(text) => {
                        let rust_chars = text.chars().count();
                        let harvested_chars = harvested_text.chars().count();
                        let first_diff = text
                            .chars()
                            .zip(harvested_text.chars())
                            .position(|(a, b)| a != b)
                            .unwrap_or_else(|| rust_chars.min(harvested_chars));
                        let window_start = first_diff.saturating_sub(12);
                        let rust_window: String = text.chars().skip(window_start).take(24).collect();
                        let harvested_window: String =
                            harvested_text.chars().skip(window_start).take(24).collect();
                        mismatches.push(format!(
                            "{key}: rust {rust_chars} chars != harvested {harvested_chars} chars | first diff @ {first_diff} | rust={rust_window:?} | harvested={harvested_window:?}"
                        ));
                    }
                    None => mismatches.push(format!("{key}: rust produced no script")),
                }
            }
        }
    }

    let unconsumed: Vec<String> = harvested.difference(&consumed).cloned().collect();

    println!(
        "payloads: {payloads_seen}  harvested: {}  compared: {compared}  text-equal: {matched}  (byte-identical: {})",
        harvested.len(),
        matched - encoding_only
    );
    println!(
        "mismatches: {}  unconsumed: {}",
        mismatches.len(),
        unconsumed.len()
    );
    for m in mismatches.iter().take(15) {
        println!("  MISMATCH    {m}");
    }
    for u in unconsumed.iter().take(15) {
        println!("  UNCONSUMED  {u}");
    }

    assert!(compared > 0, "nothing compared - corpus mapping is wrong");
    assert!(
        mismatches.is_empty(),
        "{} script(s) differ from the harvested corpus",
        mismatches.len()
    );
    assert!(
        unconsumed.is_empty(),
        "{} harvested script(s) were not reproduced",
        unconsumed.len()
    );
}

//! Differential test: hotspot script extraction vs the Python extractor.
//!
//! `$CORPUS/tools/extract_scripts.py` is the ground truth for the hotspot
//! offset layout: it reads `scriptTextOfst` (record offset +44, unsigned) and
//! cuts the NUL-terminated script out of `varBuf`, and its output covers every
//! `ON` marker in the corpus. This test re-extracts the same scripts with
//! `palace-room` and compares them, room by room and hotspot by hotspot, against
//! `scripts_clean/by_script/` and `manifest.json`.
//!
//! Skipped (not failed) when either corpus is absent, so a machine without the
//! reference data still runs the suite. Point `PALACE_SCRIPT_CORPUS` at another
//! `scripts_clean` directory to compare a different harvest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use palace_room::decode_stream;
use palace_wire::ByteOrder;
use serde_json::Value;

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn payload_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PALACE_ROOM_CORPUS")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join("colosseum").join("payloads_all")))?;
    dir.is_dir().then_some(dir)
}

fn scripts_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PALACE_SCRIPT_CORPUS")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join("colosseum").join("scripts_clean")))?;
    dir.join("manifest.json").is_file().then_some(dir)
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[test]
fn rust_extraction_matches_the_python_harvest() {
    let (Some(payloads), Some(scripts)) = (payload_dir(), scripts_dir()) else {
        eprintln!("skipping script extraction diff: no corpus found");
        return;
    };
    let Ok(manifest) = std::fs::read_to_string(scripts.join("manifest.json")) else {
        eprintln!("skipping script extraction diff: manifest unreadable");
        return;
    };
    let Ok(manifest) = serde_json::from_str::<Value>(&manifest) else {
        eprintln!("skipping script extraction diff: manifest is not JSON");
        return;
    };
    let entries = manifest
        .get("scripts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(!entries.is_empty(), "manifest lists no scripts");

    let mut by_payload: BTreeMap<String, Vec<(i32, usize, String)>> = BTreeMap::new();
    for entry in &entries {
        let Some(source) = entry.get("source_payload").and_then(Value::as_str) else {
            continue;
        };
        let Some(room_id) = entry
            .get("room_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let Some(index) = entry
            .get("hotspot_index")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
        else {
            continue;
        };
        let Some(file) = entry.get("file").and_then(Value::as_str) else {
            continue;
        };
        by_payload
            .entry(source.to_string())
            .or_default()
            .push((room_id, index, file.to_string()));
    }

    let mut matched = 0usize;
    let mut differing = 0usize;
    let mut missing_room = 0usize;
    let mut missing_hotspot = 0usize;
    let mut unreadable = 0usize;
    let mut samples: Vec<String> = Vec::new();
    let mut records_seen = 0usize;

    for (payload, wanted) in &by_payload {
        let Ok(bytes) = std::fs::read(payloads.join(payload)) else {
            unreadable += wanted.len();
            continue;
        };
        let records = decode_stream(&bytes, ByteOrder::Little);
        records_seen += records.len();
        for (room_id, index, file) in wanted {
            let room = records
                .iter()
                .filter_map(|r| r.as_ref().ok())
                .find(|r| i32::from(r.header.room_id) == *room_id);
            let Some(room) = room else {
                missing_room += 1;
                if samples.len() < 10 {
                    samples.push(format!("{file}: room {room_id} not in {payload}"));
                }
                continue;
            };
            let Some(hotspot) = room.hotspots.get(*index) else {
                missing_hotspot += 1;
                if samples.len() < 10 {
                    samples.push(format!("{file}: hotspot {index} missing in room {room_id}"));
                }
                continue;
            };
            let name = Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let Ok(raw) = std::fs::read(scripts.join("by_script").join(name)) else {
                unreadable += 1;
                continue;
            };
            let got = hotspot.script.clone().unwrap_or_default();
            let want = latin1(&raw);
            if got == want {
                matched += 1;
            } else {
                differing += 1;
                if samples.len() < 10 {
                    samples.push(format!(
                        "{file}: got {:?} want {:?}",
                        got.chars().take(50).collect::<String>(),
                        want.chars().take(50).collect::<String>()
                    ));
                }
            }
        }
    }

    let total = matched + differing + missing_room + missing_hotspot + unreadable;
    let rate = matched as f64 / total as f64;
    eprintln!(
        "script extraction: {matched}/{total} identical ({rate:.4}) across {records_seen} records; \
         differing {differing}, missing room {missing_room}, missing hotspot {missing_hotspot}, \
         unreadable {unreadable}"
    );
    for sample in &samples {
        eprintln!("  {sample}");
    }
    assert_eq!(differing, 0, "different script text than the Python extractor");
    assert_eq!(missing_room, 0, "rooms the Rust decoder did not find");
    assert_eq!(missing_hotspot, 0, "hotspots the Rust decoder did not find");
    assert_eq!(unreadable, 0, "payload or script file could not be read");
    assert!(rate >= 0.999, "match rate {rate:.4} below 99.9%");
}

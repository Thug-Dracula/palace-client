//! Corpus replay: walk every captured room payload offline.
//!
//! `~/palace-corpus/payloads_all/` holds 799 raw `MSG_ROOMDESC` payloads (no frame
//! header) harvested from a live server. Four of them concatenate more than one
//! room record. The test decodes all of them, asserts nothing panics, that every
//! record parses, and that none produces a warning. It is skipped when the
//! corpus is absent, so it never blocks a machine without the reference data.
//!
//! Point `PALACE_ROOM_CORPUS` at another directory to run against a different
//! corpus.

use std::path::PathBuf;

use palace_room::decode_stream;
use palace_wire::ByteOrder;

fn corpus_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PALACE_ROOM_CORPUS") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join("colosseum").join("payloads_all");
    dir.is_dir().then_some(dir)
}

fn payload_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("corpus directory must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("bin"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_corpus_record_parses_cleanly() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping corpus replay: no payload corpus found");
        return;
    };
    let files = payload_files(&dir);
    assert!(!files.is_empty(), "corpus directory is empty");

    let mut records = 0usize;
    let mut with_warnings = 0usize;
    let mut multi_record_files: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let bytes = std::fs::read(path).expect("payload must be readable");
        let decoded = decode_stream(&bytes, ByteOrder::Little);
        assert!(
            !decoded.is_empty(),
            "{} produced no record at all",
            path.display()
        );
        if decoded.len() > 1 {
            multi_record_files.push(format!(
                "{}:{}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                decoded.len()
            ));
        }
        for (index, result) in decoded.iter().enumerate() {
            match result {
                Ok(desc) => {
                    records += 1;
                    if !desc.is_clean() {
                        with_warnings += 1;
                        failures.push(format!(
                            "{}[{}] warnings: {:?}",
                            path.display(),
                            index,
                            desc.warnings
                        ));
                    }
                    assert_eq!(
                        desc.pictures.len(),
                        desc.header.nbr_pictures.max(0) as usize,
                        "{}[{}] picture count",
                        path.display(),
                        index
                    );
                    assert_eq!(
                        desc.hotspots.len(),
                        desc.header.nbr_hotspots.max(0) as usize,
                        "{}[{}] hotspot count",
                        path.display(),
                        index
                    );
                    assert_eq!(
                        desc.loose_props.len(),
                        desc.header.nbr_lprops.max(0) as usize,
                        "{}[{}] loose-prop count",
                        path.display(),
                        index
                    );
                    assert_eq!(
                        desc.draw_cmds.len(),
                        desc.header.nbr_draw_cmds.max(0) as usize,
                        "{}[{}] draw-command count",
                        path.display(),
                        index
                    );
                }
                Err(err) => failures.push(format!("{}[{}] error: {err}", path.display(), index)),
            }
        }
    }

    eprintln!(
        "corpus: {} files, {} records, {} files multi-record, {} records with warnings",
        files.len(),
        records,
        multi_record_files.len(),
        with_warnings
    );
    eprintln!("multi-record files: {multi_record_files:?}");
    assert!(
        failures.is_empty(),
        "corpus failures:\n{}",
        failures.join("\n")
    );
    assert_eq!(
        records,
        files.len() + 5,
        "5 extra records come from 4 concatenated captures"
    );
}

#[test]
fn the_concatenated_captures_split_into_the_expected_records() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping corpus replay: no payload corpus found");
        return;
    };
    let expected: &[(&str, usize)] = &[
        ("11021.bin", 2),
        ("11054.bin", 2),
        ("11060.bin", 3),
        ("4679.bin", 2),
    ];
    for (name, count) in expected {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("payload must be readable");
        let decoded = decode_stream(&bytes, ByteOrder::Little);
        assert_eq!(decoded.len(), *count, "{name} record count");
        assert!(
            decoded.iter().all(Result::is_ok),
            "{name} must decode cleanly"
        );
    }
}

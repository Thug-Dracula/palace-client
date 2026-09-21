//! The `.prp` reader: a synthetic fixture for the happy path, a real roster for
//! the pinned facts, and the tolerance/truncation behaviour the format demands.
//!
//! The real fixture is a local copy of `Palace - Hidden.PRP`; the test looks for
//! it at `/tmp/palace-hidden.prp` (or `PALACE_PRP_FIXTURE`). Its counts and the
//! one stale CRC are pinned below because they are the observable contract the
//! later writer must preserve — not because the reader repairs them.
//!
//! Both real-fixture tests are `#[ignore]`d: the fixture is local data that is
//! not in the repository, so a normal run — and CI — must skip them rather than
//! fail. Run them with
//! `cargo test -p palace-prop --test prp_read -- --ignored` once it is in place.

use std::panic::catch_unwind;

use palace_prop::asset_crc;
use palace_prop::prp::{AssetType, PropEncoding, PropKey, Roster};

/// A 12-byte blob header: 44x44, offsets 0, script 0, flags 0x000a (HEAD|RARE,
/// which selects the 8-bit encoding).
const BLOB_HEADER: [u8; 12] = [44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x00];

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn blob(payload: &[u8]) -> Vec<u8> {
    let mut out = BLOB_HEADER.to_vec();
    out.extend_from_slice(payload);
    out
}

/// A minimal valid roster: one `Prop` type, two records (the first named), two
/// 8-bit blobs.
///
/// Layout: header 16, data 32, map 104 => file 152.
fn synth_prp() -> Vec<u8> {
    let b0 = blob(&[1, 2, 3, 4]);
    let b1 = blob(&[5, 6, 7, 8]);
    let names: &[u8] = b"\x03One";
    let data_size = (b0.len() + b1.len()) as u32; // 32
    let map_off = 16 + data_size; // 48
    let recs_off = 24 + 12; // 36
    let names_off = recs_off + 2 * 32; // 100
    let map_size = 24 + 12 + 2 * 32 + names.len() as u32; // 104

    let mut out = Vec::new();
    push_u32(&mut out, 16);
    push_u32(&mut out, data_size);
    push_u32(&mut out, map_off);
    push_u32(&mut out, map_size);

    out.extend_from_slice(&b0);
    out.extend_from_slice(&b1);

    push_i32(&mut out, 1); // nbrTypes
    push_i32(&mut out, 2); // nbrAssets
    push_i32(&mut out, names.len() as i32); // lenNames
    push_u32(&mut out, 24); // typesOffset
    push_u32(&mut out, recs_off); // recsOffset
    push_u32(&mut out, names_off); // namesOffset

    push_i32(&mut out, AssetType::PROP as i32); // AssetTypeRec
    push_i32(&mut out, 2);
    push_i32(&mut out, 0);

    push_i32(&mut out, 1); // AssetRec 0
    push_i32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, b0.len() as u32);
    push_i32(&mut out, 0);
    push_i32(&mut out, 0); // nameOffset -> "One"
    push_u32(&mut out, 0);
    push_u32(&mut out, asset_crc(&b0[12..]));

    push_i32(&mut out, 2); // AssetRec 1
    push_i32(&mut out, 0);
    push_u32(&mut out, b0.len() as u32);
    push_u32(&mut out, b1.len() as u32);
    push_i32(&mut out, 0);
    push_i32(&mut out, -1); // unnamed
    push_u32(&mut out, 0);
    push_u32(&mut out, asset_crc(&b1[12..]));

    out.extend_from_slice(names);
    out
}

fn real_prp() -> Option<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("PALACE_PRP_FIXTURE") {
        candidates.push(path);
    }
    candidates.push("/tmp/palace-hidden.prp".to_string());
    candidates.push("/tmp/work/prp-oracle/originals/Palace - Hidden.PRP".to_string());
    candidates.iter().find_map(|path| std::fs::read(path).ok())
}

#[test]
fn a_synthetic_roster_parses_names_and_encodings() {
    let buf = synth_prp();
    let roster = Roster::parse(&buf).expect("the synthetic roster parses");

    assert_eq!(roster.records().len(), 2);
    assert_eq!(roster.types().len(), 1);
    assert_eq!(roster.types()[0].kind(), AssetType::Prop);
    assert_eq!(roster.types()[0].nbr_assets, 2);
    assert_eq!(roster.types()[0].first_asset, 0);
    assert_eq!(roster.map_header().nbr_assets, 2);
    assert!(roster.file_header().invariant_holds(buf.len()));
    assert_eq!(roster.dropped_records(), 0);

    assert_eq!(roster.names().len(), 1);
    assert_eq!(roster.names()[0], (0u32, "One".to_string()));

    let first = &roster.records()[0];
    assert_eq!(first.id(), 1);
    assert_eq!(first.name.as_deref(), Some("One"));
    assert_eq!(first.encoding, Some(PropEncoding::EightBit));
    assert_eq!(first.header.map(|header| header.width), Some(44));
    assert_eq!(asset_crc(&first.blob[12..]), first.crc());
    assert_eq!(
        roster.blob_for(PropKey::new(first.id(), first.crc())),
        Some(first.blob.as_slice())
    );

    assert_eq!(roster.records()[1].name, None);
}

#[test]
fn trailing_garbage_is_ignored_but_the_invariant_reports_it() {
    let mut buf = synth_prp();
    let clean = buf.len();
    buf.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x11]);

    let roster = Roster::parse(&buf).expect("trailing bytes must not fail the parse");
    assert_eq!(roster.records().len(), 2);
    assert!(roster.file_header().invariant_holds(clean));
    assert!(!roster.file_header().invariant_holds(buf.len()));
}

#[test]
fn an_out_of_bounds_record_is_dropped_and_counted() {
    let mut buf = synth_prp();
    // rec[1].data_offset sits at map(48) + recsOffset(36) + 32 + 8.
    let field = 48 + 36 + 32 + 8;
    buf[field..field + 4].copy_from_slice(&0xFFFF_u32.to_le_bytes());

    let roster = Roster::parse(&buf).expect("a bad record must not fail the parse");
    assert_eq!(roster.records().len(), 1);
    assert_eq!(roster.dropped_records(), 1);
    assert!(roster.records()[0].name.is_some());
}

#[test]
fn every_prefix_of_the_synthetic_roster_is_handled_without_panicking() {
    let buf = synth_prp();
    for cut in 0..=buf.len() {
        let outcome = catch_unwind(|| Roster::parse(&buf[..cut]));
        assert!(outcome.is_ok(), "parse panicked at {cut} bytes");
    }
}

#[test]
#[ignore = "reads a local copy of Palace - Hidden.PRP; set PALACE_PRP_FIXTURE or place it at /tmp/palace-hidden.prp"]
fn the_real_hidden_roster_matches_the_pinned_facts() {
    let Some(buf) = real_prp() else {
        panic!(
            "place a copy of Palace - Hidden.PRP at /tmp/palace-hidden.prp \
             or set PALACE_PRP_FIXTURE"
        );
    };
    let roster = Roster::parse(&buf).expect("the real Hidden roster parses");

    assert!(roster.file_header().invariant_holds(buf.len()));
    assert_eq!(roster.records().len(), 76);
    assert_eq!(roster.dropped_records(), 0);

    assert_eq!(roster.types().len(), 2);
    assert_eq!(roster.types()[0].kind(), AssetType::Prop);
    assert_eq!(roster.types()[0].nbr_assets, 75);
    assert_eq!(roster.types()[0].first_asset, 0);
    assert_eq!(roster.types()[1].kind(), AssetType::Fave);
    assert_eq!(roster.types()[1].nbr_assets, 1);
    assert_eq!(roster.types()[1].first_asset, 75);

    assert_eq!(roster.names().len(), 56);
    assert_eq!(roster.names()[0], (0u32, "WP".to_string()));
    assert_eq!(
        roster.records().iter().filter(|r| r.name.is_some()).count(),
        56
    );

    // The Fave sentinel is one record with no blob; keep it, do not skip it.
    assert!(roster.records()[75].blob.len() < 12);
    assert!(roster.records()[75].header.is_none());
    assert_eq!(roster.records()[75].name, None);

    // The first record is named and its 8-bit encoding is detected.
    let allblack = roster
        .records()
        .iter()
        .find(|r| r.name.as_deref() == Some("ALLBLACK"))
        .expect("ALLBLACK is a named record");
    assert_eq!(allblack.id(), 1_675_473_842);
    assert_eq!(allblack.blob.len(), 2080);
    assert_eq!(allblack.encoding, Some(PropEncoding::EightBit));

    // Every payload-bearing record's CRC matches except the one stale CRC this
    // real file carries (ALLBLACK); the reader preserves it verbatim.
    let payload: Vec<_> = roster
        .records()
        .iter()
        .filter(|r| r.blob.len() >= 12)
        .collect();
    assert_eq!(payload.len(), 75);
    let mut matched = 0usize;
    let mut stale = Vec::new();
    for record in &payload {
        if asset_crc(&record.blob[12..]) == record.crc() {
            matched += 1;
        } else {
            stale.push(record.id());
        }
    }
    assert_eq!(matched, 74);
    assert_eq!(stale, vec![1_675_473_842]);

    // A deterministic sample that avoids the stale record: all must match.
    for record in payload.iter().skip(1).step_by(4).take(20) {
        assert_eq!(
            asset_crc(&record.blob[12..]),
            record.crc(),
            "id {}",
            record.id()
        );
    }
}

#[test]
#[ignore = "reads a local copy of Palace - Hidden.PRP; set PALACE_PRP_FIXTURE or place it at /tmp/palace-hidden.prp"]
fn a_truncated_real_roster_errors_without_panicking() {
    let Some(buf) = real_prp() else {
        panic!(
            "place a copy of Palace - Hidden.PRP at /tmp/palace-hidden.prp \
             or set PALACE_PRP_FIXTURE"
        );
    };
    let truncated = &buf[..buf.len() * 9 / 10];

    match catch_unwind(|| Roster::parse(truncated)) {
        Ok(Ok(roster)) => panic!(
            "a truncated file parsed into {} records instead of failing",
            roster.records().len()
        ),
        Ok(Err(error)) => assert!(!error.to_string().is_empty(), "error must be descriptive"),
        Err(_) => panic!("Roster::parse panicked on a truncated file"),
    }
}

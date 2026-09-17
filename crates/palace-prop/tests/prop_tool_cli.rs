//! Integration tests for the `prop-tool` binary.
//!
//! `prop-tool` is the corpus-facing byte tool: it decodes, re-encodes and
//! digests props, and splits rosters. None of it was executed by the test suite
//! before. These tests run the real binary with `std::process::Command` against
//! the tiny git-tracked blobs in `crates/palace-prop/fixtures/`, plus one
//! synthetic `.prp` roster assembled in-test, and assert on the files it writes
//! and the lines it prints.
//!
//! Nothing here needs the 700 MB corpus or the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::{decode, Prop, PropFormat};

const BIN: &str = env!("CARGO_BIN_EXE_prop-tool");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("spawn prop-tool")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("process exited normally")
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("prop-tool-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn decode_prop(path: &Path) -> Prop {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    decode(&bytes).unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
}

/// The `PRGBA1` raw dump format written by `rgba` / `rgba-batch`:
/// `"PRGBA1"` + width LE + height LE + RGBA bytes.
fn parse_rgba(bytes: &[u8]) -> Option<(u32, u32, &[u8])> {
    if bytes.len() < 14 || &bytes[0..6] != b"PRGBA1" {
        return None;
    }
    let width = u32::from_le_bytes(bytes[6..10].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[10..14].try_into().ok()?);
    Some((width, height, &bytes[14..]))
}

/// A PNG's `IHDR` width/height, or `None` if it is not a PNG.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || &bytes[0..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

fn fnv1a(width: u32, height: u32, rgba: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in width
        .to_le_bytes()
        .iter()
        .chain(height.to_le_bytes().iter())
        .chain(rgba.iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// Dispatch / usage.
// ---------------------------------------------------------------------------

#[test]
fn help_is_printed_to_stderr_and_exits_zero() {
    for word in ["help", "--help", "-h"] {
        let out = run(&[word]);
        assert_eq!(code(&out), 0, "{word}");
        assert!(stdout(&out).is_empty(), "{word} writes usage to stderr");
        let text = stderr(&out);
        assert!(text.contains("usage:"), "{word}: {text}");
        assert!(text.contains("prop-tool inventory"), "{word}: {text}");
    }
}

#[test]
fn no_command_exits_two_with_usage() {
    let out = run(&[]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn unknown_command_exits_two_and_echoes_it() {
    let out = run(&["frobnicate"]);
    assert_eq!(code(&out), 2);
    let text = stderr(&out);
    assert!(text.contains("unknown command"), "{text}");
    assert!(text.contains("frobnicate"), "{text}");
    assert!(text.contains("usage:"), "{text}");
}

// ---------------------------------------------------------------------------
// inventory
// ---------------------------------------------------------------------------

#[test]
fn inventory_over_the_fixture_directory_tallies_every_blob() {
    let dir = fixture("");
    let out = run(&["inventory", dir.to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    // Eight fixtures: three 8-bit, one 20-bit, two s20-bit, one 32-bit, and the
    // deliberately malformed one.
    assert!(text.contains("8-bit=3"), "{text}");
    assert!(text.contains("16-bit=0"), "{text}");
    assert!(text.contains("20-bit=1"), "{text}");
    assert!(text.contains("s20-bit=2"), "{text}");
    assert!(text.contains("32-bit=1"), "{text}");
    assert!(text.contains("failed=1"), "{text}");
    assert!(text.contains("sources=8"), "{text}");
    assert!(text.contains("malformed_rle_overflow.bin"), "{text}");
    assert!(text.contains("FAIL"), "{text}");
}

#[test]
fn inventory_of_a_single_file_reports_it_without_a_total() {
    let out = run(&["inventory", fixture("8bit_avatar.bin").to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("8-bit=1"), "{text}");
    assert!(text.contains("failed=0"), "{text}");
    assert!(
        !text.contains("TOTAL"),
        "single path needs no total:\n{text}"
    );
}

#[test]
fn inventory_of_several_paths_prints_a_total_row() {
    let a = fixture("8bit_avatar.bin");
    let b = fixture("32bit_bit.bin");
    let out = run(&["inventory", a.to_str().unwrap(), b.to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("TOTAL"), "{}", stdout(&out));
}

#[test]
fn inventory_does_not_fail_on_an_unreadable_path() {
    // A path that does not exist yields no bytes, so it is skipped and the run
    // still succeeds — the tool reports, it does not abort.
    let out = run(&["inventory", "/nonexistent/definitely-not-a-prop.bin"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("failed=0"), "{text}");
    assert!(text.contains("sources=1"), "{text}");
}

#[test]
fn inventory_without_paths_exits_two() {
    let out = run(&["inventory"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

// ---------------------------------------------------------------------------
// rgba / png
// ---------------------------------------------------------------------------

#[test]
fn rgba_dumps_the_decoded_pixels_with_a_header() {
    let dir = TempDir::new("rgba");
    let out_file = dir.path("avatar.rgba");
    let out = run(&[
        "rgba",
        fixture("8bit_avatar.bin").to_str().unwrap(),
        out_file.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("44x44 8-bit rgba=7744 bytes"),
        "{}",
        stdout(&out)
    );

    let bytes = std::fs::read(&out_file).expect("rgba output");
    let (w, h, rgba) = parse_rgba(&bytes).expect("PRGBA1 header");
    assert_eq!((w, h), (44, 44));
    assert_eq!(rgba.len(), 7744);
    // First pixel pinned by the in-crate fixture tests.
    assert_eq!(&rgba[0..4], &[204, 127, 0, 255]);
}

#[test]
fn rgba_rejects_a_missing_input_with_exit_one() {
    let dir = TempDir::new("rgba-missing");
    let out = run(&[
        "rgba",
        "/nonexistent/prop.bin",
        dir.path("x.rgba").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/prop.bin"));
}

#[test]
fn rgba_rejects_an_undecodable_input_with_exit_one() {
    let dir = TempDir::new("rgba-bad");
    let junk = dir.path("junk.bin");
    std::fs::write(&junk, b"not a prop at all").unwrap();
    let out = run(&[
        "rgba",
        junk.to_str().unwrap(),
        dir.path("x.rgba").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("junk.bin"));
}

#[test]
fn rgba_without_two_paths_exits_two() {
    let out = run(&["rgba", fixture("8bit_avatar.bin").to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn png_writes_a_real_44x44_png() {
    let dir = TempDir::new("png");
    let out_file = dir.path("avatar.png");
    let out = run(&[
        "png",
        fixture("8bit_avatar.bin").to_str().unwrap(),
        out_file.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("->"), "{}", stdout(&out));

    let bytes = std::fs::read(&out_file).expect("png output");
    assert_eq!(png_size(&bytes), Some((44, 44)), "not a 44x44 PNG");
}

#[test]
fn png_reports_an_undecodable_prop_with_exit_one() {
    let dir = TempDir::new("png-bad");
    let junk = dir.path("junk.bin");
    std::fs::write(&junk, b"xxxx").unwrap();
    let out = run(&[
        "png",
        junk.to_str().unwrap(),
        dir.path("x.png").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("junk.bin"));
}

// ---------------------------------------------------------------------------
// rgba-batch / digests
// ---------------------------------------------------------------------------

/// A manifest with one decodable prop, one missing path and one blank line.
fn mixed_manifest(dir: &TempDir) -> PathBuf {
    let manifest = dir.path("manifest.txt");
    let contents = format!(
        "{}\n\n/nonexistent/missing.bin\n",
        fixture("8bit_avatar.bin").display()
    );
    std::fs::write(&manifest, contents).unwrap();
    manifest
}

#[test]
fn rgba_batch_writes_one_file_per_decodable_line_and_fails_on_the_rest() {
    let dir = TempDir::new("batch");
    let manifest = mixed_manifest(&dir);
    let outdir = dir.path("out");
    let out = run(&[
        "rgba-batch",
        manifest.to_str().unwrap(),
        outdir.to_str().unwrap(),
    ]);
    // One line failed, so the exit code is a failure even though work happened.
    assert_eq!(code(&out), 1);
    let text = stdout(&out);
    assert!(text.contains("wrote 1, failed 1"), "{text}");

    // Output names are manifest line numbers: line 0 succeeded, line 1 (blank)
    // is skipped, line 2 failed and produced no file.
    let written = std::fs::read(outdir.join("0.rgba")).expect("0.rgba");
    let (w, h, _) = parse_rgba(&written).expect("PRGBA1 header");
    assert_eq!((w, h), (44, 44));
    assert!(!outdir.join("1.rgba").exists());
    assert!(!outdir.join("2.rgba").exists());
}

#[test]
fn rgba_batch_of_only_good_lines_exits_zero() {
    let dir = TempDir::new("batch-ok");
    let manifest = dir.path("manifest.txt");
    std::fs::write(
        &manifest,
        format!("{}\n", fixture("s20_avatar.bin").display()),
    )
    .unwrap();
    let outdir = dir.path("out");
    let out = run(&[
        "rgba-batch",
        manifest.to_str().unwrap(),
        outdir.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("wrote 1, failed 0"));
    assert!(outdir.join("0.rgba").is_file());
}

#[test]
fn digests_match_the_library_decoder_byte_for_byte() {
    let dir = TempDir::new("digests");
    let manifest = mixed_manifest(&dir);
    let outfile = dir.path("digests.txt");
    let out = run(&[
        "digests",
        manifest.to_str().unwrap(),
        outfile.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("digests: 1 decoded, 1 failed"));

    let prop = decode_prop(&fixture("8bit_avatar.bin"));
    let image = &prop.image;
    let expected = format!(
        "0\t8-bit\t{}\t{}\t{:016x}",
        image.width(),
        image.height(),
        fnv1a(image.width(), image.height(), image.as_rgba())
    );
    let text = std::fs::read_to_string(&outfile).expect("digest output");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "unexpected digest lines: {text}");
    assert_eq!(lines[0], expected);
    assert!(lines[1].starts_with("1\tFAIL\t"), "{}", lines[1]);
    assert_eq!(prop.format(), PropFormat::EightBit);
}

#[test]
fn digests_of_a_missing_manifest_exits_one() {
    let dir = TempDir::new("digests-missing");
    let out = run(&[
        "digests",
        "/nonexistent/manifest.txt",
        dir.path("out.txt").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/manifest.txt"));
}

// ---------------------------------------------------------------------------
// extract (and the `.prp` roster path shared with inventory)
// ---------------------------------------------------------------------------

/// Assemble a minimal `.prp` roster holding one blob.
///
/// Layout (see the tool's `Roster`): a 16-byte file header whose first u32 is
/// the data offset and whose u32 at offset 8 is the map offset; at the map
/// offset a 32-byte type table followed by 32-byte records. A record's blob
/// lives at `data_offset + record.data_rel` and is `record.data_size` bytes.
fn build_prp(blob: &[u8], id: u32) -> Vec<u8> {
    let data_offset = 16usize;
    let raw_map = data_offset + blob.len();
    let map_offset = (raw_map + 3) & !3;
    let recs_offset = 32usize;
    let mut buf = vec![0u8; map_offset + recs_offset + 32];
    buf[0..4].copy_from_slice(&(data_offset as u32).to_le_bytes());
    buf[8..12].copy_from_slice(&(map_offset as u32).to_le_bytes());
    buf[map_offset + 4..map_offset + 8].copy_from_slice(&1u32.to_le_bytes());
    buf[map_offset + 16..map_offset + 20].copy_from_slice(&(recs_offset as u32).to_le_bytes());
    buf[data_offset..data_offset + blob.len()].copy_from_slice(blob);
    let rec = map_offset + recs_offset;
    buf[rec..rec + 4].copy_from_slice(&id.to_le_bytes());
    buf[rec + 8..rec + 12].copy_from_slice(&0u32.to_le_bytes());
    buf[rec + 12..rec + 16].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    buf
}

fn roster_file(dir: &TempDir) -> (PathBuf, Vec<u8>) {
    let blob = std::fs::read(fixture("8bit_head_rare.bin")).unwrap();
    let prp = dir.path("test.prp");
    std::fs::write(&prp, build_prp(&blob, 0xabcd)).unwrap();
    (prp, blob)
}

#[test]
fn extract_splits_a_roster_into_per_prop_blobs() {
    let dir = TempDir::new("extract");
    let (prp, blob) = roster_file(&dir);
    let outdir = dir.path("props");
    let out = run(&["extract", prp.to_str().unwrap(), outdir.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("extracted 1 props"),
        "{}",
        stdout(&out)
    );

    // The name embeds the format name, the record id and the record index.
    let written = outdir.join("8-bit_0000abcd_0.bin");
    assert!(written.is_file(), "expected {}", written.display());
    assert_eq!(std::fs::read(&written).unwrap(), blob, "blob round-trip");
}

#[test]
fn extract_format_filter_selects_and_skips_by_name() {
    let dir = TempDir::new("extract-format");
    let (prp, _) = roster_file(&dir);

    let keep = dir.path("keep");
    let out = run(&[
        "extract",
        prp.to_str().unwrap(),
        keep.to_str().unwrap(),
        "--format",
        "8-bit",
    ]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("extracted 1 props"));

    let skip = dir.path("skip");
    let out = run(&[
        "extract",
        prp.to_str().unwrap(),
        skip.to_str().unwrap(),
        "--format",
        "16-bit",
    ]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains("extracted 0 props"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn extract_honours_limit_and_stride() {
    let dir = TempDir::new("extract-limit");
    let (prp, _) = roster_file(&dir);

    let limited = dir.path("limited");
    let out = run(&[
        "extract",
        prp.to_str().unwrap(),
        limited.to_str().unwrap(),
        "--limit",
        "0",
    ]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("extracted 0 props"));

    // A stride of 1 over one record still writes it.
    let strided = dir.path("strided");
    let out = run(&[
        "extract",
        prp.to_str().unwrap(),
        strided.to_str().unwrap(),
        "--stride",
        "1",
    ]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("extracted 1 props"));
}

#[test]
fn extract_rejects_a_file_that_is_not_a_roster() {
    let dir = TempDir::new("extract-bad");
    let not_a_roster = dir.path("not-a-roster.bin");
    std::fs::write(&not_a_roster, b"definitely not a roster").unwrap();
    let out = run(&[
        "extract",
        not_a_roster.to_str().unwrap(),
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not a .prp"));
}

#[test]
fn extract_without_two_paths_exits_two() {
    let out = run(&["extract", fixture("8bit_avatar.bin").to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn inventory_understands_a_roster_container() {
    let dir = TempDir::new("inventory-prp");
    let (prp, _) = roster_file(&dir);
    let out = run(&["inventory", prp.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    // One record decoded as an 8-bit prop: `sources=1` proves the roster was
    // split into records rather than read as a single blob.
    assert!(text.contains("sources=1"), "{text}");
    assert!(text.contains("8-bit=1"), "{text}");
    assert!(text.contains("failed=0"), "{text}");
}

#[test]
fn png_without_two_paths_exits_two() {
    let out = run(&["png", fixture("8bit_avatar.bin").to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn png_rejects_a_missing_input_with_exit_one() {
    let dir = TempDir::new("png-missing");
    let out = run(&[
        "png",
        "/nonexistent/x.bin",
        dir.path("x.png").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/x.bin"));
}

#[test]
fn rgba_reports_an_unwritable_output_with_exit_one() {
    let dir = TempDir::new("rgba-unwritable");
    let as_dir = dir.path("out.rgba");
    std::fs::create_dir_all(&as_dir).unwrap();
    let out = run(&[
        "rgba",
        fixture("8bit_avatar.bin").to_str().unwrap(),
        as_dir.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains(as_dir.to_str().unwrap()));
}

#[test]
fn rgba_batch_without_two_paths_exits_two() {
    let out = run(&["rgba-batch", "/tmp/whatever"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn rgba_batch_reports_an_unreadable_manifest_with_exit_one() {
    let dir = TempDir::new("batch-nomanifest");
    let out = run(&[
        "rgba-batch",
        "/nonexistent/manifest.txt",
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/manifest.txt"));
}

#[test]
fn rgba_batch_reports_an_uncreatable_outdir_with_exit_one() {
    let dir = TempDir::new("batch-badout");
    let manifest = dir.path("manifest.txt");
    std::fs::write(
        &manifest,
        format!("{}\n", fixture("8bit_avatar.bin").display()),
    )
    .unwrap();
    let as_file = dir.path("not-a-dir");
    std::fs::write(&as_file, b"x").unwrap();
    let out = run(&[
        "rgba-batch",
        manifest.to_str().unwrap(),
        as_file.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not-a-dir"));
}

#[test]
fn digests_without_two_paths_exits_two() {
    let out = run(&["digests", "/tmp/manifest.txt"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn digests_reports_an_unwritable_outfile_with_exit_one() {
    let dir = TempDir::new("digests-badout");
    let manifest = dir.path("manifest.txt");
    std::fs::write(
        &manifest,
        format!("{}\n", fixture("8bit_avatar.bin").display()),
    )
    .unwrap();
    let as_dir = dir.path("out.txt");
    std::fs::create_dir_all(&as_dir).unwrap();
    let out = run(&[
        "digests",
        manifest.to_str().unwrap(),
        as_dir.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("out.txt"));
}

#[test]
fn extract_reports_an_unreadable_input_with_exit_one() {
    let dir = TempDir::new("extract-missing");
    let out = run(&[
        "extract",
        "/nonexistent/roster.prp",
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("unreadable"));
}

#[test]
fn extract_reports_an_uncreatable_outdir_with_exit_one() {
    let dir = TempDir::new("extract-badout");
    let (prp, _) = roster_file(&dir);
    let as_file = dir.path("not-a-dir");
    std::fs::write(&as_file, b"x").unwrap();
    let out = run(&["extract", prp.to_str().unwrap(), as_file.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not-a-dir"));
}

#[test]
fn inventory_recurses_into_subdirectories() {
    let dir = TempDir::new("inventory-nested");
    let nested = dir.path("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::copy(fixture("8bit_avatar.bin"), nested.join("avatar.bin")).unwrap();
    let out = run(&["inventory", dir.path("a").to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("sources=1"), "{text}");
    assert!(text.contains("8-bit=1"), "{text}");
}

#[test]
fn inventory_tolerates_a_missing_or_garbage_roster() {
    let dir = TempDir::new("inventory-badprp");
    let garbage = dir.path("garbage.prp");
    std::fs::write(&garbage, b"not a roster at all").unwrap();

    let missing = run(&["inventory", "/nonexistent/nope.prp"]);
    assert_eq!(code(&missing), 0);
    assert!(
        stdout(&missing).contains("failed=0"),
        "{}",
        stdout(&missing)
    );

    let bad = run(&["inventory", garbage.to_str().unwrap()]);
    assert_eq!(code(&bad), 0);
    assert!(stdout(&bad).contains("failed=0"), "{}", stdout(&bad));
}

#[test]
fn inventory_reports_a_truncated_zlib_payload_as_a_failure() {
    let dir = TempDir::new("inventory-zlib");
    let full = std::fs::read(fixture("32bit_bit.bin")).unwrap();
    let truncated = dir.path("truncated.bin");
    std::fs::write(&truncated, &full[..40]).unwrap();
    let out = run(&["inventory", truncated.to_str().unwrap()]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("failed=1"), "{text}");
    assert!(text.contains("32-bit=0"), "{text}");
}

// ---------------------------------------------------------------------------
// bag
// ---------------------------------------------------------------------------

/// Prefix a fixture prop with the bag's fixed 32-byte metadata block.
fn bag_blob(prop: &[u8]) -> Vec<u8> {
    let mut blob = vec![0xab; 32];
    blob.extend_from_slice(prop);
    blob
}

/// A `.pids`/`.props` pair holding `blobs` (which must already carry the prefix),
/// tiled contiguously. Identities are `(1, 2), (3, 4), ...`.
fn bag_pair(blobs: &[Vec<u8>]) -> (Vec<u8>, Vec<u8>) {
    let mut index = Vec::new();
    let mut props = Vec::new();
    for (n, blob) in blobs.iter().enumerate() {
        let a = (1 + n * 2) as u32;
        index.extend_from_slice(&a.to_be_bytes());
        index.extend_from_slice(&(a + 1).to_be_bytes());
        index.extend_from_slice(&(props.len() as u32).to_be_bytes());
        index.extend_from_slice(&(blob.len() as u32).to_be_bytes());
        props.extend_from_slice(blob);
    }
    (index, props)
}

fn write_bag(dir: &TempDir, index: &[u8], props: &[u8]) -> PathBuf {
    let bundle = dir.path("Test.bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("Test.pids"), index).unwrap();
    std::fs::write(bundle.join("Test.props"), props).unwrap();
    bundle
}

#[test]
fn help_mentions_the_bag_subcommand() {
    let out = run(&["help"]);
    assert_eq!(code(&out), 0);
    let text = stderr(&out);
    assert!(text.contains("prop-tool bag list"), "{text}");
    assert!(text.contains("prop-tool bag extract"), "{text}");
}

#[test]
fn bag_list_prints_the_index_metrics_and_one_line_per_entry() {
    let dir = TempDir::new("bag-list");
    let avatar = std::fs::read(fixture("8bit_avatar.bin")).unwrap();
    let (index, props) = bag_pair(&[bag_blob(&avatar), bag_blob(&avatar)]);
    let bundle = write_bag(&dir, &index, &props);

    let out = run(&["bag", "list", bundle.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("2 records, 2 entries, out-of-bounds=0, tiling=1/1, trailing=0"),
        "{text}"
    );
    assert!(text.contains("a\tb\toffset\tsize\tformat\tdims"), "{text}");
    assert!(text.contains("00000001\t00000002"), "{text}");
    assert!(text.contains("8-bit\t44x44"), "{text}");
}

#[test]
fn bag_list_accepts_a_pids_path_and_honours_limit() {
    let dir = TempDir::new("bag-list-pids");
    let avatar = std::fs::read(fixture("8bit_avatar.bin")).unwrap();
    let (index, props) = bag_pair(&[bag_blob(&avatar), bag_blob(&avatar)]);
    let bundle = write_bag(&dir, &index, &props);

    let out = run(&[
        "bag",
        "list",
        bundle.join("Test.pids").to_str().unwrap(),
        "--limit",
        "1",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("00000001\t00000002"), "{text}");
    assert!(
        !text.contains("00000003\t00000004"),
        "limit ignored:\n{text}"
    );
}

#[test]
fn bag_extract_writes_one_png_per_decodable_entry() {
    let dir = TempDir::new("bag-extract");
    let avatar = std::fs::read(fixture("8bit_avatar.bin")).unwrap();
    let (index, props) = bag_pair(&[bag_blob(&avatar), bag_blob(&avatar)]);
    let bundle = write_bag(&dir, &index, &props);
    let outdir = dir.path("pngs");

    let out = run(&[
        "bag",
        "extract",
        bundle.to_str().unwrap(),
        outdir.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("bag extract: wrote 2, failed 0"));

    let png = std::fs::read(outdir.join("8-bit_00000001_00000002.png")).expect("first png");
    assert_eq!(png_size(&png), Some((44, 44)));
    assert!(outdir.join("8-bit_00000003_00000004.png").is_file());
}

#[test]
fn bag_extract_honours_a_format_filter() {
    let dir = TempDir::new("bag-extract-format");
    let avatar = std::fs::read(fixture("8bit_avatar.bin")).unwrap();
    let (index, props) = bag_pair(&[bag_blob(&avatar)]);
    let bundle = write_bag(&dir, &index, &props);
    let outdir = dir.path("pngs");

    let out = run(&[
        "bag",
        "extract",
        bundle.to_str().unwrap(),
        outdir.to_str().unwrap(),
        "--format",
        "16-bit",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("wrote 0, failed 0"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn bag_extract_reports_a_missing_bundle_with_exit_one() {
    let dir = TempDir::new("bag-missing");
    let out = run(&[
        "bag",
        "extract",
        "/nonexistent/PropBag.bundle",
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/PropBag.bundle"));
}

#[test]
fn bag_without_a_subcommand_exits_two_with_usage() {
    let out = run(&["bag"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn bag_unknown_subcommand_exits_two_and_echoes_it() {
    let out = run(&["bag", "frobnicate"]);
    assert_eq!(code(&out), 2);
    let text = stderr(&out);
    assert!(text.contains("unknown bag subcommand"), "{text}");
    assert!(text.contains("frobnicate"), "{text}");
}

#[test]
fn bag_list_without_a_path_exits_two() {
    let out = run(&["bag", "list"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn bag_extract_without_an_outdir_exits_two() {
    let out = run(&["bag", "extract", "/tmp/whatever"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

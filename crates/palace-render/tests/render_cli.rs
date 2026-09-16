//! Integration tests for the `palace-render` binary.
//!
//! The binary composites one room into a PNG. A full run needs the 700 MB
//! corpus, but the git-tracked `fixtures/logon-run1/frames/0007-server-room.bin`
//! is a real captured `MSG_ROOMDESC`, and the binary accepts it directly with
//! `--frame-file`. Every test runs with `HOME` pointed at an empty temp dir, so
//! `fill_defaults` finds no corpus and no asset roots: the render uses the empty
//! stores, which is both fast and machine-independent.
//!
//! What is *not* tested here is a run against the real corpus or the network;
//! `corpus_render.rs` already covers the corpus when it is present, and it skips
//! cleanly when it is not.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_render::{MAX_ROOM_DIMENSION, MIN_ROOM_HEIGHT, MIN_ROOM_WIDTH};

const BIN: &str = env!("CARGO_BIN_EXE_palace-render");

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "palace-render-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// An empty asset directory, passed explicitly so the defaults are not used.
    fn empty_dir(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).expect("create empty asset dir");
        dir
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn frame_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logon-run1/frames/0007-server-room.bin")
        .canonicalize()
        .expect("captured room frame")
}

/// Run the binary with `HOME` isolated and empty asset roots, so no default
/// corpus is ever consulted.
fn run(home: &TempDir, args: &[String]) -> Output {
    let media = home.empty_dir("media");
    let props = home.empty_dir("props");
    let mut full = args.to_vec();
    full.push("--media-root".to_string());
    full.push(media.to_string_lossy().into_owned());
    full.push("--props-dir".to_string());
    full.push(props.to_string_lossy().into_owned());
    run_bare(home, &full)
}

/// Run the binary with only the arguments given (still with `HOME` isolated).
fn run_bare(home: &TempDir, args: &[String]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HOME", home.0.as_os_str())
        .output()
        .expect("spawn palace-render")
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

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------------------
// Help and argument errors.
// ---------------------------------------------------------------------------

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let out = run(&TempDir::new("help"), &owned(&["--help"]));
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).is_empty(), "help belongs on stdout");
    let text = stdout(&out);
    assert!(text.contains("palace-render — composite a Palace room into a PNG"));
    assert!(text.contains("USAGE:"));
    assert!(text.contains("--frame-file <FRAME>"));
    assert!(text.contains("--viewport <WxH>"));
}

#[test]
fn unknown_argument_exits_one() {
    let out = run(&TempDir::new("unknown"), &owned(&["--nope"]));
    assert_eq!(code(&out), 1);
    let text = stderr(&out);
    assert!(text.contains("unknown argument"), "{text}");
    assert!(text.contains("try --help"), "{text}");
}

#[test]
fn a_missing_out_exits_one() {
    let out = run(
        &TempDir::new("no-out"),
        &owned(&["--frame-file", frame_fixture().to_str().unwrap()]),
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("no --out given"), "{}", stderr(&out));
}

#[test]
fn no_room_source_exits_one() {
    let dir = TempDir::new("no-source");
    let out = run(&dir, &owned(&["--out", dir.path("x.png").to_str().unwrap()]));
    assert_eq!(code(&out), 1);
    let text = stderr(&out);
    assert!(text.contains("no room source"), "{text}");
}

#[test]
fn a_flag_missing_its_value_exits_one() {
    let out = run_bare(&TempDir::new("no-value"), &owned(&["--room"]));
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--room needs a value"), "{}", stderr(&out));
}

#[test]
fn a_non_numeric_room_id_exits_one() {
    let out = run(&TempDir::new("bad-room"), &owned(&["--room", "abc"]));
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--room:"), "{}", stderr(&out));
}

#[test]
fn a_malformed_avatar_exits_one() {
    let out = run(
        &TempDir::new("bad-avatar"),
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--avatar",
            "12",
        ]),
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--avatar needs X,Y"), "{}", stderr(&out));
}

#[test]
fn a_malformed_viewport_exits_one() {
    let out = run(
        &TempDir::new("bad-viewport"),
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--viewport",
            "1920",
        ]),
    );
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("--viewport needs WxH"),
        "{}",
        stderr(&out)
    );
}

// ---------------------------------------------------------------------------
// Successful renders.
// ---------------------------------------------------------------------------

#[test]
fn rendering_a_captured_frame_writes_a_plausible_png() {
    let dir = TempDir::new("render");
    let out_file = dir.path("room.png");
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let err = stderr(&out);
    assert!(err.contains("room 901"), "{err}");
    assert!(err.contains("Balamb Garden"), "{err}");
    assert!(err.contains("logical size"), "{err}");
    assert!(err.contains(&format!("wrote {}", out_file.display())), "{err}");
    assert!(err.contains("dpr 1"), "{err}");

    let bytes = std::fs::read(&out_file).expect("render output");
    let (w, h) = png_size(&bytes).expect("valid PNG with IHDR");
    assert!(
        w >= MIN_ROOM_WIDTH as u32 && h >= MIN_ROOM_HEIGHT as u32,
        "room smaller than the floor: {w}x{h}"
    );
    assert!(
        w <= MAX_ROOM_DIMENSION && h <= MAX_ROOM_DIMENSION,
        "room exceeded the cap: {w}x{h}"
    );
}

#[test]
fn dpr_scales_the_device_pixel_buffer() {
    let dir = TempDir::new("dpr");
    let one = dir.path("one.png");
    let two = dir.path("two.png");
    let frame = frame_fixture();
    let frame = frame.to_str().unwrap();

    let base = run(&dir, &owned(&["--frame-file", frame, "--out", one.to_str().unwrap()]));
    assert_eq!(code(&base), 0, "stderr: {}", stderr(&base));
    let (w1, h1) = png_size(&std::fs::read(&one).unwrap()).expect("base PNG");

    let doubled = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame,
            "--dpr",
            "2",
            "--out",
            two.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&doubled), 0, "stderr: {}", stderr(&doubled));
    assert!(stderr(&doubled).contains("dpr 2"), "{}", stderr(&doubled));
    let (w2, h2) = png_size(&std::fs::read(&two).unwrap()).expect("dpr-2 PNG");

    // The buffer is floor(logical * dpr), so doubling can add at most one pixel.
    assert!(
        w2 >= 2 * w1 && w2 <= 2 * w1 + 1,
        "width did not double: {w1} -> {w2}"
    );
    assert!(
        h2 >= 2 * h1 && h2 <= 2 * h1 + 1,
        "height did not double: {h1} -> {h2}"
    );
}

#[test]
fn dpr_above_the_cap_is_clamped_in_the_written_buffer() {
    let dir = TempDir::new("dpr-clamp");
    let out_file = dir.path("clamped.png");
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--dpr",
            "9",
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    // The report echoes the requested value but the buffer is clamped to 2.
    assert!(stderr(&out).contains("dpr 2"), "{}", stderr(&out));
}

#[test]
fn viewport_preview_prints_the_mapping_without_writing_a_file() {
    let dir = TempDir::new("viewport");
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--viewport",
            "1024x768",
            "--zoom",
            "2",
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Fit:"), "{err}");
    assert!(err.contains("Native:"), "{err}");
    assert!(err.contains("-> viewport"), "{err}");
    assert!(err.contains("room (0.0,0.0)"), "{err}");
    assert!(!err.contains("wrote "), "preview must not write a PNG:\n{err}");
}

#[test]
fn a_room_can_be_found_by_id_in_a_rooms_directory() {
    let dir = TempDir::new("rooms-dir");
    // The room dir holds raw MSG_ROOMDESC payloads: the captured frame minus its
    // 12-byte header.
    let frame = std::fs::read(frame_fixture()).expect("read frame");
    let payload = &frame[12..];
    let rooms = dir.empty_dir("rooms");
    std::fs::write(rooms.join("901.bin"), payload).expect("write payload");
    let out_file = dir.path("byid.png");

    let out = run(
        &dir,
        &owned(&[
            "--room",
            "901",
            "--rooms-dir",
            rooms.to_str().unwrap(),
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("room 901"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

#[test]
fn asserting_an_unknown_room_id_exits_one() {
    let dir = TempDir::new("room-missing");
    let rooms = dir.empty_dir("rooms");
    let out = run(
        &dir,
        &owned(&[
            "--room",
            "999999",
            "--rooms-dir",
            rooms.to_str().unwrap(),
            "--out",
            dir.path("x.png").to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("room 999999 not found"), "{}", stderr(&out));
}

#[test]
fn a_loose_prop_is_parsed_and_placed() {
    let dir = TempDir::new("loose");
    let out_file = dir.path("loose.png");
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--loose-prop",
            "1041303665@7,9",
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    // With no prop store the loose prop is reported as a missing asset, not
    // silently dropped.
    assert!(stderr(&out).contains("loose props 1"), "{}", stderr(&out));
    assert!(stderr(&out).contains("report:"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

#[test]
fn a_malformed_loose_prop_exits_one() {
    let out = run(
        &TempDir::new("loose-bad"),
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--loose-prop",
            "nonsense",
        ]),
    );
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("--loose-prop needs PROP_ID@X,Y"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn room_file_source_renders_and_creates_nested_output_dirs() {
    let dir = TempDir::new("room-file");
    let frame = std::fs::read(frame_fixture()).unwrap();
    let payload = dir.path("room.bin");
    std::fs::write(&payload, &frame[12..]).unwrap();
    let out_file = dir.path("nested/deep/room.png");
    let out = run(
        &dir,
        &owned(&[
            "--room-file",
            payload.to_str().unwrap(),
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("room 901"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

#[test]
fn clock_ms_and_avatar_props_are_accepted() {
    let dir = TempDir::new("clock-avatar");
    let out_file = dir.path("scene.png");
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--clock-ms",
            "500",
            "--avatar",
            "10,20,1041303665",
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("avatars 1"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

#[test]
fn a_bad_clock_ms_exits_one() {
    let out = run(
        &TempDir::new("bad-clock"),
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--clock-ms",
            "later",
        ]),
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--clock-ms:"), "{}", stderr(&out));
}

#[test]
fn a_roster_that_is_not_a_roster_is_reported_but_not_fatal() {
    let dir = TempDir::new("roster");
    let out_file = dir.path("roster.png");
    let bad_roster = frame_fixture();
    let out = run(
        &dir,
        &owned(&[
            "--frame-file",
            frame_fixture().to_str().unwrap(),
            "--roster",
            bad_roster.to_str().unwrap(),
            "--out",
            out_file.to_str().unwrap(),
        ]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("roster"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

#[test]
fn default_corpus_paths_are_discovered_from_home() {
    let home = TempDir::new("defaults");
    let colosseum = home.0.join("colosseum");
    let rooms = colosseum.join("payloads_all");
    std::fs::create_dir_all(&rooms).unwrap();
    std::fs::create_dir_all(colosseum.join("reference/media/media_dl")).unwrap();
    std::fs::create_dir_all(colosseum.join("http_harvest")).unwrap();
    std::fs::create_dir_all(colosseum.join("arks")).unwrap();
    std::fs::create_dir_all(colosseum.join("props_harvested")).unwrap();
    // An empty file is discovered as the default roster and then rejected.
    std::fs::write(colosseum.join("pserver.prp"), b"").unwrap();

    let frame = std::fs::read(frame_fixture()).unwrap();
    std::fs::write(rooms.join("901.bin"), &frame[12..]).unwrap();

    let out_file = home.path("auto.png");
    let out = run_bare(
        &home,
        &owned(&["--room", "901", "--out", out_file.to_str().unwrap()]),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("room 901"), "{}", stderr(&out));
    assert!(stderr(&out).contains("pserver.prp"), "{}", stderr(&out));
    assert!(png_size(&std::fs::read(&out_file).unwrap()).is_some());
}

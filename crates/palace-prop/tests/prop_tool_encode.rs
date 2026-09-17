//! Integration tests for `prop-tool encode` and `prop-tool encode-batch`.
//!
//! These are the only two commands that create a prop. The tests drive the real
//! binary over PNGs written into a temp dir, then decode the result with the
//! library to prove the pixels survived the round trip. Nothing here touches the
//! corpus or the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use palace_prop::{decode, Prop, PropFormat};

const BIN: &str = env!("CARGO_BIN_EXE_prop-tool");

/// Channel values that survive S20's 5-bit quantisation unchanged, so a decoded
/// pixel can be compared with the input byte for byte. Each is `quantize(c)`'s
/// fixed point.
const STABLE: [u8; 8] = [0, 41, 82, 123, 164, 205, 246, 255];

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
        let path =
            std::env::temp_dir().join(format!("prop-tool-enc-{tag}-{}-{n}", std::process::id()));
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

fn write_png(path: &Path, width: u32, height: u32, color: png::ColorType, data: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(data).expect("png data");
}

/// A `width`x`height` RGBA pattern drawn from [`STABLE`].
fn stable_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for i in 0..(width * height) as usize {
        rgba.extend_from_slice(&[
            STABLE[i % 8],
            STABLE[(i * 3) % 8],
            STABLE[(i * 5) % 8],
            STABLE[(i * 7) % 8],
        ]);
    }
    rgba
}

fn decode_prop(path: &Path) -> Prop {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    decode(&bytes).unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

#[test]
fn encode_round_trips_a_png_pixel_for_pixel() {
    let dir = TempDir::new("roundtrip");
    let input = dir.path("face.png");
    let output = dir.path("face.prop");
    let pixels = stable_rgba(4, 4);
    write_png(&input, 4, 4, png::ColorType::Rgba, &pixels);

    let out = run(&[
        "encode",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        "--head",
        "--h-offset",
        "-3",
        "--v-offset",
        "7",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("4x4"), "{}", stdout(&out));
    assert!(stdout(&out).contains("flags=0x0002"), "{}", stdout(&out));

    let prop = decode_prop(&output);
    assert_eq!(prop.format(), PropFormat::S20Bit);
    assert_eq!((prop.image.width(), prop.image.height()), (4, 4));
    assert!(prop.header.is_head(), "head flag lost");
    assert_eq!(prop.header.h_offset, -3);
    assert_eq!(prop.header.v_offset, 7);
    assert_eq!(
        prop.image.as_rgba(),
        pixels.as_slice(),
        "decoded pixels differ from the PNG input"
    );
}

#[test]
fn encode_without_flags_leaves_the_head_bit_clear() {
    let dir = TempDir::new("plain");
    let input = dir.path("bit.png");
    let output = dir.path("bit.prop");
    write_png(&input, 2, 2, png::ColorType::Rgba, &stable_rgba(2, 2));
    let out = run(&["encode", input.to_str().unwrap(), output.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let prop = decode_prop(&output);
    assert!(!prop.header.is_head());
    assert!(!prop.header.is_ghost());
}

#[test]
fn encode_rejects_an_odd_width_and_names_the_file() {
    let dir = TempDir::new("odd");
    let input = dir.path("odd.png");
    let output = dir.path("odd.prop");
    write_png(&input, 3, 2, png::ColorType::Rgba, &stable_rgba(3, 2));

    let out = run(&["encode", input.to_str().unwrap(), output.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    let text = stderr(&out);
    assert!(text.contains("odd.png"), "{text}");
    assert!(text.contains("3x2"), "{text}");
    assert!(text.contains("even"), "{text}");
    assert!(!output.exists(), "no prop should be written");
}

#[test]
fn encode_widens_rgb_grayscale_and_grayscale_alpha() {
    let dir = TempDir::new("widen");

    let rgb_in = dir.path("rgb.png");
    let rgb_out = dir.path("rgb.prop");
    let rgb: Vec<u8> = (0..16).flat_map(|i| [STABLE[i % 8]; 3]).collect();
    write_png(&rgb_in, 4, 4, png::ColorType::Rgb, &rgb);
    let out = run(&[
        "encode",
        rgb_in.to_str().unwrap(),
        rgb_out.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let prop = decode_prop(&rgb_out);
    for (x, expected) in (0..16).map(|i| STABLE[i % 8]).enumerate() {
        let pixel = prop.image.pixel(x as u32 % 4, x as u32 / 4).unwrap();
        assert_eq!(pixel, [expected, expected, expected, 255], "pixel {x}");
    }

    let gray_in = dir.path("gray.png");
    let gray_out = dir.path("gray.prop");
    let gray: Vec<u8> = (0..16).map(|i| STABLE[i % 8]).collect();
    write_png(&gray_in, 4, 4, png::ColorType::Grayscale, &gray);
    let out = run(&[
        "encode",
        gray_in.to_str().unwrap(),
        gray_out.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let prop = decode_prop(&gray_out);
    assert_eq!(prop.image.pixel(0, 0).unwrap(), [0, 0, 0, 255]);
    assert_eq!(prop.image.pixel(1, 0).unwrap(), [41, 41, 41, 255]);

    let ga_in = dir.path("ga.png");
    let ga_out = dir.path("ga.prop");
    let ga: Vec<u8> = (0..16)
        .flat_map(|i| [STABLE[i % 8], STABLE[(i * 3) % 8]])
        .collect();
    write_png(&ga_in, 4, 4, png::ColorType::GrayscaleAlpha, &ga);
    let out = run(&["encode", ga_in.to_str().unwrap(), ga_out.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let prop = decode_prop(&ga_out);
    assert_eq!(prop.image.pixel(0, 0).unwrap(), [0, 0, 0, 0]);
    assert_eq!(prop.image.pixel(1, 0).unwrap(), [41, 41, 41, 123]);
}

#[test]
fn encode_reports_a_missing_input_with_exit_one() {
    let dir = TempDir::new("missing");
    let out = run(&[
        "encode",
        "/nonexistent/face.png",
        dir.path("x.prop").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/face.png"));
}

#[test]
fn encode_reports_a_file_that_is_not_a_png_with_exit_one() {
    let dir = TempDir::new("notpng");
    let junk = dir.path("junk.png");
    std::fs::write(&junk, b"this is not a PNG").unwrap();
    let out = run(&[
        "encode",
        junk.to_str().unwrap(),
        dir.path("x.prop").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("junk.png"));
}

#[test]
fn encode_reports_an_unwritable_output_with_exit_one() {
    let dir = TempDir::new("badout");
    let input = dir.path("face.png");
    write_png(&input, 2, 2, png::ColorType::Rgba, &stable_rgba(2, 2));
    let as_dir = dir.path("out.prop");
    std::fs::create_dir_all(&as_dir).unwrap();
    let out = run(&["encode", input.to_str().unwrap(), as_dir.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("out.prop"));
}

#[test]
fn encode_rejects_unknown_options_and_bad_offsets_with_exit_two() {
    let dir = TempDir::new("badflags");
    let input = dir.path("face.png");
    write_png(&input, 2, 2, png::ColorType::Rgba, &stable_rgba(2, 2));
    let input = input.to_str().unwrap();
    let output = dir.path("x.prop");
    let output = output.to_str().unwrap();

    let unknown = run(&["encode", input, output, "--nope"]);
    assert_eq!(code(&unknown), 2);
    assert!(stderr(&unknown).contains("--nope"), "{}", stderr(&unknown));

    let bad_number = run(&["encode", input, output, "--h-offset", "left"]);
    assert_eq!(code(&bad_number), 2);
    assert!(
        stderr(&bad_number).contains("left"),
        "{}",
        stderr(&bad_number)
    );

    let no_value = run(&["encode", input, output, "--v-offset"]);
    assert_eq!(code(&no_value), 2);
    assert!(
        stderr(&no_value).contains("--v-offset"),
        "{}",
        stderr(&no_value)
    );
}

#[test]
fn encode_without_two_paths_exits_two() {
    let out = run(&["encode", "/tmp/only-one.png"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

// ---------------------------------------------------------------------------
// encode-batch
// ---------------------------------------------------------------------------

#[test]
fn encode_batch_writes_every_png_in_sorted_order() {
    let dir = TempDir::new("batch");
    let src = dir.path("src");
    std::fs::create_dir_all(&src).unwrap();
    for name in ["b.png", "a.png", "c.png"] {
        let pixels = stable_rgba(4, 4);
        write_png(&src.join(name), 4, 4, png::ColorType::Rgba, &pixels);
    }
    // A non-PNG file must be ignored, not rejected.
    std::fs::write(src.join("notes.txt"), b"ignore me").unwrap();

    let outdir = dir.path("out");
    let out = run(&[
        "encode-batch",
        src.to_str().unwrap(),
        outdir.to_str().unwrap(),
        "--head",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("wrote 3, failed 0"), "{text}");
    let (a, b, c) = (
        text.find("a.png").expect("a reported"),
        text.find("b.png").expect("b reported"),
        text.find("c.png").expect("c reported"),
    );
    assert!(a < b && b < c, "not sorted: {text}");
    assert!(!text.contains("notes.txt"), "{text}");

    for name in ["a", "b", "c"] {
        let prop = decode_prop(&outdir.join(format!("{name}.prop")));
        assert_eq!(prop.format(), PropFormat::S20Bit);
        assert!(prop.header.is_head(), "{name} lost the head flag");
    }
}

#[test]
fn encode_batch_keeps_going_after_an_odd_width() {
    let dir = TempDir::new("batch-partial");
    let src = dir.path("src");
    std::fs::create_dir_all(&src).unwrap();
    write_png(
        &src.join("good.png"),
        4,
        4,
        png::ColorType::Rgba,
        &stable_rgba(4, 4),
    );
    write_png(
        &src.join("bad.png"),
        3,
        2,
        png::ColorType::Rgba,
        &stable_rgba(3, 2),
    );

    let outdir = dir.path("out");
    let out = run(&[
        "encode-batch",
        src.to_str().unwrap(),
        outdir.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(
        stdout(&out).contains("wrote 1, failed 1"),
        "{}",
        stdout(&out)
    );
    assert!(stderr(&out).contains("bad.png"), "{}", stderr(&out));
    assert!(outdir.join("good.prop").is_file());
    assert!(!outdir.join("bad.prop").exists());
}

#[test]
fn encode_batch_reports_a_missing_directory_with_exit_one() {
    let dir = TempDir::new("batch-nodir");
    let out = run(&[
        "encode-batch",
        "/nonexistent/smiley-src",
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("/nonexistent/smiley-src"));
}

#[test]
fn encode_batch_reports_a_directory_without_pngs_with_exit_one() {
    let dir = TempDir::new("batch-empty");
    let src = dir.path("src");
    std::fs::create_dir_all(&src).unwrap();
    let out = run(&[
        "encode-batch",
        src.to_str().unwrap(),
        dir.path("out").to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("no *.png"), "{}", stderr(&out));
}

#[test]
fn encode_batch_reports_an_uncreatable_outdir_with_exit_one() {
    let dir = TempDir::new("batch-badout");
    let src = dir.path("src");
    std::fs::create_dir_all(&src).unwrap();
    write_png(
        &src.join("a.png"),
        2,
        2,
        png::ColorType::Rgba,
        &stable_rgba(2, 2),
    );
    let as_file = dir.path("not-a-dir");
    std::fs::write(&as_file, b"x").unwrap();
    let out = run(&[
        "encode-batch",
        src.to_str().unwrap(),
        as_file.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not-a-dir"), "{}", stderr(&out));
}

#[test]
fn encode_batch_without_two_paths_exits_two() {
    let out = run(&["encode-batch", "/tmp/only-one"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("usage:"));
}

// ---------------------------------------------------------------------------
// usage
// ---------------------------------------------------------------------------

#[test]
fn help_lists_the_encoding_commands() {
    let out = run(&["--help"]);
    assert_eq!(code(&out), 0);
    let text = stderr(&out);
    assert!(text.contains("prop-tool encode "), "{text}");
    assert!(text.contains("prop-tool encode-batch "), "{text}");
}

//! `prop-tool` — corpus tooling for `palace-prop`.
//!
//! Deliberately not a library API: this binary knows about the `.prp` roster
//! container and the local corpus layout, neither of which belongs in the codec.
//!
//! ```text
//! prop-tool inventory  <path>...        decode everything, tally by format
//! prop-tool rgba       <prop.bin> <out> dump raw RGBA for differential testing
//! prop-tool rgba-batch <manifest> <dir> dump one raw RGBA per manifest line
//! prop-tool digests    <manifest> <out> one FNV-1a digest per manifest line
//! prop-tool png        <prop.bin> <out> dump a PNG for eyeballing
//! prop-tool extract    <file.prp> <dir> [--limit N] [--format NAME] [--stride N]
//!                                       split a roster into per-prop blobs
//! prop-tool encode     <in.png> <out.prop> [--head] [--ghost]
//!                                       [--h-offset N] [--v-offset N]
//! prop-tool encode-batch <in-dir> <out-dir> [--head] [--ghost]
//!                                       [--h-offset N] [--v-offset N]
//! ```
//!
//! `inventory` accepts directories (recursed for `*.bin`) and `.prp` files. It
//! prints a machine-readable summary so the numbers in the README can be
//! regenerated rather than trusted.
//!
//! `encode` and `encode-batch` are the only paths that turn a PNG back into a
//! prop. They emit S20 — the one format the reference client produces — and
//! refuse odd widths up front, naming the file, because S20 packs pixels in
//! pairs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use palace_prop::{
    decode, decode_header, encode_s20_blob, PropEndian, PropError, PropFormat, PropImage,
    FLAG_GHOST, FLAG_HEAD,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        usage();
        return ExitCode::from(2);
    };
    match command {
        "inventory" => inventory(&args[1..]),
        "rgba" => rgba(&args[1..]),
        "rgba-batch" => rgba_batch(&args[1..]),
        "digests" => digests(&args[1..]),
        "png" => png(&args[1..]),
        "extract" => extract(&args[1..]),
        "encode" => encode(&args[1..]),
        "encode-batch" => encode_batch(&args[1..]),
        "help" | "--help" | "-h" => {
            usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command {other:?}");
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!(
        "usage:\n  \
         prop-tool inventory <path>...\n  \
         prop-tool rgba <prop.bin> <out.rgba>\n  \
         prop-tool rgba-batch <manifest> <outdir>\n  \
         prop-tool digests <manifest> <outfile>\n  \
         prop-tool png <prop.bin> <out.png>\n  \
         prop-tool extract <file.prp> <outdir> [--limit N] [--format NAME] [--stride N]\n  \
         prop-tool encode <in.png> <out.prop> [--head] [--ghost] [--h-offset N] [--v-offset N]\n  \
         prop-tool encode-batch <in-dir> <out-dir> [--head] [--ghost] [--h-offset N] [--v-offset N]"
    );
}

/// A prop blob to decode: either a file, or a slice of an already-loaded roster.
enum Origin {
    File(PathBuf),
    Slice {
        buf: Rc<Vec<u8>>,
        start: usize,
        len: usize,
    },
}

struct Source {
    label: String,
    origin: Origin,
}

impl Source {
    fn bytes(&self) -> Option<Vec<u8>> {
        match &self.origin {
            Origin::File(path) => std::fs::read(path).ok(),
            Origin::Slice { buf, start, len } => buf.get(*start..*start + *len).map(<[u8]>::to_vec),
        }
    }
}

#[derive(Default)]
struct Tally {
    decoded: BTreeMap<&'static str, u64>,
    endian: BTreeMap<&'static str, u64>,
    failed: u64,
    pixels: u64,
    failures: Vec<(String, String)>,
}

impl Tally {
    fn record(&mut self, source: &str, bytes: &[u8]) {
        match profile(bytes) {
            Ok((format, endian, pixel_count)) => {
                *self.decoded.entry(format.name()).or_default() += 1;
                *self.endian.entry(endian).or_default() += 1;
                self.pixels += pixel_count;
            }
            Err((format, reason)) => {
                self.failed += 1;
                if self.failures.len() < 80 {
                    let label = format.map_or("?".to_string(), |f| f.name().to_string());
                    self.failures.push((format!("{source} [{label}]"), reason));
                }
            }
        }
    }
}

type Failure = (Option<PropFormat>, String);

/// Decode one blob and return `(format, endian, pixel_count)` or a failure.
fn profile(bytes: &[u8]) -> Result<(PropFormat, &'static str, u64), Failure> {
    let header = decode_header(bytes).map_err(|e| (None, e.to_string()))?;
    let format = header.format();
    let endian = match header.endian {
        PropEndian::Little => "little",
        PropEndian::Big => "big",
    };
    let pixel_count =
        u64::from(header.width.unsigned_abs()) * u64::from(header.height.unsigned_abs());
    match decode(bytes) {
        Ok(prop) => Ok((prop.format(), endian, pixel_count)),
        Err(e) => Err((Some(format), describe(&e))),
    }
}

/// Collapse zlib errors, whose details are long and highly variable, so the
/// failure table stays readable.
fn describe(error: &PropError) -> String {
    match error {
        PropError::Zlib { .. } => "zlib stream rejected".to_string(),
        other => other.to_string(),
    }
}

fn inventory(paths: &[String]) -> ExitCode {
    if paths.is_empty() {
        usage();
        return ExitCode::from(2);
    }
    let mut grand = Tally::default();
    for raw in paths {
        let path = Path::new(raw);
        let sources = collect(path);
        let mut tally = Tally::default();
        for source in &sources {
            if let Some(bytes) = source.bytes() {
                tally.record(&source.label, &bytes);
            }
        }
        report(&path.display().to_string(), &tally, sources.len());
        merge(&mut grand, &tally);
    }
    if paths.len() > 1 {
        report("TOTAL", &grand, 0);
    }
    ExitCode::SUCCESS
}

fn merge(into: &mut Tally, from: &Tally) {
    for (key, value) in &from.decoded {
        *into.decoded.entry(key).or_default() += value;
    }
    for (key, value) in &from.endian {
        *into.endian.entry(key).or_default() += value;
    }
    into.failed += from.failed;
    into.pixels += from.pixels;
    into.failures.extend(from.failures.iter().cloned());
}

fn collect(path: &Path) -> Vec<Source> {
    if path.is_dir() {
        let mut out = Vec::new();
        walk(path, &mut out);
        return out;
    }
    if path.extension().is_some_and(|e| e == "prp") {
        return roster_sources(path);
    }
    vec![Source {
        label: path.display().to_string(),
        origin: Origin::File(path.to_path_buf()),
    }]
}

fn walk(dir: &Path, out: &mut Vec<Source>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "bin") {
            out.push(Source {
                label: path.display().to_string(),
                origin: Origin::File(path),
            });
        }
    }
}

/// Parse a `.prp` asset roster and yield one blob per record.
///
/// The container is documented in `~/palace-corpus/PRP-FORMAT.md`: a 16-byte file
/// header, a data region, then a type table and 32-byte records. A record's blob
/// lives at `16 + dataOffset` and is `dataSize` bytes long; the trailing `Fave`
/// sentinel has `dataSize == 0` and is skipped.
fn roster_sources(path: &Path) -> Vec<Source> {
    let Ok(buf) = std::fs::read(path) else {
        return Vec::new();
    };
    let Some(roster) = Roster::parse(&buf) else {
        return Vec::new();
    };
    let shared = Rc::new(buf);
    roster
        .records(&shared)
        .into_iter()
        .filter_map(|(id, data_rel, size)| {
            let start = roster.data_offset + data_rel;
            start.checked_add(size).filter(|end| *end <= shared.len())?;
            Some(Source {
                label: format!("{}#{id}", path.display()),
                origin: Origin::Slice {
                    buf: Rc::clone(&shared),
                    start,
                    len: size,
                },
            })
        })
        .collect()
}

/// The parts of a `.prp` this tool needs.
struct Roster {
    data_offset: usize,
    map_offset: usize,
    n_assets: usize,
    recs_offset: usize,
}

impl Roster {
    fn parse(buf: &[u8]) -> Option<Self> {
        let u32_at = |at: usize| -> Option<usize> {
            buf.get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        };
        let data_offset = u32_at(0)?;
        let map_offset = u32_at(8)?;
        let n_assets = u32_at(map_offset + 4)?;
        if n_assets > 4_000_000 {
            return None;
        }
        let recs_offset = u32_at(map_offset + 16)?;
        Some(Roster {
            data_offset,
            map_offset,
            n_assets,
            recs_offset,
        })
    }

    /// `(id, data_offset, data_size)` for every record that carries a payload.
    fn records(&self, buf: &[u8]) -> Vec<(u32, usize, usize)> {
        (0..self.n_assets)
            .filter_map(|i| {
                let at = self.map_offset + self.recs_offset + i * 32;
                let record = buf.get(at..at + 32)?;
                let id = u32::from_le_bytes(record[0..4].try_into().ok()?);
                let data_rel = u32::from_le_bytes(record[8..12].try_into().ok()?) as usize;
                let size = u32::from_le_bytes(record[12..16].try_into().ok()?) as usize;
                (size >= 12).then_some((id, data_rel, size))
            })
            .collect()
    }
}

fn report(name: &str, tally: &Tally, sources: usize) {
    let mut line = String::new();
    let _ = write!(line, "{name}\t");
    for format in [
        PropFormat::EightBit,
        PropFormat::SixteenBit,
        PropFormat::TwentyBit,
        PropFormat::S20Bit,
        PropFormat::ThirtyTwoBit,
    ] {
        let n = tally.decoded.get(format.name()).copied().unwrap_or(0);
        let _ = write!(line, "{}={n}\t", format.name());
    }
    let _ = write!(
        line,
        "failed={}\tendian={:?}\tpixels={}",
        tally.failed, tally.endian, tally.pixels
    );
    if sources > 0 {
        let _ = write!(line, "\tsources={sources}");
    }
    println!("{line}");
    for (source, reason) in &tally.failures {
        println!("\tFAIL {source}: {reason}");
    }
}

fn rgba(args: &[String]) -> ExitCode {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let prop = match decode(&bytes) {
        Ok(prop) => prop,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut out = Vec::with_capacity(prop.image.as_rgba().len() + 12);
    out.extend_from_slice(b"PRGBA1");
    out.extend_from_slice(&prop.image.width().to_le_bytes());
    out.extend_from_slice(&prop.image.height().to_le_bytes());
    out.extend_from_slice(prop.image.as_rgba());
    match std::fs::write(output, out) {
        Ok(()) => {
            println!(
                "{input}: {}x{} {} rgba={} bytes",
                prop.image.width(),
                prop.image.height(),
                prop.format().name(),
                prop.image.as_rgba().len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{output}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn png(args: &[String]) -> ExitCode {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match decode(&bytes).and_then(|prop| prop.image.write_png(output)) {
        Ok(()) => {
            println!("{input} -> {output}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{input}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Dump one `PRGBA1` file per line of a manifest of prop paths.
///
/// Output names are the manifest line number, so the Python oracle can pair its
/// own decode of line `i` with `i.rgba` without re-deriving any paths. Failures
/// are reported on stderr and produce no file, which the oracle then reports as a
/// one-sided disagreement.
fn rgba_batch(args: &[String]) -> ExitCode {
    let (Some(manifest), Some(outdir)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let list = match std::fs::read_to_string(manifest) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("{manifest}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::create_dir_all(outdir) {
        eprintln!("{outdir}: {e}");
        return ExitCode::FAILURE;
    }
    let mut ok = 0usize;
    let mut failed = 0usize;
    for (i, line) in list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
    {
        let output = Path::new(outdir).join(format!("{i}.rgba"));
        match std::fs::read(line)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                decode(&bytes).map_err(|e| e.to_string()).map(|prop| {
                    let mut out = Vec::with_capacity(prop.image.as_rgba().len() + 12);
                    out.extend_from_slice(b"PRGBA1");
                    out.extend_from_slice(&prop.image.width().to_le_bytes());
                    out.extend_from_slice(&prop.image.height().to_le_bytes());
                    out.extend_from_slice(prop.image.as_rgba());
                    out
                })
            }) {
            Ok(out) => {
                if std::fs::write(&output, out).is_ok() {
                    ok += 1;
                }
            }
            Err(e) => {
                eprintln!("line {i} ({line}): {e}");
                failed += 1;
            }
        }
    }
    println!("rgba-batch: wrote {ok}, failed {failed}, outdir {outdir}");
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// FNV-1a (64-bit) of a decoded image: dimensions followed by the RGBA bytes.
///
/// The Python oracle computes the same value, which lets the differential cover
/// the whole corpus without materialising a gigabyte of `.rgba` files. Any pixel
/// difference anywhere changes the digest.
fn image_digest(width: u32, height: u32, rgba: &[u8]) -> u64 {
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

/// Write one `index<TAB>format<TAB>width<TAB>height<TAB>digest` line per manifest
/// line, or `index<TAB>FAIL<TAB>reason` when the prop is rejected.
fn digests(args: &[String]) -> ExitCode {
    let (Some(manifest), Some(outfile)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let list = match std::fs::read_to_string(manifest) {
        Ok(list) => list,
        Err(e) => {
            eprintln!("{manifest}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut out = String::new();
    let (mut ok, mut failed) = (0usize, 0usize);
    for (i, line) in list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .enumerate()
    {
        let decoded = std::fs::read(line)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode(&bytes).map_err(|e| describe(&e)));
        match decoded {
            Ok(prop) => {
                ok += 1;
                let _ = writeln!(
                    out,
                    "{i}\t{}\t{}\t{}\t{:016x}",
                    prop.format().name(),
                    prop.image.width(),
                    prop.image.height(),
                    image_digest(
                        prop.image.width(),
                        prop.image.height(),
                        prop.image.as_rgba()
                    )
                );
            }
            Err(reason) => {
                failed += 1;
                let _ = writeln!(out, "{i}\tFAIL\t{reason}");
            }
        }
    }
    if let Err(e) = std::fs::write(outfile, out) {
        eprintln!("{outfile}: {e}");
        return ExitCode::FAILURE;
    }
    println!("digests: {ok} decoded, {failed} failed -> {outfile}");
    ExitCode::SUCCESS
}

fn extract(args: &[String]) -> ExitCode {
    let (Some(input), Some(outdir)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let flag = |name: &str| -> Option<&String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let limit = flag("--limit")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let stride = flag("--stride")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1);
    let wanted = flag("--format").map(String::as_str);
    let Ok(buf) = std::fs::read(input) else {
        eprintln!("{input}: unreadable");
        return ExitCode::FAILURE;
    };
    let Some(roster) = Roster::parse(&buf) else {
        eprintln!("{input}: not a .prp");
        return ExitCode::FAILURE;
    };
    if let Err(e) = std::fs::create_dir_all(outdir) {
        eprintln!("{outdir}: {e}");
        return ExitCode::FAILURE;
    }
    let mut written = 0usize;
    for (i, (id, data_rel, size)) in roster.records(&buf).into_iter().enumerate() {
        if i % stride != 0 || written >= limit {
            continue;
        }
        let start = roster.data_offset + data_rel;
        let Some(blob) = buf.get(start..start + size) else {
            continue;
        };
        let format = decode_header(blob)
            .map(|h| h.format().name().to_string())
            .unwrap_or_else(|_| "bad".into());
        if wanted.is_some_and(|w| w != format) {
            continue;
        }
        let path = Path::new(outdir).join(format!("{format}_{id:08x}_{i}.bin"));
        if std::fs::write(&path, blob).is_ok() {
            written += 1;
        }
    }
    println!("extracted {written} props to {outdir}");
    ExitCode::SUCCESS
}

/// The head/ghost/offset flags shared by `encode` and `encode-batch`.
struct EncodeOptions {
    head: bool,
    ghost: bool,
    h_offset: i16,
    v_offset: i16,
}

impl EncodeOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = EncodeOptions {
            head: false,
            ghost: false,
            h_offset: 0,
            v_offset: 0,
        };
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--head" => options.head = true,
                "--ghost" => options.ghost = true,
                "--h-offset" => {
                    i += 1;
                    options.h_offset = parse_offset(args, i, "--h-offset")?;
                }
                "--v-offset" => {
                    i += 1;
                    options.v_offset = parse_offset(args, i, "--v-offset")?;
                }
                other => return Err(format!("unknown option {other:?}")),
            }
            i += 1;
        }
        Ok(options)
    }

    fn flags(&self) -> u16 {
        (if self.head { FLAG_HEAD } else { 0 }) | (if self.ghost { FLAG_GHOST } else { 0 })
    }
}

fn parse_offset(args: &[String], at: usize, name: &str) -> Result<i16, String> {
    let raw = args
        .get(at)
        .ok_or_else(|| format!("{name} needs a value"))?;
    raw.parse::<i16>()
        .map_err(|_| format!("{name}: {raw:?} is not a 16-bit integer"))
}

fn encode(args: &[String]) -> ExitCode {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let options = match EncodeOptions::parse(&args[2..]) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{e}");
            usage();
            return ExitCode::from(2);
        }
    };
    match encode_file(Path::new(input), Path::new(output), &options) {
        Ok((width, height)) => {
            println!(
                "{input} -> {output} {width}x{height} flags=0x{:04x}",
                options.flags()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn encode_batch(args: &[String]) -> ExitCode {
    let (Some(indir), Some(outdir)) = (args.first(), args.get(1)) else {
        usage();
        return ExitCode::from(2);
    };
    let options = match EncodeOptions::parse(&args[2..]) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{e}");
            usage();
            return ExitCode::from(2);
        }
    };
    let inputs = match sorted_pngs(Path::new(indir)) {
        Ok(inputs) => inputs,
        Err(e) => {
            eprintln!("{indir}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if inputs.is_empty() {
        eprintln!("{indir}: no *.png files");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(outdir) {
        eprintln!("{outdir}: {e}");
        return ExitCode::FAILURE;
    }
    let (mut written, mut failed) = (0usize, 0usize);
    for input in &inputs {
        let mut output = Path::new(outdir).join(input.file_stem().unwrap_or_default());
        output.set_extension("prop");
        match encode_file(input, &output, &options) {
            Ok((width, height)) => {
                written += 1;
                println!(
                    "{} -> {} {width}x{height} flags=0x{:04x}",
                    input.display(),
                    output.display(),
                    options.flags()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("{e}");
            }
        }
    }
    println!("encode-batch: wrote {written}, failed {failed}, outdir {outdir}");
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Read one PNG as RGBA8 and encode it as an S20 blob.
///
/// The odd-width check runs before the codec so the error names the file and its
/// dimensions: S20 packs pixels in pairs and cannot represent an odd width.
fn encode_file(input: &Path, output: &Path, options: &EncodeOptions) -> Result<(u32, u32), String> {
    let (width, height, rgba) = read_png_rgba(input)?;
    if width == 0 || height == 0 || width % 2 != 0 {
        return Err(format!(
            "{}: cannot encode {width}x{height}: S20 packs pixels in pairs, so the width must be even and non-zero",
            input.display()
        ));
    }
    let image = PropImage::from_rgba(width, height, rgba)
        .map_err(|e| format!("{}: {e}", input.display()))?;
    let blob = encode_s20_blob(&image, options.h_offset, options.v_offset, options.flags())
        .map_err(|e| format!("{}: {e}", input.display()))?;
    std::fs::write(output, blob).map_err(|e| format!("{}: {e}", output.display()))?;
    Ok((width, height))
}

/// `*.png` files in `dir`, sorted by path, so `encode-batch` is deterministic.
fn sorted_pngs(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Decode one PNG to RGBA8, converting any other colour type rather than
/// letting the pixel layout silently mean something else.
fn read_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let label = path.display();
    let file = std::fs::File::open(path).map_err(|e| format!("{label}: {e}"))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Expand palettes, sub-8-bit depths and tRNS to packed 8-bit samples.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("{label}: {e}"))?;
    let needed = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{label}: image too large to decode"))?;
    let mut raw = vec![0u8; needed];
    let info = reader
        .next_frame(&mut raw)
        .map_err(|e| format!("{label}: {e}"))?;
    let pixels = info.line_size * info.height as usize;
    let data = raw
        .get(..pixels)
        .ok_or_else(|| format!("{label}: truncated PNG frame"))?;
    let rgba = expand_to_rgba(info.color_type, info.bit_depth, data).ok_or_else(|| {
        format!(
            "{label}: unsupported PNG pixel format {:?}/{:?}",
            info.color_type, info.bit_depth
        )
    })?;
    Ok((info.width, info.height, rgba))
}

/// Widen 8-bit PNG samples to RGBA. `None` means the combination cannot be
/// widened without guessing, which the caller reports instead of writing wrong
/// pixels.
fn expand_to_rgba(
    color_type: png::ColorType,
    bit_depth: png::BitDepth,
    data: &[u8],
) -> Option<Vec<u8>> {
    if bit_depth != png::BitDepth::Eight {
        return None;
    }
    let mut rgba = Vec::new();
    match color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for px in data.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &v in data {
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in data.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    Some(rgba)
}

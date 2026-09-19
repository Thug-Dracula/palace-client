//! `palace-render` — render a Palace room to a PNG, headless.
//!
//! ```text
//! cargo run -p palace-render -- --room 7022 --out renders/7022.png
//! cargo run -p palace-render -- --frame fixtures/logon-run1/frames/0007-server-room.bin --out renders/901.png
//! cargo run -p palace-render -- --room 1672 --avatar 300,200,1041303665 --dpr 2 --out renders/1672-avatars.png
//! ```
//!
//! Everything defaults to the local corpus under `$CORPUS`; see `--help`.
//! The home directory is `HOME` on Unix and `USERPROFILE` on Windows, so the
//! defaults resolve on both platforms; any default that does not exist is
//! skipped rather than guessed at.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use palace_render::{
    build::AvatarSpec, render, AnimationClock, Canvas, MediaStore, PlacedProp, PropStore,
    RenderOptions, RoomSource, Scene, SceneBuilder, ViewTransform,
};

const HELP: &str = "\
palace-render — composite a Palace room into a PNG (headless).

USAGE:
    palace-render --room <ID> --out <FILE> [OPTIONS]
    palace-render --room-file <PAYLOAD> --out <FILE> [OPTIONS]
    palace-render --frame-file <FRAME> --out <FILE> [OPTIONS]

SOURCE (exactly one):
    --room <ID>            find a room by id in --rooms-dir
    --room-file <PATH>     a raw MSG_ROOMDESC payload (no frame header)
    --frame-file <PATH>    a captured frame (12-byte header + payload)

OUTPUT:
    --out <FILE>           PNG path to write (required)
    --dpr <FLOAT>          device-pixel ratio, clamped to [1,2] (default 1)
    --clock-ms <UINT>      injected animation clock (default 0; deterministic)
    --repeat <N>           load once, then run build+render+encode N times on
                           warm stores, printing per-iteration timings
                           (default 1 = existing single-shot behaviour)

ROOM SOURCE / ASSETS:
    --rooms-dir <DIR>      room payload directory (default $CORPUS/payloads_all)
    --media-root <DIR>     background/overlay root; repeatable
                           (defaults: $CORPUS media dirs plus the
                           PalaceChat client's own Media directory)
    --props-dir <DIR>      prop blob directory; repeatable
                           (defaults: props_harvested, arks)
    --roster <FILE>        .prp prop roster (default $CORPUS/pserver.prp if present)

AVATARS:
    --avatar <X>,<Y>[,<PROP_ID>...][;<NAME>]   place an avatar (repeatable)
    --loose-prop <PROP_ID>@<X>,<Y>    place an extra loose prop (repeatable)

PREVIEW (no file written):
    --viewport <WxH>       print the room<->viewport mapping for this viewport
    --zoom <FLOAT>         zoom for --viewport (default 1)

    -h, --help             show this help
";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("palace-render: {message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Args {
    room: Option<i32>,
    room_file: Option<PathBuf>,
    frame_file: Option<PathBuf>,
    out: Option<PathBuf>,
    dpr: f64,
    clock_ms: u64,
    rooms_dir: Option<PathBuf>,
    media_roots: Vec<PathBuf>,
    props_dirs: Vec<PathBuf>,
    roster: Option<PathBuf>,
    avatars: Vec<AvatarSpec>,
    extra_props: Vec<PlacedProp>,
    viewport: Option<(f64, f64)>,
    zoom: f64,
    repeat: usize,
}

fn run(argv: Vec<String>) -> Result<(), String> {
    let args = parse_args(&argv)?;
    if let Some((width, height)) = args.viewport {
        return print_mapping(&args, width, height);
    }

    let out = args
        .out
        .clone()
        .ok_or_else(|| "no --out given (see --help)".to_string())?;
    let room = load_room(&args)?;

    let media = MediaStore::new(&args.media_roots);
    let mut props = PropStore::new();
    for dir in &args.props_dirs {
        props.add_directory(dir);
    }
    if let Some(roster) = &args.roster {
        match props.add_roster(roster) {
            Ok(count) => eprintln!("roster {}: {count} props indexed", roster.display()),
            Err(err) => eprintln!("roster {}: {err}", roster.display()),
        }
    }
    eprintln!(
        "media roots indexed {} names, prop store holds {} ids",
        media.len(),
        props.len()
    );

    let builder = SceneBuilder::new(media, props);
    let options = RenderOptions {
        dpr: args.dpr,
        clock: AnimationClock::at(args.clock_ms),
    };

    if args.repeat > 1 {
        return run_repeat(&builder, &room, options, &args, &out);
    }

    let scene = builder.build_with(&room, &args.avatars, &args.extra_props);
    report(&room, &scene, &args);
    let canvas = render(&scene, options);
    write_png(&canvas, &out)?;
    eprintln!(
        "wrote {} ({}x{} device px, dpr {})",
        out.display(),
        canvas.width(),
        canvas.height(),
        canvas.dpr()
    );
    Ok(())
}

/// `--repeat N`: everything is loaded once, then build → render → PNG encode
/// runs `N` times with warm in-memory stores, timing each stage. Iteration 0 is
/// reported but excluded from the warm mean/max because it pays first-touch
/// allocation and page faults.
fn run_repeat(
    builder: &SceneBuilder,
    room: &palace_render::RoomDesc,
    options: RenderOptions,
    args: &Args,
    out: &PathBuf,
) -> Result<(), String> {
    let n = args.repeat;
    let mut times: Vec<[f64; 3]> = Vec::with_capacity(n);
    let mut last: Option<Canvas> = None;
    for i in 0..n {
        let t0 = std::time::Instant::now();
        let scene = builder.build_with(room, &args.avatars, &args.extra_props);
        let t1 = std::time::Instant::now();
        if i == 0 {
            report(room, &scene, args);
        }
        let canvas = render(&scene, options);
        let t2 = std::time::Instant::now();
        let _bytes = canvas.to_png_bytes().map_err(|e| e.to_string())?;
        let t3 = std::time::Instant::now();

        let build_ms = t1.duration_since(t0).as_secs_f64() * 1000.0;
        let render_ms = t2.duration_since(t1).as_secs_f64() * 1000.0;
        let png_ms = t3.duration_since(t2).as_secs_f64() * 1000.0;
        let total_ms = build_ms + render_ms + png_ms;
        times.push([build_ms, render_ms, png_ms]);
        let cold = if i == 0 && n > 1 {
            " (cold, excluded from warm stats)"
        } else {
            ""
        };
        println!(
            "iter {i}: build_with {build_ms:.3} ms, render {render_ms:.3} ms, \
             to_png_bytes {png_ms:.3} ms, total {total_ms:.3} ms{cold}"
        );
        last = Some(canvas);
    }

    // Iteration 0 pays first-touch cost; the warm stats start at 1.
    let warm = &times[1..];
    let builds: Vec<f64> = warm.iter().map(|r| r[0]).collect();
    let renders: Vec<f64> = warm.iter().map(|r| r[1]).collect();
    let pngs: Vec<f64> = warm.iter().map(|r| r[2]).collect();
    let totals: Vec<f64> = warm.iter().map(|r| r[0] + r[1] + r[2]).collect();
    println!(
        "repeat {n}: warm stats over {} iteration(s) (iter 0 cold, excluded; \
         cold total {:.3} ms)",
        warm.len(),
        times[0].iter().sum::<f64>()
    );
    for (label, values) in [
        ("build_with", &builds),
        ("render", &renders),
        ("to_png_bytes", &pngs),
        ("total", &totals),
    ] {
        let (mean, max) = stats(values);
        println!("  {label:<13} mean {mean:.3} ms  max {max:.3} ms");
    }

    if let Some(canvas) = last {
        write_png(&canvas, out)?;
        println!(
            "wrote {} ({}x{} device px, dpr {})",
            out.display(),
            canvas.width(),
            canvas.height(),
            canvas.dpr()
        );
    }
    Ok(())
}

fn stats(values: &[f64]) -> (f64, f64) {
    let mut sum = 0.0f64;
    let mut max = 0.0f64;
    for value in values {
        sum += value;
        if *value > max {
            max = *value;
        }
    }
    let mean = if values.is_empty() {
        0.0
    } else {
        sum / values.len() as f64
    };
    (mean, max)
}

fn write_png(canvas: &Canvas, out: &PathBuf) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    canvas.write_png(out).map_err(|e| e.to_string())
}

fn load_room(args: &Args) -> Result<palace_render::RoomDesc, String> {
    if let Some(path) = &args.frame_file {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return RoomSource::decode_frame(&bytes).map_err(|e| e.to_string());
    }
    if let Some(path) = &args.room_file {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return RoomSource::decode(&bytes).map_err(|e| e.to_string());
    }
    let id = args
        .room
        .ok_or_else(|| "no room source: pass --room, --room-file or --frame-file".to_string())?;
    let dir = args
        .rooms_dir
        .clone()
        .ok_or_else(|| "no --rooms-dir and no default corpus found".to_string())?;
    let source = RoomSource::new(dir.clone(), palace_wire::ByteOrder::Little);
    source
        .find(id)
        .ok_or_else(|| format!("room {id} not found under {}", dir.display()))
}

fn report(room: &palace_render::RoomDesc, scene: &Scene, args: &Args) {
    eprintln!(
        "room {} {:?} artist {:?}",
        room.header.room_id, room.name, room.artist
    );
    let (w, h) = scene.size;
    let bg = scene
        .background
        .as_ref()
        .map(|b| format!("{}x{}", b.width(), b.height()))
        .unwrap_or_else(|| "none (flat backdrop)".to_string());
    eprintln!("logical size {w}x{h} (background {bg}), dpr {}", args.dpr);
    eprintln!(
        "overlays {} / {} / {} / {} | loose props {} | avatars {}",
        scene.overlays_above_nothing.len(),
        scene.overlays_above_avatars.len(),
        scene.overlays_above_name_tags.len(),
        scene.overlays_above_everything.len(),
        scene.loose_props.len(),
        scene.avatars.len(),
    );
    if !room.draw_cmds.is_empty() {
        eprintln!(
            "note: {} draw command(s) parsed but not rasterized in this milestone",
            room.draw_cmds.len()
        );
    }
    for note in &scene.notes {
        eprintln!("report: {note}");
    }
}

fn print_mapping(args: &Args, viewport_w: f64, viewport_h: f64) -> Result<(), String> {
    let room = load_room(args)?;
    let media = MediaStore::new(&args.media_roots);
    let props = PropStore::new();
    let scene = SceneBuilder::new(media, props).build(&room, &[]);
    let (w, h) = scene.logical_size();
    let viewport = palace_render::SizeF::new(viewport_w, viewport_h);
    for mode in [
        palace_render::ScaleMode::Fit,
        palace_render::ScaleMode::Native,
    ] {
        let t = ViewTransform::new(
            palace_render::SizeF::new(w, h),
            viewport,
            args.zoom,
            mode,
            args.dpr,
        );
        let rect = t.content_rect();
        eprintln!(
            "{:?}: scale {:.6} content {:.3}x{:.3} offset ({:.3},{:.3}) buffer {:?}",
            mode,
            t.scale(),
            rect.width,
            rect.height,
            rect.x,
            rect.y,
            t.buffer_size()
        );
        for (x, y) in [(0.0, 0.0), (w / 2.0, h / 2.0), (w, h)] {
            let room_pt = palace_render::PointF::new(x, y);
            let css = t.room_to_viewport(room_pt);
            let back = t.viewport_to_room(css);
            eprintln!(
                "  room ({x:.1},{y:.1}) -> viewport ({:.3},{:.3}) -> room ({:.9},{:.9})",
                css.x, css.y, back.x, back.y
            );
        }
    }
    Ok(())
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args {
        dpr: 1.0,
        zoom: 1.0,
        repeat: 1,
        ..Args::default()
    };
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let mut value = || -> Result<String, String> {
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--room" => args.room = Some(value()?.parse().map_err(|e| format!("--room: {e}"))?),
            "--room-file" => args.room_file = Some(PathBuf::from(value()?)),
            "--frame-file" => args.frame_file = Some(PathBuf::from(value()?)),
            "--out" => args.out = Some(PathBuf::from(value()?)),
            "--dpr" => args.dpr = value()?.parse().map_err(|e| format!("--dpr: {e}"))?,
            "--clock-ms" => {
                args.clock_ms = value()?.parse().map_err(|e| format!("--clock-ms: {e}"))?
            }
            "--rooms-dir" => args.rooms_dir = Some(PathBuf::from(value()?)),
            "--media-root" => args.media_roots.push(PathBuf::from(value()?)),
            "--props-dir" => args.props_dirs.push(PathBuf::from(value()?)),
            "--roster" => args.roster = Some(PathBuf::from(value()?)),
            "--zoom" => args.zoom = value()?.parse().map_err(|e| format!("--zoom: {e}"))?,
            "--repeat" => {
                let n: usize = value()?.parse().map_err(|e| format!("--repeat: {e}"))?;
                args.repeat = n.max(1);
            }
            "--avatar" => args.avatars.push(parse_avatar(&value()?)?),
            "--loose-prop" => args.extra_props.push(parse_placed_prop(&value()?)?),
            "--viewport" => args.viewport = Some(parse_viewport(&value()?)?),
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
        i += 1;
    }
    fill_defaults(&mut args);
    Ok(args)
}

fn parse_avatar(spec: &str) -> Result<AvatarSpec, String> {
    let (geometry, name) = match spec.split_once(';') {
        Some((geometry, name)) => (geometry, Some(name.to_string())),
        None => (spec, None),
    };
    let mut parts = geometry.split(',');
    let x = parts
        .next()
        .ok_or_else(|| "--avatar needs X,Y".to_string())?
        .trim()
        .parse()
        .map_err(|e| format!("--avatar x: {e}"))?;
    let y = parts
        .next()
        .ok_or_else(|| "--avatar needs X,Y".to_string())?
        .trim()
        .parse()
        .map_err(|e| format!("--avatar y: {e}"))?;
    let mut props = Vec::new();
    for part in parts {
        let id = part
            .trim()
            .parse()
            .map_err(|e| format!("--avatar prop id {part:?}: {e}"))?;
        props.push(id);
    }
    let mut avatar = AvatarSpec::new(x, y, props);
    avatar.name = name;
    Ok(avatar)
}

fn parse_placed_prop(spec: &str) -> Result<PlacedProp, String> {
    let (id, at) = spec
        .split_once('@')
        .ok_or_else(|| "--loose-prop needs PROP_ID@X,Y".to_string())?;
    let id = id
        .trim()
        .parse()
        .map_err(|e| format!("--loose-prop id {id:?}: {e}"))?;
    let (x, y) = at
        .split_once(',')
        .ok_or_else(|| "--loose-prop needs PROP_ID@X,Y".to_string())?;
    Ok(PlacedProp {
        id,
        x: x.trim()
            .parse()
            .map_err(|e| format!("--loose-prop x: {e}"))?,
        y: y.trim()
            .parse()
            .map_err(|e| format!("--loose-prop y: {e}"))?,
    })
}

fn parse_viewport(spec: &str) -> Result<(f64, f64), String> {
    let (w, h) = spec
        .split_once(['x', 'X'])
        .ok_or_else(|| "--viewport needs WxH".to_string())?;
    let w = w
        .trim()
        .parse()
        .map_err(|e| format!("--viewport width: {e}"))?;
    let h = h
        .trim()
        .parse()
        .map_err(|e| format!("--viewport height: {e}"))?;
    Ok((w, h))
}

/// The Unix client's per-user data directory below the home directory.
const UNIX_PALACE_CHAT_MEDIA_SUBDIR: &str = ".local/share/PalaceChat/Media";
/// The media directory name inside a PalaceChat data directory.
const PALACE_CHAT_MEDIA_DIR: &str = "Media";
/// The Windows profile subdirectory that holds the AppData roots, and the two
/// leaves inside it. Used only when `%APPDATA%`/`%LOCALAPPDATA%` are unset.
const WINDOWS_APPDATA_SUBDIR: &str = "AppData";
/// The roaming leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%APPDATA%`).
const WINDOWS_ROAMING_LEAF: &str = "Roaming";
/// The local leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%LOCALAPPDATA%`).
const WINDOWS_LOCAL_LEAF: &str = "Local";
/// A `PalaceChat*` scan is capped at this many directory names per root.
const MAX_PALACE_CHAT_DIRS: usize = 4;

/// The user's home directory for default asset discovery: `HOME` on Unix,
/// `USERPROFILE` on Windows (falling back to `HOME` when it is the only one
/// set, as some shells do). There is no hardcoded fallback; when no home is
/// known every home-relative default is skipped.
fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// The PalaceChat client's own media directories for this platform.
///
/// Unix uses `~/.local/share/PalaceChat/Media`; Windows uses `Media` inside
/// every `PalaceChat*` data directory under `%APPDATA%` and `%LOCALAPPDATA%`.
/// The Windows rung is built by [`windows_palace_chat_media_roots`], which
/// takes its roots as arguments so it is testable from any host.
fn palace_chat_media_roots(home: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(windows) {
        windows_palace_chat_media_roots(
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            std::env::var_os("USERPROFILE").map(PathBuf::from),
        )
    } else {
        unix_palace_chat_media_roots(home)
    }
}

/// The Unix client media directory: `~/.local/share/PalaceChat/Media`.
fn unix_palace_chat_media_roots(home: Option<&Path>) -> Vec<PathBuf> {
    home.map(|home| home.join(UNIX_PALACE_CHAT_MEDIA_SUBDIR))
        .into_iter()
        .collect()
}

/// `Media` inside every `PalaceChat*` directory under `%APPDATA%`, then under
/// `%LOCALAPPDATA%`; a root is derived from `%USERPROFILE%` when its own
/// variable is unset. The list is not existence-filtered — the caller checks.
fn windows_palace_chat_media_roots(
    app_data: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
) -> Vec<PathBuf> {
    let roaming = app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_ROAMING_LEAF));
    let local = local_app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_LOCAL_LEAF));
    let mut roots = Vec::new();
    for root in [roaming, local].into_iter().flatten() {
        for name in palace_chat_dir_names(&root) {
            roots.push(root.join(name).join(PALACE_CHAT_MEDIA_DIR));
        }
    }
    roots
}

fn derive_appdata(user_profile: &Option<PathBuf>, leaf: &str) -> Option<PathBuf> {
    user_profile
        .as_ref()
        .map(|profile| profile.join(WINDOWS_APPDATA_SUBDIR).join(leaf))
}

/// Every `PalaceChat*` directory name directly under `root`, sorted so the
/// bare `PalaceChat` (the current client) precedes versioned names such as
/// `PalaceChat 4` (the older 4.x client), capped at [`MAX_PALACE_CHAT_DIRS`].
fn palace_chat_dir_names(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.to_ascii_lowercase().starts_with("palacechat") {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        names.push(name.to_owned());
    }
    names.sort();
    names.truncate(MAX_PALACE_CHAT_DIRS);
    names
}

fn fill_defaults(args: &mut Args) {
    let home = home_dir();
    let colosseum = home.as_ref().map(|home| home.join("colosseum"));

    if args.rooms_dir.is_none() {
        if let Some(dir) = colosseum.as_ref().map(|c| c.join("payloads_all")) {
            if dir.is_dir() {
                args.rooms_dir = Some(dir);
            }
        }
    }
    if args.media_roots.is_empty() {
        // Later roots win on a name collision (see `MediaStore::new`), so the
        // corpus fallbacks come first and the client's own media comes last.
        let mut candidates = Vec::new();
        if let Some(colosseum) = &colosseum {
            candidates.push(colosseum.join("reference/media/media_dl"));
            candidates.push(colosseum.join("http_harvest"));
            candidates.push(colosseum.join("arks"));
        }
        if let Some(home) = &home {
            candidates.push(home.join("media/props"));
            candidates.push(home.join("media/props"));
        }
        candidates.extend(palace_chat_media_roots(home.as_deref()));
        if let Some(home) = &home {
            candidates.push(home.join("media/colosseum-bgs"));
        }
        for candidate in candidates {
            if candidate.is_dir() {
                args.media_roots.push(candidate);
            }
        }
    }
    if args.props_dirs.is_empty() {
        if let Some(colosseum) = &colosseum {
            for candidate in [colosseum.join("props_harvested"), colosseum.join("arks")] {
                if candidate.is_dir() {
                    args.props_dirs.push(candidate);
                }
            }
        }
    }
    if args.roster.is_none() {
        if let Some(roster) = colosseum.as_ref().map(|c| c.join("pserver.prp")) {
            if roster.is_file() {
                args.roster = Some(roster);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temporary directory that deletes itself; the platform branches are
    /// exercised against these synthetic roots, never the real user profile.
    struct TempDir(PathBuf);

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "palace-render-defaults-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unix_media_roots_are_the_xdg_palace_chat_media_dir() {
        assert_eq!(
            unix_palace_chat_media_roots(Some(Path::new("/home/user"))),
            vec![PathBuf::from("/home/user/.local/share/PalaceChat/Media")]
        );
        assert!(unix_palace_chat_media_roots(None).is_empty());
    }

    #[test]
    fn windows_media_roots_scan_both_appdata_roots() {
        let root = TempDir::new("win-media");
        let roaming = root.path().join("roaming");
        let local = root.path().join("local");
        let modern = roaming.join("PalaceChat").join("Media");
        let versioned = local.join("PalaceChat 4").join("Media");
        std::fs::create_dir_all(&modern).expect("create roaming media");
        std::fs::create_dir_all(&versioned).expect("create local media");

        assert_eq!(
            windows_palace_chat_media_roots(Some(roaming), Some(local), None),
            vec![modern, versioned],
            "%APPDATA% must come before %LOCALAPPDATA%"
        );
    }

    #[test]
    fn windows_media_roots_derive_appdata_from_user_profile() {
        let root = TempDir::new("win-profile");
        let profile = root.path().to_path_buf();
        let media = profile
            .join(WINDOWS_APPDATA_SUBDIR)
            .join(WINDOWS_ROAMING_LEAF)
            .join("PalaceChat")
            .join(PALACE_CHAT_MEDIA_DIR);
        std::fs::create_dir_all(&media).expect("create profile media");

        assert_eq!(
            windows_palace_chat_media_roots(None, None, Some(profile)),
            vec![media]
        );
    }

    #[test]
    fn windows_media_roots_are_empty_without_a_palace_chat_directory() {
        let root = TempDir::new("win-media-none");
        assert!(
            windows_palace_chat_media_roots(Some(root.path().join("roaming")), None, None)
                .is_empty()
        );
    }
}

//! `palace-render` — render a Palace room to a PNG, headless.
//!
//! ```text
//! cargo run -p palace-render -- --room 7022 --out renders/7022.png
//! cargo run -p palace-render -- --frame fixtures/logon-run1/frames/0007-server-room.bin --out renders/901.png
//! cargo run -p palace-render -- --room 1672 --avatar 300,200,1041303665 --dpr 2 --out renders/1672-avatars.png
//! ```
//!
//! Everything defaults to the local corpus under `$CORPUS`; see `--help`.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

use std::path::PathBuf;
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

ROOM SOURCE / ASSETS:
    --rooms-dir <DIR>      room payload directory (default $CORPUS/payloads_all)
    --media-root <DIR>     background/overlay root; repeatable
                           (defaults: media_dl, http_harvest, arks)
    --props-dir <DIR>      prop blob directory; repeatable
                           (defaults: props_harvested, arks)
    --roster <FILE>        .prp prop roster (default $CORPUS/pserver.prp if present)

AVATARS:
    --avatar <X>,<Y>[,<PROP_ID>...]   place an avatar (repeatable)
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
    let scene = builder.build_with(&room, &args.avatars, &args.extra_props);
    report(&room, &scene, &args);

    let options = RenderOptions {
        dpr: args.dpr,
        clock: AnimationClock::at(args.clock_ms),
    };
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
    let mut parts = spec.split(',');
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
    Ok(AvatarSpec::new(x, y, props))
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

fn fill_defaults(args: &mut Args) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "$HOME".to_string());
    let colosseum = PathBuf::from(&home).join("colosseum");
    if args.rooms_dir.is_none() {
        let dir = colosseum.join("payloads_all");
        if dir.is_dir() {
            args.rooms_dir = Some(dir);
        }
    }
    if args.media_roots.is_empty() {
        for candidate in [
            colosseum.join("reference/media/media_dl"),
            colosseum.join("http_harvest"),
            colosseum.join("arks"),
            PathBuf::from(&home).join("media/props"),
            PathBuf::from(&home).join("media/props"),
            PathBuf::from(&home).join(".local/share/PalaceChat/Media"),
            PathBuf::from(&home).join("media/colosseum-bgs"),
        ] {
            if candidate.is_dir() {
                args.media_roots.push(candidate);
            }
        }
    }
    if args.props_dirs.is_empty() {
        for candidate in [colosseum.join("props_harvested"), colosseum.join("arks")] {
            if candidate.is_dir() {
                args.props_dirs.push(candidate);
            }
        }
    }
    if args.roster.is_none() {
        let roster = colosseum.join("pserver.prp");
        if roster.is_file() {
            args.roster = Some(roster);
        }
    }
}

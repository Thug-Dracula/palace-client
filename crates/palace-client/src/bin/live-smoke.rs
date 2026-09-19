//! Live smoke test: run the real runtime against a server for a few seconds.
//!
//! Not part of the hermetic suite. Run explicitly:
//!
//! ```text
//! PALACE_USER=Smoke cargo run -p palace-client --bin live-smoke
//! PALACE_SMOKE_SECS=15 PALACE_SMOKE_ROOM=817 cargo run -p palace-client --bin live-smoke
//! PALACE_DEBUG_FRAMES=1 cargo run -p palace-client --bin live-smoke
//! PALACE_SMOKE_SAY=1 cargo run -p palace-client --bin live-smoke
//! ```
//!
//! The harness stays silent in chat by default. With `PALACE_SMOKE_SAY=1` set
//! it sends the single word `test`; every other phase runs either way.

use std::time::{Duration, Instant};

use palace_client::{trace, ClientConfig, ClientEvent, ClientHandle, ClientRuntime};

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn report(event: &ClientEvent) -> bool {
    println!("{}", trace::describe_client_event(event));
    match event {
        ClientEvent::Rooms { rooms } => {
            for room in rooms.iter().take(100) {
                println!("        #{} {:?} users={}", room.id, room.name, room.users);
            }
        }
        ClientEvent::Users { users } => {
            for user in users {
                println!(
                    "        #{} {:?} face={} color={} props={} at ({},{})",
                    user.id,
                    user.name,
                    user.face,
                    user.color,
                    user.props.len(),
                    user.x,
                    user.y
                );
            }
        }
        ClientEvent::Screen { screen } => {
            for note in &screen.notes {
                println!("        note: {note}");
            }
        }
        ClientEvent::Script {
            effects, problems, ..
        } => {
            for effect in effects {
                println!("        effect: {effect}");
            }
            if !problems.is_empty() {
                println!("        problems: {problems:?}");
            }
        }
        ClientEvent::Status { .. }
        | ClientEvent::Banner { .. }
        | ClientEvent::RoomEntered { .. }
        | ClientEvent::Chat { .. }
        | ClientEvent::Note { .. }
        | ClientEvent::Sound { .. }
        | ClientEvent::MidiPlay { .. }
        | ClientEvent::MidiLoop { .. }
        | ClientEvent::MidiStop
        | ClientEvent::Beep
        | ClientEvent::Tooltip { .. } => {}
    }
    matches!(event, ClientEvent::Screen { .. })
}

async fn pump_for(
    stream: &mut palace_client::ClientEventStream,
    handle: &ClientHandle,
    limit: Duration,
    target: i32,
    switched: &mut bool,
    click_room: Option<(f64, f64)>,
    clicked: &mut bool,
) {
    let deadline = tokio::time::Instant::now() + limit;
    let mut self_known = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        match tokio::time::timeout(remaining, stream.recv()).await {
            Ok(Some(event)) => {
                report(&event);
                if let ClientEvent::RoomEntered { room } = &event {
                    if !*switched && target != 0 && room.id != target {
                        println!("[goto] -> #{target}");
                        handle.goto_room(target);
                        *switched = true;
                    }
                }
                if let ClientEvent::Users { users } = &event {
                    self_known |= users.iter().any(|user| user.is_self);
                }
                if let ClientEvent::Screen { screen } = &event {
                    let in_target = target == 0 || screen.room_id == target;
                    if let (Some((rx, ry)), false, true, true) =
                        (click_room, *clicked, in_target, self_known)
                    {
                        let g = &screen.geometry;
                        let vx = g.content_x + rx * g.scale;
                        let vy = g.content_y + ry * g.scale;
                        println!(
                            "--> click room ({rx},{ry}) = viewport ({vx:.1},{vy:.1}) \
                             (content {:.1},{:.1} scale {:.3})",
                            g.content_x, g.content_y, g.scale
                        );
                        handle.click(vx, vy);
                        *clicked = true;
                    }
                }
            }
            Ok(None) | Err(_) => return,
        }
    }
}

async fn sweep_viewports(
    stream: &mut palace_client::ClientEventStream,
    handle: &ClientHandle,
    cases: &[(f64, f64, f64, f64, bool)],
) {
    for (width, height, dpr, zoom, native) in cases.iter().copied() {
        handle.set_viewport(width, height, dpr, zoom, native);
        println!("--> set_viewport {width}x{height} dpr={dpr} zoom={zoom} native={native}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                println!("    (no screen event)");
                break;
            }
            match tokio::time::timeout(remaining, stream.recv()).await {
                Ok(Some(ClientEvent::Screen { screen })) => {
                    let g = &screen.geometry;
                    println!(
                        "    screen v{} room {}x{} vp {:.0}x{:.0} scale {:.4} content {:.1}x{:.1} at ({:.1},{:.1}) buffer {}x{} dpr {} native {}",
                        screen.version,
                        g.room_w,
                        g.room_h,
                        g.viewport_w,
                        g.viewport_h,
                        g.scale,
                        g.content_w,
                        g.content_h,
                        g.content_x,
                        g.content_y,
                        g.bitmap_w,
                        g.bitmap_h,
                        g.dpr,
                        g.native
                    );
                    break;
                }
                Ok(Some(other)) => {
                    let _ = report(&other);
                }
                Ok(None) | Err(_) => break,
            }
        }
    }
}

/// One action in `PALACE_SMOKE_STEPS`, so a live route can be replayed.
enum Step {
    /// `room:<id>`: request a goto and wait for the arrival.
    Goto(i32),
    /// `click:<x>,<y>`: click the next frame in the room coordinate space.
    Click(f64, f64),
    /// `script:<text>`: run a script body through the input path.
    Script(String),
    /// `wait:<secs>`: let the session run before the next step.
    Wait(u64),
}

enum Awaiting {
    Room(i32),
    Screen,
    Delay(tokio::time::Instant),
}

/// Parse `PALACE_SMOKE_STEPS` — `;`-separated `kind:argument` steps.
fn parse_steps() -> Vec<Step> {
    let Ok(spec) = std::env::var("PALACE_SMOKE_STEPS") else {
        return Vec::new();
    };
    spec.split(';').filter_map(parse_step).collect()
}

fn parse_step(raw: &str) -> Option<Step> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (kind, arg) = raw.split_once(':')?;
    match kind.trim() {
        "room" => arg.trim().parse().ok().map(Step::Goto),
        "click" => {
            let (x, y) = arg.split_once(',')?;
            Some(Step::Click(x.trim().parse().ok()?, y.trim().parse().ok()?))
        }
        "script" => Some(Step::Script(arg.to_string())),
        "wait" => arg.trim().parse().ok().map(Step::Wait),
        _ => None,
    }
}

/// Run the steps in order, reporting every event as it arrives.
async fn run_steps(
    stream: &mut palace_client::ClientEventStream,
    handle: &ClientHandle,
    steps: &[Step],
    limit: Duration,
) {
    let overall = tokio::time::Instant::now() + limit;
    let mut room = 0i32;
    let mut click_origin: Option<(f64, f64, f64)> = None;
    let mut index = 0usize;
    let mut awaiting: Option<Awaiting> = None;

    while index < steps.len() {
        if let Some(Awaiting::Delay(deadline)) = awaiting {
            if tokio::time::Instant::now() >= deadline {
                awaiting = None;
            }
        }
        if awaiting.is_none() {
            match &steps[index] {
                Step::Goto(id) => {
                    if room == *id {
                        println!("[step] already in #{id}");
                    } else {
                        println!("[step] goto #{id}");
                        handle.goto_room(*id);
                        awaiting = Some(Awaiting::Room(*id));
                    }
                }
                Step::Click(x, y) => {
                    let Some((content_x, content_y, scale)) = click_origin else {
                        awaiting = Some(Awaiting::Screen);
                        continue;
                    };
                    let vx = content_x + x * scale;
                    let vy = content_y + y * scale;
                    println!("[step] click ({x},{y}) = viewport ({vx:.1},{vy:.1})");
                    handle.click(vx, vy);
                    awaiting = Some(Awaiting::Screen);
                }
                Step::Script(text) => {
                    println!("[step] script {text:?}");
                    handle.run_script(text.clone());
                    awaiting = Some(Awaiting::Screen);
                }
                Step::Wait(secs) => {
                    println!("[step] wait {secs}s");
                    awaiting = Some(Awaiting::Delay(
                        tokio::time::Instant::now() + Duration::from_secs(*secs),
                    ));
                }
            }
            index += 1;
            continue;
        }

        let now = tokio::time::Instant::now();
        let deadline = match awaiting {
            Some(Awaiting::Delay(at)) => at.min(overall),
            _ => overall,
        };
        if now >= deadline {
            if matches!(awaiting, Some(Awaiting::Delay(_))) {
                awaiting = None;
                continue;
            }
            println!("[step] timeout with {} step(s) left", steps.len() - index);
            return;
        }
        match tokio::time::timeout(deadline - now, stream.recv()).await {
            Ok(Some(event)) => {
                report(&event);
                match &event {
                    ClientEvent::RoomEntered { room: entered } => {
                        room = entered.id;
                        click_origin = None;
                        if matches!(awaiting, Some(Awaiting::Room(id)) if id == room) {
                            awaiting = None;
                        }
                    }
                    ClientEvent::Screen { screen } if screen.room_id == room => {
                        let g = &screen.geometry;
                        click_origin = Some((g.content_x, g.content_y, g.scale));
                        if matches!(awaiting, Some(Awaiting::Screen)) {
                            awaiting = None;
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => return,
            Err(_) => {
                if matches!(awaiting, Some(Awaiting::Delay(_))) {
                    awaiting = None;
                } else {
                    println!("[step] timeout with {} step(s) left", steps.len() - index);
                    return;
                }
            }
        }
    }
    println!("[step] all {} step(s) done", steps.len());
}

fn main() {
    let cfg = ClientConfig {
        host: env_or("PALACE_HOST", "localhost"),
        port: env_or("PALACE_PORT", "9998").parse().unwrap_or(9998),
        username: env_or("PALACE_USER", "Smoke"),
        ..ClientConfig::default()
    };

    let seconds: u64 = env_or("PALACE_SMOKE_SECS", "15").parse().unwrap_or(15);
    let target: i32 = env_or("PALACE_SMOKE_ROOM", "0").parse().unwrap_or(0);
    let click_room: Option<(f64, f64)> = env_or("PALACE_CLICK_ROOM", "")
        .split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)));

    println!(
        "live-smoke: {}:{} as {:?}, {seconds}s, goto {}",
        cfg.host, cfg.port, cfg.username, target
    );

    let (handle, mut stream) = ClientRuntime::spawn(cfg);
    if let Ok(spec) = std::env::var("PALACE_VIEWPORT") {
        let parts: Vec<f64> = spec
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .collect();
        if let [width, height, dpr] = parts.as_slice() {
            handle.set_viewport(*width, *height, *dpr, 1.0, false);
            println!("[viewport] {width}x{height} dpr={dpr}");
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut switched = false;
    let mut clicked = false;
    let steps = parse_steps();
    if steps.is_empty() {
        runtime.block_on(pump_for(
            &mut stream,
            &handle,
            Duration::from_secs(seconds),
            target,
            &mut switched,
            click_room,
            &mut clicked,
        ));
    } else {
        runtime.block_on(run_steps(
            &mut stream,
            &handle,
            &steps,
            Duration::from_secs(seconds),
        ));
    }
    runtime.block_on(pump_for(
        &mut stream,
        &handle,
        Duration::from_secs(12),
        target,
        &mut switched,
        click_room,
        &mut clicked,
    ));

    if env_or("PALACE_SMOKE_VIEWPORTS", "0") == "1" {
        println!("[sweep] zoom / 1:1 / resize through the runtime command path");
        runtime.block_on(sweep_viewports(
            &mut stream,
            &handle,
            &[
                (960.0, 540.0, 1.0, 1.0, false),
                (960.0, 540.0, 1.0, 0.5, false),
                (960.0, 540.0, 1.0, 3.0, false),
                (960.0, 540.0, 1.0, 1.0, true),
                (640.0, 480.0, 2.0, 1.0, false),
                (1920.0, 1080.0, 2.0, 2.0, false),
            ],
        ));
    }

    if let Ok(source) = std::env::var("PALACE_RUN_SCRIPT") {
        let before_version = handle.frames().version();
        let before = handle.frames().png();
        println!("--> run_script {source:?} (frame version {before_version})");
        handle.run_script(source);
        runtime.block_on(pump_for(
            &mut stream,
            &handle,
            Duration::from_secs(4),
            target,
            &mut switched,
            None,
            &mut clicked,
        ));
        let after = handle.frames().png();
        println!(
            "[script-frame] version {before_version} -> {}; {} -> {} bytes; frame changed: {}",
            handle.frames().version(),
            before.as_ref().map_or(0, Vec::len),
            after.as_ref().map_or(0, Vec::len),
            before != after
        );
        if let Some(png) = after {
            let path = std::env::temp_dir().join("palace-after-script.png");
            let _ = std::fs::write(&path, png);
            println!("[script-frame] saved {}", path.display());
        }
    }

    if env_or("PALACE_SMOKE_SAY", "0") == "1" {
        handle.say("test");
    }
    std::thread::sleep(Duration::from_millis(1500));
    runtime.block_on(pump_for(
        &mut stream,
        &handle,
        Duration::from_secs(2),
        target,
        &mut switched,
        None,
        &mut clicked,
    ));

    let version = handle.frames().version();
    if let Some(png) = handle.frames().png() {
        let path = std::env::temp_dir().join("palace-smoke.png");
        match std::fs::write(&path, &png) {
            Ok(()) => println!(
                "frame saved: {} ({} bytes, version {version})",
                path.display(),
                png.len()
            ),
            Err(error) => println!("could not save frame: {error}"),
        }
    } else {
        println!("no frame was produced");
    }

    let _ = Instant::now();
    handle.disconnect();
    std::thread::sleep(Duration::from_millis(300));
}

//! Live smoke test: run the real runtime against a server for a few seconds.
//!
//! Not part of the hermetic suite. Run explicitly:
//!
//! ```text
//! PALACE_USER=Smoke cargo run -p palace-client --bin live-smoke
//! PALACE_SMOKE_SECS=15 PALACE_SMOKE_ROOM=817 cargo run -p palace-client --bin live-smoke
//! PALACE_DEBUG_FRAMES=1 cargo run -p palace-client --bin live-smoke
//! ```

use std::time::{Duration, Instant};

use palace_client::{ClientConfig, ClientEvent, ClientHandle, ClientRuntime};

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

fn report(event: &ClientEvent) -> bool {
    match event {
        ClientEvent::Status { status, message } => println!("[status] {status:?} {message:?}"),
        ClientEvent::Banner { banner } => println!(
            "[banner] {} v{:?} media={:?} users={:?}",
            banner.name.as_deref().unwrap_or("?"),
            banner.version,
            banner.media_base,
            banner.total_users
        ),
        ClientEvent::Rooms { rooms } => {
            println!("[rooms] {}", rooms.len());
            for room in rooms.iter().take(100) {
                println!("        #{} {:?} users={}", room.id, room.name, room.users);
            }
        }
        ClientEvent::Users { users } => {
            println!("[users] {}", users.len());
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
        ClientEvent::RoomEntered { room } => {
            println!("[room] #{} {:?} users={}", room.id, room.name, room.users)
        }
        ClientEvent::Chat { line } => {
            println!("[chat:{:?}] {}: {}", line.kind, line.name, line.text)
        }
        ClientEvent::Screen { screen } => {
            println!(
                "[screen] v{} room {} {}x{} buffer {}x{} scale {:.3} dpr {} avatars={} loose={} pending={}",
                screen.version,
                screen.room_id,
                screen.geometry.room_w,
                screen.geometry.room_h,
                screen.geometry.bitmap_w,
                screen.geometry.bitmap_h,
                screen.geometry.scale,
                screen.geometry.dpr,
                screen.avatars,
                screen.loose_props,
                screen.props_pending
            );
            for note in &screen.notes {
                println!("        note: {note}");
            }
        }
        ClientEvent::Script {
            event,
            fired,
            effects,
            problems,
        } => {
            println!("[script] ON {event}: {fired} handler(s) fired");
            for effect in effects {
                println!("        effect: {effect}");
            }
            if !problems.is_empty() {
                println!("        problems: {problems:?}");
            }
        }
        ClientEvent::Note { text } => println!("[note] {text}"),
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
                if let ClientEvent::Screen { screen } = &event {
                    if let (Some((rx, ry)), false) = (click_room, *clicked) {
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
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut switched = false;
    let mut clicked = false;
    runtime.block_on(pump_for(
        &mut stream,
        &handle,
        Duration::from_secs(seconds),
        target,
        &mut switched,
        click_room,
        &mut clicked,
    ));
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

    handle.say("live-smoke ping");
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

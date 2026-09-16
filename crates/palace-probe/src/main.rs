//! `palace-probe` — a headless Palace client.
//!
//! Connects to a pserver, performs the `MSG_TIYID` / `MSG_LOGON` handshake,
//! logs on, prints the decoded room and user lists, and (optionally) writes a
//! replayable fixture corpus to disk.
//!
//! It is deliberately a *probe*: no GUI, no rendering, no scripting. Its job is
//! to prove the protocol layer and to capture the asset all later milestones
//! test against.

mod cli;
mod session;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use palace_wire::byteorder::ByteOrder;
use palace_wire::fixture::Direction;
use palace_wire::frame::Frame;
use palace_wire::messages::{describe_error, Message};
use palace_wire::opcode;

use crate::session::Session;

/// Exit code used for a failed session.
const EXIT_FAILURE: u8 = 1;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            let code = e.exit_code() as u8;
            if code == 0 {
                println!("{}", e.message());
            } else {
                eprintln!("{}", e.message());
            }
            return ExitCode::from(code);
        }
    };

    if args.list_opcodes {
        print_opcode_table();
        return ExitCode::SUCCESS;
    }

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Print the opcode table the README documents.
fn print_opcode_table() {
    println!("{:<14} {:<10} VALUE", "NAME", "MNEMONIC");
    for (value, mnemonic, name) in opcode::TABLE {
        println!("{name:<14} {mnemonic:<10} 0x{value:08x}");
    }
    println!("\n{} opcodes known.", opcode::TABLE.len());
}

/// Everything the probe discovered, gathered for one coherent report.
#[derive(Debug, Default)]
struct Report {
    byte_order: String,
    user_id: i32,
    server_version: Option<String>,
    server_name: Option<String>,
    media_server: Option<String>,
    room_count: usize,
    rooms: Vec<RoomRow>,
    user_count: usize,
    users: Vec<UserRow>,
    room_user_count: usize,
    logon_frame_count: usize,
    opcodes: BTreeMap<String, usize>,
    unknown_opcodes: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct RoomRow {
    id: i32,
    flags: u16,
    users: u16,
    name: String,
}

#[derive(Debug)]
struct UserRow {
    id: i32,
    flags: u16,
    room_id: i16,
    name: String,
}

fn run(args: &cli::Args) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut report = Report::default();
    let mut session = Session::connect(&args.host, args.port)?;

    let handshake = session.handshake()?;
    report.byte_order = handshake.byte_order.label().to_string();
    report.user_id = handshake.user_id();
    session.start_capture(
        format!("{}:{}", args.host, args.port),
        handshake.frame.clone(),
    );

    log(
        args,
        &format!(
            "connected to {}:{} — server speaks {}-endian, assigned user id {}",
            args.host,
            args.port,
            handshake.byte_order.label(),
            handshake.user_id()
        ),
    );

    let record = palace_wire::messages::reference_logon_record(&args.user, args.desired_room);
    log(
        args,
        &format!("sending MSG_LOGON (regi) as {:?}", record.user_name),
    );
    session.send(record.logon_frame(handshake.byte_order))?;

    let burst = session.drain(args.quiet, args.hard)?;
    report.logon_frame_count = burst.len();
    log(args, &format!("logon burst: {} frames", burst.len()));
    let mut saw_room_desc = false;
    for frame in &burst {
        match decode_frame(frame, handshake.byte_order, args.verbose) {
            Some(Message::ServerVersion(v)) => report.server_version = Some(v.to_string()),
            Some(Message::ServerInfo(info)) => report.server_name = Some(info.name.clone()),
            Some(Message::HttpServer(http)) => report.media_server = Some(http.url.clone()),
            Some(Message::RoomDescription(desc)) => {
                saw_room_desc = true;
                log(
                    args,
                    &format!(
                        "  entered room {} {:?} (picture {:?})",
                        desc.header.room_id, desc.name, desc.picture
                    ),
                );
            }
            Some(Message::RoomUsers(users)) => report.room_user_count = users.users.len(),
            Some(_) => {}
            None => {}
        }
    }
    if !saw_room_desc && args.verbose {
        log(args, "  (no room description in burst)");
    }

    report.room_count = fetch_room_list(&mut session, handshake.byte_order, args, &mut report)?;
    report.user_count = fetch_user_list(&mut session, handshake.byte_order, args, &mut report)?;

    log(args, "sending MSG_LOGOFF (bye )");
    session.send(Frame::empty(opcode::LOGOFF, 0))?;

    if let Some(fx) = session.fixture() {
        report.opcodes.clear();
        let mut unknown = BTreeMap::new();
        for cf in &fx.frames {
            *report
                .opcodes
                .entry(cf.frame.opcode.mnemonic())
                .or_insert(0) += 1;
            if !cf.frame.opcode.is_known() {
                *unknown
                    .entry(format!("0x{:08x}", cf.frame.opcode.value()))
                    .or_insert(0) += 1;
            }
        }
        report.unknown_opcodes = unknown;
    }

    if let Some(dir) = &args.capture {
        write_fixture(&session, dir)?;
    }

    print_report(&report, args);
    Ok(())
}

fn fetch_room_list(
    session: &mut Session,
    order: ByteOrder,
    args: &cli::Args,
    report: &mut Report,
) -> std::result::Result<usize, Box<dyn std::error::Error>> {
    log(args, "requesting MSG_LISTOFALLROOMS (rLst)");
    session.send(Frame::empty(opcode::LISTOFALLROOMS, 0))?;
    let response = session.wait_for(args.hard, |f| f.opcode == opcode::LISTOFALLROOMS)?;
    let Some(frame) = response else {
        log(args, "  no room list response (timed out)");
        return Ok(0);
    };
    match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
        Ok(Message::RoomList(list)) => {
            report.rooms = list
                .rooms
                .iter()
                .map(|r| RoomRow {
                    id: r.room_id,
                    flags: r.flags,
                    users: r.user_count,
                    name: r.name.clone(),
                })
                .collect();
            Ok(list.rooms.len())
        }
        Ok(other) => {
            log(
                args,
                &format!("  unexpected room list reply: {}", other.describe()),
            );
            Ok(0)
        }
        Err(err) => {
            log(
                args,
                &format!("  {}", describe_error(opcode::LISTOFALLROOMS, &err)),
            );
            Ok(0)
        }
    }
}

fn fetch_user_list(
    session: &mut Session,
    order: ByteOrder,
    args: &cli::Args,
    report: &mut Report,
) -> std::result::Result<usize, Box<dyn std::error::Error>> {
    log(args, "requesting MSG_LISTOFALLUSERS (uLst)");
    session.send(Frame::empty(opcode::LISTOFALLUSERS, 0))?;
    let response = session.wait_for(args.hard, |f| f.opcode == opcode::LISTOFALLUSERS)?;
    let Some(frame) = response else {
        log(args, "  no user list response (timed out)");
        return Ok(0);
    };
    match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
        Ok(Message::UserList(list)) => {
            report.users = list
                .users
                .iter()
                .map(|u| UserRow {
                    id: u.user_id,
                    flags: u.flags,
                    room_id: u.room_id,
                    name: u.name.clone(),
                })
                .collect();
            Ok(list.users.len())
        }
        Ok(other) => {
            log(
                args,
                &format!("  unexpected user list reply: {}", other.describe()),
            );
            Ok(0)
        }
        Err(err) => {
            log(
                args,
                &format!("  {}", describe_error(opcode::LISTOFALLUSERS, &err)),
            );
            Ok(0)
        }
    }
}

/// Decode a frame, logging unknown opcodes in hex and skipping malformed ones.
fn decode_frame(frame: &Frame, order: ByteOrder, verbose: bool) -> Option<Message> {
    match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
        Ok(Message::Unknown {
            opcode: op,
            ref_num,
            payload_len,
        }) => {
            println!(
                "  [!] unknown opcode 0x{:08x} ({:?}) refNum={} len={} payload[0..32]={}",
                op.value(),
                op.mnemonic(),
                ref_num,
                payload_len,
                hex_preview(&frame.payload, 32)
            );
            None
        }
        Ok(msg) => {
            if verbose {
                println!("  <- {}", msg.describe());
            }
            Some(msg)
        }
        Err(err) => {
            println!("  [!] {}", describe_error(frame.opcode, &err));
            None
        }
    }
}

fn hex_preview(bytes: &[u8], max: usize) -> String {
    let shown = bytes.len().min(max);
    let mut out: String = bytes[..shown].iter().map(|b| format!("{b:02x}")).collect();
    if bytes.len() > shown {
        out.push_str("..");
    }
    out
}

fn write_fixture(
    session: &Session,
    dir: &Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    match session.fixture() {
        Some(fx) => {
            fx.write_to_dir(dir)?;
            let server_frames = fx.server_frames().count();
            let client_frames = fx.client_frames().count();
            println!(
                "fixture: {} frames ({server_frames} server, {client_frames} client) -> {}",
                fx.len(),
                dir.display()
            );
            for cf in &fx.frames {
                let arrow = match cf.direction {
                    Direction::Client => "->",
                    Direction::Server => "<-",
                };
                println!(
                    "  {arrow} {:>2} {:<12} {:?}",
                    cf.seq,
                    cf.frame.opcode.mnemonic(),
                    cf.decoded
                );
            }
        }
        None => println!("fixture: capture was not started (no handshake)"),
    }
    Ok(())
}

fn log(args: &cli::Args, message: &str) {
    if !args.json {
        println!("{message}");
    }
}

fn print_report(report: &Report, args: &cli::Args) {
    if args.json {
        let rooms: Vec<serde_json::Value> = report
            .rooms
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id, "name": r.name, "flags": r.flags, "users": r.users
                })
            })
            .collect();
        let users: Vec<serde_json::Value> = report
            .users
            .iter()
            .map(|u| {
                serde_json::json!({
                    "id": u.id, "name": u.name, "flags": u.flags, "room_id": u.room_id
                })
            })
            .collect();
        let summary = serde_json::json!({
            "byte_order": report.byte_order,
            "user_id": report.user_id,
            "server_version": report.server_version,
            "server_name": report.server_name,
            "media_server": report.media_server,
            "room_count": report.room_count,
            "rooms": rooms,
            "user_count": report.user_count,
            "users": users,
            "room_user_count": report.room_user_count,
            "logon_frame_count": report.logon_frame_count,
            "opcodes": report.opcodes,
            "unknown_opcodes": report.unknown_opcodes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).unwrap_or_default()
        );
        println!(
            "SUMMARY rooms={} users={}",
            report.room_count, report.user_count
        );
        return;
    }

    println!();
    println!("== session ==");
    println!("  byte order   : {}", report.byte_order);
    println!("  user id      : {}", report.user_id);
    println!(
        "  server name  : {}",
        report.server_name.as_deref().unwrap_or("(unknown)")
    );
    println!(
        "  version      : {}",
        report.server_version.as_deref().unwrap_or("(unknown)")
    );
    println!(
        "  media server : {}",
        report.media_server.as_deref().unwrap_or("(none)")
    );
    println!("  logon frames : {}", report.logon_frame_count);

    println!();
    println!("== room list: {} rooms ==", report.room_count);
    let limit = if args.verbose { usize::MAX } else { 40 };
    for row in report.rooms.iter().take(limit) {
        println!(
            "  {:6}  users={:3}  flags=0x{:04x}  {}",
            row.id, row.users, row.flags, row.name
        );
    }
    if report.rooms.len() > limit {
        println!("  ... {} more (use --verbose)", report.rooms.len() - limit);
    }

    println!();
    println!("== user list: {} users ==", report.user_count);
    for row in &report.users {
        println!(
            "  {:6}  room={:6}  flags=0x{:04x}  {}",
            row.id, row.room_id, row.flags, row.name
        );
    }
    println!();
    println!("  users in entry room (rprs): {}", report.room_user_count);

    if !report.opcodes.is_empty() {
        println!();
        println!("== opcodes observed ==");
        for (mnemonic, count) in &report.opcodes {
            println!("  {mnemonic:<6} x{count}");
        }
    }
    if !report.unknown_opcodes.is_empty() {
        println!("== unknown opcodes ==");
        for (value, count) in &report.unknown_opcodes {
            println!("  {value} x{count}");
        }
    }

    println!();
    println!(
        "SUMMARY rooms={} users={}",
        report.room_count, report.user_count
    );
}

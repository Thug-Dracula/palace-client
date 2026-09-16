//! Integration tests for the `palace-probe` binary.
//!
//! Two layers:
//!
//! * **Argument/help/error paths** — the same paths the in-crate `cli.rs` unit
//!   tests cover, but exercised through the real process, so `main`'s dispatch,
//!   the exit codes and the stdout/stderr split are all checked.
//! * **A hermetic loopback session** — a throwaway `TcpListener` on 127.0.0.1
//!   replays a recorded handshake and burst, then answers the room and user list
//!   requests. No external server, no network, no corpus: everything the probe
//!   needs is in this test or in the git-tracked `fixtures/logon-run1` capture.
//!   This is what drives `run`, the report writer and the `Session` itself.
//!
//! The one thing that needs a real server — a live session against a real
//! pserver — is deliberately absent. It is what `cargo run -p palace-probe`
//! without `--host` is for, and it is not a hermetic test.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;
use palace_wire::opcode;
use palace_wire::Opcode;

const BIN: &str = env!("CARGO_BIN_EXE_palace-probe");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("spawn palace-probe")
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

/// A port nothing is listening on: bind an ephemeral listener, note the port and
/// drop it. A connect to it is refused immediately, with no DNS traffic.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

// ---------------------------------------------------------------------------
// Argument parsing, help and error paths (no server involved).
// ---------------------------------------------------------------------------

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let out = run(&["--help"]);
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).is_empty(), "help must not write to stderr");
    let text = stdout(&out);
    assert!(text.contains("palace-probe — headless Palace protocol probe"));
    assert!(text.contains("USAGE:"));
    assert!(text.contains("--list-opcodes"));
    assert!(text.contains("--capture <DIR>"));
}

#[test]
fn short_help_flag_is_the_same_path() {
    let out = run(&["-h"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("USAGE:"));
}

#[test]
fn list_opcodes_prints_the_table_and_exits_zero() {
    let out = run(&["--list-opcodes"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("NAME"));
    assert!(text.contains("MNEMONIC"));
    assert!(text.contains("VALUE"));
    // A known row and the trailing count line.
    assert!(text.contains("ALTLOGONREPLY"), "table row missing:\n{text}");
    assert!(text.contains("0x72657032"), "hex value missing:\n{text}");
    assert!(
        text.contains("opcodes known."),
        "count line missing:\n{text}"
    );
    assert!(stderr(&out).is_empty());
}

#[test]
fn unknown_option_exits_two_and_prints_usage_to_stderr() {
    let out = run(&["--definitely-not-a-flag"]);
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty(), "usage on error belongs on stderr");
    let text = stderr(&out);
    assert!(text.contains("unknown option"));
    assert!(text.contains("USAGE:"));
}

#[test]
fn invalid_port_exits_two_and_names_the_option() {
    let out = run(&["--port", "not-a-number"]);
    assert_eq!(code(&out), 2);
    let text = stderr(&out);
    assert!(text.contains("invalid --port"), "{text}");
    assert!(text.contains("not-a-number"));
}

#[test]
fn a_missing_value_for_a_flag_exits_two() {
    let out = run(&["--host"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--host requires a value"));
}

#[test]
fn a_non_positive_timeout_exits_two() {
    let out = run(&["--timeout", "0"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--timeout must be a positive number"));
}

#[test]
fn an_empty_user_name_is_rejected() {
    let out = run(&["--user="]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--user must not be empty"));
}

#[test]
fn a_refused_connection_is_a_clean_error_with_exit_one() {
    let port = closed_port();
    let out = run(&["--host", "127.0.0.1", "--port", &port.to_string()]);
    assert_eq!(code(&out), 1, "stderr: {}", stderr(&out));
    assert!(stdout(&out).is_empty(), "no report should be printed");
    let text = stderr(&out);
    assert!(text.starts_with("error:"), "unexpected stderr: {text}");
}

// ---------------------------------------------------------------------------
// Hermetic loopback session: a fake server replaying a recorded run.
// ---------------------------------------------------------------------------

fn frame_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logon-run1/frames")
        .canonicalize()
        .expect("fixture frames directory")
}

fn captured(name: &str) -> Vec<u8> {
    std::fs::read(frame_dir().join(name)).unwrap_or_else(|e| panic!("reading {name}: {e}"))
}

fn read_client_frame(stream: &mut TcpStream) -> Option<(u32, i32, Vec<u8>)> {
    let mut header = [0u8; 12];
    stream.read_exact(&mut header).ok()?;
    let op = u32::from_le_bytes(header[0..4].try_into().ok()?);
    let len = u32::from_le_bytes(header[4..8].try_into().ok()?) as usize;
    let ref_num = i32::from_le_bytes(header[8..12].try_into().ok()?);
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    Some((op, ref_num, payload))
}

#[derive(Clone, Copy)]
enum Lists {
    Captured,
    Silent,
    Malformed,
}

#[derive(Clone, Copy)]
struct ServerPlan {
    include_room: bool,
    lists: Lists,
    huge_length: bool,
    delay_lists: bool,
}

impl ServerPlan {
    fn normal(include_room: bool) -> Self {
        ServerPlan {
            include_room,
            lists: Lists::Captured,
            huge_length: false,
            delay_lists: false,
        }
    }

    fn silent_lists() -> Self {
        ServerPlan {
            lists: Lists::Silent,
            ..ServerPlan::normal(true)
        }
    }

    fn malformed_lists() -> Self {
        ServerPlan {
            lists: Lists::Malformed,
            ..ServerPlan::normal(true)
        }
    }

    fn huge_length() -> Self {
        ServerPlan {
            huge_length: true,
            ..ServerPlan::normal(true)
        }
    }

    fn delayed_lists() -> Self {
        ServerPlan {
            delay_lists: true,
            ..ServerPlan::normal(true)
        }
    }
}

/// Run the fake server on its own thread and return the port it listens on.
///
/// The plan chooses what the "server" does, so the probe's tolerance of a
/// missing room description, a slow reply, a truncated list body and an
/// impossible frame length can each be driven from an offline test.
fn spawn_fake_server(plan: ServerPlan) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().expect("fake server addr").port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let order = ByteOrder::Little;

        // The banner: a zero-length MSG_TIYID carrying the assigned user id.
        let handshake = Frame::new(opcode::TIYID, 4242, Vec::new())
            .encode(order)
            .expect("encode handshake");
        if stream.write_all(&handshake).is_err() {
            return;
        }

        if plan.huge_length {
            // A header whose length field is impossible: the probe must refuse
            // it rather than attempt to allocate. Keep the socket open until the
            // client goes away so the write itself never races the error.
            let mut bogus = Vec::new();
            bogus.extend_from_slice(&opcode::PING.value().to_le_bytes());
            bogus.extend_from_slice(&u32::MAX.to_le_bytes());
            bogus.extend_from_slice(&0i32.to_le_bytes());
            let _ = stream.write_all(&bogus);
            while read_client_frame(&mut stream).is_some() {}
            return;
        }

        while let Some((op, _ref_num, _payload)) = read_client_frame(&mut stream) {
            if op == opcode::LOGON.value() {
                let mut burst = Vec::new();
                for name in [
                    "0002-server-vers.bin",
                    "0003-server-sinf.bin",
                    "0006-server-HTTP.bin",
                ] {
                    burst.extend_from_slice(&captured(name));
                }
                if plan.include_room {
                    for name in [
                        "0007-server-room.bin",
                        "0008-server-rprs.bin",
                        "0009-server-endr.bin",
                    ] {
                        burst.extend_from_slice(&captured(name));
                    }
                }
                // An unknown opcode with a payload longer than the hex preview
                // (which truncates), a keepalive the probe answers with a pong,
                // and a known opcode with a truncated body that must be logged
                // and skipped.
                burst.extend_from_slice(
                    &Frame::new(Opcode::new(0xdead_beef), 0, vec![0xab; 40])
                        .encode(order)
                        .expect("encode unknown"),
                );
                burst.extend_from_slice(
                    &Frame::empty(opcode::PING, 7)
                        .encode(order)
                        .expect("encode ping"),
                );
                burst.extend_from_slice(
                    &Frame::new(opcode::ROOMDESC, 0, vec![1, 2, 3])
                        .encode(order)
                        .expect("encode truncated room"),
                );
                if stream.write_all(&burst).is_err() {
                    return;
                }
            } else if op == opcode::LISTOFALLROOMS.value() {
                if plan.delay_lists {
                    std::thread::sleep(Duration::from_millis(700));
                }
                match plan.lists {
                    Lists::Silent => {}
                    Lists::Malformed => {
                        // refNum says "1 room" but the body is truncated, so the
                        // decoder must fail cleanly.
                        let frame = Frame::new(opcode::LISTOFALLROOMS, 1, vec![1, 2, 3]);
                        if stream.write_all(&frame.encode(order).unwrap()).is_err() {
                            return;
                        }
                    }
                    Lists::Captured => {
                        if stream.write_all(&captured("0012-server-rLst.bin")).is_err() {
                            return;
                        }
                    }
                }
            } else if op == opcode::LISTOFALLUSERS.value() {
                if plan.delay_lists {
                    std::thread::sleep(Duration::from_millis(700));
                }
                match plan.lists {
                    Lists::Silent => {}
                    Lists::Malformed => {
                        let frame = Frame::new(opcode::LISTOFALLUSERS, 1, vec![1, 2, 3]);
                        if stream.write_all(&frame.encode(order).unwrap()).is_err() {
                            return;
                        }
                    }
                    Lists::Captured => {
                        if stream.write_all(&captured("0014-server-uLst.bin")).is_err() {
                            return;
                        }
                    }
                }
            } else if op == opcode::LOGOFF.value() {
                break;
            }
        }
    });
    port
}

fn session_args(port: u16) -> Vec<String> {
    vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        // Keep the burst short and deterministic.
        "--quiet".into(),
        "0.2".into(),
        "--hard".into(),
        "1".into(),
    ]
}

fn args_with_hard(port: u16, hard: &str) -> Vec<String> {
    let mut args = session_args(port);
    let at = args
        .iter()
        .position(|a| a == "--hard")
        .expect("--hard is present");
    args[at + 1] = hard.to_string();
    args
}

#[test]
fn loopback_session_prints_a_prose_report() {
    let port = spawn_fake_server(ServerPlan::normal(true));
    let args = session_args(port);
    let out = Command::new(BIN).args(&args).output().expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));

    let text = stdout(&out);
    assert!(
        text.contains(&format!("connected to 127.0.0.1:{port}")),
        "{text}"
    );
    assert!(text.contains("server speaks little-endian"), "{text}");
    assert!(text.contains("assigned user id 4242"), "{text}");

    // The recorded banner/burst carried a version, a server name and a media URL.
    assert!(text.contains("== session =="), "{text}");
    assert!(text.contains("version      : 1.22"), "{text}");
    assert!(text.contains("server name  : Balamb Garden"), "{text}");
    assert!(
        text.contains("media server : https://media.palace.example.info/palace/media"),
        "{text}"
    );
    // 9 frames in the burst: vers, sinf, HTTP, room, rprs, endr, unknown, ping,
    // truncated room.
    assert!(text.contains("logon frames : 9"), "{text}");
    assert!(text.contains("users in entry room (rprs): 1"), "{text}");

    // The room list is answered from the real 81-room capture and truncated in
    // the default (non-verbose) report.
    assert!(text.contains("== room list: 81 rooms =="), "{text}");
    assert!(text.contains("Balamb Hotel"), "{text}");
    assert!(text.contains("41 more (use --verbose)"), "{text}");
    assert!(text.contains("== user list: 2 users =="), "{text}");
    assert!(text.contains("SUMMARY rooms=81 users=2"), "{text}");

    // An unknown opcode is reported, with its over-long payload hex preview
    // truncated, as is the truncated known one.
    assert!(text.contains("unknown opcode 0xdeadbeef"), "{text}");
    assert!(text.contains("len=40"), "{text}");
    assert!(text.contains("payload[0..32]=abab"), "{text}");
    assert!(text.contains(".."), "{text}");
    assert!(
        text.matches("[!]").count() >= 2,
        "expected unknown + malformed notes:\n{text}"
    );
    assert!(text.contains("== unknown opcodes =="), "{text}");
    assert!(text.contains("0xdeadbeef x1"), "{text}");
}

#[test]
fn verbose_session_lists_every_room_and_flags_a_missing_room_description() {
    let port = spawn_fake_server(ServerPlan::normal(false));
    let mut args = session_args(port);
    args.push("--verbose".into());
    let out = Command::new(BIN).args(&args).output().expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));

    let text = stdout(&out);
    assert!(text.contains("(no room description in burst)"), "{text}");
    assert!(text.contains("== room list: 81 rooms =="), "{text}");
    assert!(
        !text.contains("more (use --verbose)"),
        "verbose report must not truncate:\n{text}"
    );
    // Every decoded frame is echoed when verbose.
    assert!(text.contains("<- server version 1.22"), "{text}");
}

#[test]
fn json_session_emits_a_machine_readable_summary() {
    let port = spawn_fake_server(ServerPlan::normal(true));
    let mut args = session_args(port);
    args.push("--json".into());
    let out = Command::new(BIN).args(&args).output().expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));

    let text = stdout(&out);
    assert!(text.contains("\"byte_order\": \"little\""), "{text}");
    assert!(text.contains("\"user_id\": 4242"), "{text}");
    assert!(
        text.contains("\"server_name\": \"Balamb Garden\""),
        "{text}"
    );
    assert!(text.contains("\"server_version\": \"1.22\""), "{text}");
    assert!(text.contains("\"room_count\": 81"), "{text}");
    assert!(text.contains("\"user_count\": 2"), "{text}");
    assert!(text.contains("\"room_user_count\": 1"), "{text}");
    assert!(text.contains("\"0xdeadbeef\": 1"), "{text}");
    // The one-line trailer is printed for both formats.
    assert!(text.contains("SUMMARY rooms=81 users=2"), "{text}");
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "palace-probe-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn capture_writes_a_replayable_fixture_directory() {
    let port = spawn_fake_server(ServerPlan::normal(true));
    let dir = TempDir::new("capture");
    let mut args = session_args(port);
    args.push("--capture".into());
    args.push(dir.0.display().to_string());
    let out = Command::new(BIN).args(&args).output().expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));

    let text = stdout(&out);
    assert!(text.contains("fixture:"), "{text}");
    assert!(text.contains("frames"), "{text}");

    let manifest = dir.0.join("manifest.json");
    assert!(manifest.is_file(), "fixture manifest was not written");
    let manifest_text = std::fs::read_to_string(&manifest).expect("read manifest");
    assert!(manifest_text.contains("\"byte_order\": \"little\""));
    assert!(
        manifest_text.contains("\"server\": \"127.0.0.1:"),
        "{manifest_text}"
    );

    let frames = dir.0.join("frames");
    let count = std::fs::read_dir(&frames)
        .expect("frames dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("bin"))
        .count();
    // Handshake + 9 server frames + logon + rLst + uLst + bye, at minimum.
    assert!(count >= 10, "only {count} frames captured");
}

#[test]
fn unanswered_list_requests_are_reported_as_timeouts() {
    let port = spawn_fake_server(ServerPlan::silent_lists());
    let out = Command::new(BIN)
        .args(args_with_hard(port, "0.3"))
        .output()
        .expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("no room list response (timed out)"), "{text}");
    assert!(text.contains("no user list response (timed out)"), "{text}");
    assert!(text.contains("SUMMARY rooms=0 users=0"), "{text}");
}

#[test]
fn malformed_list_bodies_are_logged_without_aborting_the_run() {
    let port = spawn_fake_server(ServerPlan::malformed_lists());
    let out = Command::new(BIN)
        .args(session_args(port))
        .output()
        .expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("failed to decode rLst(LISTOFALLROOMS)"),
        "{text}"
    );
    assert!(
        text.contains("failed to decode uLst(LISTOFALLUSERS)"),
        "{text}"
    );
    assert!(text.contains("SUMMARY rooms=0 users=0"), "{text}");
}

#[test]
fn a_slow_list_reply_is_still_awaited() {
    let port = spawn_fake_server(ServerPlan::delayed_lists());
    let out = Command::new(BIN)
        .args(args_with_hard(port, "3"))
        .output()
        .expect("run probe");
    assert_eq!(code(&out), 0, "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).contains("SUMMARY rooms=81 users=2"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn an_impossible_frame_length_is_a_clean_error() {
    let port = spawn_fake_server(ServerPlan::huge_length());
    let out = Command::new(BIN)
        .args(session_args(port))
        .output()
        .expect("run probe");
    assert_eq!(code(&out), 1, "stdout:\n{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.starts_with("error:"), "{err}");
    assert!(err.contains("implausible frame payload length"), "{err}");
}

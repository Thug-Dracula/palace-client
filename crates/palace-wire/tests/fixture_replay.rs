//! Offline fixture replay: decode a captured session with no server involved,
//! in both byte orders, and assert the decoded structures.
//!
//! This is the test the later milestones lean on. If it passes, `palace-wire`
//! can decode real traffic; if it fails, nothing above it matters.

use std::path::{Path, PathBuf};

use palace_wire::byteorder::{ByteOrder, Writer};
use palace_wire::fixture::{Direction, Fixture, FIXTURE_FORMAT};
use palace_wire::messages::{reference_logon_record, Message};
use palace_wire::opcode::{self, Opcode};

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("palace-wire-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn real_capture_replays_and_decodes_offline() {
    let dir = fixture_dir("logon-run1");
    let fixture = Fixture::load(&dir).expect("fixture must load");
    fixture.verify().expect("fixture must be self-consistent");

    assert_eq!(fixture.format, FIXTURE_FORMAT);
    assert_eq!(fixture.byte_order, ByteOrder::Little);
    assert!(!fixture.frames.is_empty());

    let first = &fixture.frames[0];
    assert_eq!(first.direction, Direction::Server);
    assert_eq!(first.frame.opcode, opcode::TIYID);
    let user_id = first.frame.ref_num;
    assert!(user_id > 0, "server must assign a positive user id");

    // The client's logon must be present and decode to the reference record.
    let logon = fixture
        .client_frames()
        .find(|f| f.frame.opcode == opcode::LOGON)
        .expect("client logon frame");
    let decoded = Message::decode(
        logon.frame.opcode,
        logon.frame.ref_num,
        &logon.frame.payload,
        fixture.byte_order,
    )
    .expect("logon must decode");
    match decoded {
        Message::Logon(rec) => {
            assert_eq!(logon.frame.payload.len(), 128);
            assert_eq!(&rec.reserved, b"SCOUT1");
            assert!(!rec.user_name.is_empty());
        }
        other => panic!("expected a logon record, got {other:?}"),
    }

    // The logon burst must contain the messages the protocol promises.
    let burst_opcodes: Vec<Opcode> = fixture.server_frames().map(|f| f.frame.opcode).collect();
    for required in [
        opcode::VERSION,
        opcode::SERVERINFO,
        opcode::USERLOG,
        opcode::HTTPSERVER,
        opcode::ROOMDESC,
        opcode::ROOMDESCEND,
        opcode::USERNEW,
    ] {
        assert!(
            burst_opcodes.contains(&required),
            "logon burst is missing {}",
            required.describe()
        );
    }
}

#[test]
fn room_list_decodes_with_a_matching_count() {
    let fixture = Fixture::load(&fixture_dir("logon-run1")).unwrap();
    let frame = fixture
        .server_frames()
        .map(|f| &f.frame)
        .find(|f| f.opcode == opcode::LISTOFALLROOMS)
        .expect("room list frame");
    let list = match Message::decode(
        frame.opcode,
        frame.ref_num,
        &frame.payload,
        fixture.byte_order,
    )
    .unwrap()
    {
        Message::RoomList(list) => list,
        other => panic!("expected a room list, got {other:?}"),
    };
    assert_eq!(list.rooms.len(), frame.ref_num as usize);
    assert!(!list.rooms.is_empty(), "the target server always has rooms");
    assert!(
        list.rooms.iter().all(|r| !r.name.is_empty()),
        "every room must have a name"
    );
    assert!(
        list.rooms.iter().any(|r| r.name.contains("Balamb")),
        "the entry room family must be present"
    );
}

#[test]
fn user_list_decodes_with_a_matching_count() {
    let fixture = Fixture::load(&fixture_dir("logon-run1")).unwrap();
    let frame = fixture
        .server_frames()
        .map(|f| &f.frame)
        .find(|f| f.opcode == opcode::LISTOFALLUSERS)
        .expect("user list frame");
    let list = match Message::decode(
        frame.opcode,
        frame.ref_num,
        &frame.payload,
        fixture.byte_order,
    )
    .unwrap()
    {
        Message::UserList(list) => list,
        other => panic!("expected a user list, got {other:?}"),
    };
    assert_eq!(list.users.len(), frame.ref_num as usize);
    assert!(!list.users.is_empty(), "we are logged on, so a user exists");
    assert!(list.users.iter().any(|u| u.room_id > 0));
}

#[test]
fn room_description_decodes_the_entry_room() {
    let fixture = Fixture::load(&fixture_dir("logon-run1")).unwrap();
    let frame = fixture
        .server_frames()
        .map(|f| &f.frame)
        .find(|f| f.opcode == opcode::ROOMDESC)
        .expect("room description frame");
    let desc = match Message::decode(
        frame.opcode,
        frame.ref_num,
        &frame.payload,
        fixture.byte_order,
    )
    .unwrap()
    {
        Message::RoomDescription(desc) => desc,
        other => panic!("expected a room description, got {other:?}"),
    };
    assert_eq!(desc.name, "Balamb Garden");
    assert!(!desc.picture.is_empty());
    assert!(desc.header.room_id > 0);
    assert_eq!(desc.var_data.len(), desc.header.len_vars as usize);
}

#[test]
fn a_synthetic_session_decodes_identically_in_both_orders() {
    // A real big-endian pserver does not exist for us to capture from, so the
    // big-endian path is exercised with a synthetic session built under each
    // order. Frame *values* must be identical; only the bytes differ.
    let mut little = None;
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let fixture = synthetic_session(order);
        fixture.verify().expect("synthetic session must verify");

        let dir = temp_dir(&format!("synthetic-{}", order.label()));
        fixture.write_to_dir(&dir).unwrap();
        let reloaded = Fixture::load(&dir).unwrap();
        assert_eq!(reloaded.byte_order, order);
        reloaded.verify().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        let snapshot = snapshot(&reloaded);
        match little.take() {
            None => little = Some(snapshot),
            Some(reference) => assert_eq!(reference, snapshot),
        }
    }
}

fn synthetic_session(order: ByteOrder) -> Fixture {
    let mut fixture = Fixture::new("synthetic:9998", order);
    fixture.push(
        Direction::Server,
        palace_wire::Frame::empty(opcode::TIYID, 42),
    );
    fixture.push(
        Direction::Client,
        reference_logon_record("probe", 0).logon_frame(order),
    );

    let mut w = Writer::new(order);
    w.write_u32(0x0000_2e7f);
    w.write_pstring("Balamb Garden");
    fixture.push(
        Direction::Server,
        palace_wire::Frame::new(opcode::SERVERINFO, 42, w.into_vec()),
    );

    let mut w = Writer::new(order);
    w.write_i32(186);
    w.write_u16(0x0200);
    w.write_u16(2);
    w.write_pstring_aligned("Entrance");
    w.write_i32(901);
    w.write_u16(0x0114);
    w.write_u16(2);
    w.write_pstring_aligned("Balamb Garden");
    fixture.push(
        Direction::Server,
        palace_wire::Frame::new(opcode::LISTOFALLROOMS, 2, w.into_vec()),
    );

    let mut w = Writer::new(order);
    w.write_i32(12);
    w.write_u16(0);
    w.write_i16(901);
    w.write_pstring_aligned("probe");
    fixture.push(
        Direction::Server,
        palace_wire::Frame::new(opcode::LISTOFALLUSERS, 1, w.into_vec()),
    );

    fixture.push(
        Direction::Server,
        palace_wire::Frame::empty(opcode::PING, 5),
    );
    fixture.push(
        Direction::Client,
        palace_wire::Frame::empty(opcode::PONG, 5),
    );
    fixture.push(
        Direction::Client,
        palace_wire::Frame::empty(opcode::LOGOFF, 0),
    );
    fixture
}

/// The order-independent view of a fixture: opcode, refNum and decoded text.
fn snapshot(fixture: &Fixture) -> Vec<(Opcode, i32, String)> {
    fixture
        .frames
        .iter()
        .map(|cf| (cf.frame.opcode, cf.frame.ref_num, cf.decoded.clone()))
        .collect()
}

#[test]
fn unknown_opcodes_in_a_fixture_do_not_abort_replay() {
    // A fixture with an opcode outside the table must still replay: tolerant
    // parsing is a requirement, not a nicety.
    let mut fixture = Fixture::new("synthetic:0", ByteOrder::Little);
    fixture.push(
        Direction::Server,
        palace_wire::Frame::empty(opcode::TIYID, 1),
    );
    fixture.push(
        Direction::Server,
        palace_wire::Frame::new(Opcode::new(0x0bad_f00d), 7, vec![1, 2, 3, 4]),
    );
    assert!(fixture.verify().is_ok());

    let dir = temp_dir("unknown");
    fixture.write_to_dir(&dir).unwrap();
    let reloaded = Fixture::load(&dir).unwrap();
    assert_eq!(reloaded.frames.len(), 2);
    assert!(!reloaded.frames[1].frame.opcode.is_known());
    assert!(reloaded.frames[1].decoded.contains("unknown"));
    std::fs::remove_dir_all(&dir).unwrap();
}

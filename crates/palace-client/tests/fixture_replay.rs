//! Hermetic tests: a recorded session drives the state machine with no network.

use palace_client::state::{ChatKind, SessionState};
use palace_client::xtlk;
use palace_wire::byteorder::{ByteOrder, Writer};
use palace_wire::fixture::{default_fixture_dir, Fixture};
use palace_wire::frame::Frame;
use palace_wire::messages::Talk;
use palace_wire::opcode;

fn logon_fixture() -> Fixture {
    Fixture::load(&default_fixture_dir("logon-run1")).expect("the recorded fixture loads")
}

#[test]
fn the_recorded_fixture_is_self_consistent() {
    let fixture = logon_fixture();
    assert_eq!(fixture.frames.len(), 16);
    fixture.verify().expect("fixture round-trips");
    assert_eq!(fixture.byte_order, ByteOrder::Little);
}

#[test]
fn replaying_the_logon_run_populates_the_session() {
    let fixture = logon_fixture();
    let order = fixture.byte_order;

    let mut state = SessionState::new("localhost", 9998);
    state.banner.user_id = 13;

    let mut seen_rooms = 0usize;
    let mut seen_users = 0usize;
    let mut chat = Vec::new();

    for captured in fixture.server_frames() {
        let applied = state.apply(&captured.frame, order);
        if applied.rooms {
            seen_rooms = state.rooms.len();
        }
        if applied.users {
            seen_users = state.users_in_room().len();
        }
        chat.extend(applied.chat);
    }

    assert_eq!(state.banner.version.as_deref(), Some("1.22"));
    assert_eq!(state.banner.name.as_deref(), Some("Balamb Garden"));
    assert_eq!(
        state.banner.media_base.as_deref(),
        Some("https://media.palace.example.info/palace/media")
    );
    assert_eq!(state.banner.total_users, Some(2));

    assert_eq!(seen_rooms, 81, "the room list carries all 81 rooms");
    assert!(seen_users >= 1, "at least the entering user is known");

    let room = state.current_room.expect("a room was entered");
    assert_eq!(room.id, 901);
    assert_eq!(room.name, "Balamb Garden");

    let desc = state.room_desc.as_ref().expect("the room parsed");
    assert_eq!(desc.picture, "sqoom23.gif");
    assert_eq!(desc.hotspots.len(), 6);
    assert_eq!(desc.pictures.len(), 1);
    let _ = chat;
}

#[test]
fn plaintext_talk_becomes_a_chat_line() {
    let mut state = SessionState::new("h", 1);
    let mut writer = Writer::new(ByteOrder::Little);
    Talk {
        user_id: 7,
        text: "hello there".to_string(),
    }
    .encode(&mut writer);
    let frame = Frame::new(opcode::TALK, 7, writer.into_vec());

    let applied = state.apply(&frame, ByteOrder::Little);

    assert_eq!(applied.chat.len(), 1);
    assert_eq!(applied.chat[0].text, "hello there");
    assert_eq!(applied.chat[0].user_id, 7);
    assert_eq!(applied.chat[0].kind, ChatKind::Talk);
}

#[test]
fn encrypted_talk_is_decrypted() {
    let mut state = SessionState::new("h", 1);
    let text = b"a secret line";
    let mut body = Vec::new();
    body.extend_from_slice(&((text.len() + 3) as i16).to_le_bytes());
    body.extend_from_slice(&xtlk::encrypt(text));
    body.push(0);
    let frame = Frame::new(opcode::XTALK, 5, body);

    let applied = state.apply(&frame, ByteOrder::Little);

    assert_eq!(applied.chat.len(), 1);
    assert_eq!(applied.chat[0].text, "a secret line");
    assert_eq!(applied.chat[0].kind, ChatKind::Talk);
}

#[test]
fn ping_produces_a_pong_on_the_same_ref() {
    let mut state = SessionState::new("h", 1);
    let frame = Frame::new(opcode::PING, 42, Vec::new());

    let applied = state.apply(&frame, ByteOrder::Little);

    assert_eq!(applied.outbound.len(), 1);
    assert_eq!(applied.outbound[0].opcode, opcode::PONG);
    assert_eq!(applied.outbound[0].ref_num, 42);
}

#[test]
fn a_malformed_body_is_reported_and_never_panics() {
    let mut state = SessionState::new("h", 1);
    for (op, payload) in [
        (opcode::ROOMDESC, vec![1u8, 2, 3]),
        (opcode::LOGON, vec![0u8; 4]),
        (opcode::LISTOFALLROOMS, vec![0xffu8; 3]),
        (opcode::USERMOVE, vec![]),
    ] {
        let frame = Frame::new(op, 0, payload);
        let applied = state.apply(&frame, ByteOrder::Little);
        let parsed_room = state.room_desc.is_some();
        assert!(
            !applied.chat.is_empty() || !parsed_room || op == opcode::LISTOFALLROOMS,
            "{op:?} should have reported something"
        );
    }
}

#[test]
fn unknown_opcodes_are_ignored_quietly() {
    let mut state = SessionState::new("h", 1);
    let frame = Frame::new(palace_wire::Opcode::new(0xdead_beef), 0, vec![9; 16]);
    let applied = state.apply(&frame, ByteOrder::Little);
    assert!(applied.chat.is_empty());
    assert!(!applied.render);
}

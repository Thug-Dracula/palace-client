//! Byte-exact tests for the effect → protocol encoders.
//!
//! Layouts are the ones OpenPalace's `PalaceClient.as` senders write, so a
//! change here must be justified against that source.

use palace_host::{effect_frame, Effect, PenState, WireContext};
use palace_wire::byteorder::ByteOrder;
use palace_wire::opcode;

fn ctx() -> WireContext {
    WireContext {
        byte_order: ByteOrder::Little,
        user_id: 13,
        room_id: 901,
        room_width: 512,
        room_height: 384,
        self_pos: (200, 100),
        pen: PenState {
            pos: (10, 20),
            rgb: (255, 0, 0),
            size: 1,
            front: false,
        },
    }
}

#[test]
fn say_encodes_as_a_talk_with_our_ref() {
    let frame = effect_frame(
        &Effect::Say {
            text: "hi".to_string(),
        },
        &ctx(),
    )
    .expect("talk frame");
    assert_eq!(frame.opcode, opcode::TALK);
    assert_eq!(frame.ref_num, 13);
    assert_eq!(frame.payload, b"hi\0");
}

#[test]
fn private_message_encodes_target_then_text() {
    let frame = effect_frame(
        &Effect::PrivateMessage {
            user: 7,
            text: "psst".to_string(),
        },
        &ctx(),
    )
    .expect("whisper frame");
    assert_eq!(frame.opcode, opcode::WHISPER);
    assert_eq!(frame.ref_num, 13);
    let mut want = Vec::new();
    want.extend_from_slice(&7i32.to_le_bytes());
    want.extend_from_slice(b"psst\0");
    assert_eq!(frame.payload, want);
}

#[test]
fn goto_room_is_the_navr_frame_the_server_expects() {
    let frame = effect_frame(&Effect::GotoRoom { room: 817 }, &ctx()).expect("navR");
    assert_eq!(frame.opcode, opcode::ROOMGOTO);
    assert_eq!(frame.ref_num, 13);
    assert_eq!(frame.payload, 817u16.to_le_bytes(), "2-byte payload");
}

#[test]
fn move_clamps_to_the_room_and_writes_y_then_x() {
    let frame = effect_frame(&Effect::MoveUserAbs { x: 600, y: 400 }, &ctx()).expect("uLoc");
    assert_eq!(frame.opcode, opcode::USERMOVE);
    assert_eq!(frame.ref_num, 13);
    assert_eq!(
        frame.payload,
        [106u8, 1, 234, 1],
        "y = 362 (384-22), x = 490 (512-22)"
    );
}

#[test]
fn move_clamps_low_to_22() {
    let frame = effect_frame(&Effect::MoveUserAbs { x: 0, y: 5 }, &ctx()).expect("uLoc");
    assert_eq!(frame.payload, [22u8, 0, 22, 0]);
}

#[test]
fn relative_move_uses_our_position() {
    let frame = effect_frame(&Effect::MoveUserRel { dx: 10, dy: -10 }, &ctx()).expect("uLoc");
    assert_eq!(frame.payload, [90u8, 0, 210, 0], "y=90 x=210");
}

#[test]
fn spot_state_carries_room_spot_and_state() {
    let frame = effect_frame(&Effect::SetSpotState { spot: 2, state: 1 }, &ctx()).expect("sSta");
    assert_eq!(frame.opcode, opcode::SPOTSTATE);
    assert_eq!(frame.ref_num, 0);
    assert_eq!(frame.payload, [0x85, 0x03, 2, 0, 1, 0]);
}

#[test]
fn door_lock_and_unlock_carry_room_and_spot() {
    let lock = effect_frame(&Effect::Lock { spot: 4 }, &ctx()).expect("lock");
    assert_eq!(lock.opcode, opcode::DOORLOCK);
    assert_eq!(lock.payload, [0x85, 0x03, 4, 0]);
    let unlock = effect_frame(&Effect::Unlock { spot: 4 }, &ctx()).expect("unlock");
    assert_eq!(unlock.opcode, opcode::DOORUNLOCK);
    assert_eq!(unlock.payload, [0x85, 0x03, 4, 0]);
}

#[test]
fn set_props_writes_count_then_id_crc_pairs() {
    let frame = effect_frame(&Effect::SetProps { props: vec![7, 9] }, &ctx()).expect("usrP");
    assert_eq!(frame.opcode, opcode::USERPROP);
    assert_eq!(frame.ref_num, 13);
    let mut want = Vec::new();
    want.extend_from_slice(&2i32.to_le_bytes());
    for id in [7i32, 9] {
        want.extend_from_slice(&id.to_le_bytes());
        want.extend_from_slice(&0u32.to_le_bytes());
    }
    assert_eq!(frame.payload, want);
}

#[test]
fn a_loose_prop_carries_id_crc_y_x() {
    let frame = effect_frame(
        &Effect::AddLooseProp {
            prop: 0x4000_0001,
            x: 10,
            y: 20,
        },
        &ctx(),
    )
    .expect("nPrp");
    assert_eq!(frame.opcode, opcode::PROPNEW);
    assert_eq!(
        frame.payload,
        [0x01, 0x00, 0x00, 0x40, 0, 0, 0, 0, 20, 0, 10, 0]
    );
}

#[test]
fn removing_a_loose_prop_writes_its_index() {
    let frame = effect_frame(&Effect::RemoveLooseProp { index: 3 }, &ctx()).expect("dPrp");
    assert_eq!(frame.opcode, opcode::PROPDEL);
    assert_eq!(frame.payload, 3i32.to_le_bytes());
}

#[test]
fn moving_a_loose_prop_writes_index_y_x() {
    let frame = effect_frame(
        &Effect::MoveLooseProp {
            index: 3,
            x: 10,
            y: 20,
        },
        &ctx(),
    )
    .expect("mPrp");
    assert_eq!(frame.opcode, opcode::PROPMOVE);
    assert_eq!(frame.payload, [3, 0, 0, 0, 20, 0, 10, 0]);
}

#[test]
fn a_line_is_a_path_draw_record() {
    let frame = effect_frame(
        &Effect::DrawLine {
            x1: 1,
            y1: 2,
            x2: 3,
            y2: 4,
        },
        &ctx(),
    )
    .expect("draw");
    assert_eq!(frame.opcode, opcode::DRAW);
    assert_eq!(frame.ref_num, 0);
    let body = &frame.payload;
    assert_eq!(body.len(), 10 + 2 + 2 + 6 + 8);
    assert_eq!(&body[0..2], &[0, 0], "next offset");
    assert_eq!(&body[2..4], &[0, 0], "reserved");
    assert_eq!(
        u16::from_le_bytes([body[4], body[5]]),
        0,
        "command DC_PATH | flags back layer"
    );
    assert_eq!(u16::from_le_bytes([body[6], body[7]]), 18, "operand length");
    assert_eq!(&body[8..10], &[0, 0], "data offset");
    assert_eq!(&body[10..12], &[1, 0], "pen size");
    assert_eq!(&body[12..14], &[1, 0], "one segment");
    assert_eq!(&body[14..20], &[0xFF, 0xFF, 0, 0, 0, 0], "r,g,b doubled");
    assert_eq!(&body[20..24], &[2, 0, 1, 0], "point 1 (y,x)");
    assert_eq!(&body[24..28], &[4, 0, 3, 0], "point 2 (y,x)");
}

#[test]
fn penfront_sets_the_front_layer_flag() {
    let mut context = ctx();
    context.pen.front = true;
    let frame = effect_frame(
        &Effect::DrawLine {
            x1: 0,
            y1: 0,
            x2: 1,
            y2: 1,
        },
        &context,
    )
    .expect("draw");
    assert_eq!(
        u16::from_le_bytes([frame.payload[4], frame.payload[5]]),
        0x8000
    );
}

#[test]
fn paintclear_is_a_detonate_with_no_operand() {
    let frame = effect_frame(&Effect::PaintClear, &ctx()).expect("detonate");
    assert_eq!(frame.opcode, opcode::DRAW);
    assert_eq!(frame.payload.len(), 10);
    assert_eq!(u16::from_le_bytes([frame.payload[4], frame.payload[5]]), 3);
    assert_eq!(u16::from_le_bytes([frame.payload[6], frame.payload[7]]), 0);
}

#[test]
fn local_only_effects_have_no_frame() {
    for effect in [
        Effect::PlaySound {
            name: "garden".to_string(),
        },
        Effect::MidiStop,
        Effect::GotoUrl {
            url: "http://example.invalid".to_string(),
        },
        Effect::StatusMessage {
            text: "hi".to_string(),
        },
        Effect::DimRoom { percent: 50 },
    ] {
        assert!(
            effect_frame(&effect, &ctx()).is_none(),
            "{effect} should stay local"
        );
    }
}

#[test]
fn encoders_follow_the_session_byte_order() {
    let mut context = ctx();
    context.byte_order = ByteOrder::Big;
    let frame = effect_frame(&Effect::SetSpotState { spot: 2, state: 1 }, &context).expect("sSta");
    assert_eq!(frame.payload, [0x03, 0x85, 0, 2, 0, 1]);
}

//! The wire encoders `tests/wire.rs` does not reach: `SAYAT`, `SETCOLOR`,
//! `SETFACE`, `LINETO`, and the small-room movement clamp.
//!
//! Layouts are the ones OpenPalace's `PalaceClient.as` senders write, matching
//! the convention stated at the top of `tests/wire.rs`.

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
            pos: (0, 0),
            rgb: (255, 0, 0),
            size: 1,
            front: false,
        },
    }
}

#[test]
fn sayat_encodes_the_position_into_the_talk_text() {
    let frame = effect_frame(
        &Effect::SayAt {
            text: "hi".to_owned(),
            x: 3,
            y: 4,
        },
        &ctx(),
    )
    .expect("talk frame");
    assert_eq!(frame.opcode, opcode::TALK);
    assert_eq!(frame.ref_num, 13);
    assert_eq!(frame.payload, b"@3,4 hi\0");
}

#[test]
fn colour_and_face_encode_clamped_to_the_sixteen_value_palette() {
    let color = effect_frame(&Effect::SetColor { color: 99 }, &ctx()).expect("uCol");
    assert_eq!(color.opcode, opcode::USERCOLOR);
    assert_eq!(color.ref_num, 13);
    assert_eq!(color.payload, 15i16.to_le_bytes());

    let face = effect_frame(&Effect::SetFace { face: -3 }, &ctx()).expect("uFac");
    assert_eq!(face.opcode, opcode::USERFACE);
    assert_eq!(face.ref_num, 13);
    assert_eq!(face.payload, 0i16.to_le_bytes());
}

#[test]
fn a_relative_line_is_a_path_draw_record() {
    let frame = effect_frame(
        &Effect::DrawLineRel {
            x1: 0,
            y1: 0,
            x2: 5,
            y2: 6,
        },
        &ctx(),
    )
    .expect("draw");
    assert_eq!(frame.opcode, opcode::DRAW);
    assert_eq!(frame.ref_num, 0);
    let body = &frame.payload;
    assert_eq!(body.len(), 10 + 2 + 2 + 6 + 8);
    assert_eq!(&body[10..12], &[1, 0], "pen size from the context");
    assert_eq!(&body[12..14], &[1, 0], "one segment");
    assert_eq!(&body[20..24], &[0, 0, 0, 0], "point 1 (y,x)");
    assert_eq!(&body[24..28], &[6, 0, 5, 0], "point 2 (y,x)");
}

#[test]
fn a_room_narrower_than_the_margin_clamps_to_its_own_size() {
    let mut context = ctx();
    context.room_width = 30;
    context.room_height = 30;
    let frame = effect_frame(&Effect::MoveUserAbs { x: 100, y: 100 }, &context).expect("uLoc");
    assert_eq!(frame.payload, [30u8, 0, 30, 0], "y then x, pinned at the wall");
}

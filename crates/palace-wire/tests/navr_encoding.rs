//! `navR` (`MSG_ROOMGOTO`) wire form.
//!
//! Regression: the frame body is the room id alone. Adding a `u32 2` body was a
//! bug that made servers navigate to room 2. The expected bytes below are what
//! `palace_walker.py`'s `make_navr` emits.

use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::navr_frame;
use palace_wire::opcode;

#[test]
fn navr_body_is_only_the_room_id() {
    let frame = navr_frame(817, 0, ByteOrder::Little);
    assert_eq!(frame.opcode, opcode::ROOMGOTO);
    assert_eq!(frame.payload.len(), 2);
    assert_eq!(frame.payload, vec![0x31, 0x03]);
    assert_eq!(frame.encoded_len(), 14);
}

#[test]
fn navr_encoded_bytes_match_the_reference_client() {
    let bytes = navr_frame(817, 42, ByteOrder::Little)
        .encode(ByteOrder::Little)
        .expect("encodes");
    #[rustfmt::skip]
    let expected: Vec<u8> = vec![
        0x52, 0x76, 0x61, 0x6e, // "navR"
        0x02, 0x00, 0x00, 0x00, // length = 2 (payload), not a body constant
        0x2a, 0x00, 0x00, 0x00, // refNum = 42
        0x31, 0x03,             // room 817
    ];
    assert_eq!(bytes, expected);
}

#[test]
fn navr_is_big_endian_aware() {
    let bytes = navr_frame(0x1234, 0, ByteOrder::Big)
        .encode(ByteOrder::Big)
        .expect("encodes");
    assert_eq!(&bytes[4..8], &[0x00, 0x00, 0x00, 0x02]);
    assert_eq!(&bytes[12..14], &[0x12, 0x34]);
}

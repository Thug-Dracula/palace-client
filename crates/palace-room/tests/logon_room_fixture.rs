//! Replay the `room` frame from the shared `logon-run1` capture.
//!
//! `palace-wire` captures whole frames in wire order (12-byte header included);
//! this test strips the frame header through `palace_wire::Frame` and decodes
//! the room body with `palace-room`, pinning the values the design notes call
//! out for Balamb Garden.

use std::path::Path;

use palace_room::decode_payload;
use palace_wire::byteorder::ByteOrder;
use palace_wire::opcode;
use palace_wire::Frame;

fn room_frame_bytes() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logon-run1/frames/0007-server-room.bin");
    std::fs::read(path).expect("shared room fixture must be present")
}

#[test]
fn balamb_garden_room_frame_decodes() {
    let frame = Frame::decode_from(&room_frame_bytes(), ByteOrder::Little).expect("frame decodes");
    assert_eq!(frame.opcode, opcode::ROOMDESC);

    let desc = decode_payload(&frame.payload, ByteOrder::Little).expect("room body decodes");
    assert!(desc.is_clean(), "warnings: {:?}", desc.warnings);
    assert_eq!(desc.trailing_len, 4);

    assert_eq!(desc.header.room_id, 901);
    assert_eq!(desc.name, "Balamb Garden");
    assert_eq!(desc.picture, "sqoom23.gif");
    assert_eq!(desc.artist, "Psycrow");
    assert_eq!(desc.password, "");

    assert_eq!(desc.pictures.len(), 1);
    assert_eq!(desc.pictures[0].name.as_deref(), Some("notebar.gif"));

    assert_eq!(desc.hotspots.len(), 6);
    assert!(desc.hotspots.iter().all(|h| h.points.len() == 4));
    assert!(desc.hotspots.iter().all(|h| h.script.is_some()));
    assert_eq!(desc.loose_props.len(), 0);
    assert_eq!(desc.draw_cmds.len(), 0);

    let entry_door = &desc.hotspots[0];
    assert_eq!(entry_door.id, 1);
    assert_eq!(entry_door.hotspot_type, 1);
    assert_eq!(entry_door.dest, 1100);
    assert!(entry_door
        .script
        .as_deref()
        .unwrap_or_default()
        .contains("ON LEAVE"));
}

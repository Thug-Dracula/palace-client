//! Byte-level fixtures for the `RoomDesc` walker.
//!
//! A synthetic room is assembled the way pserver's `Room::Serialise` does — the
//! 40-byte header first, then name/picture/artist/password strings, image
//! names, hotspot point/state arrays, overlay and hotspot arrays, loose props
//! and draw commands — but the offsets are tracked explicitly, so every
//! sub-structure is pinned. The same logical room is built in both byte orders,
//! and a set of corrupted payloads proves the walker degrades instead of
//! panicking.

use palace_room::{
    decode_payload, decode_stream, draw_cmd, draw_flags, RoomDesc, RoomWarning, ROOM_REC_LEN,
};
use palace_wire::byteorder::{ByteOrder, Writer};

#[derive(Clone, Copy, Default)]
struct Header {
    room_flags: u32,
    faces_id: i32,
    room_id: i16,
    room_name_ofst: i16,
    pict_name_ofst: i16,
    artist_name_ofst: i16,
    password_ofst: i16,
    nbr_hotspots: i16,
    hotspot_ofst: i16,
    nbr_pictures: i16,
    picture_ofst: i16,
    nbr_draw_cmds: i16,
    first_draw_cmd: i16,
    nbr_people: i16,
    nbr_lprops: i16,
    first_lprop: i16,
    reserved: i16,
    len_vars: i16,
}

fn encode_payload(order: ByteOrder, h: &Header, var: &[u8]) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_u32(h.room_flags);
    w.write_i32(h.faces_id);
    w.write_i16(h.room_id);
    w.write_i16(h.room_name_ofst);
    w.write_i16(h.pict_name_ofst);
    w.write_i16(h.artist_name_ofst);
    w.write_i16(h.password_ofst);
    w.write_i16(h.nbr_hotspots);
    w.write_i16(h.hotspot_ofst);
    w.write_i16(h.nbr_pictures);
    w.write_i16(h.picture_ofst);
    w.write_i16(h.nbr_draw_cmds);
    w.write_i16(h.first_draw_cmd);
    w.write_i16(h.nbr_people);
    w.write_i16(h.nbr_lprops);
    w.write_i16(h.first_lprop);
    w.write_i16(h.reserved);
    w.write_i16(h.len_vars);
    let mut payload = w.into_vec();
    assert_eq!(payload.len(), ROOM_REC_LEN);
    payload.extend_from_slice(var);
    payload.extend_from_slice(&[0, 0, 0, 0]);
    payload
}

fn put_i16(buf: &mut Vec<u8>, order: ByteOrder, value: i16) {
    buf.extend_from_slice(&match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    });
}

fn put_u16(buf: &mut Vec<u8>, order: ByteOrder, value: u16) {
    buf.extend_from_slice(&match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    });
}

fn put_i32(buf: &mut Vec<u8>, order: ByteOrder, value: i32) {
    buf.extend_from_slice(&match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    });
}

fn put_u32(buf: &mut Vec<u8>, order: ByteOrder, value: u32) {
    buf.extend_from_slice(&match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    });
}

fn put_i16_at(buf: &mut [u8], order: ByteOrder, at: usize, value: i16) {
    let bytes = match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    };
    buf[at..at + 2].copy_from_slice(&bytes);
}

fn put_pstring(buf: &mut Vec<u8>, s: &str) -> i16 {
    let offset = buf.len() as i16;
    buf.push(s.len() as u8);
    buf.extend_from_slice(s.as_bytes());
    offset
}

fn put_cstring(buf: &mut Vec<u8>, s: &str) -> i16 {
    let offset = buf.len() as i16;
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
    offset
}

fn align4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}

struct Built {
    payload: Vec<u8>,
    lprop0: usize,
    lprop1: usize,
    draw0: usize,
}

fn build_room(order: ByteOrder) -> Built {
    let mut var = vec![0u8, 0];

    let name = put_pstring(&mut var, "Test Room");
    let pict = put_pstring(&mut var, "room.png");
    let artist = put_pstring(&mut var, "Artist");
    let password = put_pstring(&mut var, "sekrit");
    let pic_name = put_pstring(&mut var, "overlay.png");

    let points_ofst = var.len() as i16;
    for (y, x) in [(10i16, 20i16), (30, 40), (50, 60), (70, 80)] {
        put_i16(&mut var, order, y);
        put_i16(&mut var, order, x);
    }

    let states_ofst = var.len() as i16;
    for (id, reserved, y, x) in [(3i16, 0i16, 0i16, 0i16), (20, 0, 5, 6)] {
        put_i16(&mut var, order, id);
        put_i16(&mut var, order, reserved);
        put_i16(&mut var, order, y);
        put_i16(&mut var, order, x);
    }

    let hot_name = put_pstring(&mut var, "Door");
    let hot_script = put_cstring(&mut var, "ON SELECT {\n}");

    align4(&mut var);
    let picture_ofst = var.len() as i16;
    put_i32(&mut var, order, 0);
    put_i16(&mut var, order, 1);
    put_i16(&mut var, order, pic_name);
    put_i16(&mut var, order, -1);
    put_i16(&mut var, order, 0);

    let hotspot_ofst = var.len() as i16;
    put_i32(&mut var, order, 1);
    put_i32(&mut var, order, 0x02);
    put_i32(&mut var, order, 0);
    put_i32(&mut var, order, 0);
    put_i16(&mut var, order, 100);
    put_i16(&mut var, order, 200);
    put_i16(&mut var, order, 7);
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 4);
    put_i16(&mut var, order, points_ofst);
    put_i16(&mut var, order, 1);
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 2);
    put_i16(&mut var, order, states_ofst);
    put_i16(&mut var, order, hot_name);
    put_i16(&mut var, order, hot_script);
    put_i16(&mut var, order, 0);

    let lprop0 = var.len() as i16;
    put_i16(&mut var, order, lprop0 + 24);
    put_i16(&mut var, order, 0);
    put_u32(&mut var, order, 0xC0FF_EE00);
    put_u32(&mut var, order, 0x1234_5678);
    put_i32(&mut var, order, 1);
    put_i32(&mut var, order, 0);
    put_i16(&mut var, order, 300);
    put_i16(&mut var, order, 400);
    let lprop1 = var.len() as i16;
    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 0);
    put_u32(&mut var, order, 0xAB);
    put_u32(&mut var, order, 0xCD);
    put_i32(&mut var, order, 2);
    put_i32(&mut var, order, 9);
    put_i16(&mut var, order, 350);
    put_i16(&mut var, order, 450);

    let draw0 = var.len() as i16;
    let mut data = Vec::new();
    put_i16(&mut data, order, 2);
    put_i16(&mut data, order, 1);
    data.extend_from_slice(&[0x10, 0x10, 0x20, 0x20, 0x30, 0x30]);
    for (y, x) in [(1i16, 2i16), (3, 4)] {
        put_i16(&mut data, order, y);
        put_i16(&mut data, order, x);
    }
    data.extend_from_slice(&[0xFF, 0x10, 0x20, 0x30]);
    data.extend_from_slice(&[0x80, 0x40, 0x50, 0x60]);
    let draw1 = draw0 + 10 + data.len() as i16;
    put_i16(&mut var, order, draw1);
    put_i16(&mut var, order, 0);
    put_u16(
        &mut var,
        order,
        ((draw_flags::LAYER_FRONT as u16) << 8) | draw_cmd::PATH as u16,
    );
    put_u16(&mut var, order, data.len() as u16);
    put_i16(&mut var, order, 10);
    var.extend_from_slice(&data);

    put_i16(&mut var, order, 0);
    put_i16(&mut var, order, 0);
    put_u16(&mut var, order, draw_cmd::DETONATE as u16);
    put_u16(&mut var, order, 0);
    put_i16(&mut var, order, 10);

    let header = Header {
        room_flags: 0x0114,
        room_id: 901,
        room_name_ofst: name,
        pict_name_ofst: pict,
        artist_name_ofst: artist,
        password_ofst: password,
        nbr_hotspots: 1,
        hotspot_ofst,
        nbr_pictures: 1,
        picture_ofst,
        nbr_draw_cmds: 2,
        first_draw_cmd: draw0,
        nbr_people: 1,
        nbr_lprops: 2,
        first_lprop: lprop0,
        reserved: 0,
        len_vars: var.len() as i16,
        ..Header::default()
    };

    Built {
        payload: encode_payload(order, &header, &var),
        lprop0: lprop0 as usize,
        lprop1: lprop1 as usize,
        draw0: draw0 as usize,
    }
}

fn decode(order: ByteOrder) -> RoomDesc {
    decode_payload(&build_room(order).payload, order).expect("synthetic room must decode")
}

#[test]
fn decodes_every_sub_structure() {
    let desc = decode(ByteOrder::Little);
    assert!(desc.is_clean(), "warnings: {:?}", desc.warnings);
    assert_eq!(desc.trailing_len, 4);
    assert_eq!(desc.header.room_id, 901);
    assert_eq!(desc.name, "Test Room");
    assert_eq!(desc.picture, "room.png");
    assert_eq!(desc.artist, "Artist");
    assert_eq!(desc.password, "sekrit");

    assert_eq!(desc.pictures.len(), 1);
    let pic = &desc.pictures[0];
    assert_eq!(pic.pic_id, 1);
    assert_eq!(pic.trans_color, -1);
    assert_eq!(pic.name.as_deref(), Some("overlay.png"));

    assert_eq!(desc.hotspots.len(), 1);
    let spot = &desc.hotspots[0];
    assert_eq!(spot.id, 7);
    assert_eq!(spot.hotspot_type, 1);
    assert_eq!((spot.loc.v, spot.loc.h), (100, 200));
    assert_eq!(spot.points.len(), 4);
    assert_eq!((spot.points[3].v, spot.points[3].h), (70, 80));
    assert_eq!(spot.states.len(), 2);
    assert_eq!(spot.states[1].pict_id, 20);
    assert_eq!((spot.states[1].pic_loc.v, spot.states[1].pic_loc.h), (5, 6));
    assert_eq!(spot.name.as_deref(), Some("Door"));
    assert_eq!(spot.script.as_deref(), Some("ON SELECT {\n}"));

    assert_eq!(desc.loose_props.len(), 2);
    assert_eq!(desc.loose_props[0].spec.id, 0xC0FF_EE00);
    assert_ne!(desc.loose_props[0].next_ofst, 0);
    assert_eq!(desc.loose_props[1].next_ofst, 0);
    assert_eq!(
        (desc.loose_props[0].loc.v, desc.loose_props[0].loc.h),
        (300, 400)
    );

    assert_eq!(desc.draw_cmds.len(), 2);
    let draw = &desc.draw_cmds[0];
    assert_eq!(draw.command, draw_cmd::PATH);
    assert!(draw.is_front_layer());
    assert_eq!(draw.cmd_length, 26);
    let payload = draw.payload.as_ref().expect("path operand must decode");
    assert_eq!(payload.pen_size, 2);
    assert_eq!(payload.num_points, 1);
    assert_eq!(payload.pen_rgb, [0x10, 0x20, 0x30]);
    assert_eq!(payload.points.len(), 2);
    assert_eq!((payload.points[1].v, payload.points[1].h), (3, 4));
    assert_eq!(payload.line_rgba, Some([0xFF, 0x10, 0x20, 0x30]));
    assert_eq!(payload.fill_rgba, Some([0x80, 0x40, 0x50, 0x60]));

    assert_eq!(desc.draw_cmds[1].command, draw_cmd::DETONATE);
    assert!(desc.draw_cmds[1].payload.is_none());
}

#[test]
fn both_byte_orders_decode_to_the_same_values() {
    let le = decode(ByteOrder::Little);
    let be = decode(ByteOrder::Big);
    assert_eq!(le.header, be.header);
    assert_eq!(le.name, be.name);
    assert_eq!(le.picture, be.picture);
    assert_eq!(le.artist, be.artist);
    assert_eq!(le.password, be.password);
    assert_eq!(le.pictures, be.pictures);
    assert_eq!(le.hotspots, be.hotspots);
    assert_eq!(le.loose_props, be.loose_props);
    assert_eq!(le.draw_cmds.len(), be.draw_cmds.len());
    for (a, b) in le.draw_cmds.iter().zip(&be.draw_cmds) {
        assert_eq!(a.next_ofst, b.next_ofst);
        assert_eq!(a.command, b.command);
        assert_eq!(a.flags, b.flags);
        assert_eq!(a.cmd_length, b.cmd_length);
        assert_eq!(a.data_ofst, b.data_ofst);
        assert_eq!(a.payload, b.payload);
    }
    assert_ne!(le.var_data, be.var_data);
}

#[test]
fn stream_splits_concatenated_records() {
    let one = build_room(ByteOrder::Little).payload;
    let mut payload = one.clone();
    payload.extend_from_slice(&one);
    let records = decode_stream(&payload, ByteOrder::Little);
    assert_eq!(records.len(), 2);
    for record in records {
        let desc = record.expect("both records must decode");
        assert_eq!(desc.name, "Test Room");
        assert!(desc.is_clean());
    }
}

#[test]
fn truncated_var_buf_is_an_error() {
    let built = build_room(ByteOrder::Little);
    let truncated = &built.payload[..ROOM_REC_LEN + 10];
    assert!(decode_payload(truncated, ByteOrder::Little).is_err());
}

#[test]
fn truncated_header_never_panics() {
    for len in 0..ROOM_REC_LEN {
        let payload = vec![0u8; len];
        assert!(decode_payload(&payload, ByteOrder::Little).is_err());
    }
}

#[test]
fn out_of_range_picture_array_warns_and_degrades() {
    let mut payload = build_room(ByteOrder::Little).payload;
    payload[24..26].copy_from_slice(&30000i16.to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert!(desc.pictures.is_empty());
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::ArrayTruncated { field, .. } if *field == "pictureOfst")));
}

#[test]
fn absent_hotspot_offset_warns() {
    let mut payload = build_room(ByteOrder::Little).payload;
    payload[20..22].copy_from_slice(&0i16.to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert!(desc.hotspots.is_empty());
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::AbsentOffset { field, .. } if *field == "hotspotOfst")));
}

#[test]
fn negative_artist_offset_warns() {
    let mut payload = build_room(ByteOrder::Little).payload;
    payload[14..16].copy_from_slice(&(-1i16).to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert_eq!(desc.artist, "");
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::NegativeOffset { field, .. } if *field == "artistNameOfst")));
}

#[test]
fn hotspot_point_array_out_of_range_warns() {
    let first = decode(ByteOrder::Little);
    let points_field = ROOM_REC_LEN + first.header.hotspot_ofst as usize + 26;
    let mut payload = build_room(ByteOrder::Little).payload;
    payload[points_field..points_field + 2].copy_from_slice(&30000i16.to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert_eq!(desc.hotspots.len(), 1);
    assert!(desc.hotspots[0].points.is_empty());
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::ArrayTruncated { field, .. } if *field == "Hotspot.ptsOfst")));
}

#[test]
fn loose_prop_link_cycle_is_broken() {
    let built = build_room(ByteOrder::Little);
    let mut payload = built.payload.clone();
    let at = ROOM_REC_LEN + built.lprop0;
    payload[at..at + 2].copy_from_slice(&(built.lprop0 as i16).to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert_eq!(desc.loose_props.len(), 1);
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::LinkedCycle { field, .. } if *field == "firstLProp")));
}

#[test]
fn packed_loose_props_with_zero_links_use_stride_fallback() {
    let built = build_room(ByteOrder::Little);
    let mut payload = built.payload.clone();
    for offset in [built.lprop0, built.lprop1] {
        let at = ROOM_REC_LEN + offset;
        payload[at..at + 2].copy_from_slice(&0i16.to_le_bytes());
    }
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert_eq!(desc.loose_props.len(), 2);
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::LinkedFallback { field, .. } if *field == "firstLProp")));
}

#[test]
fn draw_operand_out_of_range_warns_but_chain_continues() {
    let built = build_room(ByteOrder::Little);
    let mut payload = built.payload.clone();
    let at = ROOM_REC_LEN + built.draw0 + 6;
    payload[at..at + 2].copy_from_slice(&60000u16.to_le_bytes());
    let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
    assert_eq!(desc.draw_cmds.len(), 2);
    assert!(desc.draw_cmds[0].data.is_empty());
    assert!(desc.draw_cmds[0].payload.is_none());
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::DrawDataOutOfRange { .. })));
}

#[test]
fn unterminated_hotspot_script_warns() {
    let order = ByteOrder::Little;
    let mut var = vec![0u8, 0];
    let hotspot_ofst = var.len() as i16;
    var.extend_from_slice(&[0u8; 48]);
    let script_ofst = var.len() as i16;
    var.extend_from_slice(b"abc");
    put_i16_at(&mut var, order, hotspot_ofst as usize + 44, script_ofst);

    let header = Header {
        nbr_hotspots: 1,
        hotspot_ofst,
        len_vars: var.len() as i16,
        ..Header::default()
    };
    let payload = encode_payload(order, &header, &var);
    let desc = decode_payload(&payload, order).unwrap();
    assert!(desc
        .warnings
        .iter()
        .any(|w| matches!(w, RoomWarning::ScriptUnterminated { .. })));
}

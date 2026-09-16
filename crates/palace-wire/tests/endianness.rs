//! Byte fixtures in **both** endiannesses.
//!
//! The server is little-endian today, but the protocol explicitly makes the
//! client responsible for coping with a mismatch, and getting it wrong is a
//! silent total failure. These tests pin the exact bytes of a handshake, a
//! logon and the list messages under both orders.

use palace_wire::byteorder::{ByteOrder, Reader, Writer};
use palace_wire::frame::{read_handshake, Frame, HEADER_LEN};
use palace_wire::messages::{reference_logon_record, Message, RoomList, UserList};
use palace_wire::opcode;
use palace_wire::Opcode;

/// The 4-byte banner each kind of server sends.
#[test]
fn banner_bytes_identify_byte_order() {
    assert_eq!(ByteOrder::from_banner(b"ryit").unwrap(), ByteOrder::Little);
    assert_eq!(ByteOrder::from_banner(b"tiyr").unwrap(), ByteOrder::Big);
    assert!(ByteOrder::from_banner(b"pser").is_err());
}

#[test]
fn framing_header_is_byte_swapped_between_orders() {
    let frame = Frame::new(opcode::LISTOFALLROOMS, 0x0102_0304, vec![0xde, 0xad]);

    let le = frame.encode(ByteOrder::Little).unwrap();
    let be = frame.encode(ByteOrder::Big).unwrap();

    assert_eq!(le.len(), HEADER_LEN + 2);
    assert_eq!(be.len(), le.len());

    // mnemonic: little-endian wire stores it reversed
    assert_eq!(&le[0..4], b"tsLr");
    assert_eq!(&be[0..4], b"rLst");
    // length: 2
    assert_eq!(&le[4..8], &2u32.to_le_bytes());
    assert_eq!(&be[4..8], &2u32.to_be_bytes());
    // refNum: 0x01020304
    assert_eq!(&le[8..12], &0x0102_0304i32.to_le_bytes());
    assert_eq!(&be[8..12], &0x0102_0304i32.to_be_bytes());

    assert_eq!(Frame::decode_from(&le, ByteOrder::Little).unwrap(), frame);
    assert_eq!(Frame::decode_from(&be, ByteOrder::Big).unwrap(), frame);
}

#[test]
fn handshake_decodes_from_raw_bytes_in_both_orders() {
    // Little-endian server: banner `ryit`, length 0, user id 300.
    let mut le = b"ryit".to_vec();
    le.extend_from_slice(&0u32.to_le_bytes());
    le.extend_from_slice(&300i32.to_le_bytes());
    let hs = read_handshake(&mut le.as_slice()).unwrap();
    assert_eq!(hs.byte_order, ByteOrder::Little);
    assert_eq!(hs.user_id(), 300);

    // Big-endian server: banner `tiyr`, length 0, user id 300.
    let mut be = b"tiyr".to_vec();
    be.extend_from_slice(&0u32.to_be_bytes());
    be.extend_from_slice(&300i32.to_be_bytes());
    let hs = read_handshake(&mut be.as_slice()).unwrap();
    assert_eq!(hs.byte_order, ByteOrder::Big);
    assert_eq!(hs.user_id(), 300);
}

#[test]
fn logon_payload_matches_the_walker_packet_in_little_endian() {
    // Frame body only: the first 12 bytes of the walker's packet are the header.
    let expected: Vec<u8> = [
        "e923fb328f1918aa045269636f0000000000000000000000",
        "000000000000000000000000000000000000000000000000",
        "000000000000000000000000000000000000000000000000",
        "080000804f6e54e0666dc992401901004019010040190100",
        "000053434f555431ccab0700410000005101000001000000",
        "0100000000000000",
    ]
    .concat()
    .as_bytes()
    .chunks(2)
    .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
    .collect();
    assert_eq!(expected.len(), 128);
    let record = reference_logon_record("Rico", 0);
    assert_eq!(record.encode_to_vec(ByteOrder::Little), expected);
}

#[test]
fn logon_payload_under_big_endian_swaps_only_multi_byte_fields() {
    let record = reference_logon_record("Rico", 0);
    let le = record.encode_to_vec(ByteOrder::Little);
    let be = record.encode_to_vec(ByteOrder::Big);

    // Scalar fields are byte-reversed.
    assert_eq!(&le[0..4], &record.crc.to_le_bytes());
    assert_eq!(&be[0..4], &record.crc.to_be_bytes());
    assert_eq!(&le[76..80], &record.puid_ctr.to_le_bytes());
    assert_eq!(&be[76..80], &record.puid_ctr.to_be_bytes());

    // String and reserved bytes are order-independent.
    assert_eq!(&le[8..40], &be[8..40]);
    assert_eq!(&le[98..104], &be[98..104]);

    let mut r = Reader::new(&be, ByteOrder::Big);
    assert_eq!(
        palace_wire::messages::AuxRegistrationRec::decode(&mut r).unwrap(),
        record
    );
}

#[test]
fn list_messages_decode_in_both_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut w = Writer::new(order);
        w.write_i32(186);
        w.write_u16(0x0200);
        w.write_u16(2);
        w.write_pstring_aligned("Entrance");
        w.write_i32(202);
        w.write_u16(0);
        w.write_u16(0);
        w.write_pstring_aligned("Balamb Garden");
        let rooms_bytes = w.into_vec();

        let mut r = Reader::new(&rooms_bytes, order);
        let rooms = RoomList::decode(2, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(rooms.rooms[1].name, "Balamb Garden");

        let mut w = Writer::new(order);
        w.write_i32(3);
        w.write_u16(0x0008);
        w.write_i16(901);
        w.write_pstring_aligned("Rico");
        let users_bytes = w.into_vec();
        let mut r = Reader::new(&users_bytes, order);
        let users = UserList::decode(1, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(users.users[0].name, "Rico");
        assert_eq!(users.users[0].flags, 0x0008);
    }
}

#[test]
fn a_whole_session_frames_correctly_in_both_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut stream = Vec::new();
        let mut w = Writer::new(order);

        Frame::empty(opcode::TIYID, 42).write_to(&mut w).unwrap();
        reference_logon_record("probe", 0)
            .logon_frame(order)
            .write_to(&mut w)
            .unwrap();
        Frame::empty(opcode::PING, 1).write_to(&mut w).unwrap();
        Frame::empty(opcode::LOGOFF, 0).write_to(&mut w).unwrap();
        stream.extend_from_slice(w.as_slice());

        let frames = Frame::decode_all(&stream, order).unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0].opcode, opcode::TIYID);
        assert_eq!(frames[1].opcode, opcode::LOGON);
        assert_eq!(frames[1].payload.len(), 128);
        assert_eq!(frames[2].opcode, opcode::PING);
        assert_eq!(frames[3].opcode, opcode::LOGOFF);

        // Every frame re-encodes to its original bytes.
        let reencoded = frames
            .iter()
            .flat_map(|f| f.encode(order).unwrap())
            .collect::<Vec<u8>>();
        assert_eq!(reencoded, stream);
    }
}

#[test]
fn unknown_opcode_survives_framing_in_both_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let frame = Frame::new(Opcode::new(0xfeed_face), 9, vec![1, 2, 3]);
        let bytes = frame.encode(order).unwrap();
        let back = Frame::decode_from(&bytes, order).unwrap();
        assert_eq!(back, frame);
        let msg = Message::decode(back.opcode, back.ref_num, &back.payload, order).unwrap();
        assert!(matches!(msg, Message::Unknown { .. }));
        assert!(!msg.describe().is_empty());
    }
}

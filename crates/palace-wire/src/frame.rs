//! Message framing.
//!
//! Every Palace message — in both directions, for the whole life of a
//! connection — is:
//!
//! ```text
//! struct ClientMsg {
//!     uint32 eventType;   // 4-channel ASCII mnemonic, in session byte order
//!     uint32 length;      // length of the message body
//!     sint32 refNum;      // arbitrary integer operand
//!     uint8  msg[length]; // message body
//! };
//! ```
//!
//! The same framing covers the server's very first packet (`MSG_TIYID`), whose
//! `length` is 0 and whose `refNum` is the user id assigned to the client.
//!
//! There is no terminator, no checksum and no version field: the only way to
//! stay in sync is to trust `length` exactly, which is why the decoder refuses
//! implausibly large lengths rather than allocating blindly.

use std::io::Read;

use crate::byteorder::{ByteOrder, Reader, Writer};
use crate::error::{Result, WireError, MAX_PAYLOAD_LEN};
use crate::opcode::{Opcode, ROOMDESCEND, TIYID};

/// Size of the fixed frame header: `eventType` + `length` + `refNum`.
pub const HEADER_LEN: usize = 12;

/// A single decoded Palace message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The message type.
    pub opcode: Opcode,
    /// The `refNum` operand. Its meaning depends on the opcode (user id,
    /// request id, room/user count, version number, ping counter, …).
    pub ref_num: i32,
    /// The raw message body.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Build a frame from its parts.
    pub fn new(opcode: Opcode, ref_num: i32, payload: Vec<u8>) -> Self {
        Frame {
            opcode,
            ref_num,
            payload,
        }
    }

    /// Build an empty-payload frame, the shape most client requests take.
    pub fn empty(opcode: Opcode, ref_num: i32) -> Self {
        Frame {
            opcode,
            ref_num,
            payload: Vec::new(),
        }
    }

    /// Total encoded size, including the 12-byte header.
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    /// Encode this frame using the session byte order.
    pub fn encode(&self, order: ByteOrder) -> Result<Vec<u8>> {
        let mut w = Writer::with_capacity(order, self.encoded_len());
        self.write_to(&mut w)?;
        Ok(w.into_vec())
    }

    /// Encode into an existing writer.
    pub fn write_to(&self, w: &mut Writer) -> Result<()> {
        let len = u32::try_from(self.payload.len()).map_err(|_| WireError::PayloadTooLarge {
            length: self.payload.len(),
        })?;
        w.write_u32(self.opcode.value());
        w.write_u32(len);
        w.write_i32(self.ref_num);
        w.write_bytes(&self.payload);
        Ok(())
    }

    /// Decode one frame, reading exactly as many bytes as the header claims.
    ///
    /// Unknown opcodes are returned as-is (with [`Opcode::is_known`] false); the
    /// caller decides to log and skip. Only framing-level problems are errors.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Frame> {
        let opcode = Opcode::new(reader.read_u32()?);
        let length = reader.read_u32()?;
        if length > MAX_PAYLOAD_LEN {
            return Err(WireError::ImplausibleLength {
                length,
                max: MAX_PAYLOAD_LEN,
            });
        }
        let ref_num = reader.read_i32()?;
        let payload = reader.read_bytes(length as usize)?.to_vec();
        Ok(Frame {
            opcode,
            ref_num,
            payload,
        })
    }

    /// Decode a single frame from a complete byte buffer. Errors with
    /// [`WireError::TrailingBytes`] if the buffer holds more than one frame.
    pub fn decode_from(bytes: &[u8], order: ByteOrder) -> Result<Frame> {
        let mut r = Reader::new(bytes, order);
        let frame = Frame::decode(&mut r)?;
        if r.remaining() != 0 {
            return Err(WireError::TrailingBytes {
                remaining: r.remaining(),
            });
        }
        Ok(frame)
    }

    /// Decode as many whole frames as `bytes` contains. Trailing bytes that do
    /// not form a complete frame are an error.
    pub fn decode_all(bytes: &[u8], order: ByteOrder) -> Result<Vec<Frame>> {
        let mut r = Reader::new(bytes, order);
        let mut out = Vec::new();
        while r.remaining() >= HEADER_LEN {
            out.push(Frame::decode(&mut r)?);
        }
        if r.remaining() != 0 {
            return Err(WireError::TrailingBytes {
                remaining: r.remaining(),
            });
        }
        Ok(out)
    }

    /// Render the payload as contiguous printable ASCII (NULs become newlines),
    /// for logs and `MSG_ROOMDESC` previews. Non-printable bytes become `.`.
    pub fn printable_payload(&self) -> String {
        self.payload
            .iter()
            .map(|&b| match b {
                0 => '\n',
                0x20..=0x7e => b as char,
                _ => '.',
            })
            .collect()
    }
}

/// The result of reading the server's opening banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// The byte order the rest of the session must use.
    pub byte_order: ByteOrder,
    /// The `MSG_TIYID` frame. `ref_num` is the user id the server assigned us.
    pub frame: Frame,
}

impl Handshake {
    /// The user id the server assigned to this connection.
    pub fn user_id(&self) -> i32 {
        self.frame.ref_num
    }
}

/// Read the server banner from any blocking byte source and return the detected
/// [`ByteOrder`] together with the decoded `MSG_TIYID` frame.
///
/// This is the mandatory first step of every Palace session. The four banner
/// bytes are read first, the byte order is decided, and only then is the rest
/// of the header interpreted — so no integer is ever read under the wrong
/// order.
pub fn read_handshake<R: Read>(source: &mut R) -> Result<Handshake> {
    let mut banner = [0u8; 4];
    source.read_exact(&mut banner)?;
    let byte_order = ByteOrder::from_banner(&banner)?;

    let mut rest = [0u8; HEADER_LEN - 4];
    source.read_exact(&mut rest)?;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(&banner);
    header.extend_from_slice(&rest);

    let mut r = Reader::new(&header, byte_order);
    let payload_len = {
        let opcode = r.read_u32()?;
        let length = r.read_u32()?;
        if length > MAX_PAYLOAD_LEN {
            return Err(WireError::ImplausibleLength {
                length,
                max: MAX_PAYLOAD_LEN,
            });
        }
        let _ref_num = r.read_i32()?;
        let _ = opcode;
        length as usize
    };

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        source.read_exact(&mut payload)?;
    }

    let mut r = Reader::new(&header, byte_order);
    let opcode = Opcode::new(r.read_u32()?);
    let _length = r.read_u32()?;
    let ref_num = r.read_i32()?;
    let frame = Frame {
        opcode,
        ref_num,
        payload,
    };

    if frame.opcode != TIYID {
        // Some servers send a trivial error packet first (e.g. `down` when the
        // server is shutting down). Surface it rather than mis-parsing.
        return Err(WireError::UnknownBanner { banner });
    }
    Ok(Handshake { byte_order, frame })
}

/// Convenience: build a `MSG_ROOMGOTO` (`navR`) frame requesting `room_id`.
///
/// Payload is exactly the 16-bit room id; the header length field is `2`. Both
/// `palace_walker.py::make_navr` and OpenPalace's `actuallyGotoRoom` write that
/// `2` as the frame length, not as body bytes — a body of `[u32 2][u16 room]`
/// (the previous form) makes the server navigate to room 2. Pinned by
/// `tests/navr_encoding.rs`.
pub fn navr_frame(room_id: u16, ref_num: i32, order: ByteOrder) -> Frame {
    let mut w = Writer::with_capacity(order, 2);
    w.write_u16(room_id);
    Frame::new(crate::opcode::ROOMGOTO, ref_num, w.into_vec())
}

/// True when the opcode ends a room description stream.
pub fn is_room_desc_end(opcode: Opcode) -> bool {
    opcode == ROOMDESCEND
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::{LOGON, PING, ROOMDESC};

    fn sample(order: ByteOrder) -> Vec<u8> {
        let mut w = Writer::new(order);
        w.write_u32(LOGON.value());
        w.write_u32(5);
        w.write_i32(-7);
        w.write_bytes(b"hello");
        w.into_vec()
    }

    #[test]
    fn round_trip_little_endian() {
        let bytes = sample(ByteOrder::Little);
        // Little-endian wire: 'regi' appears byte-reversed as "iger".
        assert_eq!(&bytes[..4], b"iger");
        assert_eq!(&bytes[4..8], &5u32.to_le_bytes());
        let frame = Frame::decode_from(&bytes, ByteOrder::Little).unwrap();
        assert_eq!(frame.opcode, LOGON);
        assert_eq!(frame.ref_num, -7);
        assert_eq!(frame.payload, b"hello");
        assert_eq!(frame.encode(ByteOrder::Little).unwrap(), bytes);
    }

    #[test]
    fn round_trip_big_endian() {
        let bytes = sample(ByteOrder::Big);
        // Big-endian wire: 'regi' appears in mnemonic order.
        assert_eq!(&bytes[..4], b"regi");
        assert_eq!(&bytes[4..8], &5u32.to_be_bytes());
        let frame = Frame::decode_from(&bytes, ByteOrder::Big).unwrap();
        assert_eq!(frame.opcode, LOGON);
        assert_eq!(frame.ref_num, -7);
        assert_eq!(frame.payload, b"hello");
        assert_eq!(frame.encode(ByteOrder::Big).unwrap(), bytes);
    }

    #[test]
    fn the_same_logical_frame_differs_only_when_ints_exceed_one_byte() {
        let le = Frame::new(LOGON, 0x0102_0304, vec![0xaa])
            .encode(ByteOrder::Little)
            .unwrap();
        let be = Frame::new(LOGON, 0x0102_0304, vec![0xaa])
            .encode(ByteOrder::Big)
            .unwrap();
        assert_ne!(le, be);
        assert_eq!(
            Frame::decode_from(&le, ByteOrder::Little).unwrap().ref_num,
            0x0102_0304
        );
        assert_eq!(
            Frame::decode_from(&be, ByteOrder::Big).unwrap().ref_num,
            0x0102_0304
        );
    }

    #[test]
    fn decoding_with_the_wrong_order_is_detected_by_opcode_or_length() {
        // Decoding a little-endian frame as big-endian yields a bogus opcode and
        // an implausible length — never a silent success.
        let bytes = sample(ByteOrder::Little);
        let err = Frame::decode_from(&bytes, ByteOrder::Big).unwrap_err();
        assert!(
            matches!(
                err,
                WireError::ImplausibleLength { .. } | WireError::UnexpectedEof { .. }
            ),
            "expected a framing error, got {err:?}"
        );
    }

    #[test]
    fn decode_all_walks_multiple_frames() {
        let mut w = Writer::new(ByteOrder::Big);
        Frame::empty(PING, 1).write_to(&mut w).unwrap();
        Frame::new(ROOMDESC, 0, b"abc".to_vec())
            .write_to(&mut w)
            .unwrap();
        let frames = Frame::decode_all(w.as_slice(), ByteOrder::Big).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].opcode, PING);
        assert_eq!(frames[1].payload, b"abc");
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = sample(ByteOrder::Little);
        bytes.push(0);
        assert!(matches!(
            Frame::decode_from(&bytes, ByteOrder::Little),
            Err(WireError::TrailingBytes { remaining: 1 })
        ));
    }

    #[test]
    fn implausible_length_is_rejected_without_allocating() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_u32(ROOMDESC.value());
        w.write_u32(MAX_PAYLOAD_LEN + 1);
        w.write_i32(0);
        let err = Frame::decode_from(w.as_slice(), ByteOrder::Little).unwrap_err();
        assert!(matches!(err, WireError::ImplausibleLength { .. }));
    }

    #[test]
    fn handshake_reads_banner_and_payload_in_both_orders() {
        // A server banner is just a zero-length TIYID frame carrying the user id.
        for (order, banner) in [(ByteOrder::Little, *b"ryit"), (ByteOrder::Big, *b"tiyr")] {
            let mut w = Writer::new(order);
            w.write_u32(TIYID.value());
            w.write_u32(0);
            w.write_i32(2828);
            let bytes = w.into_vec();
            assert_eq!(&bytes[..4], &banner);
            let hs = read_handshake(&mut bytes.as_slice()).unwrap();
            assert_eq!(hs.byte_order, order);
            assert_eq!(hs.user_id(), 2828);
        }
    }

    #[test]
    fn handshake_rejects_http_tunnel_mode() {
        let mut bytes = b"pser".to_vec();
        bytes.extend_from_slice(&[0u8; 8]);
        let err = read_handshake(&mut bytes.as_slice()).unwrap_err();
        assert!(matches!(err, WireError::UnsupportedTransport { .. }));
    }

    #[test]
    fn handshake_reads_a_nonempty_payload() {
        // The framing code must not assume the banner frame is empty.
        let mut w = Writer::new(ByteOrder::Little);
        w.write_u32(TIYID.value());
        w.write_u32(3);
        w.write_i32(9);
        w.write_bytes(b"xyz");
        let hs = read_handshake(&mut w.as_slice()).unwrap();
        assert_eq!(hs.frame.payload, b"xyz");
        assert_eq!(hs.user_id(), 9);
    }
}

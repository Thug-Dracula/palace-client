//! `palace-room` — the `MSG_ROOMDESC` (`room`) decoder.
//!
//! A room description is the hardest message in the Palace protocol: a flat
//! 40-byte header whose fields are **byte offsets into a trailing
//! variable-length buffer**. Nothing is reached directly. This crate walks that
//! offset graph — room name, background picture, artist, password, image
//! overlays, hotspots (with their polygon points, states, names and scripts),
//! loose props and draw commands — and never trusts an offset before checking
//! it.
//!
//! It lives in its own crate rather than inside [`palace_wire`] because
//! `palace-wire` is deliberately a dependency-free framing/opcode layer while
//! this is a large, stateful model builder. The split mirrors the workspace
//! table in `~/palace-corpus/TAURI-CLIENT-SCOPE.md` (`palace-room` depends on
//! `palace-wire`). `palace-wire`'s existing minimal `RoomDescription` is left
//! untouched, so its tests and callers keep working.
//!
//! ## Layout at a glance
//!
//! ```text
//! RoomRec (40 bytes, little- or big-endian per session)
//!   0  uint32 roomFlags          10  sint16 roomNameOfst     20  sint16 hotspotOfst
//!   4  sint32 facesID            12  sint16 pictNameOfst     22  sint16 nbrPictures
//!   8  sint16 roomID             14  sint16 artistNameOfst   24  sint16 pictureOfst
//!                               16  sint16 passwordOfst     26  sint16 nbrDrawCmds
//!                               18  sint16 nbrHotspots      28  sint16 firstDrawCmd
//!   30 sint16 nbrPeople          32  sint16 nbrLProps        34  sint16 firstLProp
//!   36 sint16 reserved           38  sint16 lenVars
//! 40  uint8  varBuf[lenVars]
//!     ... alignment padding (size - lenVars - 40 bytes)
//! ```
//!
//! `README.md` has the full field-by-field table, every record size and the
//! exact traversal rules. The short version:
//!
//! * **Pictures** and **hotspots** are fixed-stride arrays reached from
//!   `pictureOfst` / `hotspotOfst` (12 and 48 bytes per entry).
//! * **Loose props** and **draw commands** are **linked lists** reached from
//!   `firstLProp` / `firstDrawCmd` and followed through each record's
//!   `nextOfst` (24-byte records; 10-byte draw headers plus their operand).
//! * All strings inside `varBuf` are `PString`s (one length byte, then the
//!   bytes, Latin-1). Hotspot scripts are NUL-terminated `CString`s.
//! * Coordinates are signed 16-bit pixels; see the README for the one open
//!   question about hotspot polygon points.
//!
//! ## Tolerant by construction
//!
//! Parsing a malformed payload never panics and never silently invents data. A
//! bad offset degrades the affected sub-structure and appends a
//! [`RoomWarning`]; the raw buffer is always available as
//! [`RoomDesc::var_data`]. The fixed header and `varBuf` are the only mandatory
//! parts — truncating *them* is a [`WireError`](palace_wire::WireError).
//!
//! ## Example
//!
//! ```
//! use palace_room::{decode_payload, RoomDesc};
//! use palace_wire::ByteOrder;
//!
//! // A 40-byte header declaring an empty room (lenVars = 0).
//! let mut payload = vec![0u8; 40];
//! payload[38] = 0; // lenVars = 0
//! let desc: RoomDesc = decode_payload(&payload, ByteOrder::Little).unwrap();
//! assert_eq!(desc.name, "");
//! assert!(desc.is_clean());
//! ```

mod model;
mod parse;

pub use model::{
    draw_cmd, draw_flags, DrawCmd, DrawPayload, Hotspot, HotspotState, LooseProp, LoosePropSpec,
    PictureOverlay, RoomDesc, RoomWarning, DRAW_CMD_DATA_OFFSET, DRAW_CMD_HEADER_LEN, HOTSPOT_LEN,
    HOTSPOT_STATE_LEN, LOOSE_PROP_LEN, PICTURE_OVERLAY_LEN, POINT_LEN, ROOM_REC_LEN,
};
pub use parse::{decode_draw_record, decode_payload, decode_stream, MAX_RECORD_GAP};

// Re-export the wire types callers need to read a header or a point, so the
// crate can be used without reaching into `palace-wire` directly.
pub use palace_wire::byteorder::ByteOrder;
pub use palace_wire::messages::{Point, RoomRec};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_room_is_clean() {
        let payload = vec![0u8; ROOM_REC_LEN];
        let desc = decode_payload(&payload, ByteOrder::Little).unwrap();
        assert_eq!(desc.name, "");
        assert!(desc.pictures.is_empty());
        assert!(desc.hotspots.is_empty());
        assert!(desc.loose_props.is_empty());
        assert!(desc.draw_cmds.is_empty());
        assert_eq!(desc.trailing_len, 0);
        assert!(desc.is_clean());
    }

    #[test]
    fn truncated_header_errors() {
        let payload = vec![0u8; ROOM_REC_LEN - 1];
        assert!(decode_payload(&payload, ByteOrder::Little).is_err());
    }

    #[test]
    fn negative_len_vars_errors() {
        let mut payload = vec![0u8; ROOM_REC_LEN];
        payload[38..40].copy_from_slice(&(-1i16).to_le_bytes());
        assert!(decode_payload(&payload, ByteOrder::Little).is_err());
    }

    /// A PATH record with an explicit `dataOfst`, two points and both PC5
    /// alpha tails. Its `cmdLength` is 26 = 10 prefix + 2 points * 4 + 8 tail.
    fn draw_record_bytes() -> Vec<u8> {
        let mut w = palace_wire::Writer::new(ByteOrder::Little);
        w.write_u16(0); // link.nextOfst
        w.write_u16(0); // link.reserved
        w.write_u16(draw_cmd::PATH as u16 | ((draw_flags::LAYER_FRONT as u16) << 8));
        w.write_u16(26); // cmdLength
        w.write_u16(10); // dataOfst (vestigial)
        w.write_i16(3); // penSize
        w.write_i16(1); // numPoints (2 points follow)
        w.write_u8(0x10);
        w.write_u8(0x10); // red, doubled
        w.write_u8(0x20);
        w.write_u8(0x20); // green, doubled
        w.write_u8(0x30);
        w.write_u8(0x30); // blue, doubled
        w.write_i16(10);
        w.write_i16(20); // p0 = (y 10, x 20)
        w.write_i16(1);
        w.write_i16(-2); // p1 = (y 1, x -2)
        w.write_u8(0x80);
        w.write_u8(1);
        w.write_u8(2);
        w.write_u8(3); // line RGBA
        w.write_u8(0x40);
        w.write_u8(4);
        w.write_u8(5);
        w.write_u8(6); // fill RGBA
        w.into_vec()
    }

    #[test]
    fn a_known_draw_record_parses_every_field() {
        let bytes = draw_record_bytes();
        assert_eq!(bytes.len(), 10 + 26);
        let (cmd, warnings) = decode_draw_record(&bytes, ByteOrder::Little);
        assert!(warnings.is_empty(), "a well-formed record warns nothing");
        assert_eq!(cmd.command, draw_cmd::PATH);
        assert_eq!(cmd.flags, draw_flags::LAYER_FRONT);
        assert_eq!(cmd.cmd_length, 26);
        assert_eq!(cmd.data_ofst, 10);
        assert!(cmd.is_front_layer());
        assert!(!cmd.is_filled());

        let payload = cmd.payload.expect("the operand decodes");
        assert_eq!(payload.pen_size, 3);
        assert_eq!(payload.num_points, 1);
        assert_eq!(payload.pen_rgb, [0x10, 0x20, 0x30]);
        assert_eq!(
            payload.points,
            vec![Point::new(10, 20), Point::new(1, -2)],
            "points are (y, x) in wire order, kept as sent"
        );
        assert_eq!(payload.line_rgba, Some([0x80, 1, 2, 3]));
        assert_eq!(payload.fill_rgba, Some([0x40, 4, 5, 6]));
    }

    #[test]
    fn a_truncated_draw_record_warns_without_panicking() {
        // Shorter than the 10-byte header at all.
        let (cmd, warnings) = decode_draw_record(&[0u8; 4], ByteOrder::Little);
        assert_eq!(cmd, DrawCmd::default());
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, RoomWarning::DrawHeaderTruncated { available: 4 })),
            "the missing header is reported, not guessed: {warnings:?}"
        );

        // A header whose operand runs past the slice end.
        let mut w = palace_wire::Writer::new(ByteOrder::Little);
        w.write_u16(0);
        w.write_u16(0);
        w.write_u16(draw_cmd::PATH as u16);
        w.write_u16(64); // claims 64 operand bytes
        w.write_u16(10);
        w.write_i16(3);
        w.write_i16(1);
        let bytes = w.into_vec();
        let (cmd, warnings) = decode_draw_record(&bytes, ByteOrder::Little);
        assert_eq!(cmd.command, draw_cmd::PATH);
        assert!(cmd.data.is_empty(), "no operand bytes are invented");
        assert!(cmd.payload.is_none());
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, RoomWarning::DrawDataOutOfRange { .. })),
            "the out-of-range operand is reported: {warnings:?}"
        );
    }
}

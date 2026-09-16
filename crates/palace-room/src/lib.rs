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
//! table in `$CORPUS/TAURI-CLIENT-SCOPE.md` (`palace-room` depends on
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
pub use parse::{decode_payload, decode_stream, MAX_RECORD_GAP};

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
}

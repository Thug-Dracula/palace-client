//! `MSG_DRAW` (`draw`): one drawing operation for the current room.
//!
//! The body is a **single** draw record, byte-for-byte the same layout as one
//! entry of a room description's linked `firstDrawCmd` list (protocol reference
//! §3.9; `OpenPalace` `PalaceClient.as:1608` `handleDrawCommand` reads the whole
//! frame body into one `PalaceDrawRecord.readData`). It is *not* a linked list
//! and it has no room header: the record starts at byte 0.
//!
//! ## Why this type only carries the bytes
//!
//! The record's fields are the draw-command model, and that model lives in
//! `palace-room` (which depends on `palace-wire`, not the other way round). To
//! keep the dependency direction intact, this message keeps the record body
//! **verbatim** and the caller parses it where the parsing types live
//! (`palace_room::decode_draw_record`). That also means a record whose operand
//! this milestone does not understand (`DC_Text`) is not lost.

use crate::byteorder::Reader;
use crate::error::Result;

/// A `MSG_DRAW` (`draw`) body: one raw draw record.
///
/// The buffer is the 10-byte `DrawRecord` header followed by its `cmdLength`
/// operand bytes, exactly as received. An empty body is carried too
/// rather than rejected: the record parser is what reports a truncated record,
/// and it never panics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Draw {
    /// The complete record, from its first header byte to the last operand byte.
    pub body: Vec<u8>,
}

impl Draw {
    /// Decode a `draw` body by taking every remaining byte as the record.
    ///
    /// The frame's payload length *is* the record length, so no in-body length
    /// field decides where it ends.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let len = r.remaining();
        Ok(Draw {
            body: r.read_bytes(len)?.to_vec(),
        })
    }

    /// The record body length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.body.len()
    }

    /// True when the record body is empty (no header at all).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::ByteOrder;

    #[test]
    fn a_draw_body_is_carried_verbatim_and_consumes_the_reader() {
        let body: Vec<u8> = (0..16).collect();
        let mut r = Reader::new(&body, ByteOrder::Little);
        let draw = Draw::decode(&mut r).expect("decode");
        assert_eq!(draw.body, body, "every byte of the record is kept");
        assert_eq!(draw.len(), 16);
        assert!(!draw.is_empty());
        assert!(r.is_empty(), "the body is exactly the frame payload");
    }

    #[test]
    fn an_empty_body_is_carried_not_rejected() {
        let mut r = Reader::new(&[], ByteOrder::Little);
        let draw = Draw::decode(&mut r).expect("an empty body is still a Draw");
        assert!(draw.is_empty());
        assert_eq!(draw, Draw::default());
    }
}

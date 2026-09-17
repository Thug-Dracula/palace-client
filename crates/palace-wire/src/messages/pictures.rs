//! Picture-overlay messages: `MSG_PICTMOVE`.
//!
//! A picture is addressed by the `HotspotID` of the hotspot that owns it, not
//! by the `picID` of its own `PictureRec`: the protocol reference calls the
//! field "the HotspotID of the picture itself, in that room" (:1416).

use super::user::Point;
use crate::byteorder::Reader;
use crate::error::Result;

/// `MSG_PICTMOVE` (`pLoc`): a `RoomID`, a `HotspotID` and the picture's new
/// absolute [`Point`], 8 bytes. Protocol reference §3.23 (:1400-1418). `pos` is
/// "the new position to which the picture is to be relocated" (:1418) — a whole
/// location, not a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictMove {
    /// Room containing the picture.
    pub room_id: i16,
    /// Hotspot id of the picture in that room.
    pub spot_id: i16,
    /// The picture's new offset, absolute rather than a delta.
    pub position: Point,
}

impl PictMove {
    /// Decode a `pLoc` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(PictMove {
            room_id: r.read_i16()?,
            spot_id: r.read_i16()?,
            position: Point::decode(r)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::{ByteOrder, Writer};

    #[test]
    fn pict_move_is_eight_bytes_room_then_spot_then_point() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(86);
            w.write_i16(105);
            Point::new(300, 400).encode(&mut w);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 8);
            let mut r = Reader::new(&bytes, order);
            let msg = PictMove::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(
                msg,
                PictMove {
                    room_id: 86,
                    spot_id: 105,
                    position: Point::new(300, 400)
                }
            );
        }
    }

    #[test]
    fn pict_move_reads_signed_ids() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(-2);
        w.write_i16(-32000);
        Point::new(-1, -2).encode(&mut w);
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let msg = PictMove::decode(&mut r).unwrap();
        assert_eq!(msg.room_id, -2);
        assert_eq!(msg.spot_id, -32000);
        assert_eq!(msg.position, Point::new(-1, -2));
    }

    #[test]
    fn truncated_pict_move_is_an_error() {
        let mut r = Reader::new(&[0u8; 7], ByteOrder::Little);
        assert!(PictMove::decode(&mut r).is_err());
    }
}

//! Hotspot messages: `MSG_DOORLOCK`, `MSG_DOORUNLOCK`, `MSG_SPOTSTATE`,
//! `MSG_SPOTNEW`, `MSG_SPOTDEL`, `MSG_SPOTMOVE`.
//!
//! The bodies are fixed-width and begin with a `RoomID` then a `HotspotID`,
//! both `sint16` (:221-223) — except `SPOTDEL`, which carries only the id, and
//! the bodyless `SPOTNEW`. The frame `refNum` is unused in all of them and
//! should be `0` (:811, :1902, :1914, :1936, :1948).

use super::user::Point;
use crate::byteorder::Reader;
use crate::error::{Result, WireError};

/// `MSG_DOORLOCK` (`lock`) / `MSG_DOORUNLOCK` (`unlo`): a `RoomID` then the
/// `HotspotID` of the door, 4 bytes. The two opcodes share this body and differ
/// only in meaning (:809). Protocol reference §3.8 (:807-822).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoorLock {
    /// Room containing the door.
    pub room_id: i16,
    /// Hotspot id of the door in that room.
    pub door_id: i16,
}

impl DoorLock {
    /// Decode a `lock`/`unlo` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(DoorLock {
            room_id: r.read_i16()?,
            door_id: r.read_i16()?,
        })
    }
}

/// `MSG_SPOTSTATE` (`sSta`): a `RoomID`, a `HotspotID` and the hotspot's new
/// `sint16 state`, 6 bytes. Protocol reference §3.41 (:1944-1962).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotState {
    /// Room containing the hotspot.
    pub room_id: i16,
    /// Hotspot id in that room.
    pub spot_id: i16,
    /// The new value for the hotspot's `state` field.
    pub state: i16,
}

impl SpotState {
    /// Decode an `sSta` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(SpotState {
            room_id: r.read_i16()?,
            spot_id: r.read_i16()?,
            state: r.read_i16()?,
        })
    }
}

/// `MSG_SPOTNEW` (`opSn`): a hotspot was created. The body is empty — the
/// protocol reference says "There are no parameters in this message, so the
/// length field should be 0 and the msg field should be empty" (:1938-1939).
/// Protocol reference §3.40 (:1930-1942). It carries no identity for the new
/// hotspot, so the new room arrives separately as `MSG_ROOMSETDESC` (:1932).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpotNew;

impl SpotNew {
    /// Decode an `opSn` body, which must be empty.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        if !r.is_empty() {
            return Err(WireError::TrailingBytes {
                remaining: r.remaining(),
            });
        }
        Ok(SpotNew)
    }
}

/// `MSG_SPOTDEL` (`opSd`): the `HotspotID` of the hotspot to delete, 2 bytes.
/// Protocol reference §3.38 (:1898-1908). The deletion is always "from the
/// current room" (:1900), so unlike its siblings this body carries no `RoomID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotDel {
    /// Hotspot id to remove from the room the client is showing.
    pub spot_id: i16,
}

impl SpotDel {
    /// Decode an `opSd` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(SpotDel {
            spot_id: r.read_i16()?,
        })
    }
}

/// `MSG_SPOTMOVE` (`coLs`): a `RoomID`, a `HotspotID` and the hotspot's new
/// absolute [`Point`], 8 bytes. Protocol reference §3.39 (:1910-1928). `pos` is
/// "the new position to which the hotspot is to be relocated" (:1928) — a whole
/// location, not a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpotMove {
    /// Room containing the hotspot.
    pub room_id: i16,
    /// Hotspot id in that room.
    pub spot_id: i16,
    /// The hotspot's new position, absolute rather than a delta.
    pub position: Point,
}

impl SpotMove {
    /// Decode a `coLs` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(SpotMove {
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
    fn door_lock_is_four_bytes_room_then_door() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(887);
            w.write_i16(1);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 4);
            let mut r = Reader::new(&bytes, order);
            let msg = DoorLock::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(
                msg,
                DoorLock {
                    room_id: 887,
                    door_id: 1
                }
            );
        }
    }

    #[test]
    fn door_lock_reads_signed_ids() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(-2);
        w.write_i16(-32000);
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let msg = DoorLock::decode(&mut r).unwrap();
        assert_eq!(msg.room_id, -2, "RoomID is a sint16");
        assert_eq!(msg.door_id, -32000, "HotspotID is a sint16");
    }

    #[test]
    fn spot_state_is_six_bytes_room_then_spot_then_state() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(86);
            w.write_i16(105);
            w.write_i16(1);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 6);
            let mut r = Reader::new(&bytes, order);
            let msg = SpotState::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(
                msg,
                SpotState {
                    room_id: 86,
                    spot_id: 105,
                    state: 1
                }
            );
        }
    }

    #[test]
    fn truncated_spot_messages_are_errors() {
        let mut r = Reader::new(&[0u8; 3], ByteOrder::Little);
        assert!(DoorLock::decode(&mut r).is_err());
        let mut r = Reader::new(&[0u8; 5], ByteOrder::Little);
        assert!(SpotState::decode(&mut r).is_err());
        let mut r = Reader::new(&[0u8; 4], ByteOrder::Little);
        assert!(SpotState::decode(&mut r).is_err());
    }

    #[test]
    fn spot_new_is_an_empty_body() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut r = Reader::new(&[], order);
            assert_eq!(SpotNew::decode(&mut r).unwrap(), SpotNew);
            assert!(r.is_empty());
        }
    }

    #[test]
    fn a_spot_new_with_any_body_is_malformed() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut r = Reader::new(&[0u8; 1], order);
            assert!(
                SpotNew::decode(&mut r).is_err(),
                "opSn declares length 0 (:1938)"
            );
        }
    }

    #[test]
    fn spot_del_is_two_bytes_with_no_room() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(-7);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 2);
            let mut r = Reader::new(&bytes, order);
            let msg = SpotDel::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(msg.spot_id, -7, "HotspotID is a signed sint16");
        }
        let mut r = Reader::new(&[0u8; 1], ByteOrder::Little);
        assert!(SpotDel::decode(&mut r).is_err());
    }

    #[test]
    fn spot_move_is_eight_bytes_room_then_spot_then_point() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(887);
            w.write_i16(33);
            Point::new(240, 120).encode(&mut w);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 8);
            let mut r = Reader::new(&bytes, order);
            let msg = SpotMove::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(msg.room_id, 887);
            assert_eq!(msg.spot_id, 33);
            assert_eq!(msg.position, Point::new(240, 120), "the point is (v, h)");
        }
        let mut r = Reader::new(&[0u8; 7], ByteOrder::Little);
        assert!(SpotMove::decode(&mut r).is_err());
    }
}

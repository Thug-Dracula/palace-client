//! Hotspot-state messages: `MSG_DOORLOCK`, `MSG_DOORUNLOCK`, `MSG_SPOTSTATE`.
//!
//! All three bodies are fixed-width and begin with a `RoomID` then a
//! `HotspotID`, both `sint16` (:221-223). The frame `refNum` is unused in all
//! three and should be `0` (:811, :1948).

use crate::byteorder::Reader;
use crate::error::Result;

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
}

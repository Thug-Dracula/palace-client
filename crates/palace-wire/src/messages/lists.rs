//! `MSG_LISTOFALLROOMS` (`rLst`) and `MSG_LISTOFALLUSERS` (`uLst`), plus the
//! in-room user list `MSG_USERLIST` (`rprs`), which shares the `uLst` shape.

use crate::byteorder::Reader;
use crate::error::{Result, WireError};

/// One entry of `MSG_LISTOFALLROOMS`. Protocol reference §3.17.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomListRec {
    /// Room id. Stored as a 32-bit field even though room ids are 16-bit.
    pub room_id: i32,
    /// `RF_*` room flags.
    pub flags: u16,
    /// Number of users currently in the room.
    pub user_count: u16,
    /// Room name (`PString`, padded so the field length is a multiple of 4).
    pub name: String,
}

/// `MSG_LISTOFALLROOMS` response. `refNum` carries the room count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomList {
    /// Rooms, in the order the server sent them.
    pub rooms: Vec<RoomListRec>,
}

impl RoomList {
    /// Decode `count` room records (the frame's `refNum`).
    pub fn decode(count: i32, r: &mut Reader<'_>) -> Result<Self> {
        let count = require_count(count)?;
        let mut rooms = Vec::with_capacity(count);
        for _ in 0..count {
            rooms.push(RoomListRec {
                room_id: r.read_i32()?,
                flags: r.read_u16()?,
                user_count: r.read_u16()?,
                name: r.read_pstring_aligned()?,
            });
        }
        Ok(RoomList { rooms })
    }

    /// Room ids in wire order.
    pub fn ids(&self) -> Vec<i32> {
        self.rooms.iter().map(|r| r.room_id).collect()
    }
}

/// One entry of `MSG_LISTOFALLUSERS` (and of `MSG_USERLIST`).
///
/// Protocol reference §3.18: `UserID`, `sint16 flags`, `RoomID`,
/// `PString name` — the name padded so its total length is a multiple of 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListRec {
    /// User id.
    pub user_id: i32,
    /// `U_*` user flags.
    pub flags: u16,
    /// Id of the room the user is in.
    pub room_id: i16,
    /// User name.
    pub name: String,
}

/// `MSG_LISTOFALLUSERS` / `MSG_USERLIST` body. `refNum` carries the user count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserList {
    /// Users, in the order the server sent them.
    pub users: Vec<UserListRec>,
}

impl UserList {
    /// Decode `count` user records (the frame's `refNum`).
    pub fn decode(count: i32, r: &mut Reader<'_>) -> Result<Self> {
        let count = require_count(count)?;
        let mut users = Vec::with_capacity(count);
        for _ in 0..count {
            users.push(UserListRec {
                user_id: r.read_i32()?,
                flags: r.read_u16()?,
                room_id: r.read_i16()?,
                name: r.read_pstring_aligned()?,
            });
        }
        Ok(UserList { users })
    }
}

fn require_count(count: i32) -> Result<usize> {
    if count < 0 {
        return Err(WireError::ImplausibleLength {
            length: count as u32,
            max: crate::error::MAX_PAYLOAD_LEN,
        });
    }
    Ok(count as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::{ByteOrder, Writer};

    fn write_room(w: &mut Writer, id: i32, flags: u16, users: u16, name: &str) {
        w.write_i32(id);
        w.write_u16(flags);
        w.write_u16(users);
        w.write_pstring_aligned(name);
    }

    #[test]
    fn live_room_list_prefix_decodes() {
        // The first two records of the real capture: ids 186 and 202, both in
        // the Entrance room name family.
        let mut w = Writer::new(ByteOrder::Little);
        write_room(&mut w, 186, 0x0200, 2, "Entrance (All Language)");
        write_room(&mut w, 202, 0x0000, 0, "Entrance (All Language) 2");

        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let list = RoomList::decode(2, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(list.rooms.len(), 2);
        assert_eq!(list.rooms[0].room_id, 186);
        assert_eq!(list.rooms[0].flags, 0x0200);
        assert_eq!(list.rooms[0].user_count, 2);
        assert_eq!(list.rooms[0].name, "Entrance (All Language)");
        assert_eq!(list.rooms[1].room_id, 202);
        assert_eq!(list.ids(), vec![186, 202]);
    }

    #[test]
    fn aligned_names_have_expected_width() {
        assert_eq!(crate::byteorder::round_up_4(1 + 4), 8);
        assert_eq!(crate::byteorder::round_up_4(1 + 23), 24);
        assert_eq!(crate::byteorder::round_up_4(1 + 24), 28);
    }

    #[test]
    fn user_list_record_is_16_bytes_for_a_four_char_name() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(4);
        w.write_u16(0);
        w.write_i16(901);
        w.write_pstring_aligned("Rico");
        assert_eq!(w.len(), 16);
    }

    #[test]
    fn user_list_tolerates_garbage_padding() {
        // The live pserver pads with uninitialised bytes; the parser must skip
        // the padding rather than insist it is zero.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4i32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&901i16.to_le_bytes());
        bytes.extend_from_slice(&[4, b'R', b'i', b'c', b'o', 0x00, 0x6c, 0x65]);
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let list = UserList::decode(1, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(list.users[0].name, "Rico");
        assert_eq!(list.users[0].room_id, 901);
    }

    #[test]
    fn negative_counts_are_rejected() {
        let mut r = Reader::new(&[], ByteOrder::Little);
        assert!(RoomList::decode(-1, &mut r).is_err());
        assert!(UserList::decode(-1, &mut r).is_err());
    }

    #[test]
    fn truncated_list_is_an_error_not_a_panic() {
        let mut r = Reader::new(&[0u8; 3], ByteOrder::Little);
        assert!(RoomList::decode(1, &mut r).is_err());
    }
}

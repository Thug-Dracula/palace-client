//! User-related message bodies: the full `UserRec` and the small
//! notifications around it.

use crate::byteorder::{Reader, Writer};
use crate::error::{Result, WireError};

/// A signed 16-bit screen coordinate pair.
///
/// The protocol reference names the fields `v` (vertical) and `h`
/// (horizontal); Taj names them `y` and `x` in the same order. Coordinates are
/// absolute pixels — Palace has no DPI concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    /// Vertical coordinate (`y`).
    pub v: i16,
    /// Horizontal coordinate (`x`).
    pub h: i16,
}

impl Point {
    /// Construct a point.
    pub const fn new(v: i16, h: i16) -> Self {
        Point { v, h }
    }

    /// Decode a [`Point`].
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Point {
            v: r.read_i16()?,
            h: r.read_i16()?,
        })
    }

    /// Encode a [`Point`].
    pub fn encode(&self, w: &mut Writer) {
        w.write_i16(self.v);
        w.write_i16(self.h);
    }
}

/// An asset reference: an id plus the CRC that validates its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AssetSpec {
    /// Asset id.
    pub id: i32,
    /// Asset CRC.
    pub crc: u32,
}

impl AssetSpec {
    /// Number of prop slots in a [`UserRec`].
    pub const USER_PROP_SLOTS: usize = 9;

    /// Decode an [`AssetSpec`].
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(AssetSpec {
            id: r.read_i32()?,
            crc: r.read_u32()?,
        })
    }

    /// Encode an [`AssetSpec`].
    pub fn encode(&self, w: &mut Writer) {
        w.write_i32(self.id);
        w.write_u32(self.crc);
    }
}

/// The full user record, as carried by `MSG_USERLIST` (`rprs`) entries and by
/// `MSG_USERNEW` (`nprs`). Protocol reference §3.49.
///
/// Encoded size is always 124 bytes — the `propSpec` array is sent at full
/// length even when only `nbr_props` entries are meaningful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRec {
    /// User id.
    pub user_id: i32,
    /// Screen position of the user's avatar.
    pub room_pos: Point,
    /// Prop slots, always nine on the wire.
    pub prop_spec: [AssetSpec; AssetSpec::USER_PROP_SLOTS],
    /// Room the user is in (redundant with the room we are in).
    pub room_id: i16,
    /// Face selector, 0-15.
    pub face_nbr: i16,
    /// Colour selector, 0-15.
    pub color_nbr: i16,
    /// Unused.
    pub away_flag: i16,
    /// Unused.
    pub open_to_msgs: i16,
    /// Number of meaningful entries in [`UserRec::prop_spec`].
    pub nbr_props: i16,
    /// User name, `Str31`.
    pub name: String,
}

impl UserRec {
    /// Encoded size, in bytes.
    pub const LEN: usize = 124;

    /// Decode a full user record.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let user_id = r.read_i32()?;
        let room_pos = Point::decode(r)?;
        let mut prop_spec = [AssetSpec::default(); AssetSpec::USER_PROP_SLOTS];
        for slot in &mut prop_spec {
            *slot = AssetSpec::decode(r)?;
        }
        let rec = UserRec {
            user_id,
            room_pos,
            prop_spec,
            room_id: r.read_i16()?,
            face_nbr: r.read_i16()?,
            color_nbr: r.read_i16()?,
            away_flag: r.read_i16()?,
            open_to_msgs: r.read_i16()?,
            nbr_props: r.read_i16()?,
            name: r.read_str31()?,
        };
        Ok(rec)
    }

    /// Encode a full user record.
    pub fn encode(&self, w: &mut Writer) {
        w.write_i32(self.user_id);
        self.room_pos.encode(w);
        for slot in &self.prop_spec {
            slot.encode(w);
        }
        w.write_i16(self.room_id);
        w.write_i16(self.face_nbr);
        w.write_i16(self.color_nbr);
        w.write_i16(self.away_flag);
        w.write_i16(self.open_to_msgs);
        w.write_i16(self.nbr_props);
        w.write_str31(&self.name);
    }
}

/// `MSG_USERNEW` (`nprs`): `refNum` is the new user's id, body is a [`UserRec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNew {
    /// Id of the user who entered.
    pub user_id: i32,
    /// Their full record.
    pub record: UserRec,
}

impl UserNew {
    /// Decode an `nprs` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserNew {
            user_id: ref_num,
            record: UserRec::decode(r)?,
        })
    }
}

/// `MSG_USEREXIT` (`eprs`): empty body, `refNum` is the departing user id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserExit {
    /// Id of the user who left the room.
    pub user_id: i32,
}

impl UserExit {
    /// Build from a frame `refNum`.
    pub fn from_ref_num(ref_num: i32) -> Self {
        UserExit { user_id: ref_num }
    }
}

/// `MSG_USERMOVE` (`uLoc`): `refNum` is the user id, body is a [`Point`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserMove {
    /// Id of the user who moved.
    pub user_id: i32,
    /// New position.
    pub position: Point,
}

impl UserMove {
    /// Decode a `uLoc` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserMove {
            user_id: ref_num,
            position: Point::decode(r)?,
        })
    }
}

/// `MSG_USERSTATUS` (`uSta`): `refNum` is the user id, the body begins with a
/// `sint16` flag word.
///
/// The live server sends 44 bytes here while Taj and the protocol reference
/// only describe the leading flag word, so the remainder is preserved verbatim
/// in [`UserStatus::raw`] rather than guessed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserStatus {
    /// Id of the user this status applies to (always our own id in practice).
    pub user_id: i32,
    /// `U_*` flags.
    pub flags: u16,
    /// The complete body, undecoded past the flag word.
    pub raw: Vec<u8>,
}

impl UserStatus {
    /// Decode a `uSta` body from the reader's remaining bytes.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let raw = r.remaining_slice().to_vec();
        if raw.len() < 2 {
            return Err(WireError::UnexpectedEof {
                needed: 2,
                available: raw.len(),
            });
        }
        let flags = r.read_u16()?;
        r.skip(r.remaining())?;
        Ok(UserStatus {
            user_id: ref_num,
            flags,
            raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::ByteOrder;

    fn sample_user(w: &mut Writer, id: i32, name: &str) {
        w.write_i32(id);
        Point::new(162, 367).encode(w);
        for i in 0..AssetSpec::USER_PROP_SLOTS {
            AssetSpec {
                id: i as i32,
                crc: 0,
            }
            .encode(w);
        }
        w.write_i16(901);
        w.write_i16(0);
        w.write_i16(0);
        w.write_i16(0);
        w.write_i16(0);
        w.write_i16(0);
        w.write_str31(name);
    }

    #[test]
    fn user_rec_is_124_bytes_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            sample_user(&mut w, 3, "Rico");
            assert_eq!(w.len(), UserRec::LEN);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            let rec = UserRec::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(rec.user_id, 3);
            assert_eq!(rec.name, "Rico");
            assert_eq!(rec.room_pos, Point::new(162, 367));
            assert_eq!(rec.room_id, 901);
            assert_eq!(rec.prop_spec[8].id, 8);
        }
    }

    #[test]
    fn user_new_uses_frame_ref_num_as_user_id() {
        let mut w = Writer::new(ByteOrder::Little);
        sample_user(&mut w, 3, "Rico");
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let n = UserNew::decode(3, &mut r).unwrap();
        assert_eq!(n.user_id, 3);
        assert_eq!(n.record.name, "Rico");
    }

    #[test]
    fn user_status_keeps_undecoded_tail() {
        // The live `uSta` has a 44-byte body; only the first two bytes are
        // defined by the reference, so the rest must survive untouched.
        let mut body = vec![0x08, 0x00];
        body.extend_from_slice(&[0xabu8; 42]);
        let mut r = Reader::new(&body, ByteOrder::Little);
        let s = UserStatus::decode(3, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(s.flags, 0x0008);
        assert_eq!(s.raw.len(), 44);
    }

    #[test]
    fn truncated_user_status_is_an_error() {
        let mut r = Reader::new(&[0u8], ByteOrder::Little);
        assert!(UserStatus::decode(1, &mut r).is_err());
    }
}

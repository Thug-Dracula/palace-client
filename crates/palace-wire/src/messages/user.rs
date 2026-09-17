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

/// The wire bound on `nbrProps` for `USERPROP`/`USERDESC` (protocol reference
/// §3.55 :2034 and §3.56 :2175 both say 0-9).
const MAX_WIRE_PROPS: i32 = 9;

/// Decode the `sint32 nbrProps` + `AssetSpec[nbrProps]` tail shared by
/// [`UserProp`] and [`UserDesc`].
///
/// The declared count is checked against the bytes actually present before any
/// allocation: a count that disagrees with the body length is an error, never a
/// silent truncation or a zero-padded list.
fn decode_prop_list(r: &mut Reader<'_>, nbr_props: i32) -> Result<Vec<AssetSpec>> {
    if !(0..=MAX_WIRE_PROPS).contains(&nbr_props) {
        return Err(WireError::ImplausibleLength {
            length: nbr_props as u32,
            max: MAX_WIRE_PROPS as u32,
        });
    }
    let needed = nbr_props as usize * 8;
    let available = r.remaining();
    if available < needed {
        return Err(WireError::UnexpectedEof { needed, available });
    }
    if available > needed {
        return Err(WireError::TrailingBytes {
            remaining: available - needed,
        });
    }
    let mut props = Vec::with_capacity(nbr_props as usize);
    for _ in 0..nbr_props {
        props.push(AssetSpec::decode(r)?);
    }
    Ok(props)
}

/// `MSG_USERFACE` (`usrF`): `refNum` is the user id, body is a `sint16` face
/// selector. Protocol reference §3.46 (:2053-2061).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserFace {
    /// The user whose face changed — the frame `refNum`, not a body field.
    pub user_id: i32,
    /// Face selector, 0-15.
    pub face_nbr: i16,
}

impl UserFace {
    /// Decode a `usrF` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserFace {
            user_id: ref_num,
            face_nbr: r.read_i16()?,
        })
    }
}

/// `MSG_USERCOLOR` (`usrC`): `refNum` is the user id, body is a `sint16`
/// colour selector. Protocol reference §3.44 (:2005-2013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserColor {
    /// The user whose colour changed — the frame `refNum`, not a body field.
    pub user_id: i32,
    /// Colour selector, 0-15.
    pub color_nbr: i16,
}

impl UserColor {
    /// Decode a `usrC` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserColor {
            user_id: ref_num,
            color_nbr: r.read_i16()?,
        })
    }
}

/// `MSG_USERNAME` (`usrN`): `refNum` is the user id, body is a bare `PString`
/// holding the new name. Protocol reference §3.52 (:2136-2146).
///
/// The body is a **plain** `PString` — one length byte then that many characters
/// — and *not* the 4-byte-aligned form used for a name embedded in a fixed
/// record (`RoomListRec` :1167, `UserRec` :1235). The reference defines
/// `struct PString { uint8 length; char chars[length]; }` (:164-167) and only
/// marks a field `/* padded to align length */` where alignment is required;
/// `struct ClientMsg_userName { PString name; }` carries no such marker, so
/// there is no padding to write or skip. The same body encodes the server's
/// broadcast of a successful rename and its revert-on-failure reply, so a
/// received message applies verbatim.
///
/// `refNum` is the id of the user being renamed: the server ignores it on a
/// request but a client needs it to apply another user's rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserName {
    /// The user whose name changed — the frame `refNum`.
    pub user_id: i32,
    /// The new name, decoded as Latin-1.
    pub name: String,
}

impl UserName {
    /// Decode a `usrN` body: a plain `PString`.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserName {
            user_id: ref_num,
            name: r.read_pstring()?,
        })
    }

    /// Encode a `usrN` body: a plain `PString`.
    pub fn encode(&self, w: &mut Writer) {
        w.write_pstring(&self.name);
    }
}

/// `MSG_USERPROP` (`usrP`): `refNum` is the user id, body is the user's complete
/// worn list — `sint32 nbrProps` then `AssetSpec[nbrProps]` (:2170-2177). The
/// body always carries the whole list and omits unused slots, so it replaces the
/// previous props rather than extending them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProp {
    /// The user whose worn props changed — the frame `refNum`.
    pub user_id: i32,
    /// The complete list, in order.
    pub props: Vec<AssetSpec>,
}

impl UserProp {
    /// Decode a `usrP` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let nbr_props = r.read_i32()?;
        Ok(UserProp {
            user_id: ref_num,
            props: decode_prop_list(r, nbr_props)?,
        })
    }
}

/// `MSG_USERDESC` (`usrD`): `refNum` is the user id, body is face, colour and
/// the complete worn list in one message (:2023-2028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDesc {
    /// The user whose appearance changed — the frame `refNum`.
    pub user_id: i32,
    /// Face selector, 0-15.
    pub face_nbr: i16,
    /// Colour selector, 0-15.
    pub color_nbr: i16,
    /// The complete worn list, in order.
    pub props: Vec<AssetSpec>,
}

impl UserDesc {
    /// Decode a `usrD` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let face_nbr = r.read_i16()?;
        let color_nbr = r.read_i16()?;
        let nbr_props = r.read_i32()?;
        Ok(UserDesc {
            user_id: ref_num,
            face_nbr,
            color_nbr,
            props: decode_prop_list(r, nbr_props)?,
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

    fn prop(id: i32, crc: u32) -> AssetSpec {
        AssetSpec { id, crc }
    }

    #[test]
    fn user_face_and_color_read_a_single_sint16_body() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i16(7);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            assert_eq!(
                UserFace::decode(42, &mut r).unwrap(),
                UserFace {
                    user_id: 42,
                    face_nbr: 7
                }
            );
            assert!(r.is_empty());

            let mut w = Writer::new(order);
            w.write_i16(3);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            assert_eq!(
                UserColor::decode(42, &mut r).unwrap(),
                UserColor {
                    user_id: 42,
                    color_nbr: 3
                }
            );
            assert!(r.is_empty());
        }
    }

    #[test]
    fn user_name_reads_a_plain_pstring_and_uses_the_frame_ref_num() {
        // `struct ClientMsg_userName { PString name; }` :2144-2146. A bare
        // PString has no alignment padding, so the body is exactly
        // `len + chars`; `decode_exact` in the parent module relies on that.
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_pstring("Rico");
            let bytes = w.into_vec();
            assert_eq!(bytes, vec![4, b'R', b'i', b'c', b'o']);
            let mut r = Reader::new(&bytes, order);
            let msg = UserName::decode(42, &mut r).unwrap();
            assert!(r.is_empty(), "a plain PString consumes the whole body");
            assert_eq!(msg.user_id, 42, "the frame refNum is the renamed user");
            assert_eq!(msg.name, "Rico");
        }
    }

    #[test]
    fn user_name_round_trips_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let want = UserName {
                user_id: 7,
                name: "Balamb".to_string(),
            };
            let mut w = Writer::new(order);
            want.encode(&mut w);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            assert_eq!(UserName::decode(7, &mut r).unwrap(), want);
            assert!(r.is_empty());
        }
    }

    #[test]
    fn user_name_is_latin1() {
        let body = [4u8, b'c', b'a', b'f', 0xe9];
        let mut r = Reader::new(&body, ByteOrder::Little);
        assert_eq!(UserName::decode(1, &mut r).unwrap().name, "caf\u{e9}");
    }

    #[test]
    fn user_prop_decodes_the_complete_variable_list() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_i32(2);
            prop(0x1122_3344, 0xdead_beef).encode(&mut w);
            prop(0xA26F_9DE3u32 as i32, 0).encode(&mut w);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 4 + 8 * 2);
            let mut r = Reader::new(&bytes, order);
            let msg = UserProp::decode(9, &mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(msg.user_id, 9);
            assert_eq!(
                msg.props,
                vec![
                    prop(0x1122_3344, 0xdead_beef),
                    prop(0xA26F_9DE3u32 as i32, 0)
                ]
            );
        }
    }

    #[test]
    fn user_desc_reads_face_color_and_props_together() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(11);
        w.write_i16(5);
        w.write_i32(1);
        prop(77, 88).encode(&mut w);
        let bytes = w.into_vec();
        assert_eq!(bytes.len(), 8 + 8);
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let msg = UserDesc::decode(3, &mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(msg.user_id, 3);
        assert_eq!(msg.face_nbr, 11);
        assert_eq!(msg.color_nbr, 5);
        assert_eq!(msg.props, vec![prop(77, 88)]);
    }

    #[test]
    fn user_prop_rejects_a_count_that_disagrees_with_the_body_length() {
        // Claims two props, carries one.
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(2);
        prop(1, 2).encode(&mut w);
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        assert!(matches!(
            UserProp::decode(1, &mut r),
            Err(WireError::UnexpectedEof { .. })
        ));

        // Claims one prop, carries two.
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(1);
        prop(1, 2).encode(&mut w);
        prop(3, 4).encode(&mut w);
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        assert!(matches!(
            UserProp::decode(1, &mut r),
            Err(WireError::TrailingBytes { remaining: 8 })
        ));
    }

    #[test]
    fn user_prop_rejects_negative_and_oversized_counts() {
        for count in [-1i32, 10] {
            let mut w = Writer::new(ByteOrder::Little);
            w.write_i32(count);
            w.write_bytes(&[0u8; 80]);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, ByteOrder::Little);
            assert!(
                matches!(
                    UserProp::decode(1, &mut r),
                    Err(WireError::ImplausibleLength { max: 9, .. })
                ),
                "count {count} is outside 0-9"
            );
        }
    }

    #[test]
    fn user_prop_with_zero_props_is_an_empty_list() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(0);
        let bytes = w.into_vec();
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let msg = UserProp::decode(4, &mut r).unwrap();
        assert_eq!(msg.user_id, 4);
        assert!(msg.props.is_empty());
    }
}

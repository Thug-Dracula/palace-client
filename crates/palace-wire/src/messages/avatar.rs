//! Type 1 avatar messages and the `EXTENDEDINFO` reply that advertises limits.
//!
//! Layouts and constants are taken from the Palace PP SDK headers and confirmed
//! against the compiled Unix `pserver`'s debug type table (see the spec note
//! `$CORPUS/TYPE1-AVATARS.md`):
//!
//! * `ClientMsg_avatarFlags { uint16 avatarFlags }` — `m-protocol.h:90-92`,
//!   debug struct `t155`.
//! * `ClientMsg_avatarQuery { uint8 hash[20] }` — `m-protocol.h:94-96`,
//!   debug struct `t156`.
//! * `ClientMsg_avatarSend { uint8 hash[20]; uint32 flags; uint32 dataSize;
//!   uint8 data[dataSize] }` — `m-protocol.h:98-105`, debug struct `t157`.
//! * `ExtendedInfoAvatar { uint32 formats; uint16 maxPayload; uint16 maxHeight;
//!   uint16 maxWidth; uint16 reserved }` — `mansion.h:530-541`.
//! * `ClientMsg_extendedInfo_request { uint32 flags; ExtendedInfo info[] }` and
//!   `ClientMsg_extendedInfo_response { ExtendedInfo info[] }` — debug structs
//!   `t167`/`t169`. The response has no leading flags word.
//!
//! The 20-byte avatar hash is a SHA-1 digest of the image bytes. The spec note
//! did not name the algorithm, but the compiled server exports `computeAvatarHash`
//! and `verifyAvatarHash` next to `SHA1_Init`/`SHA1_Update`/`SHA1_Final`
//! (`task34_pserver.strings`), so SHA-1 is the server's own choice.

use std::fmt;

use crate::byteorder::{ByteOrder, Reader, Writer};
use crate::error::{Result, WireError};
use crate::frame::Frame;
use crate::opcode;

/// Length of a Type 1 avatar's content hash.
pub const AVATAR_HASH_LEN: usize = 20;

/// `avatarType` for a classic prop-based avatar (`mansion.h:303`).
pub const AT_PROP: i16 = 0;
/// `avatarType` for a Type 1 single-image avatar (`mansion.h:304`).
pub const AT_AVATAR: i16 = 1;

/// `AVFORM_GIF` (`mansion.h:531`).
pub const AVFORM_GIF: u32 = 0x0001;
/// `AVFORM_JPEG` (`mansion.h:532`).
pub const AVFORM_JPEG: u32 = 0x0002;
/// `AVFORM_PNG99A` (`mansion.h:533`).
pub const AVFORM_PNG99A: u32 = 0x0004;
/// `AVFORM_MNG` (`mansion.h:534`).
pub const AVFORM_MNG: u32 = 0x0008;
/// `AVFORM_FLASH` (`mansion.h:535`).
pub const AVFORM_FLASH: u32 = 0x0010;

/// `AVATAR_SEND_DATA`: the `sAva` body carries image bytes (`m-protocol.h:101`).
pub const AVATAR_SEND_DATA: u32 = 0;
/// `AVATAR_SEND_URL`: the `sAva` body carries a URL (`m-protocol.h:102`).
pub const AVATAR_SEND_URL: u32 = 1;

/// `SI_AVATAR` request flag (`mansion.h:556`).
pub const SI_AVATAR: u32 = 0x0000_0800;
/// `SI_AVATAR_URL` request flag (`mansion.h:545`).
pub const SI_AVATAR_URL: u32 = 0x0000_0001;
/// `SI_HTTP_URL` request flag (`mansion.h:552`).
pub const SI_HTTP_URL: u32 = 0x0000_0040;

/// `SI_INF_AVATAR` — the `'AVAT'` reply key (`mansion.h:578`).
pub const SI_INF_AVATAR: i32 = 0x4156_4154;
/// `SI_INF_AURL` — the `'AURL'` default avatar URL key (`mansion.h:567`).
pub const SI_INF_AURL: i32 = 0x4155_524c;
/// `SI_INF_HURL` — the `'HURL'` HTTP media URL key (`mansion.h:573`).
pub const SI_INF_HURL: i32 = 0x4855_524c;

/// `AF_HorizontalFlip` (`mansion.h:306`).
pub const AF_HORIZONTAL_FLIP: u16 = 0x0001;
/// `AF_VerticalFlip` (`mansion.h:307`).
pub const AF_VERTICAL_FLIP: u16 = 0x0002;
/// `AF_InhibitAnimation` (`mansion.h:308`).
pub const AF_INHIBIT_ANIMATION: u16 = 0x0004;
/// `AF_ValidFlags` (`mansion.h:309`).
pub const AF_VALID_FLAGS: u16 = 0x0007;

/// A 20-byte Type 1 avatar content hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct AvatarHash(pub [u8; AVATAR_HASH_LEN]);

impl AvatarHash {
    /// The all-zero hash, which the server uses for "no Type 1 avatar".
    pub const ZERO: AvatarHash = AvatarHash([0u8; AVATAR_HASH_LEN]);

    /// Wrap raw hash bytes.
    #[must_use]
    pub const fn new(bytes: [u8; AVATAR_HASH_LEN]) -> Self {
        AvatarHash(bytes)
    }

    /// The raw hash bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; AVATAR_HASH_LEN] {
        &self.0
    }

    /// Whether every byte is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|byte| *byte == 0)
    }

    /// Lowercase hex, the form the `palace://type1-avatar/` route keys on.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(AVATAR_HASH_LEN * 2);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }

    /// Parse a 40-character hex string, case-insensitively.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != AVATAR_HASH_LEN * 2 {
            return None;
        }
        let mut out = [0u8; AVATAR_HASH_LEN];
        let bytes = text.as_bytes();
        for (index, slot) in out.iter_mut().enumerate() {
            let high = hex_nibble(bytes[index * 2])?;
            let low = hex_nibble(bytes[index * 2 + 1])?;
            *slot = (high << 4) | low;
        }
        Some(AvatarHash(out))
    }

    /// Decode a raw 20-byte hash.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let bytes = r.read_bytes(AVATAR_HASH_LEN)?;
        let mut out = [0u8; AVATAR_HASH_LEN];
        out.copy_from_slice(bytes);
        Ok(AvatarHash(out))
    }

    /// Encode a raw 20-byte hash.
    pub fn encode(&self, w: &mut Writer) {
        w.write_bytes(&self.0);
    }
}

impl fmt::Debug for AvatarHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AvatarHash({})", self.to_hex())
    }
}

impl fmt::Display for AvatarHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One `ExtendedInfo` entry: a 4-character key, a length, then its bytes.
///
/// `ExtendedInfo { sint32 id; sint32 length; uint8 buf[length] }`
/// (`m-protocol.h:130-134`, debug struct `t165`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtendedInfoEntry {
    /// The 4-character key as a little-endian integer (`'AVAT'`).
    pub id: i32,
    /// The entry's bytes.
    pub data: Vec<u8>,
}

impl ExtendedInfoEntry {
    /// Decode one entry from the reply stream.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let id = r.read_i32()?;
        let length = r.read_i32()?;
        if length < 0 {
            return Err(WireError::ImplausibleLength {
                length: length as u32,
                max: u32::MAX,
            });
        }
        let length = length as usize;
        let available = r.remaining();
        if length > available {
            return Err(WireError::UnexpectedEof {
                needed: length,
                available,
            });
        }
        let data = r.read_bytes(length)?.to_vec();
        Ok(ExtendedInfoEntry { id, data })
    }

    /// The entry's key as four ASCII characters, when printable.
    ///
    /// The key is an integer whose **big-endian** bytes spell the four
    /// characters (the same convention [`Opcode`](crate::opcode::Opcode) uses),
    /// so `SI_INF_AVATAR` renders as `AVAT`.
    #[must_use]
    pub fn key(&self) -> String {
        self.id
            .to_be_bytes()
            .iter()
            .map(|byte| {
                if (0x20..0x7f).contains(byte) {
                    *byte as char
                } else {
                    '.'
                }
            })
            .collect()
    }

    /// The entry's bytes as a NUL-terminated C string, without the terminator.
    #[must_use]
    pub fn cstring(&self) -> String {
        let end = self
            .data
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.data.len());
        String::from_utf8_lossy(&self.data[..end]).into_owned()
    }
}

/// The `EXTENDEDINFO` (`sInf`) reply: a bare sequence of [`ExtendedInfoEntry`].
///
/// `ClientMsg_extendedInfo_response { ExtendedInfo info[] }` (debug struct
/// `t169`) has no leading flags word; the request does. This decoder reads only
/// the reply shape, which is what a server sends.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExtendedInfoReply {
    /// The reply's entries, in arrival order.
    pub entries: Vec<ExtendedInfoEntry>,
}

impl ExtendedInfoReply {
    /// Decode every entry the body carries.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let mut entries = Vec::new();
        while !r.is_empty() {
            entries.push(ExtendedInfoEntry::decode(r)?);
        }
        Ok(ExtendedInfoReply { entries })
    }

    /// The entry for `id`, if the server returned it.
    #[must_use]
    pub fn get(&self, id: i32) -> Option<&ExtendedInfoEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The decoded Type 1 avatar limits (`'AVAT'`), if present and well-formed.
    #[must_use]
    pub fn avatar(&self, order: ByteOrder) -> Option<ExtendedInfoAvatar> {
        ExtendedInfoAvatar::decode(self.get(SI_INF_AVATAR)?.data.as_slice(), order).ok()
    }

    /// The `'AURL'` default avatar URL, if the server returned one.
    #[must_use]
    pub fn avatar_url(&self) -> Option<String> {
        let url = self.get(SI_INF_AURL)?.cstring();
        (!url.is_empty()).then_some(url)
    }

    /// The `'HURL'` media base URL, if the server returned one.
    #[must_use]
    pub fn http_url(&self) -> Option<String> {
        let url = self.get(SI_INF_HURL)?.cstring();
        (!url.is_empty()).then_some(url)
    }
}

/// The `'AVAT'` block: which image formats the server accepts and its caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExtendedInfoAvatar {
    /// Bit mask of permitted formats (`AVFORM_*`).
    pub formats: u32,
    /// Maximum payload, in kilobytes.
    pub max_payload: u16,
    /// Maximum image height, in pixels.
    pub max_height: u16,
    /// Maximum image width, in pixels.
    pub max_width: u16,
    /// Reserved; ignored.
    pub reserved: u16,
}

impl ExtendedInfoAvatar {
    /// Encoded size, in bytes.
    pub const LEN: usize = 12;

    /// Decode the 12-byte `ExtendedInfoAvatar` body in the session byte order.
    pub fn decode(data: &[u8], order: ByteOrder) -> Result<Self> {
        let mut r = Reader::new(data, order);
        if data.len() < Self::LEN {
            return Err(WireError::UnexpectedEof {
                needed: Self::LEN,
                available: data.len(),
            });
        }
        Ok(ExtendedInfoAvatar {
            formats: r.read_u32()?,
            max_payload: r.read_u16()?,
            max_height: r.read_u16()?,
            max_width: r.read_u16()?,
            reserved: r.read_u16()?,
        })
    }

    /// Encode the 12-byte body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.formats);
        w.write_u16(self.max_payload);
        w.write_u16(self.max_height);
        w.write_u16(self.max_width);
        w.write_u16(self.reserved);
    }
}

/// Build the client's `EXTENDEDINFO` request: a leading flags word.
///
/// `ClientMsg_extendedInfo_request { uint32 flags; ExtendedInfo info[] }`
/// (`m-protocol.h:136-139`, debug struct `t167`). The `info` array is empty on
/// a request: the flags name what the client wants.
pub fn extended_info_request_frame(flags: u32, order: ByteOrder) -> Frame {
    let mut w = Writer::with_capacity(order, 4);
    w.write_u32(flags);
    Frame::new(opcode::EXTENDEDINFO, 0, w.into_vec())
}

/// `MSG_AVATARQUERY` (`qAva`): a client asks whether the server holds a hash.
///
/// `ClientMsg_avatarQuery { uint8 hash[20] }` (`m-protocol.h:94-96`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvatarQuery {
    /// The hash being asked about.
    pub hash: AvatarHash,
}

impl AvatarQuery {
    /// Decode a `qAva` body.
    pub fn decode(_ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(AvatarQuery {
            hash: AvatarHash::decode(r)?,
        })
    }

    /// Encode a `qAva` body.
    pub fn encode(&self, w: &mut Writer) {
        self.hash.encode(w);
    }

    /// Build a complete `qAva` frame.
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        let mut w = Writer::with_capacity(order, AVATAR_HASH_LEN);
        self.encode(&mut w);
        Ok(Frame::new(opcode::AVATARQUERY, 0, w.into_vec()))
    }
}

/// `MSG_AVATARSEND` (`sAva`): the avatar itself, as bytes or a URL.
///
/// `ClientMsg_avatarSend { uint8 hash[20]; uint32 flags; uint32 dataSize;
/// uint8 data[dataSize] }` (`m-protocol.h:98-105`). The same shape travels in
/// both directions: a client uploads with it and a server answers a `qAva` with
/// it. `flags` is [`AVATAR_SEND_DATA`] or [`AVATAR_SEND_URL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarSend {
    /// The content hash the payload belongs to.
    pub hash: AvatarHash,
    /// [`AVATAR_SEND_DATA`] or [`AVATAR_SEND_URL`].
    pub flags: u32,
    /// The image bytes, or the URL bytes.
    pub data: Vec<u8>,
}

impl AvatarSend {
    /// Decode an `sAva` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let hash = AvatarHash::decode(r)?;
        let flags = r.read_u32()?;
        let data_size = r.read_u32()? as usize;
        let available = r.remaining();
        if data_size > available {
            return Err(WireError::UnexpectedEof {
                needed: data_size,
                available,
            });
        }
        let data = r.read_bytes(data_size)?.to_vec();
        Ok(AvatarSend { hash, flags, data })
    }

    /// Encode an `sAva` body.
    pub fn encode(&self, w: &mut Writer) {
        self.hash.encode(w);
        w.write_u32(self.flags);
        w.write_u32(self.data.len() as u32);
        w.write_bytes(&self.data);
    }

    /// Build a complete `sAva` frame.
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        let mut w = Writer::with_capacity(order, 28 + self.data.len());
        self.encode(&mut w);
        Ok(Frame::new(opcode::AVATARSEND, 0, w.into_vec()))
    }

    /// Whether the payload is a URL rather than image bytes.
    #[must_use]
    pub fn is_url(&self) -> bool {
        self.flags == AVATAR_SEND_URL
    }

    /// The URL text, when [`AvatarSend::is_url`] and the bytes are UTF-8.
    #[must_use]
    pub fn url(&self) -> Option<String> {
        if !self.is_url() {
            return None;
        }
        let end = self
            .data
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.data.len());
        String::from_utf8(self.data[..end].to_vec()).ok()
    }
}

/// `MSG_AVATARFLAGS` (`fAva`): a user's Type 1 avatar flags.
///
/// The SDK request body is `uint16 avatarFlags` (`m-protocol.h:90-92`); the
/// frame `refNum` names the user. The modern server reference also carries a
/// `uint32 uploadCaps` after the flags (`AvatarFlagsInfo`), so a 4-byte tail is
/// preserved in [`AvatarFlags::upload_caps`] rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvatarFlags {
    /// The user the flags belong to — the frame `refNum`.
    pub user_id: i32,
    /// `AF_*` flags.
    pub flags: u16,
    /// An optional upload-capability word from the modern server reference.
    pub upload_caps: Option<u32>,
}

impl AvatarFlags {
    /// Decode an `fAva` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let flags = r.read_u16()?;
        let upload_caps = if r.remaining() >= 4 {
            Some(r.read_u32()?)
        } else {
            None
        };
        r.skip(r.remaining())?;
        Ok(AvatarFlags {
            user_id: ref_num,
            flags,
            upload_caps,
        })
    }

    /// Encode an `fAva` body: just the flags word.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u16(self.flags);
    }

    /// Build a complete `fAva` frame naming `user_id`.
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        let mut w = Writer::with_capacity(order, 2);
        self.encode(&mut w);
        Ok(Frame::new(opcode::AVATARFLAGS, self.user_id, w.into_vec()))
    }
}

/// `USERPROP` (`usrP`) in its Type 1 form: no worn props, just the identity.
///
/// `ClientMsg_userProp_avatar { sint32 nbrProps (0); sint16 avatarType;
/// uint16 avatarFlags; uint8 hash[20] }` (`m-protocol.h:444-451`). The plain
/// [`UserProp`](super::UserProp) decoder rejects the 24-byte tail, so the parent
/// module recognises this shape first and routes it here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserPropAvatar {
    /// The user whose avatar changed — the frame `refNum`.
    pub user_id: i32,
    /// `AT_AVATAR` for a Type 1 avatar.
    pub avatar_type: i16,
    /// `AF_*` flags.
    pub avatar_flags: u16,
    /// The content hash.
    pub hash: AvatarHash,
}

impl UserPropAvatar {
    /// Recognise and decode the Type 1 tail, or return `None` for a plain body.
    ///
    /// Requires `nbrProps == 0`, exactly 24 bytes after the count, and
    /// `avatarType == AT_AVATAR`, so a malformed prop list is never mistaken for
    /// an avatar.
    #[must_use]
    pub fn from_payload(ref_num: i32, payload: &[u8], order: ByteOrder) -> Option<Self> {
        let mut r = Reader::new(payload, order);
        let nbr_props = r.read_i32().ok()?;
        if nbr_props != 0 || r.remaining() != 24 {
            return None;
        }
        let avatar_type = r.read_i16().ok()?;
        if avatar_type != AT_AVATAR {
            return None;
        }
        let avatar_flags = r.read_u16().ok()?;
        let hash = AvatarHash::decode(&mut r).ok()?;
        Some(UserPropAvatar {
            user_id: ref_num,
            avatar_type,
            avatar_flags,
            hash,
        })
    }

    /// Encode the body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_i32(0);
        w.write_i16(self.avatar_type);
        w.write_u16(self.avatar_flags);
        self.hash.encode(w);
    }

    /// Build a complete `usrP` frame.
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        let mut w = Writer::with_capacity(order, 28);
        self.encode(&mut w);
        Ok(Frame::new(opcode::USERPROP, self.user_id, w.into_vec()))
    }
}

/// `USERDESC` (`usrD`) in its Type 1 form: face, colour and avatar identity.
///
/// `ClientMsg_userDesc_avatar { sint16 faceNbr; sint16 colorNbr; sint32
/// nbrProps (0); sint16 avatarType; uint16 avatarFlags; uint8 hash[20] }`
/// (`m-protocol.h:390-397`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserDescAvatar {
    /// The user whose appearance changed — the frame `refNum`.
    pub user_id: i32,
    /// Face selector.
    pub face_nbr: i16,
    /// Colour selector.
    pub color_nbr: i16,
    /// `AT_AVATAR` for a Type 1 avatar.
    pub avatar_type: i16,
    /// `AF_*` flags.
    pub avatar_flags: u16,
    /// The content hash.
    pub hash: AvatarHash,
}

impl UserDescAvatar {
    /// Recognise and decode the Type 1 tail, or return `None` for a plain body.
    #[must_use]
    pub fn from_payload(ref_num: i32, payload: &[u8], order: ByteOrder) -> Option<Self> {
        let mut r = Reader::new(payload, order);
        let face_nbr = r.read_i16().ok()?;
        let color_nbr = r.read_i16().ok()?;
        let nbr_props = r.read_i32().ok()?;
        if nbr_props != 0 || r.remaining() != 24 {
            return None;
        }
        let avatar_type = r.read_i16().ok()?;
        if avatar_type != AT_AVATAR {
            return None;
        }
        let avatar_flags = r.read_u16().ok()?;
        let hash = AvatarHash::decode(&mut r).ok()?;
        Some(UserDescAvatar {
            user_id: ref_num,
            face_nbr,
            color_nbr,
            avatar_type,
            avatar_flags,
            hash,
        })
    }

    /// Encode the body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_i16(self.face_nbr);
        w.write_i16(self.color_nbr);
        w.write_i32(0);
        w.write_i16(self.avatar_type);
        w.write_u16(self.avatar_flags);
        self.hash.encode(w);
    }

    /// Build a complete `usrD` frame.
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        let mut w = Writer::with_capacity(order, 32);
        self.encode(&mut w);
        Ok(Frame::new(opcode::USERDESC, self.user_id, w.into_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(seed: u8) -> AvatarHash {
        let mut bytes = [0u8; AVATAR_HASH_LEN];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = seed.wrapping_add(index as u8);
        }
        AvatarHash(bytes)
    }

    #[test]
    fn hash_hex_round_trips() {
        let original = hash(7);
        let text = original.to_hex();
        assert_eq!(text.len(), 40);
        assert_eq!(AvatarHash::from_hex(&text), Some(original));
        assert_eq!(AvatarHash::from_hex(&text.to_uppercase()), Some(original));
        assert_eq!(AvatarHash::from_hex("nope"), None);
        assert!(AvatarHash::ZERO.is_zero());
        assert!(!original.is_zero());
    }

    #[test]
    fn extended_info_avatar_decodes_the_live_reply_bytes() {
        // The live `'AVAT'` body captured from localhost: formats 0x7,
        // maxPayload 14 KB, 132 x 132 (spec note §2/§6).
        let body = [
            0x07, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x84, 0x00, 0x84, 0x00, 0x00, 0x00,
        ];
        let info =
            ExtendedInfoAvatar::decode(&body, ByteOrder::Little).expect("the live body decodes");
        assert_eq!(info.formats, AVFORM_GIF | AVFORM_JPEG | AVFORM_PNG99A);
        assert_eq!(info.max_payload, 14);
        assert_eq!(info.max_height, 132);
        assert_eq!(info.max_width, 132);
        assert_eq!(info.reserved, 0);
    }

    #[test]
    fn extended_info_reply_parses_avat_and_hurl_without_a_flags_word() {
        // Reproduces the 74-byte live reply: 'AVAT'(12) then 'HURL'(46). There is
        // no leading flags word — the request has one, the reply does not.
        let mut body = Vec::new();
        body.extend_from_slice(&SI_INF_AVATAR.to_le_bytes());
        body.extend_from_slice(&12u32.to_le_bytes());
        body.extend_from_slice(&[0x07, 0, 0, 0, 0x0e, 0, 0x84, 0, 0x84, 0, 0, 0]);
        let url = b"https://media.palace.example.info/palace/media\0";
        body.extend_from_slice(&SI_INF_HURL.to_le_bytes());
        body.extend_from_slice(&(url.len() as u32).to_le_bytes());
        body.extend_from_slice(url);

        let mut r = Reader::new(&body, ByteOrder::Little);
        let reply = ExtendedInfoReply::decode(&mut r).expect("the live reply decodes");
        assert!(r.is_empty());
        assert_eq!(reply.entries.len(), 2);
        let avatar = reply.avatar(ByteOrder::Little).expect("'AVAT' is present");
        assert_eq!(avatar.max_payload, 14);
        assert_eq!(avatar.max_height, 132);
        assert_eq!(
            reply.http_url().as_deref(),
            Some("https://media.palace.example.info/palace/media")
        );
        assert_eq!(reply.avatar_url(), None, "the server returned no 'AURL'");
        assert_eq!(reply.get(SI_INF_AVATAR).expect("entry").key(), "AVAT");
    }

    #[test]
    fn extended_info_request_is_a_flags_word_only() {
        let frame =
            extended_info_request_frame(SI_AVATAR | SI_AVATAR_URL | SI_HTTP_URL, ByteOrder::Little);
        assert_eq!(frame.opcode, opcode::EXTENDEDINFO);
        assert_eq!(frame.ref_num, 0);
        assert_eq!(frame.payload, (0x841u32).to_le_bytes());
    }

    #[test]
    fn avatar_query_round_trips() {
        let want = AvatarQuery { hash: hash(3) };
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let frame = want.frame(order).expect("qAva encodes");
            assert_eq!(frame.opcode, opcode::AVATARQUERY);
            let mut r = Reader::new(&frame.payload, order);
            assert_eq!(AvatarQuery::decode(0, &mut r).expect("qAva decodes"), want);
            assert!(r.is_empty());
        }
    }

    #[test]
    fn avatar_send_data_round_trips() {
        let want = AvatarSend {
            hash: hash(9),
            flags: AVATAR_SEND_DATA,
            data: vec![1, 2, 3, 4, 5],
        };
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let frame = want.frame(order).expect("sAva encodes");
            assert_eq!(frame.opcode, opcode::AVATARSEND);
            let mut r = Reader::new(&frame.payload, order);
            let got = AvatarSend::decode(&mut r).expect("sAva decodes");
            assert!(r.is_empty());
            assert_eq!(got, want);
            assert!(!got.is_url());
            assert_eq!(got.url(), None);
        }
    }

    #[test]
    fn avatar_send_url_round_trips_and_trims_the_nul() {
        let want = AvatarSend {
            hash: hash(1),
            flags: AVATAR_SEND_URL,
            data: b"https://example.test/a.gif\0".to_vec(),
        };
        let frame = want.frame(ByteOrder::Little).expect("sAva encodes");
        let mut r = Reader::new(&frame.payload, ByteOrder::Little);
        let got = AvatarSend::decode(&mut r).expect("sAva decodes");
        assert!(got.is_url());
        assert_eq!(got.url().as_deref(), Some("https://example.test/a.gif"));
    }

    #[test]
    fn a_short_avatar_send_is_an_error() {
        let mut r = Reader::new(&[0u8; 20], ByteOrder::Little);
        assert!(AvatarSend::decode(&mut r).is_err());
    }

    #[test]
    fn avatar_flags_accepts_the_plain_and_upload_caps_forms() {
        let mut r = Reader::new(&[0x03, 0x00], ByteOrder::Little);
        assert_eq!(
            AvatarFlags::decode(4, &mut r).expect("plain fAva decodes"),
            AvatarFlags {
                user_id: 4,
                flags: 0x0003,
                upload_caps: None
            }
        );

        let mut body = vec![0x01, 0x00];
        body.extend_from_slice(&0xdead_beefu32.to_le_bytes());
        let mut r = Reader::new(&body, ByteOrder::Little);
        assert_eq!(
            AvatarFlags::decode(4, &mut r).expect("extended fAva decodes"),
            AvatarFlags {
                user_id: 4,
                flags: 0x0001,
                upload_caps: Some(0xdead_beef)
            }
        );
    }

    #[test]
    fn avatar_flags_frame_names_the_user_in_the_ref_num() {
        let flags = AvatarFlags {
            user_id: 12,
            flags: AF_HORIZONTAL_FLIP,
            upload_caps: None,
        };
        let frame = flags.frame(ByteOrder::Little).expect("fAva encodes");
        assert_eq!(frame.opcode, opcode::AVATARFLAGS);
        assert_eq!(frame.ref_num, 12);
        assert_eq!(frame.payload, vec![0x01, 0x00]);
    }

    #[test]
    fn user_prop_avatar_is_recognised_only_for_the_type1_shape() {
        let want = UserPropAvatar {
            user_id: 5,
            avatar_type: AT_AVATAR,
            avatar_flags: 0,
            hash: hash(2),
        };
        let mut w = Writer::new(ByteOrder::Little);
        want.encode(&mut w);
        let body = w.into_vec();
        assert_eq!(body.len(), 28);
        assert_eq!(
            UserPropAvatar::from_payload(5, &body, ByteOrder::Little),
            Some(want)
        );

        // A plain zero-prop list (4 bytes) is not an avatar.
        assert_eq!(
            UserPropAvatar::from_payload(5, &0i32.to_le_bytes(), ByteOrder::Little),
            None
        );
        // A 24-byte tail that is not AT_AVATAR is rejected.
        let mut wrong = [0i32.to_le_bytes().to_vec(), AT_PROP.to_le_bytes().to_vec()].concat();
        wrong.extend_from_slice(&[0u8; AVATAR_HASH_LEN]);
        assert_eq!(
            UserPropAvatar::from_payload(5, &wrong, ByteOrder::Little),
            None
        );
    }

    #[test]
    fn user_desc_avatar_is_recognised_only_for_the_type1_shape() {
        let want = UserDescAvatar {
            user_id: 5,
            face_nbr: 2,
            color_nbr: 3,
            avatar_type: AT_AVATAR,
            avatar_flags: AF_VERTICAL_FLIP,
            hash: hash(6),
        };
        let mut w = Writer::new(ByteOrder::Little);
        want.encode(&mut w);
        let body = w.into_vec();
        assert_eq!(body.len(), 32);
        assert_eq!(
            UserDescAvatar::from_payload(5, &body, ByteOrder::Little),
            Some(want)
        );
        assert_eq!(
            UserDescAvatar::from_payload(5, &[0u8; 8], ByteOrder::Little),
            None
        );
    }

    #[test]
    fn entry_key_names_are_the_four_ascii_bytes() {
        let entry = ExtendedInfoEntry {
            id: SI_INF_AVATAR,
            data: Vec::new(),
        };
        assert_eq!(entry.key(), "AVAT");
        assert_eq!(
            ExtendedInfoEntry {
                id: SI_INF_HURL,
                data: Vec::new()
            }
            .key(),
            "HURL"
        );
    }
}

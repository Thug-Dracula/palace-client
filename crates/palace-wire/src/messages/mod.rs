//! Decoders for the Palace message bodies this milestone needs.
//!
//! Every struct here traces its byte layout to the 1999 Communities.com
//! protocol reference (`PalaceProtocolRef.txt`), cross-checked against Taj's
//! C# structs, QPalace's `message.hpp`, OpenPalace's AS3 client, pserver's C++
//! source, and — where they disagreed — against live traffic.
//!
//! The single entry point is [`Message::decode`]. It never fails on an unknown
//! opcode: it returns [`Message::Unknown`] so the caller can log and skip.

mod asset;
mod avatar;
mod chat;
mod draw;
mod lists;
mod logon;
mod pictures;
mod props;
mod room;
mod server;
mod spots;
mod user;

pub use asset::PropUpload;
pub use avatar::{
    extended_info_request_frame, AvatarFlags, AvatarHash, AvatarQuery, AvatarSend,
    ExtendedInfoAvatar, ExtendedInfoEntry, ExtendedInfoReply, UserDescAvatar, UserPropAvatar,
    AF_HORIZONTAL_FLIP, AF_INHIBIT_ANIMATION, AF_VALID_FLAGS, AF_VERTICAL_FLIP, AT_AVATAR, AT_PROP,
    AVATAR_HASH_LEN, AVATAR_SEND_DATA, AVATAR_SEND_URL, AVFORM_FLASH, AVFORM_GIF, AVFORM_JPEG,
    AVFORM_MNG, AVFORM_PNG99A, SI_AVATAR, SI_AVATAR_URL, SI_HTTP_URL, SI_INF_AURL, SI_INF_AVATAR,
    SI_INF_HURL,
};
pub use chat::{Talk, Whisper};
pub use draw::Draw;
pub use lists::{RoomList, RoomListRec, RoomUserList, UserList, UserListRec};
pub use logon::{
    authenticating_logon_record, authenticating_logon_record_with_identity, authresponse_frame,
    aux_flags, client_logon_record, client_logon_record_with_identity, reference_logon_record,
    Authenticate, AuxRegistrationRec, ClientIdentity, ClientProfile, Puid, ReferenceProfile,
};
pub use pictures::PictMove;
pub use props::{PropDel, PropMove, PropNew};
pub use room::{NavError, RoomDescription, RoomRec};
pub use server::{
    AltLogonReply, HttpServer, ServerDown, ServerDownReason, ServerInfo, ServerVersion, UserLog,
};
pub use spots::{DoorLock, SpotDel, SpotMove, SpotNew, SpotState};
pub use user::{
    AssetSpec, Point, UserColor, UserDesc, UserExit, UserFace, UserMove, UserName, UserNew,
    UserProp, UserRec, UserStatus,
};

use crate::byteorder::{ByteOrder, Reader};
use crate::error::{Result, WireError};
use crate::opcode::{self, Opcode};

/// A decoded Palace message.
///
/// `payload_len` is retained for the variants where the raw body matters (or
/// where we only understand part of it).
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// `regi` — a logon request (client → server) or its recorded form.
    Logon(AuxRegistrationRec),
    /// `rep2` — alternate logon reply, an echo of the client's own record.
    AltLogonReply(AuxRegistrationRec),
    /// `vers` — server version, encoded in `refNum`.
    ServerVersion(ServerVersion),
    /// `sinf` — server permissions and name.
    ServerInfo(ServerInfo),
    /// `HTTP` — media server base URL.
    HttpServer(HttpServer),
    /// `down` — the server is dropping the connection; the reason is the frame
    /// `refNum`.
    ServerDown(ServerDown),
    /// `log ` — a user logged onto the server.
    UserLog(UserLog),
    /// `rLst` — the room list.
    RoomList(RoomList),
    /// `uLst` — the server-wide user list.
    UserList(UserList),
    /// `rprs` — users in the current room, as full `UserRec` records.
    RoomUsers(RoomUserList),
    /// `nprs` — a user entered the current room.
    UserNew(UserNew),
    /// `eprs` — a user left the current room.
    UserExit(UserExit),
    /// `uLoc` — a user moved.
    UserMove(UserMove),
    /// `usrF` — a user's face changed.
    UserFace(UserFace),
    /// `usrC` — a user's colour changed.
    UserColor(UserColor),
    /// `usrN` — a user's name changed, or a failed rename reverted.
    UserName(UserName),
    /// `usrP` — a user's complete worn prop list.
    UserProp(UserProp),
    /// `usrP` in its Type 1 form: no worn props, an avatar hash instead.
    UserPropAvatar(UserPropAvatar),
    /// `usrD` — a user's face, colour and props together.
    UserDesc(UserDesc),
    /// `usrD` in its Type 1 form: face, colour and an avatar hash.
    UserDescAvatar(UserDescAvatar),
    /// `sInf` — an `EXTENDEDINFO` reply, including the `'AVAT'` avatar limits.
    ExtendedInfo(ExtendedInfoReply),
    /// `fAva` — a user's Type 1 avatar flags.
    AvatarFlags(AvatarFlags),
    /// `qAva` — a query for a 20-byte avatar hash.
    AvatarQuery(AvatarQuery),
    /// `sAva` — an avatar image or URL.
    AvatarSend(AvatarSend),
    /// `uSta` — own user status flags.
    UserStatus(UserStatus),
    /// `room` — room description.
    RoomDescription(RoomDescription),
    /// `nPrp` — a loose prop was added to the room.
    PropNew(PropNew),
    /// `mPrp` — a loose prop moved.
    PropMove(PropMove),
    /// `dPrp` — a loose prop was deleted.
    PropDel(PropDel),
    /// `lock` — a door was locked.
    DoorLock(DoorLock),
    /// `unlo` — a door was unlocked.
    DoorUnlock(DoorLock),
    /// `sSta` — a hotspot's state changed.
    SpotState(SpotState),
    /// `opSn` — a hotspot was created; the body is empty.
    SpotNew,
    Authenticate,
    /// `opSd` — a hotspot was deleted.
    SpotDel(SpotDel),
    /// `coLs` — a hotspot moved to an absolute position.
    SpotMove(SpotMove),
    /// `pLoc` — a picture moved to an absolute position.
    PictMove(PictMove),
    /// `draw` — one draw record for the current room. The record body is kept
    /// raw; `palace_room::decode_draw_record` parses it.
    Draw(Draw),
    /// `endr` — end of room description.
    RoomDescEnd,
    /// `talk` — public chat.
    Talk(Talk),
    /// `whis` — private chat.
    Whisper(Whisper),
    /// `ping` — keepalive request; `refNum` is echoed back in the pong.
    Ping(i32),
    /// `pong` — keepalive response.
    Pong(i32),
    /// `sErr` — the server refused a room change; `refNum` is the reason.
    NavError(NavError),
    /// `bye ` — logoff.
    Logoff,
    /// `tiyr` — server banner (normally consumed by
    /// [`read_handshake`](crate::frame::read_handshake)).
    TiyId,
    /// An opcode this milestone does not decode. Carried, not dropped, so it can
    /// be logged and later understood.
    Unknown {
        opcode: Opcode,
        ref_num: i32,
        payload_len: usize,
    },
}

impl Message {
    /// Decode a message body for `opcode`.
    ///
    /// Returns `Ok(Message::Unknown { .. })` for opcodes outside the table. A
    /// *known* opcode with a malformed body is a real error, which lets the
    /// caller log it distinctly while still keeping the session alive.
    pub fn decode(
        opcode: Opcode,
        ref_num: i32,
        payload: &[u8],
        order: ByteOrder,
    ) -> Result<Message> {
        let mut r = Reader::new(payload, order);
        Message::decode_into(opcode, ref_num, &mut r)
    }

    /// Like [`Message::decode`] but reads from an existing cursor, so a caller
    /// can inspect how many bytes the body consumed.
    pub fn decode_into(opcode: Opcode, ref_num: i32, r: &mut Reader<'_>) -> Result<Message> {
        let msg = match opcode {
            opcode::TIYID => Message::TiyId,
            opcode::LOGON => Message::Logon(AuxRegistrationRec::decode(r)?),
            opcode::ALTLOGONREPLY => Message::AltLogonReply(AuxRegistrationRec::decode(r)?),
            opcode::VERSION => Message::ServerVersion(ServerVersion::from_ref_num(ref_num)),
            opcode::SERVERINFO => Message::ServerInfo(ServerInfo::decode(r)?),
            opcode::HTTPSERVER => Message::HttpServer(HttpServer::decode(r)?),
            opcode::SERVERDOWN => Message::ServerDown(ServerDown::decode(ref_num, r)?),
            opcode::USERLOG => Message::UserLog(UserLog::decode(ref_num, r)?),
            opcode::LISTOFALLROOMS => Message::RoomList(RoomList::decode(ref_num, r)?),
            opcode::LISTOFALLUSERS => Message::UserList(UserList::decode(ref_num, r)?),
            opcode::USERLIST => Message::RoomUsers(RoomUserList::decode(ref_num, r)?),
            opcode::USERNEW => Message::UserNew(UserNew::decode(ref_num, r)?),
            opcode::USEREXIT => Message::UserExit(UserExit::from_ref_num(ref_num)),
            opcode::USERMOVE => Message::UserMove(UserMove::decode(ref_num, r)?),
            opcode::USERFACE => Message::UserFace(UserFace::decode(ref_num, r)?),
            opcode::USERCOLOR => Message::UserColor(UserColor::decode(ref_num, r)?),
            opcode::USERNAME => Message::UserName(UserName::decode(ref_num, r)?),
            opcode::USERPROP => {
                match UserPropAvatar::from_payload(ref_num, r.remaining_slice(), r.order()) {
                    Some(avatar) => Message::UserPropAvatar(avatar),
                    None => Message::UserProp(UserProp::decode(ref_num, r)?),
                }
            }
            opcode::USERDESC => {
                match UserDescAvatar::from_payload(ref_num, r.remaining_slice(), r.order()) {
                    Some(avatar) => Message::UserDescAvatar(avatar),
                    None => Message::UserDesc(UserDesc::decode(ref_num, r)?),
                }
            }
            opcode::EXTENDEDINFO => Message::ExtendedInfo(ExtendedInfoReply::decode(r)?),
            opcode::AVATARFLAGS => Message::AvatarFlags(AvatarFlags::decode(ref_num, r)?),
            opcode::AVATARQUERY => Message::AvatarQuery(AvatarQuery::decode(ref_num, r)?),
            opcode::AVATARSEND => Message::AvatarSend(AvatarSend::decode(r)?),
            opcode::USERSTATUS => Message::UserStatus(UserStatus::decode(ref_num, r)?),
            opcode::ROOMDESC => Message::RoomDescription(RoomDescription::decode(r)?),
            opcode::PROPNEW => Message::PropNew(PropNew::decode(r)?),
            opcode::PROPMOVE => Message::PropMove(PropMove::decode(r)?),
            opcode::PROPDEL => Message::PropDel(PropDel::decode(r)?),
            opcode::DOORLOCK => Message::DoorLock(DoorLock::decode(r)?),
            opcode::DOORUNLOCK => Message::DoorUnlock(DoorLock::decode(r)?),
            opcode::SPOTSTATE => Message::SpotState(SpotState::decode(r)?),
            opcode::SPOTNEW => {
                SpotNew::decode(r)?;
                Message::SpotNew
            }
            opcode::SPOTDEL => Message::SpotDel(SpotDel::decode(r)?),
            opcode::SPOTMOVE => Message::SpotMove(SpotMove::decode(r)?),
            opcode::PICTMOVE => Message::PictMove(PictMove::decode(r)?),
            opcode::DRAW => Message::Draw(Draw::decode(r)?),
            opcode::ROOMDESCEND => Message::RoomDescEnd,
            opcode::TALK => Message::Talk(Talk::decode(ref_num, r)?),
            opcode::WHISPER => Message::Whisper(Whisper::decode(ref_num, r)?),
            opcode::PING => Message::Ping(ref_num),
            opcode::PONG => Message::Pong(ref_num),
            opcode::NAVERROR => Message::NavError(NavError::from_ref_num(ref_num)),
            opcode::LOGOFF => Message::Logoff,
            opcode::AUTHENTICATE => {
                Authenticate::decode(r)?;
                Message::Authenticate
            }
            _ => Message::Unknown {
                opcode,
                ref_num,
                payload_len: r.remaining(),
            },
        };
        Ok(msg)
    }

    /// One-line human-readable summary, used by the probe and stored in every
    /// fixture manifest entry.
    pub fn describe(&self) -> String {
        match self {
            Message::Logon(rec) => format!(
                "logon: user={:?} reserved={:?} caps=up:{:#x}/down:{:#x} auxFlags={:#010x}",
                rec.user_name,
                String::from_utf8_lossy(&rec.reserved),
                rec.upload_caps,
                rec.download_caps,
                rec.aux_flags
            ),
            Message::AltLogonReply(rec) => format!(
                "alternate logon reply (echo): user={:?} auxFlags={:#010x} puidCtr={:#010x} reserved={:?}",
                rec.user_name,
                rec.aux_flags,
                rec.puid_ctr,
                String::from_utf8_lossy(&rec.reserved)
            ),
            Message::ServerVersion(v) => format!("server version {v}"),
            Message::ServerInfo(i) => format!(
                "server info: name={:?} permissions={:#010x} ({})",
                i.name,
                i.permissions,
                summarize_server_permissions(i.permissions)
            ),
            Message::HttpServer(h) => format!("media server: {}", h.url),
            Message::ServerDown(d) => d.describe(),
            Message::UserLog(l) => {
                format!("user logon: user_id={} total_users={}", l.user_id, l.user_count)
            }
            Message::RoomList(l) => format!("room list: {} rooms", l.rooms.len()),
            Message::UserList(l) => format!("user list: {} users", l.users.len()),
            Message::RoomUsers(l) => format!("room user list: {} users", l.users.len()),
            Message::UserNew(n) => format!("user entered: id={} name={:?}", n.user_id, n.record.name),
            Message::UserExit(e) => format!("user left: id={}", e.user_id),
            Message::UserMove(m) => format!(
                "user moved: id={} v={} h={}",
                m.user_id, m.position.v, m.position.h
            ),
            Message::UserFace(f) => format!("user face: id={} face={}", f.user_id, f.face_nbr),
            Message::UserColor(c) => format!("user color: id={} color={}", c.user_id, c.color_nbr),
            Message::UserName(n) => format!("user name: id={} name={:?}", n.user_id, n.name),
            Message::UserProp(p) => format!(
                "user props: id={} props={}",
                p.user_id,
                summarize_asset_specs(&p.props)
            ),
            Message::UserPropAvatar(p) => format!(
                "user avatar: id={} type={} flags={:#06x} hash={}",
                p.user_id, p.avatar_type, p.avatar_flags, p.hash
            ),
            Message::UserDesc(d) => format!(
                "user desc: id={} face={} color={} props={}",
                d.user_id,
                d.face_nbr,
                d.color_nbr,
                summarize_asset_specs(&d.props)
            ),
            Message::UserDescAvatar(d) => format!(
                "user desc avatar: id={} face={} color={} flags={:#06x} hash={}",
                d.user_id, d.face_nbr, d.color_nbr, d.avatar_flags, d.hash
            ),
            Message::ExtendedInfo(info) => format!(
                "extended info: {} entry(ies) [{}]",
                info.entries.len(),
                info.entries
                    .iter()
                    .map(ExtendedInfoEntry::key)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Message::AvatarFlags(f) => format!(
                "avatar flags: id={} flags={:#06x} uploadCaps={:?}",
                f.user_id, f.flags, f.upload_caps
            ),
            Message::AvatarQuery(q) => format!("avatar query: hash={}", q.hash),
            Message::AvatarSend(s) => format!(
                "avatar send: hash={} flags={} bytes={}",
                s.hash,
                s.flags,
                s.data.len()
            ),
            Message::UserStatus(s) => format!(
                "user status: user_id={} flags={:#06x} ({}) raw_len={}",
                s.user_id,
                s.flags,
                summarize_user_flags(s.flags),
                s.raw.len()
            ),
            Message::RoomDescription(d) => format!(
                "room description: id={} name={:?} picture={:?} artist={:?} hotspots={} pictures={} draws={} props={} people={} var_bytes={}",
                d.header.room_id,
                d.name,
                d.picture,
                d.artist,
                d.header.nbr_hotspots,
                d.header.nbr_pictures,
                d.header.nbr_draw_cmds,
                d.header.nbr_lprops,
                d.header.nbr_people,
                d.header.len_vars
            ),
            Message::RoomDescEnd => "end of room description".to_string(),
            Message::PropNew(p) => format!(
                "new prop: id={} crc={:#010x} v={} h={}",
                p.spec.id, p.spec.crc, p.position.v, p.position.h
            ),
            Message::PropMove(p) => format!(
                "move prop: index={} v={} h={}",
                p.prop_num, p.position.v, p.position.h
            ),
            Message::PropDel(p) => format!("delete prop: index={}", p.prop_num),
            Message::DoorLock(d) => format!("lock door: room={} door={}", d.room_id, d.door_id),
            Message::DoorUnlock(d) => {
                format!("unlock door: room={} door={}", d.room_id, d.door_id)
            }
            Message::SpotState(s) => format!(
                "spot state: room={} spot={} state={}",
                s.room_id, s.spot_id, s.state
            ),
            Message::SpotNew => "new spot (no body)".to_string(),
            Message::Authenticate => "authenticate request (no body)".to_string(),
            Message::SpotDel(d) => format!("delete spot: spot={}", d.spot_id),
            Message::SpotMove(m) => format!(
                "move spot: room={} spot={} v={} h={}",
                m.room_id, m.spot_id, m.position.v, m.position.h
            ),
            Message::PictMove(m) => format!(
                "move picture: room={} spot={} v={} h={}",
                m.room_id, m.spot_id, m.position.v, m.position.h
            ),
            Message::Draw(d) => format!("draw record: {} raw byte(s)", d.len()),
            Message::Talk(t) => format!("talk: user_id={} {:?}", t.user_id, t.text),
            Message::Whisper(w) => format!(
                "whisper: from={} to={} {:?}",
                w.user_id, w.target_id, w.text
            ),
            Message::Ping(n) => format!("ping (refNum={n})"),
            Message::Pong(n) => format!("pong (refNum={n})"),
            Message::NavError(e) => e.describe(),
            Message::Logoff => "bye (logoff)".to_string(),
            Message::TiyId => "handshake MSG_TIYID".to_string(),
            Message::Unknown { opcode, ref_num, payload_len } => format!(
                "unknown opcode {} refNum={} payload={} bytes",
                opcode.describe(),
                ref_num,
                payload_len
            ),
        }
    }
}

/// Decode `payload` and error if bytes remain — used by tests and by the
/// fixture loader to prove a body was consumed exactly.
pub fn decode_exact(
    opcode: Opcode,
    ref_num: i32,
    payload: &[u8],
    order: ByteOrder,
) -> Result<Message> {
    let mut r = Reader::new(payload, order);
    let msg = Message::decode_into(opcode, ref_num, &mut r)?;
    let is_unknown = matches!(msg, Message::Unknown { .. });
    if !is_unknown && !r.is_empty() {
        return Err(WireError::TrailingBytes {
            remaining: r.remaining(),
        });
    }
    Ok(msg)
}

/// Turn a decode failure into a short, allocation-light log line. Used by the
/// probe so a malformed known message never aborts the session.
pub fn describe_error(opcode: Opcode, err: &WireError) -> String {
    format!("failed to decode {}: {err}", opcode.describe())
}

fn summarize_server_permissions(p: u32) -> String {
    const BITS: &[(u32, &str)] = &[
        (0x0001, "AllowGuests"),
        (0x0002, "AllowCyborgs"),
        (0x0004, "AllowPainting"),
        (0x0008, "AllowCustomProps"),
        (0x0010, "AllowWizards"),
        (0x0020, "WizardsMayKill"),
        (0x0040, "WizardsMayAuthor"),
        (0x0080, "PlayersMayKill"),
        (0x0100, "CyborgsMayKill"),
        (0x0200, "DeathPenalty"),
        (0x0400, "PurgeInactiveProps"),
        (0x0800, "KillFlooders"),
        (0x1000, "NoSpoofing"),
        (0x2000, "MemberCreatedRooms"),
    ];
    summarize_bits(p, BITS)
}

fn summarize_user_flags(f: u16) -> String {
    const BITS: &[(u32, &str)] = &[
        (0x0001, "SuperUser"),
        (0x0002, "God"),
        (0x0004, "Kill"),
        (0x0008, "Guest"),
        (0x0010, "Banished"),
        (0x0020, "Penalized"),
        (0x0040, "CommError"),
        (0x0080, "Gag"),
        (0x0100, "Pin"),
        (0x0200, "Hide"),
        (0x0400, "RejectESP"),
        (0x0800, "RejectPrivate"),
        (0x1000, "PropGag"),
    ];
    summarize_bits(f as u32, BITS)
}

fn summarize_asset_specs(specs: &[AssetSpec]) -> String {
    let ids: Vec<String> = specs
        .iter()
        .map(|spec| (spec.id as u32).to_string())
        .collect();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(",")
    }
}

fn summarize_bits(value: u32, bits: &[(u32, &str)]) -> String {
    let mut names: Vec<&str> = bits
        .iter()
        .filter(|(bit, _)| value & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    if names.is_empty() {
        names.push("none");
    }
    names.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::Writer;
    use crate::opcode::{
        AUTHENTICATE, DOORLOCK, DOORUNLOCK, DRAW, LISTOFALLROOMS, LOGOFF, NAVERROR, PICTMOVE, PING,
        PROPMOVE, SERVERDOWN, SPOTDEL, SPOTMOVE, SPOTNEW, SPOTSTATE, TALK, USERFACE, USERNAME,
        USERPROP,
    };

    #[test]
    fn unknown_opcodes_decode_to_unknown_not_error() {
        let msg = Message::decode(Opcode::new(0xdead_beef), 42, &[1, 2, 3], ByteOrder::Little)
            .expect("unknown opcodes must not error");
        match msg {
            Message::Unknown {
                opcode,
                ref_num,
                payload_len,
            } => {
                assert_eq!(opcode.value(), 0xdead_beef);
                assert_eq!(ref_num, 42);
                assert_eq!(payload_len, 3);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn empty_keepalive_and_logoff_decode() {
        assert_eq!(
            Message::decode(PING, 7, &[], ByteOrder::Little).unwrap(),
            Message::Ping(7)
        );
        assert_eq!(
            Message::decode(LOGOFF, 0, &[], ByteOrder::Little).unwrap(),
            Message::Logoff
        );
    }

    #[test]
    fn malformed_known_message_is_an_error() {
        assert!(Message::decode(LISTOFALLROOMS, 5, &[0u8; 3], ByteOrder::Little).is_err());
    }

    #[test]
    fn the_server_down_message_reaches_its_arm() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let down = Message::decode(SERVERDOWN, 12, &[], order).unwrap();
            assert_eq!(
                down,
                Message::ServerDown(ServerDown {
                    reason: ServerDownReason::Banished,
                    message: None,
                }),
                "the refNum is the reason and the body is empty"
            );
            assert!(
                !matches!(down, Message::Unknown { .. }),
                "down must no longer fall through to Unknown"
            );
            assert!(down.describe().contains("banished"));

            let mut w = Writer::new(order);
            w.write_cstring("you were ejected");
            let verbose = Message::decode(SERVERDOWN, 16, &w.into_vec(), order).unwrap();
            assert_eq!(
                verbose,
                Message::ServerDown(ServerDown {
                    reason: ServerDownReason::Verbose,
                    message: Some("you were ejected".to_string()),
                })
            );
            assert!(verbose.describe().contains("you were ejected"));
        }
    }

    #[test]
    fn describe_is_total() {
        let m = Message::decode(TALK, 1, b"hi\0", ByteOrder::Little).unwrap();
        assert!(m.describe().contains("hi"));
        let unknown = Message::Unknown {
            opcode: Opcode::new(0x0102_0304),
            ref_num: 0,
            payload_len: 0,
        };
        assert!(unknown.describe().contains("unknown"));
    }

    #[test]
    fn the_new_appearance_and_prop_messages_reach_their_arms() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(7);
        let face = Message::decode(USERFACE, 5, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(
            face,
            Message::UserFace(UserFace {
                user_id: 5,
                face_nbr: 7
            })
        );
        assert!(face.describe().contains("face=7"));

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(1);
        w.write_i32(99);
        w.write_u32(0xabcd);
        let props = Message::decode(USERPROP, 5, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(
            props,
            Message::UserProp(UserProp {
                user_id: 5,
                props: vec![AssetSpec {
                    id: 99,
                    crc: 0xabcd
                }],
            })
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(3);
        w.write_i16(2);
        w.write_i16(4);
        let mv = Message::decode(PROPMOVE, 0, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(
            mv,
            Message::PropMove(PropMove {
                prop_num: 3,
                position: Point::new(2, 4)
            })
        );
        assert!(mv.describe().contains("index=3"));
    }

    #[test]
    fn the_username_message_reaches_its_arm() {
        // A plain PString body: length byte + name, no alignment padding.
        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring("bob");
        let msg = Message::decode(USERNAME, 42, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(
            msg,
            Message::UserName(UserName {
                user_id: 42,
                name: "bob".to_string()
            })
        );
        assert!(msg.describe().contains("id=42"));
        assert!(msg.describe().contains("bob"));

        // A body with padding after the PString is a known message with a
        // malformed body, so the strict decoder refuses it.
        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring("bob");
        w.write_bytes(&[0, 0, 0]);
        assert!(decode_exact(USERNAME, 42, &w.into_vec(), ByteOrder::Little).is_err());
    }

    #[test]
    fn the_door_and_spot_state_messages_reach_their_arms() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(901);
        w.write_i16(1);
        let door_body = w.into_vec();
        assert_eq!(door_body.len(), 4);

        let lock = Message::decode(DOORLOCK, 0, &door_body, ByteOrder::Little).unwrap();
        assert_eq!(
            lock,
            Message::DoorLock(DoorLock {
                room_id: 901,
                door_id: 1
            })
        );
        assert!(lock.describe().contains("door=1"));

        let unlock = Message::decode(DOORUNLOCK, 0, &door_body, ByteOrder::Little).unwrap();
        assert_eq!(
            unlock,
            Message::DoorUnlock(DoorLock {
                room_id: 901,
                door_id: 1
            })
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(901);
        w.write_i16(1);
        w.write_i16(1);
        let spot = Message::decode(SPOTSTATE, 0, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(
            spot,
            Message::SpotState(SpotState {
                room_id: 901,
                spot_id: 1,
                state: 1
            })
        );
        assert!(spot.describe().contains("state=1"));
    }

    #[test]
    fn the_draw_message_reaches_its_arm_with_the_record_verbatim() {
        // A 16-byte record: enough for the 10-byte header plus a stub operand.
        let body: Vec<u8> = (0..16).collect();
        let msg = Message::decode(DRAW, 0, &body, ByteOrder::Little).unwrap();
        assert_eq!(
            msg,
            Message::Draw(Draw { body: body.clone() }),
            "the record body is carried unchanged"
        );
        assert!(
            !matches!(msg, Message::Unknown { .. }),
            "draw must no longer fall through to Unknown"
        );
        let text = msg.describe();
        assert!(
            text.contains("draw"),
            "the summary names the message: {text}"
        );
        assert!(
            text.contains("16"),
            "the summary reports the record size: {text}"
        );
    }

    #[test]
    fn an_authenticate_request_is_bodyless() {
        assert_eq!(
            Message::decode(AUTHENTICATE, 0, &[], ByteOrder::Little).unwrap(),
            Message::Authenticate
        );
        assert!(
            Message::decode(AUTHENTICATE, 0, &[0u8; 2], ByteOrder::Little).is_err(),
            "a body here would mean the layout is wrong, so it must be refused"
        );
        assert!(Message::Authenticate.describe().contains("authenticate"));
    }

    #[test]
    fn the_room_change_messages_reach_their_arms() {
        assert_eq!(
            Message::decode(SPOTNEW, 0, &[], ByteOrder::Little).unwrap(),
            Message::SpotNew
        );
        assert!(Message::decode(SPOTNEW, 0, &[0u8; 2], ByteOrder::Little).is_err());

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(4);
        let del = Message::decode(SPOTDEL, 0, &w.into_vec(), ByteOrder::Little).unwrap();
        assert_eq!(del, Message::SpotDel(SpotDel { spot_id: 4 }));
        assert!(del.describe().contains("spot=4"));

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(887);
        w.write_i16(33);
        Point::new(240, 120).encode(&mut w);
        let body = w.into_vec();
        assert_eq!(body.len(), 8);

        let mv = Message::decode(SPOTMOVE, 0, &body, ByteOrder::Little).unwrap();
        assert_eq!(
            mv,
            Message::SpotMove(SpotMove {
                room_id: 887,
                spot_id: 33,
                position: Point::new(240, 120)
            })
        );
        assert!(mv.describe().contains("v=240 h=120"));

        let pic = Message::decode(PICTMOVE, 0, &body, ByteOrder::Little).unwrap();
        assert_eq!(
            pic,
            Message::PictMove(PictMove {
                room_id: 887,
                spot_id: 33,
                position: Point::new(240, 120)
            })
        );
        assert!(pic.describe().contains("spot=33"));
    }

    #[test]
    fn naverror_names_each_documented_reason_and_does_not_name_an_unknown_one() {
        let documented = [
            (0, "public error"),
            (1, "unknown room"),
            (2, "room full"),
            (3, "room closed"),
            (4, "author"),
            (5, "palace full"),
        ];
        for (code, phrase) in documented {
            let msg = Message::decode(NAVERROR, code, &[], ByteOrder::Little).unwrap();
            let Message::NavError(err) = msg else {
                panic!("code {code} must decode to NavError");
            };
            assert_eq!(
                NavError::from_ref_num(code),
                err,
                "the refNum identifies the reason"
            );
            let text = err.describe();
            assert!(
                text.contains("room change failed"),
                "the failure must say what happened: {text}"
            );
            assert!(
                text.contains(phrase),
                "code {code} should be reported as {phrase:?}, got {text:?}"
            );
        }

        let Message::NavError(err) = Message::decode(NAVERROR, 77, &[], ByteOrder::Little).unwrap()
        else {
            panic!("an undocumented code must still decode to NavError");
        };
        let text = err.describe();
        assert!(text.contains("77"), "the raw code must be reported: {text}");
        assert!(
            text.contains("undocumented"),
            "an unknown code must be flagged, not named: {text}"
        );
        for (code, phrase) in documented {
            assert!(
                !text.contains(phrase),
                "unknown code {code} must not borrow the name {phrase:?}: {text}"
            );
        }
    }
}

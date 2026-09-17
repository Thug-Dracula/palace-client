//! Decoders for the Palace message bodies this milestone needs.
//!
//! Every struct here traces its byte layout to the 1999 Communities.com
//! protocol reference (`PalaceProtocolRef.txt`), cross-checked against Taj's
//! C# structs, QPalace's `message.hpp`, OpenPalace's AS3 client, pserver's C++
//! source, and — where they disagreed — against live traffic.
//!
//! The single entry point is [`Message::decode`]. It never fails on an unknown
//! opcode: it returns [`Message::Unknown`] so the caller can log and skip.

mod chat;
mod lists;
mod logon;
mod props;
mod room;
mod server;
mod user;

pub use chat::{Talk, Whisper};
pub use lists::{RoomList, RoomListRec, UserList, UserListRec};
pub use logon::{aux_flags, reference_logon_record, AuxRegistrationRec, ReferenceProfile};
pub use props::{PropDel, PropMove, PropNew};
pub use room::{RoomDescription, RoomRec};
pub use server::{AltLogonReply, HttpServer, ServerInfo, ServerVersion, UserLog};
pub use user::{
    AssetSpec, Point, UserColor, UserDesc, UserExit, UserFace, UserMove, UserNew, UserProp,
    UserRec, UserStatus,
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
    /// `log ` — a user logged onto the server.
    UserLog(UserLog),
    /// `rLst` — the room list.
    RoomList(RoomList),
    /// `uLst` — the server-wide user list.
    UserList(UserList),
    /// `rprs` — users in the current room.
    RoomUsers(UserList),
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
    /// `usrP` — a user's complete worn prop list.
    UserProp(UserProp),
    /// `usrD` — a user's face, colour and props together.
    UserDesc(UserDesc),
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
            opcode::USERLOG => Message::UserLog(UserLog::decode(ref_num, r)?),
            opcode::LISTOFALLROOMS => Message::RoomList(RoomList::decode(ref_num, r)?),
            opcode::LISTOFALLUSERS => Message::UserList(UserList::decode(ref_num, r)?),
            opcode::USERLIST => Message::RoomUsers(UserList::decode(ref_num, r)?),
            opcode::USERNEW => Message::UserNew(UserNew::decode(ref_num, r)?),
            opcode::USEREXIT => Message::UserExit(UserExit::from_ref_num(ref_num)),
            opcode::USERMOVE => Message::UserMove(UserMove::decode(ref_num, r)?),
            opcode::USERFACE => Message::UserFace(UserFace::decode(ref_num, r)?),
            opcode::USERCOLOR => Message::UserColor(UserColor::decode(ref_num, r)?),
            opcode::USERPROP => Message::UserProp(UserProp::decode(ref_num, r)?),
            opcode::USERDESC => Message::UserDesc(UserDesc::decode(ref_num, r)?),
            opcode::USERSTATUS => Message::UserStatus(UserStatus::decode(ref_num, r)?),
            opcode::ROOMDESC => Message::RoomDescription(RoomDescription::decode(r)?),
            opcode::PROPNEW => Message::PropNew(PropNew::decode(r)?),
            opcode::PROPMOVE => Message::PropMove(PropMove::decode(r)?),
            opcode::PROPDEL => Message::PropDel(PropDel::decode(r)?),
            opcode::ROOMDESCEND => Message::RoomDescEnd,
            opcode::TALK => Message::Talk(Talk::decode(ref_num, r)?),
            opcode::WHISPER => Message::Whisper(Whisper::decode(ref_num, r)?),
            opcode::PING => Message::Ping(ref_num),
            opcode::PONG => Message::Pong(ref_num),
            opcode::LOGOFF => Message::Logoff,
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
            Message::UserProp(p) => format!(
                "user props: id={} props={}",
                p.user_id,
                summarize_asset_specs(&p.props)
            ),
            Message::UserDesc(d) => format!(
                "user desc: id={} face={} color={} props={}",
                d.user_id,
                d.face_nbr,
                d.color_nbr,
                summarize_asset_specs(&d.props)
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
            Message::Talk(t) => format!("talk: user_id={} {:?}", t.user_id, t.text),
            Message::Whisper(w) => format!(
                "whisper: from={} to={} {:?}",
                w.user_id, w.target_id, w.text
            ),
            Message::Ping(n) => format!("ping (refNum={n})"),
            Message::Pong(n) => format!("pong (refNum={n})"),
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
    use crate::opcode::{LISTOFALLROOMS, LOGOFF, PING, PROPMOVE, TALK, USERFACE, USERPROP};

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
}

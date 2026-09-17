//! Session state derived from the wire.
//!
//! [`SessionState::apply`] is the whole protocol-to-model translation: it takes
//! one decoded frame, updates the model, and reports what changed. It does no
//! I/O, holds no socket, and never panics on a malformed body — a bad frame is
//! recorded as an error chat line and ignored. That makes it the unit the
//! recorded-fixture tests drive.

use std::collections::BTreeMap;

use palace_render::RoomDesc;
use palace_room::{LooseProp, LoosePropSpec};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::{navr_frame, Frame};
use palace_wire::messages::{self, AssetSpec, Message, Point};
use palace_wire::opcode;
use serde::Serialize;

use crate::xtlk;

/// Where the connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Facts learned from the logon burst.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ServerBanner {
    pub host: String,
    pub port: u16,
    pub byte_order: String,
    pub user_id: i32,
    pub version: Option<String>,
    pub name: Option<String>,
    pub media_base: Option<String>,
    pub total_users: Option<usize>,
}

/// One row of the server's room list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomInfo {
    pub id: i32,
    pub name: String,
    pub users: u16,
    pub flags: u32,
}

/// A user the session knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserInfo {
    pub id: i32,
    pub name: String,
    pub face: i16,
    pub color: i16,
    pub room_id: i16,
    pub x: i16,
    pub y: i16,
    pub props: Vec<u32>,
    pub away: bool,
    pub is_self: bool,
}

/// The flavour of a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    Talk,
    Whisper,
    System,
    Error,
}

/// One line in the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatLine {
    pub seq: u64,
    pub user_id: i32,
    pub name: String,
    pub text: String,
    pub kind: ChatKind,
}

/// What one frame changed, so the runtime knows what to re-emit or re-render.
#[derive(Debug, Default, Clone)]
pub struct Applied {
    pub banner: bool,
    pub rooms: bool,
    pub users: bool,
    pub room_entered: bool,
    pub chat: Vec<ChatLine>,
    pub render: bool,
    pub media_base: bool,
    pub outbound: Vec<Frame>,
}

/// The live model of one session.
#[derive(Debug)]
pub struct SessionState {
    pub status: ConnectionStatus,
    pub banner: ServerBanner,
    pub rooms: Vec<RoomInfo>,
    pub users: BTreeMap<i32, UserInfo>,
    pub room_users: Vec<i32>,
    pub current_room: Option<RoomInfo>,
    pub room_desc: Option<RoomDesc>,
    /// Client-side `DIMROOM` level for the current room; `1.0` is undimmed.
    pub room_dim: f64,
    /// Client-side `HIDEAVATARS` flag; entering a room clears it.
    pub avatars_hidden: bool,
    /// Client-side `SETPICOPACITY`, keyed by hotspot id and state index.
    pub pic_opacity: BTreeMap<(i16, i16), f64>,
    pub chat: Vec<ChatLine>,
    chat_seq: u64,
    last_error: Option<String>,
}

impl SessionState {
    /// A fresh, disconnected state for `host:port`.
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        SessionState {
            status: ConnectionStatus::Disconnected,
            banner: ServerBanner {
                host: host.into(),
                port,
                ..ServerBanner::default()
            },
            rooms: Vec::new(),
            users: BTreeMap::new(),
            room_users: Vec::new(),
            current_room: None,
            room_desc: None,
            room_dim: 1.0,
            avatars_hidden: false,
            pic_opacity: BTreeMap::new(),
            chat: Vec::new(),
            chat_seq: 0,
            last_error: None,
        }
    }

    /// The most recent error, if any.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Record a status transition and, on error, a transcript line.
    pub fn set_status(&mut self, status: ConnectionStatus) {
        self.status = status;
    }

    /// Append a local (non-server) transcript line.
    pub fn system_line(&mut self, kind: ChatKind, text: impl Into<String>) -> ChatLine {
        let line = self.push_chat(-1, "system".to_string(), text.into(), kind);
        if kind == ChatKind::Error {
            self.last_error = Some(line.text.clone());
        }
        line
    }

    fn push_chat(&mut self, user_id: i32, name: String, text: String, kind: ChatKind) -> ChatLine {
        self.chat_seq += 1;
        let line = ChatLine {
            seq: self.chat_seq,
            user_id,
            name,
            text,
            kind,
        };
        self.chat.push(line.clone());
        if self.chat.len() > 500 {
            self.chat.remove(0);
        }
        line
    }

    /// Replace one transcript line's text, returning the updated line.
    ///
    /// `ON INCHAT` handlers rewrite incoming chat, so the stored transcript has
    /// to follow the text the user actually sees.
    pub fn rewrite_chat_line(&mut self, seq: u64, text: String) -> Option<ChatLine> {
        let line = self.chat.iter_mut().find(|line| line.seq == seq)?;
        line.text = text;
        Some(line.clone())
    }

    fn name_of(&self, user_id: i32) -> String {
        self.users
            .get(&user_id)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| format!("user #{user_id}"))
    }

    /// Reset per-room state before entering a new room.
    pub fn begin_room_change(&mut self) {
        self.room_users.clear();
        self.room_desc = None;
        self.current_room = None;
    }

    /// Build a `navR` frame for a room id, carrying our user id as the ref.
    #[must_use]
    pub fn navigate_frame(&self, room_id: i32) -> Frame {
        navr_frame(
            room_id.clamp(0, u16::MAX as i32) as u16,
            self.banner.user_id,
            self.byte_order(),
        )
    }

    /// The session's byte order, as the handshake negotiated it.
    #[must_use]
    pub fn byte_order(&self) -> ByteOrder {
        match self.banner.byte_order.as_str() {
            "big" => ByteOrder::Big,
            _ => ByteOrder::Little,
        }
    }

    /// Apply one inbound frame. Never fails: unknown or malformed bodies become
    /// transcript errors and leave the model untouched.
    pub fn apply(&mut self, frame: &Frame, order: ByteOrder) -> Applied {
        let mut applied = Applied::default();

        if frame.opcode == opcode::XTALK || frame.opcode == opcode::XWHISPER {
            if let Some(text) = xtlk::decode_payload(&frame.payload, order) {
                let kind = if frame.opcode == opcode::XTALK {
                    ChatKind::Talk
                } else {
                    ChatKind::Whisper
                };
                let name = self.name_of(frame.ref_num);
                let line = self.push_chat(frame.ref_num, name, text, kind);
                applied.chat.push(line);
            } else {
                let line = self.system_line(
                    ChatKind::Error,
                    "an encrypted chat line could not be decoded",
                );
                applied.chat.push(line);
            }
            return applied;
        }

        let message = match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
            Ok(m) => m,
            Err(err) => {
                let line = self.system_line(
                    ChatKind::Error,
                    messages::describe_error(frame.opcode, &err),
                );
                applied.chat.push(line);
                return applied;
            }
        };

        match message {
            Message::ServerVersion(v) => {
                self.banner.version = Some(v.to_string());
                applied.banner = true;
            }
            Message::ServerInfo(info) => {
                self.banner.name = Some(info.name.clone());
                applied.banner = true;
            }
            Message::HttpServer(http) => {
                self.banner.media_base = Some(http.url.clone());
                applied.banner = true;
                applied.media_base = true;
            }
            Message::UserLog(log) => {
                self.banner.total_users = Some(log.user_count.max(0) as usize);
                applied.banner = true;
            }
            Message::RoomList(list) => {
                self.rooms = list
                    .rooms
                    .iter()
                    .map(|r| RoomInfo {
                        id: r.room_id,
                        name: r.name.clone(),
                        users: r.user_count,
                        flags: u32::from(r.flags),
                    })
                    .collect();
                applied.rooms = true;
            }
            Message::UserList(list) => {
                if frame.opcode == opcode::USERLIST {
                    self.room_users = list.users.iter().map(|u| u.user_id).collect();
                    for rec in &list.users {
                        let entry = self.users.entry(rec.user_id).or_insert_with(|| UserInfo {
                            id: rec.user_id,
                            name: rec.name.clone(),
                            face: 0,
                            color: 0,
                            room_id: rec.room_id,
                            x: 0,
                            y: 0,
                            props: Vec::new(),
                            away: false,
                            is_self: rec.user_id == self.banner.user_id,
                        });
                        if !rec.name.is_empty() {
                            entry.name = rec.name.clone();
                        }
                        entry.room_id = rec.room_id;
                        entry.is_self = rec.user_id == self.banner.user_id;
                    }
                    self.merge_room_users();
                    applied.users = true;
                } else {
                    for rec in &list.users {
                        self.users
                            .entry(rec.user_id)
                            .and_modify(|u| {
                                if !rec.name.is_empty() {
                                    u.name = rec.name.clone();
                                }
                                u.room_id = rec.room_id;
                            })
                            .or_insert_with(|| UserInfo {
                                id: rec.user_id,
                                name: rec.name.clone(),
                                face: 0,
                                color: 0,
                                room_id: rec.room_id,
                                x: 0,
                                y: 0,
                                props: Vec::new(),
                                away: false,
                                is_self: rec.user_id == self.banner.user_id,
                            });
                    }
                    applied.users = true;
                }
            }
            Message::UserNew(new) => {
                let rec = &new.record;
                let props: Vec<u32> = rec
                    .prop_spec
                    .iter()
                    .take(rec.nbr_props.clamp(0, 9) as usize)
                    .map(|spec| spec.id)
                    .filter(|id| *id > 0)
                    .map(|id| id as u32)
                    .collect();
                let entry = UserInfo {
                    id: rec.user_id,
                    name: rec.name.clone(),
                    face: rec.face_nbr,
                    color: rec.color_nbr,
                    room_id: rec.room_id,
                    x: rec.room_pos.h,
                    y: rec.room_pos.v,
                    props,
                    away: rec.away_flag != 0,
                    is_self: rec.user_id == self.banner.user_id,
                };
                if frame.opcode == opcode::USERNEW {
                    self.users.insert(rec.user_id, entry);
                    if !self.room_users.contains(&rec.user_id) {
                        self.room_users.push(rec.user_id);
                    }
                } else {
                    self.users.insert(rec.user_id, entry);
                }
                self.merge_room_users();
                applied.users = true;
                applied.render = true;
            }
            Message::UserMove(mv) => {
                let name = self.name_of(mv.user_id);
                let entry = self.users.entry(mv.user_id).or_insert_with(|| UserInfo {
                    id: mv.user_id,
                    name,
                    face: 0,
                    color: 0,
                    room_id: 0,
                    x: mv.position.h,
                    y: mv.position.v,
                    props: Vec::new(),
                    away: false,
                    is_self: mv.user_id == self.banner.user_id,
                });
                entry.x = mv.position.h;
                entry.y = mv.position.v;
                applied.render = true;
            }
            Message::UserExit(exit) => {
                self.users.remove(&exit.user_id);
                self.room_users.retain(|id| *id != exit.user_id);
                applied.users = true;
                applied.render = true;
            }
            Message::UserStatus(_) => {}
            Message::UserFace(face) => {
                let changed = self.set_user_face(face.user_id, face.face_nbr);
                applied.users = changed;
                applied.render = changed;
            }
            Message::UserColor(color) => {
                let changed = self.set_user_color(color.user_id, color.color_nbr);
                applied.users = changed;
                applied.render = changed;
            }
            Message::UserProp(prop) => {
                let changed = self.set_user_props(prop.user_id, &prop.props);
                applied.users = changed;
                applied.render = changed;
            }
            Message::UserDesc(desc) => {
                let face = self.set_user_face(desc.user_id, desc.face_nbr);
                let color = self.set_user_color(desc.user_id, desc.color_nbr);
                let props = self.set_user_props(desc.user_id, &desc.props);
                if face || color || props {
                    applied.users = true;
                    applied.render = true;
                }
            }
            Message::PropNew(new) => {
                applied.render = self.add_loose_prop(new.spec, new.position);
            }
            Message::PropMove(mv) => {
                if self.move_loose_prop(mv.prop_num, mv.position) {
                    applied.render = true;
                } else if self.room_desc.is_some() {
                    let line = self.system_line(
                        ChatKind::System,
                        format!("ignored PROPMOVE for absent prop index {}", mv.prop_num),
                    );
                    applied.chat.push(line);
                }
            }
            Message::PropDel(del) => {
                if self.remove_loose_prop(del.prop_num) {
                    applied.render = true;
                } else if del.prop_num != -1 && self.room_desc.is_some() {
                    let line = self.system_line(
                        ChatKind::System,
                        format!("ignored PROPDEL for absent prop index {}", del.prop_num),
                    );
                    applied.chat.push(line);
                }
            }
            Message::RoomDescription(_) => {
                match palace_room::decode_payload(&frame.payload, order) {
                    Ok(room) => {
                        self.current_room = Some(RoomInfo {
                            id: room.header.room_id as i32,
                            name: room.name.clone(),
                            users: room.header.nbr_people.max(0) as u16,
                            flags: room.header.room_flags,
                        });
                        self.room_desc = Some(room);
                        self.room_dim = 1.0;
                        self.avatars_hidden = false;
                        self.pic_opacity.clear();
                        applied.room_entered = true;
                        applied.render = true;
                    }
                    Err(err) => {
                        let line = self.system_line(
                            ChatKind::Error,
                            format!("room descriptor did not parse: {err}"),
                        );
                        applied.chat.push(line);
                    }
                }
            }
            Message::Talk(talk) => {
                let name = self.name_of(talk.user_id);
                let line = self.push_chat(talk.user_id, name, talk.text, ChatKind::Talk);
                applied.chat.push(line);
            }
            Message::Whisper(whisper) => {
                let name = self.name_of(whisper.user_id);
                let line = self.push_chat(whisper.user_id, name, whisper.text, ChatKind::Whisper);
                applied.chat.push(line);
            }
            Message::Ping(ref_num) => {
                applied.outbound.push(Frame::empty(opcode::PONG, ref_num));
            }
            Message::Logoff => {
                applied.chat.push(self.system_line(
                    ChatKind::System,
                    format!(
                        "server sent {} ref={} len={}",
                        frame.opcode.describe(),
                        frame.ref_num,
                        frame.payload.len()
                    ),
                ));
            }
            Message::Unknown { opcode: op, .. } if op.is_known() => {
                let line = self.system_line(
                    ChatKind::System,
                    format!("ignored unimplemented opcode {}", op.describe()),
                );
                applied.chat.push(line);
            }
            _ => {}
        }

        applied
    }

    /// A user the session knows about, or `None` for an id that cannot name one.
    ///
    /// User ids are positive, and the reference server relays `USERCOLOR` with
    /// `refNum` 0 rather than the sender's id, so a non-positive ref is never
    /// trusted. An unknown positive id is ignored rather than inserted: the
    /// appearance messages do not describe a whole new user.
    fn known_user_mut(&mut self, user_id: i32) -> Option<&mut UserInfo> {
        if user_id <= 0 {
            return None;
        }
        self.users.get_mut(&user_id)
    }

    fn set_user_face(&mut self, user_id: i32, face: i16) -> bool {
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        let changed = user.face != face;
        user.face = face;
        changed
    }

    fn set_user_color(&mut self, user_id: i32, color: i16) -> bool {
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        let changed = user.color != color;
        user.color = color;
        changed
    }

    /// Replace a user's worn props wholesale, in the order received.
    ///
    /// `USERPROP` always carries the complete current list with unused slots
    /// omitted, so this assigns rather than merges. Asset ids are stored as the
    /// bit pattern's unsigned value, the namespace the prop store uses.
    fn set_user_props(&mut self, user_id: i32, props: &[AssetSpec]) -> bool {
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        let worn: Vec<u32> = props.iter().map(|spec| spec.id as u32).collect();
        if user.props == worn {
            return false;
        }
        user.props = worn;
        true
    }

    /// Append a loose prop to the room.
    ///
    /// The index later `PROPMOVE`/`PROPDEL` address is the list length before
    /// the append: the server appends, and two reference clients that prepend at
    /// index 0 would desync every later index.
    pub(crate) fn add_loose_prop(&mut self, spec: AssetSpec, loc: Point) -> bool {
        let Some(room) = self.room_desc.as_mut() else {
            return false;
        };
        room.loose_props.push(LooseProp {
            next_ofst: 0,
            reserved: 0,
            spec: LoosePropSpec {
                id: spec.id as u32,
                crc: spec.crc,
            },
            flags: 0,
            ref_con: 0,
            loc,
        });
        true
    }

    /// Move one loose prop by its insertion-order index. `loc` is absolute.
    pub(crate) fn move_loose_prop(&mut self, prop_num: i32, loc: Point) -> bool {
        let Some(room) = self.room_desc.as_mut() else {
            return false;
        };
        let Ok(index) = usize::try_from(prop_num) else {
            return false;
        };
        let Some(prop) = room.loose_props.get_mut(index) else {
            return false;
        };
        prop.loc = loc;
        true
    }

    /// Delete one loose prop by its insertion-order index.
    ///
    /// `-1` clears the room (:1454). Any other out-of-range index is ignored
    /// rather than clearing: a stale index must not empty the room.
    pub(crate) fn remove_loose_prop(&mut self, prop_num: i32) -> bool {
        let Some(room) = self.room_desc.as_mut() else {
            return false;
        };
        if prop_num == -1 {
            let changed = !room.loose_props.is_empty();
            room.loose_props.clear();
            return changed;
        }
        let Ok(index) = usize::try_from(prop_num) else {
            return false;
        };
        if index >= room.loose_props.len() {
            return false;
        }
        room.loose_props.remove(index);
        true
    }

    fn merge_room_users(&mut self) {
        let known: Vec<i32> = self
            .room_users
            .iter()
            .copied()
            .filter(|id| self.users.contains_key(id))
            .collect();
        self.room_users = known;
        self.room_users.sort_unstable();
        self.room_users.dedup();
    }

    /// The users currently in the entered room, in a stable order.
    #[must_use]
    pub fn users_in_room(&self) -> Vec<UserInfo> {
        let room_id = self.current_room.as_ref().map(|r| r.id as i16);
        let mut out: Vec<UserInfo> = self
            .users
            .values()
            .filter(|u| match room_id {
                Some(id) => u.room_id == id || self.room_users.contains(&u.id),
                None => true,
            })
            .cloned()
            .collect();
        out.sort_by_key(|u| (u.y, u.x, u.id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_wire::byteorder::Writer;

    const SELF: i32 = 13;

    fn room_state() -> SessionState {
        let mut state = SessionState::new("test", 1);
        state.banner.user_id = SELF;
        state.room_desc = Some(
            palace_room::decode_payload(
                include_bytes!("../../../fixtures/rooms/86.bin"),
                ByteOrder::Little,
            )
            .expect("the fixture room decodes"),
        );
        state
    }

    fn add_user(state: &mut SessionState, id: i32) {
        state.users.insert(
            id,
            UserInfo {
                id,
                name: format!("user-{id}"),
                face: 0,
                color: 0,
                room_id: 901,
                x: 0,
                y: 0,
                props: Vec::new(),
                away: false,
                is_self: id == SELF,
            },
        );
    }

    fn prop_body(specs: &[(i32, u32)]) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(specs.len() as i32);
        for (id, crc) in specs {
            w.write_i32(*id);
            w.write_u32(*crc);
        }
        w.into_vec()
    }

    fn move_body(index: i32, loc: Point) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(index);
        loc.encode(&mut w);
        w.into_vec()
    }

    fn loose_ids(state: &SessionState) -> Vec<u32> {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .loose_props
            .iter()
            .map(|prop| prop.spec.id)
            .collect()
    }

    #[test]
    fn wire_face_and_color_update_only_a_known_positive_user() {
        let mut state = room_state();
        add_user(&mut state, 21);

        let applied = state.apply(
            &Frame::new(opcode::USERFACE, 21, 7i16.to_le_bytes().to_vec()),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        assert_eq!(state.users[&21].face, 7);

        let applied = state.apply(
            &Frame::new(opcode::USERCOLOR, 21, 3i16.to_le_bytes().to_vec()),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        assert_eq!(state.users[&21].color, 3);

        // The reference server relays USERCOLOR with refNum 0 rather than the
        // sender's id; it must name no user and create none.
        let before = state.users.clone();
        let applied = state.apply(
            &Frame::new(opcode::USERFACE, 0, 9i16.to_le_bytes().to_vec()),
            ByteOrder::Little,
        );
        assert!(!applied.users && !applied.render);
        assert_eq!(state.users, before);

        let applied = state.apply(
            &Frame::new(opcode::USERFACE, 99, 9i16.to_le_bytes().to_vec()),
            ByteOrder::Little,
        );
        assert!(!applied.users && !applied.render);
        assert!(!state.users.contains_key(&99));
    }

    #[test]
    fn wire_userprop_replaces_the_worn_list_wholesale() {
        let mut state = room_state();
        add_user(&mut state, 21);

        let applied = state.apply(
            &Frame::new(
                opcode::USERPROP,
                21,
                prop_body(&[(0x1122_3344, 0xdead_beef), (0xA26F_9DE3u32 as i32, 0)]),
            ),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        assert_eq!(state.users[&21].props, vec![0x1122_3344, 0xA26F_9DE3]);

        let applied = state.apply(
            &Frame::new(opcode::USERPROP, 21, prop_body(&[(40, 0), (20, 0)])),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        assert_eq!(
            state.users[&21].props,
            vec![40, 20],
            "the list is replaced, not merged or re-sorted"
        );

        let applied = state.apply(
            &Frame::new(opcode::USERPROP, 21, prop_body(&[])),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        assert!(
            state.users[&21].props.is_empty(),
            "nbrProps 0 clears the list"
        );
    }

    #[test]
    fn wire_userdesc_sets_face_color_and_props_in_one_frame() {
        let mut state = room_state();
        add_user(&mut state, SELF);

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(9);
        w.write_i16(3);
        w.write_bytes(&prop_body(&[(111, 7), (222, 0)]));
        let applied = state.apply(
            &Frame::new(opcode::USERDESC, SELF, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(applied.users && applied.render);
        let user = &state.users[&SELF];
        assert_eq!((user.face, user.color), (9, 3));
        assert_eq!(user.props, vec![111, 222]);
        assert!(user.is_self);
    }

    #[test]
    fn prop_messages_add_move_and_delete_by_insertion_index() {
        let mut state = room_state();
        let base = loose_ids(&state).len();

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(1111);
        w.write_u32(0xdead_beef);
        Point::new(240, 120).encode(&mut w);
        let applied = state.apply(
            &Frame::new(opcode::PROPNEW, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(applied.render);
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(2222);
        w.write_u32(0);
        Point::new(20, 10).encode(&mut w);
        assert!(
            state
                .apply(
                    &Frame::new(opcode::PROPNEW, 0, w.into_vec()),
                    ByteOrder::Little
                )
                .render
        );
        assert_eq!(loose_ids(&state)[base..], [1111, 2222]);

        let added = &state.room_desc.as_ref().unwrap().loose_props[base];
        assert_eq!(added.spec.crc, 0xdead_beef, "PROPNEW carries a real CRC");
        assert_eq!(
            (added.loc.h, added.loc.v),
            (120, 240),
            "loc stores x in h and y in v"
        );
        assert_eq!(
            (added.flags, added.ref_con, added.next_ofst, added.reserved),
            (0, 0, 0, 0),
            "the wire carries no flags or refCon"
        );

        // The moved-to position is absolute, not a delta.
        let applied = state.apply(
            &Frame::new(
                opcode::PROPMOVE,
                0,
                move_body(base as i32 + 1, Point::new(400, 300)),
            ),
            ByteOrder::Little,
        );
        assert!(applied.render);
        let moved = &state.room_desc.as_ref().unwrap().loose_props[base + 1];
        assert_eq!((moved.loc.h, moved.loc.v), (300, 400));

        let applied = state.apply(
            &Frame::new(opcode::PROPMOVE, 0, move_body(999, Point::new(1, 1))),
            ByteOrder::Little,
        );
        assert!(!applied.render);
        assert!(
            applied
                .chat
                .iter()
                .any(|line| line.text.contains("ignored PROPMOVE")),
            "an out-of-range move is reported"
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(base as i32);
        let applied = state.apply(
            &Frame::new(opcode::PROPDEL, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert_eq!(loose_ids(&state)[base..], [2222]);

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(999);
        let applied = state.apply(
            &Frame::new(opcode::PROPDEL, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(!applied.render);
        assert_eq!(loose_ids(&state).len(), base + 1);
        assert!(
            applied
                .chat
                .iter()
                .any(|line| line.text.contains("ignored PROPDEL")),
            "an out-of-range delete is reported"
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(-1);
        let applied = state.apply(
            &Frame::new(opcode::PROPDEL, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert!(state.room_desc.as_ref().unwrap().loose_props.is_empty());
    }
}

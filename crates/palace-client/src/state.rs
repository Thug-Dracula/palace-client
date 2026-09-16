//! Session state derived from the wire.
//!
//! [`SessionState::apply`] is the whole protocol-to-model translation: it takes
//! one decoded frame, updates the model, and reports what changed. It does no
//! I/O, holds no socket, and never panics on a malformed body — a bad frame is
//! recorded as an error chat line and ignored. That makes it the unit the
//! recorded-fixture tests drive.

use std::collections::BTreeMap;

use palace_render::RoomDesc;
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::{navr_frame, Frame};
use palace_wire::messages::{self, Message};
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

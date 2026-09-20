//! Session state derived from the wire.
//!
//! [`SessionState::apply`] is the whole protocol-to-model translation: it takes
//! one decoded frame, updates the model, and reports what changed. It does no
//! I/O, holds no socket, and never panics on a malformed body — a bad frame is
//! recorded as an error chat line and ignored. That makes it the unit the
//! recorded-fixture tests drive.

use std::collections::{BTreeMap, HashSet};

use palace_asset::ScriptFetch;
use palace_host::ScriptEvent;
use palace_render::{DrawList, RoomDesc};
use palace_room::{LooseProp, LoosePropSpec};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::{navr_frame, Frame};
use palace_wire::messages::{
    self, authresponse_frame, AssetSpec, AvatarHash, AvatarQuery, AvatarSend, Message, Point,
    UserRec, AT_AVATAR, AT_PROP,
};
use palace_wire::opcode;
use serde::Serialize;

use crate::runtime::{
    move_spot_in_room, remove_local_hotspot, set_local_spot_state, set_pic_offset_in_room,
};
use crate::secret::Secret;
use crate::type1::Type1AvatarLimits;
use crate::xtlk;

/// A door hotspot's `state` field is its lock: `HS_Unlock` is 0 and `HS_Lock`
/// is 1 (protocol reference :1677-1680).
pub(crate) const HS_UNLOCK: i16 = 0;
pub(crate) const HS_LOCK: i16 = 1;

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
    /// `AT_PROP` for a classic prop avatar, `AT_AVATAR` for a Type 1 avatar.
    pub avatar_type: i16,
    /// `AF_*` flags for a Type 1 avatar.
    pub avatar_flags: u16,
    /// The Type 1 avatar's 20-byte content hash, lowercase hex, when it has one.
    pub avatar_hash: Option<String>,
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

/// A script event a decoded frame asks the host to run.
///
/// [`SessionState::apply`] is the only place a frame's meaning is known, so it
/// records each event here and the runtime dispatches it after the model is
/// updated. `spot` names the hotspot the handler is scoped to (LOCK, UNLOCK,
/// STATECHANGE); `None` runs the room-level handlers, exactly as
/// `Enter`/`InChat` do through `run_dispatch(..., only)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptStimulus {
    /// The event to run.
    pub event: ScriptEvent,
    /// The hotspot the event is scoped to, or `None` for a room-level event.
    pub spot: Option<i32>,
}

/// What one frame changed, so the runtime knows what to re-emit or re-render.
#[derive(Debug, Default, Clone)]
pub struct Applied {
    pub banner: bool,
    pub rooms: bool,
    pub users: bool,
    /// A remote user's reported position changed, so the runtime must publish
    /// the avatar roster even though no user-list frame arrived.
    pub users_moved: bool,
    pub room_entered: bool,
    pub chat: Vec<ChatLine>,
    pub render: bool,
    pub media_base: bool,
    /// The server advertised (or changed) its Type 1 avatar limits.
    pub avatar_limits: bool,
    /// An inbound `sAva`: the runtime caches its bytes or URL by hash.
    pub avatar_send: Option<AvatarSend>,
    pub outbound: Vec<Frame>,
    /// Script events this frame asks the host to run, in decode order.
    ///
    /// The runtime dispatches each exactly once, after the model has taken the
    /// frame's change, so a handler sees the state the frame produced.
    pub scripts: Vec<ScriptStimulus>,
    /// Set when the frame was a `SERVERDOWN`: the server's reason for ending
    /// the session, which the runtime turns into a status and a transcript line
    /// and acts on by stopping without reconnecting.
    pub disconnect: Option<messages::ServerDown>,
}

/// The sign-in name and password used to answer an `auth` challenge.
#[derive(Debug)]
struct Credential {
    user_name: String,
    password: Secret,
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
    /// The room a `navR` is leaving, remembered until the destination's
    /// descriptor arrives. A refused change (`sErr`) restores it, so a dead
    /// door cannot strand the session with no room and no hotspots.
    previous_room: Option<RoomInfo>,
    pub room_desc: Option<RoomDesc>,
    /// Client-side `DIMROOM` level for the current room; `1.0` is undimmed.
    pub room_dim: f64,
    /// Client-side `HIDEAVATARS` flag; entering a room clears it.
    pub avatars_hidden: bool,
    /// Client-side `HIDESMILEYS` flag.
    pub hide_smileys: bool,
    /// Client-side `LOCKUSERPROPS` flag.
    pub lock_user_props: bool,
    /// Client-side `AUTOUSERLAYER` flag.
    pub auto_user_layer: bool,
    /// Client-side `SETPICOPACITY`, keyed by hotspot id and state index.
    pub pic_opacity: BTreeMap<(i16, i16), f64>,
    /// Client-side `SETPICBRIGHTNESS`, keyed by hotspot id and state index.
    pub pic_brightness: BTreeMap<(i16, i16), i32>,
    /// Client-side `SETPICSATURATION`, keyed by hotspot id and state index.
    pub pic_saturation: BTreeMap<(i16, i16), i32>,
    /// The hover tooltip text a script set with `SETTOOLTIP`, or `None` after
    /// `CLEARTOOLTIP`. Local-only: it is never sent to the server.
    pub tooltip: Option<String>,
    /// The current room's paint: the room's own stored draw commands plus every
    /// `DRAW` that has arrived since. Scoped to one room, so a `DRAW` for a room
    /// this session has left cannot appear on the new room's canvas.
    pub draw: DrawList,
    /// `LOADSCRIPT`/`HTTPGET` jobs not yet handed to the fetch worker, and the
    /// room they came from. Drained by the runtime each loop; cleared on room
    /// change so a left room's late effects cannot fetch into the new room.
    pub pending_fetches: Vec<ScriptFetch>,
    pending_fetch_room: Option<i32>,
    /// Hotspots whose script text a `SETSPOTSCRIPT` changed and whose handler set
    /// must be re-parsed. The runtime owns the engine, so the apply path queues
    /// the spot and the main loop drains it. Cleared on room change, like
    /// `pending_fetches`, so a left room's spot cannot name the new room's.
    pub pending_spot_scripts: Vec<i32>,
    /// Prop ids a `LOADPROPS` asked the client to preload. The runtime unions
    /// them with the room's props when it queues asset requests, so a later
    /// `DONPROP` does not have to wait on a fresh fetch. Cleared on room change.
    pub pending_prop_loads: Vec<u32>,
    pub chat: Vec<ChatLine>,
    chat_seq: u64,
    last_error: Option<String>,
    /// The credential that answers an `auth` challenge, when one is configured.
    credential: Option<Credential>,
    /// The server's Type 1 avatar limits from its `'AVAT'` block, once asked for.
    pub avatar_limits: Option<Type1AvatarLimits>,
    /// Hashes a `qAva` has already been sent for, so a repeated user record does
    /// not spam the server with the same query.
    requested_avatars: HashSet<AvatarHash>,
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
            previous_room: None,
            room_desc: None,
            room_dim: 1.0,
            avatars_hidden: false,
            hide_smileys: false,
            lock_user_props: false,
            auto_user_layer: false,
            pic_opacity: BTreeMap::new(),
            pic_brightness: BTreeMap::new(),
            pic_saturation: BTreeMap::new(),
            tooltip: None,
            draw: DrawList::new(),
            pending_fetches: Vec::new(),
            pending_fetch_room: None,
            pending_spot_scripts: Vec::new(),
            pending_prop_loads: Vec::new(),
            chat: Vec::new(),
            chat_seq: 0,
            last_error: None,
            credential: None,
            avatar_limits: None,
            requested_avatars: HashSet::new(),
        }
    }

    /// Arm the session to answer an `auth` challenge from `user_name` with
    /// `password`.
    pub fn set_credential(&mut self, user_name: impl Into<String>, password: Secret) {
        self.credential = Some(Credential {
            user_name: user_name.into(),
            password,
        });
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

    fn user_info_from_record(&self, rec: &UserRec) -> UserInfo {
        let record_props: Vec<u32> = rec
            .prop_spec
            .iter()
            .take(rec.nbr_props.clamp(0, 9) as usize)
            .map(|spec| spec.id)
            .filter(|id| *id > 0)
            .map(|id| id as u32)
            .collect();
        // A worn list we authored (`PalaceUser.setProps` sends it through
        // `updatePropsOnServer`) outranks the record: the server's snapshot can
        // predate a room script's `SETPROPS` and would otherwise undress us on
        // entry. An empty local list claims nothing, so the record fills it in.
        let props = match self.users.get(&rec.user_id) {
            Some(local) if rec.user_id == self.banner.user_id && !local.props.is_empty() => {
                local.props.clone()
            }
            _ => record_props,
        };
        UserInfo {
            id: rec.user_id,
            name: rec.name.clone(),
            face: rec.face_nbr,
            color: rec.color_nbr,
            room_id: rec.room_id,
            x: rec.room_pos.h,
            y: rec.room_pos.v,
            props,
            // The field the protocol reference calls `awayFlag` is `avatarType`
            // in the PP SDK and the compiled server (`mansion.h:315`,
            // `UserRec:t110 avatarType:8,688,16`). A Type 1 user is not away,
            // so only a classic record's nonzero value means away.
            away: rec.away_flag != 0 && rec.avatar_type() != AT_AVATAR,
            is_self: rec.user_id == self.banner.user_id,
            avatar_type: rec.avatar_type(),
            avatar_flags: rec.avatar_flags(),
            avatar_hash: rec.type1_hash().map(|hash| hash.to_hex()),
        }
    }

    /// Reset per-room state before entering a new room.
    pub fn begin_room_change(&mut self) {
        self.room_users.clear();
        self.room_desc = None;
        self.previous_room = self.current_room.take();
        // Paint is room state: leaving must not leave another room's strokes on
        // the canvas. The arriving room seeds its own list instead.
        self.draw = DrawList::new();
        self.pending_fetches.clear();
        self.pending_fetch_room = None;
        self.pending_spot_scripts.clear();
        self.pending_prop_loads.clear();
    }

    /// Start the draw list from a room's own stored commands.
    ///
    /// A room descriptor carries the commands painted when it is entered
    /// (`firstDrawCmd` chain). Received `DRAW` messages append on top of these;
    /// this is the base they append to.
    pub fn load_room_draw(&mut self, room: &RoomDesc) {
        self.draw = DrawList::from_commands(room.draw_cmds.iter().cloned());
    }

    /// Build a `navR` frame for a room id, carrying our user id as the ref.
    #[must_use]
    pub fn navigate_frame(&self, room_id: i32) -> Frame {
        // RoomID is 16-bit on the wire, so narrow by truncation to its low 16
        // bits; clamping to 65535 would send a different, non-existent room
        // (73251 -> 7715, not 65535).
        navr_frame(room_id as u16, self.banner.user_id, self.byte_order())
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
                            avatar_type: AT_PROP,
                            avatar_flags: 0,
                            avatar_hash: None,
                        });
                }
                applied.users = true;
            }
            Message::RoomUsers(list) => {
                self.room_users = list.users.iter().map(|u| u.user_id).collect();
                for rec in &list.users {
                    let entry = self.user_info_from_record(rec);
                    if let Some(hash) = entry.avatar_hash.as_deref().and_then(AvatarHash::from_hex)
                    {
                        self.queue_avatar_query(rec.user_id, hash, &mut applied);
                    }
                    self.users.insert(rec.user_id, entry);
                }
                self.merge_room_users();
                applied.users = true;
                applied.render = true;
            }
            Message::UserNew(new) => {
                let rec = &new.record;
                let entry = self.user_info_from_record(rec);
                if let Some(hash) = entry.avatar_hash.as_deref().and_then(AvatarHash::from_hex) {
                    self.queue_avatar_query(rec.user_id, hash, &mut applied);
                }
                self.users.insert(rec.user_id, entry);
                if !self.room_users.contains(&rec.user_id) {
                    self.room_users.push(rec.user_id);
                }
                self.merge_room_users();
                applied.users = true;
                applied.render = true;
                if rec.user_id != self.banner.user_id {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::UserEnter,
                        spot: None,
                    });
                }
            }
            Message::UserMove(mv) => {
                if mv.user_id == self.banner.user_id {
                    // The client already moved its own avatar when the click
                    // was made; only redraw when the server genuinely disagrees
                    // with the predicted position.
                    let reconciled = self.reconcile_self_move(mv.position.h, mv.position.v);
                    if crate::trace::enabled() {
                        crate::trace::state(&format!(
                            "self_move echo id={} at=({},{}) redrew={reconciled}",
                            mv.user_id, mv.position.h, mv.position.v
                        ));
                    }
                    if reconciled {
                        applied.render = true;
                    }
                } else {
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
                        is_self: false,
                        avatar_type: AT_PROP,
                        avatar_flags: 0,
                        avatar_hash: None,
                    });
                    entry.x = mv.position.h;
                    entry.y = mv.position.v;
                    applied.users_moved = true;
                    applied.render = true;
                }
            }
            Message::UserExit(exit) => {
                // The server reports us leaving the room we came from as part
                // of a room change, and `USERNAME` reads this very entry, so the
                // self entry outlives the gap until the destination names us
                // again. The room-scoped list still drops us, so the avatar is
                // not drawn in the wrong room.
                let was_known = if exit.user_id == self.banner.user_id {
                    self.users.contains_key(&exit.user_id)
                } else {
                    self.users.remove(&exit.user_id).is_some()
                };
                self.room_users.retain(|id| *id != exit.user_id);
                applied.users = true;
                applied.render = true;
                if was_known && exit.user_id != self.banner.user_id {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::UserLeave,
                        spot: None,
                    });
                }
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
            Message::UserName(name) => {
                let changed = self.set_user_name(name.user_id, &name.name);
                applied.users = changed;
                applied.render = changed;
                if changed {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::NameChange,
                        spot: None,
                    });
                }
            }
            Message::UserProp(prop) => {
                let changed = self.set_user_props(prop.user_id, &prop.props);
                applied.users = changed;
                applied.render = changed;
                if changed {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::PropChange,
                        spot: None,
                    });
                }
            }
            Message::UserPropAvatar(prop) => {
                let changed = self.set_user_avatar_identity(
                    prop.user_id,
                    prop.avatar_type,
                    prop.avatar_flags,
                    prop.hash,
                );
                if changed {
                    applied.users = true;
                    applied.render = true;
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::PropChange,
                        spot: None,
                    });
                }
                self.queue_avatar_query(prop.user_id, prop.hash, &mut applied);
            }
            Message::UserDescAvatar(desc) => {
                let face = self.set_user_face(desc.user_id, desc.face_nbr);
                let color = self.set_user_color(desc.user_id, desc.color_nbr);
                let avatar = self.set_user_avatar_identity(
                    desc.user_id,
                    desc.avatar_type,
                    desc.avatar_flags,
                    desc.hash,
                );
                if face || color || avatar {
                    applied.users = true;
                    applied.render = true;
                }
                self.queue_avatar_query(desc.user_id, desc.hash, &mut applied);
            }
            Message::ExtendedInfo(reply) => {
                if let Some(info) = reply.avatar(order) {
                    let limits = Type1AvatarLimits::from_info(info);
                    if self.avatar_limits != Some(limits) {
                        self.avatar_limits = Some(limits);
                        applied.avatar_limits = true;
                    }
                }
            }
            Message::AvatarFlags(flags) => {
                if let Some(user) = self.users.get_mut(&flags.user_id) {
                    if user.avatar_flags != flags.flags {
                        user.avatar_flags = flags.flags;
                        applied.users = true;
                        applied.render = true;
                    }
                }
            }
            Message::AvatarSend(send) => {
                applied.avatar_send = Some(send);
            }
            Message::AvatarQuery(_) => {
                // A server-to-client `qAva` asks whether we hold another user's
                // avatar. We do not relay cached avatars back to the server yet;
                // the runtime answers inbound `sAva` by caching it for display.
            }
            Message::UserDesc(desc) => {
                let face = self.set_user_face(desc.user_id, desc.face_nbr);
                let color = self.set_user_color(desc.user_id, desc.color_nbr);
                let props = self.set_user_props(desc.user_id, &desc.props);
                if face || color || props {
                    applied.users = true;
                    applied.render = true;
                }
                if props {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::PropChange,
                        spot: None,
                    });
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
            Message::DoorLock(lock) => {
                let door = i32::from(lock.door_id);
                applied.render = set_local_spot_state(self, lock.room_id, door, i32::from(HS_LOCK));
                if applied.render {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::Lock,
                        spot: Some(door),
                    });
                }
            }
            Message::DoorUnlock(lock) => {
                let door = i32::from(lock.door_id);
                applied.render =
                    set_local_spot_state(self, lock.room_id, door, i32::from(HS_UNLOCK));
                if applied.render {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::Unlock,
                        spot: Some(door),
                    });
                }
            }
            Message::SpotState(spot) => {
                let spot_id = i32::from(spot.spot_id);
                let previous = self.spot_state_in_room(spot.room_id, spot_id);
                applied.render =
                    set_local_spot_state(self, spot.room_id, spot_id, i32::from(spot.state));
                if previous.is_some_and(|previous| previous != spot.state) {
                    applied.scripts.push(ScriptStimulus {
                        event: ScriptEvent::StateChange,
                        spot: Some(spot_id),
                    });
                }
            }
            Message::SpotDel(del) => {
                applied.render = remove_local_hotspot(self, i32::from(del.spot_id));
            }
            Message::SpotMove(mv) => {
                applied.render = move_spot_in_room(
                    self,
                    mv.room_id,
                    i32::from(mv.spot_id),
                    i32::from(mv.position.h),
                    i32::from(mv.position.v),
                );
            }
            Message::PictMove(pm) => {
                applied.render = set_pic_offset_in_room(
                    self,
                    pm.room_id,
                    i32::from(pm.spot_id),
                    None,
                    i32::from(pm.position.h),
                    i32::from(pm.position.v),
                );
            }
            Message::SpotNew => {
                applied.chat.push(self.system_line(
                    ChatKind::System,
                    "server announced a new hotspot without describing it; this room is stale",
                ));
            }
            // The record layout is `palace-room`'s model, so it is parsed where
            // that model lives. A record with no complete header names no
            // command: appending the parser's default would leave a phantom undo
            // entry, so it is ignored. A full header always parses (the parser
            // never panics) and its command is applied.
            Message::Draw(draw) if draw.len() >= palace_room::DRAW_CMD_HEADER_LEN => {
                let (cmd, _warnings) = palace_room::decode_draw_record(&draw.body, order);
                self.draw.apply(cmd);
                applied.render = true;
            }
            Message::Draw(_) => {}
            Message::Authenticate => match &self.credential {
                Some(credential) => applied.outbound.push(authresponse_frame(
                    &credential.user_name,
                    credential.password.expose(),
                    order,
                )),
                None => {
                    applied.chat.push(self.system_line(
                        ChatKind::Error,
                        "server asked this client to authenticate; it cannot answer yet, so logon will not complete",
                    ));
                }
            },
            Message::NavError(err) => {
                applied
                    .chat
                    .push(self.system_line(ChatKind::Error, err.describe()));
                if let Some(room) = self.previous_room.take() {
                    applied.outbound.push(self.navigate_frame(room.id));
                    self.current_room = Some(room);
                }
            }
            Message::RoomDescription(_) => {
                match palace_room::decode_payload(&frame.payload, order) {
                    Ok(room) => {
                        // `room_entered` says a description arrived; `arrived`
                        // says it is for a room we were not already in. A
                        // repeated description or a spot update must not re-run
                        // the lifecycle, so the two differ.
                        let arrived = self.current_room.as_ref().map(|info| info.id)
                            != Some(i32::from(room.header.room_id));
                        self.current_room = Some(RoomInfo {
                            id: i32::from(room.header.room_id),
                            name: room.name.clone(),
                            users: room.header.nbr_people.max(0) as u16,
                            flags: room.header.room_flags,
                        });
                        if arrived || self.room_desc.is_none() {
                            self.load_room_draw(&room);
                        }
                        self.room_desc = Some(room);
                        self.previous_room = None;
                        self.room_dim = 1.0;
                        self.avatars_hidden = false;
                        self.pic_opacity.clear();
                        applied.room_entered = true;
                        applied.render = true;
                        if arrived {
                            // The reference client runs the room lifecycle in
                            // this order once per arrival, after the room is set
                            // and its spots are available: ROOMLOAD, ENTER,
                            // ROOMREADY. Recording them here keeps the meaning in
                            // the decoder and lets the runtime dispatch them in
                            // order through `dispatch_scripts`.
                            applied.scripts.extend([
                                ScriptStimulus {
                                    event: ScriptEvent::RoomLoad,
                                    spot: None,
                                },
                                ScriptStimulus {
                                    event: ScriptEvent::Enter,
                                    spot: None,
                                },
                                ScriptStimulus {
                                    event: ScriptEvent::RoomReady,
                                    spot: None,
                                },
                            ]);
                        }
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
            Message::ServerDown(down) => {
                self.status = ConnectionStatus::Disconnected;
                applied.disconnect = Some(down);
                applied.scripts.push(ScriptStimulus {
                    event: ScriptEvent::SignOff,
                    spot: None,
                });
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

    /// The `state` field of a hotspot in `room_id`, when that room is the one
    /// being shown and the hotspot exists. `None` otherwise, which is how a
    /// `SPOTSTATE` for another room (or an absent spot) avoids a change event.
    fn spot_state_in_room(&self, room_id: i16, spot: i32) -> Option<i16> {
        let room = self.room_desc.as_ref()?;
        if room.header.room_id != room_id {
            return None;
        }
        let spot = i16::try_from(spot).ok()?;
        room.hotspots
            .iter()
            .find(|hotspot| hotspot.id == spot)
            .map(|hotspot| hotspot.state)
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

    /// Apply a server-reported rename (`usrN`) to a known user.
    ///
    /// Assigns rather than merging: the spec makes the received name
    /// authoritative, which is how a failed rename is reverted — the server
    /// sends the previous name back and this puts it in place. An unknown user
    /// is ignored; `usrN` renames someone the client already knows, it does not
    /// introduce a new user.
    fn set_user_name(&mut self, user_id: i32, name: &str) -> bool {
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        if user.name == name {
            return false;
        }
        user.name = name.to_string();
        true
    }

    /// Replace a user's worn props wholesale, in the order received.
    ///
    /// `USERPROP` always carries the complete current list with unused slots
    /// omitted, so this assigns rather than merges. Asset ids are stored as the
    /// bit pattern's unsigned value, the namespace the prop store uses.
    fn set_user_props(&mut self, user_id: i32, props: &[AssetSpec]) -> bool {
        let self_id = self.banner.user_id;
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        let worn: Vec<u32> = props.iter().map(|spec| spec.id as u32).collect();
        // Same rule as `user_info_from_record`: a worn list we authored outranks
        // a stale server snapshot, so an empty `usrP`/`usrD` echo must not
        // undress us after a room script's `SETPROPS`.
        if user_id == self_id && !user.props.is_empty() && worn.is_empty() {
            return false;
        }
        if user.props == worn {
            return false;
        }
        user.props = worn;
        true
    }

    /// Set a user's Type 1 avatar identity: type, flags and hash together.
    ///
    /// Assigns the hash as hex so it survives JSON; a `AT_PROP` update clears
    /// it. Returns whether anything changed.
    fn set_user_avatar_identity(
        &mut self,
        user_id: i32,
        avatar_type: i16,
        avatar_flags: u16,
        hash: AvatarHash,
    ) -> bool {
        let Some(user) = self.known_user_mut(user_id) else {
            return false;
        };
        let hex = (avatar_type == AT_AVATAR && !hash.is_zero()).then(|| hash.to_hex());
        if user.avatar_type == avatar_type
            && user.avatar_flags == avatar_flags
            && user.avatar_hash == hex
        {
            return false;
        }
        user.avatar_type = avatar_type;
        user.avatar_flags = avatar_flags;
        user.avatar_hash = hex;
        true
    }

    /// Set the signed-in user's Type 1 avatar to `hash`.
    pub(crate) fn set_self_avatar_hash(&mut self, hash: AvatarHash) -> bool {
        let user_id = self.banner.user_id;
        self.set_user_avatar_identity(user_id, AT_AVATAR, 0, hash)
    }

    /// Return the signed-in user to a classic prop avatar.
    pub(crate) fn clear_self_avatar_hash(&mut self) -> bool {
        let user_id = self.banner.user_id;
        self.set_user_avatar_identity(user_id, AT_PROP, 0, AvatarHash::ZERO)
    }

    /// Queue one `qAva` for a hash we do not hold, so the server sends its bytes.
    ///
    /// Only remote users are queried: our own avatar's bytes are cached the
    /// moment we set them. Each hash is queried at most once per session.
    fn queue_avatar_query(&mut self, user_id: i32, hash: AvatarHash, applied: &mut Applied) {
        if hash.is_zero() || user_id == self.banner.user_id {
            return;
        }
        if !self.requested_avatars.insert(hash) {
            return;
        }
        if let Ok(frame) = (AvatarQuery { hash }).frame(self.byte_order()) {
            applied.outbound.push(frame);
        }
    }

    /// Remember that we hold a hash, so a later record does not re-query it.
    pub(crate) fn mark_avatar_known(&mut self, hash: AvatarHash) {
        self.requested_avatars.insert(hash);
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

    /// Apply the local user's own move to the model at once.
    ///
    /// The reference server relays our own `uLoc` only to the *other* users in
    /// the room, so the client is authoritative for where its avatar is drawn.
    /// The wire message still goes out alongside; it is simply not awaited.
    /// Returns whether the position changed.
    pub fn predict_self_move(&mut self, x: i32, y: i32) -> bool {
        let id = self.banner.user_id;
        if let Some(user) = self.users.get_mut(&id) {
            let changed = user.x != x as i16 || user.y != y as i16;
            user.x = x as i16;
            user.y = y as i16;
            return changed;
        }
        // The server has not named us in this room yet. Drop the move only if
        // there is nowhere to draw it; otherwise insert ourselves so the click
        // is applied and rendered now, and let the server's record reconcile it
        // when it arrives.
        let room_id = self.current_room.as_ref().map(|room| room.id as i16);
        if room_id.is_none() {
            return false;
        }
        let name = self.name_of(id);
        self.users.insert(
            id,
            UserInfo {
                id,
                name,
                face: 0,
                color: 0,
                room_id: room_id.unwrap_or(0),
                x: x as i16,
                y: y as i16,
                props: Vec::new(),
                away: false,
                is_self: true,
                avatar_type: AT_PROP,
                avatar_flags: 0,
                avatar_hash: None,
            },
        );
        if !self.room_users.contains(&id) {
            self.room_users.push(id);
        }
        true
    }

    /// Reconcile a server-reported position for the local user.
    ///
    /// Prediction means the server's value normally matches what is already
    /// shown, in which case there is nothing to redraw and this returns
    /// `false`. When the server genuinely disagrees (a validation correction),
    /// snap the model to it and return `true`.
    pub fn reconcile_self_move(&mut self, x: i16, y: i16) -> bool {
        let id = self.banner.user_id;
        if let Some(user) = self.users.get_mut(&id) {
            if user.x == x && user.y == y {
                return false;
            }
            user.x = x;
            user.y = y;
            return true;
        }
        let name = self.name_of(id);
        self.users.insert(
            id,
            UserInfo {
                id,
                name,
                face: 0,
                color: 0,
                room_id: 0,
                x,
                y,
                props: Vec::new(),
                away: false,
                is_self: true,
                avatar_type: AT_PROP,
                avatar_flags: 0,
                avatar_hash: None,
            },
        );
        true
    }

    /// The users currently in the entered room, in a stable order.
    ///
    /// The signed-in user is always present, mirroring PalaceChat's
    /// `currentRoom.users`: a room change clears the room-scoped list before the
    /// destination's list arrives, and `WHONAME`/`USERNAME` must still resolve
    /// from `ON ENTER`.
    #[must_use]
    pub fn users_in_room(&self) -> Vec<UserInfo> {
        let room_id = self.current_room.as_ref().map(|r| r.id as i16);
        let mut out: Vec<UserInfo> = self
            .users
            .values()
            .filter(|u| {
                u.id == self.banner.user_id
                    || match room_id {
                        Some(id) => u.room_id == id || self.room_users.contains(&u.id),
                        None => true,
                    }
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
    use palace_wire::byteorder::{Reader, Writer};

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
                avatar_type: AT_PROP,
                avatar_flags: 0,
                avatar_hash: None,
            },
        );
    }

    fn user_rec(user_id: i32, name: &str) -> UserRec {
        UserRec {
            user_id,
            room_pos: Point::default(),
            prop_spec: [AssetSpec::default(); AssetSpec::USER_PROP_SLOTS],
            room_id: 0,
            face_nbr: 0,
            color_nbr: 0,
            away_flag: 0,
            open_to_msgs: 0,
            nbr_props: 0,
            name: name.to_owned(),
        }
    }

    fn user_at(id: i32, x: i16, y: i16) -> UserInfo {
        UserInfo {
            id,
            name: format!("user-{id}"),
            face: 0,
            color: 0,
            room_id: 901,
            x,
            y,
            props: Vec::new(),
            away: false,
            is_self: id == SELF,
            avatar_type: AT_PROP,
            avatar_flags: 0,
            avatar_hash: None,
        }
    }

    #[test]
    fn a_self_move_applies_at_once_and_an_agreeing_echo_is_not_a_redraw() {
        let mut state = room_state();
        add_user(&mut state, SELF);
        assert!(
            state.predict_self_move(123, 45),
            "the click moves the local avatar before any server reply"
        );
        assert!(
            !state.predict_self_move(123, 45),
            "moving to the same spot changes nothing"
        );
        assert!(
            !state.reconcile_self_move(123, 45),
            "the server echoing what we already show is a no-op"
        );
        assert!(
            state.reconcile_self_move(120, 40),
            "a server position that genuinely differs snaps the avatar"
        );
        let me = state.users.get(&SELF).expect("the self user is known");
        assert_eq!((me.x, me.y), (120, 40));
    }

    #[test]
    fn a_click_before_the_server_names_us_still_moves_our_avatar() {
        let mut state = room_state();
        state.current_room = Some(RoomInfo {
            id: 901,
            name: "Balamb Garden".to_string(),
            users: 0,
            flags: 0,
        });
        assert!(
            !state.users.contains_key(&SELF),
            "the server has not named us in the room yet"
        );
        assert!(
            state.predict_self_move(200, 150),
            "the click still moves the local avatar"
        );
        assert!(
            state
                .users_in_room()
                .iter()
                .any(|user| user.is_self && user.x == 200 && user.y == 150),
            "and the predicted self is in the room, so the renderer draws it"
        );
    }

    #[test]
    fn a_remote_move_is_drawn_at_the_reported_position_at_once() {
        let mut state = room_state();
        add_user(&mut state, 21);

        let mut w = Writer::new(ByteOrder::Little);
        Point::new(40, 120).encode(&mut w);
        let applied = state.apply(
            &Frame::new(opcode::USERMOVE, 21, w.into_vec()),
            ByteOrder::Little,
        );

        assert!(applied.users_moved, "a remote move must publish a roster");
        let drawn = state
            .users_in_room()
            .into_iter()
            .find(|user| user.id == 21)
            .expect("the moved user is drawn");
        assert_eq!(
            (drawn.x, drawn.y),
            (120, 40),
            "the drawn position is exactly the reported one, with no interpolation"
        );
    }

    /// The compositor draws avatars in the order this returns, so the order must
    /// be a documented total order — (y, x, id) — and not whatever order the map
    /// happened to be built in.
    #[test]
    fn users_in_room_returns_a_canonical_order_whatever_the_insertion_order() {
        let users = [
            user_at(510, 30, 40),
            user_at(507, 30, 90),
            user_at(509, 30, 40),
            user_at(502, 10, 90),
            user_at(505, 30, 40),
        ];
        let expected: Vec<i32> = vec![505, 509, 510, 502, 507];
        let orders = [
            vec![0, 1, 2, 3, 4],
            vec![4, 3, 2, 1, 0],
            vec![2, 4, 1, 0, 3],
        ];
        for order in orders {
            let mut state = SessionState::new("test", 1);
            state.room_users.clear();
            for index in order {
                state.users.insert(users[index].id, users[index].clone());
            }
            let got: Vec<i32> = state.users_in_room().iter().map(|u| u.id).collect();
            assert_eq!(
                got, expected,
                "the canonical order must not depend on insertion order"
            );
        }
        // Sorted by (y, x, id): y=40 first, then the two at y=90 with x=10
        // before x=30; equal positions ordered by id.
        let expected_order: Vec<(i16, i16, i32)> = vec![
            (40, 30, 505),
            (40, 30, 509),
            (40, 30, 510),
            (90, 10, 502),
            (90, 30, 507),
        ];
        let mut state = SessionState::new("test", 1);
        state.room_users.clear();
        for user in &users {
            state.users.insert(user.id, user.clone());
        }
        assert_eq!(
            state
                .users_in_room()
                .iter()
                .map(|u| (u.y, u.x, u.id))
                .collect::<Vec<_>>(),
            expected_order
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

    /// One raw `DRAW` record: the 10-byte header then a `PATH` operand with a
    /// red PC5 tail. `points` are `(v, h)` = `(y, x)`, first absolute and the
    /// rest deltas, exactly as the wire carries them.
    fn draw_body(command: u8, flags: u8, points: &[(i16, i16)]) -> Vec<u8> {
        let mut operand: Vec<u8> = Vec::new();
        operand.extend_from_slice(&1i16.to_le_bytes());
        operand.extend_from_slice(&(points.len().saturating_sub(1) as i16).to_le_bytes());
        operand.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        for (v, h) in points {
            operand.extend_from_slice(&v.to_le_bytes());
            operand.extend_from_slice(&h.to_le_bytes());
        }
        // PC5 tail: line RGBA then fill RGBA, each `[a, r, g, b]`.
        operand.extend_from_slice(&[255, 255, 0, 0]);
        operand.extend_from_slice(&[255, 255, 0, 0]);

        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.extend_from_slice(&(u16::from(command) | (u16::from(flags) << 8)).to_le_bytes());
        bytes.extend_from_slice(&(operand.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&10i16.to_le_bytes());
        bytes.extend_from_slice(&operand);
        bytes
    }

    fn draw_frame(command: u8, flags: u8, points: &[(i16, i16)]) -> Frame {
        Frame::new(opcode::DRAW, 0, draw_body(command, flags, points))
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
    fn an_authenticate_request_is_reported_rather_than_ignored() {
        let mut state = room_state();

        let applied = state.apply(
            &Frame::new(opcode::AUTHENTICATE, 0, Vec::new()),
            ByteOrder::Little,
        );

        assert!(
            applied
                .chat
                .iter()
                .any(|line| line.text.contains("authenticate")),
            "a server asking for authentication must be reported, because silence here is an unexplained stall: {:?}",
            applied.chat
        );
    }

    #[test]
    fn a_configured_credential_answers_an_authenticate_challenge() {
        let mut state = room_state();
        state.set_credential("Rico", Secret::new("hunter2"));

        let applied = state.apply(
            &Frame::new(opcode::AUTHENTICATE, 0, Vec::new()),
            ByteOrder::Little,
        );

        assert!(
            applied.chat.is_empty(),
            "an answerable challenge is answered, not reported: {:?}",
            applied.chat
        );
        assert_eq!(applied.outbound.len(), 1, "exactly one reply frame");
        let frame = &applied.outbound[0];
        assert_eq!(frame.opcode, opcode::AUTHRESPONSE);
        assert_eq!(frame.ref_num, 0);
        assert_eq!(
            frame.payload, b"\x0cRico:hunter2",
            "the body is the PString user:password"
        );
    }

    #[test]
    fn without_a_credential_an_authenticate_challenge_is_reported_and_not_answered() {
        let mut state = room_state();

        let applied = state.apply(
            &Frame::new(opcode::AUTHENTICATE, 0, Vec::new()),
            ByteOrder::Little,
        );

        assert!(
            applied.outbound.is_empty(),
            "with no credential there is nothing to send"
        );
        let line = applied
            .chat
            .iter()
            .find(|line| line.kind == ChatKind::Error)
            .expect("the stall must still be reported");
        assert_eq!(
            line.text,
            "server asked this client to authenticate; it cannot answer yet, so logon will not complete"
        );
    }

    #[test]
    fn a_refused_room_change_is_reported_to_the_user() {
        let mut state = room_state();

        // The user asked for room 7715 and the server refused it as unknown.
        let applied = state.apply(
            &Frame::new(opcode::NAVERROR, 1, Vec::new()),
            ByteOrder::Little,
        );

        let line = applied
            .chat
            .iter()
            .find(|line| line.text.contains("room change failed"))
            .unwrap_or_else(|| {
                panic!(
                    "a refused room change must be visible, not silent: {:?}",
                    applied.chat
                )
            });
        assert_eq!(line.kind, ChatKind::Error);
        assert!(
            line.text.contains("unknown room"),
            "the failure must be named in words, not just a number: {:?}",
            line.text
        );
        assert_eq!(state.last_error(), Some(line.text.as_str()));
    }

    #[test]
    fn a_refused_room_change_re_requests_the_room_we_never_left() {
        let mut state = room_state();
        let room_id = i32::from(state.room_desc.as_ref().expect("a room").header.room_id);
        state.current_room = Some(RoomInfo {
            id: room_id,
            name: "Balamb Garden".to_string(),
            users: 0,
            flags: 0,
        });

        state.begin_room_change();

        let applied = state.apply(
            &Frame::new(opcode::NAVERROR, 1, Vec::new()),
            ByteOrder::Little,
        );

        let frame = applied
            .outbound
            .iter()
            .find(|frame| frame.opcode == opcode::ROOMGOTO)
            .expect("a refused change must re-request the room we are still in");
        let mut reader = Reader::new(&frame.payload, ByteOrder::Little);
        assert_eq!(reader.read_u16().expect("the room id"), room_id as u16);
        assert_eq!(
            state.current_room.as_ref().map(|room| room.id),
            Some(room_id),
            "the room we never left is current again"
        );

        let desc = Frame::new(
            opcode::ROOMDESC,
            0,
            include_bytes!("../../../fixtures/rooms/86.bin").to_vec(),
        );
        let applied = state.apply(&desc, ByteOrder::Little);
        assert!(
            state.room_desc.is_some(),
            "the re-requested descriptor restores the room"
        );
        assert!(
            !applied
                .scripts
                .iter()
                .any(|stimulus| stimulus.event == ScriptEvent::Enter),
            "recovering a refused change must not re-run the room's ENTER lifecycle"
        );
    }

    #[test]
    fn navigate_frame_truncates_ids_above_65535_to_their_low_16_bits() {
        let state = room_state();
        for (requested, expected) in [(73251i32, 7715u16), (73202, 7666)] {
            let frame = state.navigate_frame(requested);
            let encoded = frame.encode(ByteOrder::Little).expect("encodes");
            assert_eq!(
                &encoded[12..14],
                expected.to_le_bytes(),
                "room {requested} must encode as {expected} (low 16 bits)"
            );
        }
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

    fn door_body(room_id: i16, door_id: i16) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(room_id);
        w.write_i16(door_id);
        w.into_vec()
    }

    fn spot_state_body(room_id: i16, spot_id: i16, value: i16) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(room_id);
        w.write_i16(spot_id);
        w.write_i16(value);
        w.into_vec()
    }

    fn hotspot_state(state: &SessionState, id: i16) -> i16 {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .hotspots
            .iter()
            .find(|hotspot| hotspot.id == id)
            .expect("the hotspot is in the room")
            .state
    }

    #[test]
    fn door_lock_and_unlock_set_the_local_hotspot_state() {
        let mut state = room_state();
        assert_eq!(
            hotspot_state(&state, 7),
            HS_UNLOCK,
            "the fixture door starts unlocked"
        );

        let applied = state.apply(
            &Frame::new(opcode::DOORLOCK, 0, door_body(86, 7)),
            ByteOrder::Little,
        );
        assert!(applied.render, "the lock recomposes the room");
        assert_eq!(hotspot_state(&state, 7), HS_LOCK);
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::Lock,
                spot: Some(7),
            }],
            "a lock runs the door's ON LOCK"
        );

        let applied = state.apply(
            &Frame::new(opcode::DOORUNLOCK, 0, door_body(86, 7)),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert_eq!(hotspot_state(&state, 7), HS_UNLOCK);
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::Unlock,
                spot: Some(7),
            }],
            "an unlock runs the door's ON UNLOCK"
        );
    }

    #[test]
    fn spot_state_sets_the_value_the_message_carries() {
        let mut state = room_state();
        let applied = state.apply(
            &Frame::new(opcode::SPOTSTATE, 0, spot_state_body(86, 105, 1)),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert_eq!(hotspot_state(&state, 105), 1);
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::StateChange,
                spot: Some(105),
            }],
            "a changed spot runs that hotspot's ON STATECHANGE"
        );

        let applied = state.apply(
            &Frame::new(opcode::SPOTSTATE, 0, spot_state_body(86, 105, 1)),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "the same value is not a state change"
        );
    }

    #[test]
    fn a_foreign_room_spot_message_records_no_script_event() {
        let mut state = room_state();
        for (opcode, body) in [
            (opcode::DOORLOCK, door_body(999, 7)),
            (opcode::DOORUNLOCK, door_body(999, 7)),
            (opcode::SPOTSTATE, spot_state_body(999, 105, 1)),
        ] {
            let applied = state.apply(&Frame::new(opcode, 0, body), ByteOrder::Little);
            assert!(
                applied.scripts.is_empty(),
                "{} for another room must run nothing here",
                opcode.describe()
            );
        }
        let applied = state.apply(
            &Frame::new(opcode::DOORLOCK, 0, door_body(86, 32000)),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "a lock for a hotspot this room lacks runs nothing"
        );
    }

    #[test]
    fn a_name_change_records_a_room_level_namechange_once() {
        let mut state = room_state();
        add_user(&mut state, 21);

        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring("Rico");
        let applied = state.apply(
            &Frame::new(opcode::USERNAME, 21, w.into_vec()),
            ByteOrder::Little,
        );
        assert_eq!(state.users[&21].name, "Rico");
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::NameChange,
                spot: None,
            }],
            "usrN runs the room's ON NAMECHANGE"
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring("Rico");
        let applied = state.apply(
            &Frame::new(opcode::USERNAME, 21, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "a usrN that does not change the name runs nothing"
        );

        let mut w = Writer::new(ByteOrder::Little);
        w.write_pstring("Ghost");
        let applied = state.apply(
            &Frame::new(opcode::USERNAME, 99, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "usrN renames a known user, it does not introduce one"
        );
    }

    #[test]
    fn only_a_non_self_exit_records_a_room_level_userleave() {
        let mut state = room_state();
        add_user(&mut state, 21);

        let applied = state.apply(
            &Frame::new(opcode::USEREXIT, 21, Vec::new()),
            ByteOrder::Little,
        );
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::UserLeave,
                spot: None,
            }],
            "another user's exit runs the room's ON USERLEAVE"
        );

        add_user(&mut state, SELF);
        let applied = state.apply(
            &Frame::new(opcode::USEREXIT, SELF, Vec::new()),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "our own exit must not run ON USERLEAVE"
        );

        let applied = state.apply(
            &Frame::new(opcode::USEREXIT, 99, Vec::new()),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "an exit for an unknown user runs nothing"
        );
    }

    #[test]
    fn a_self_exit_keeps_the_identity_username_answers_from() {
        let mut state = room_state();
        add_user(&mut state, SELF);
        let name = state.users[&SELF].name.clone();

        state.apply(
            &Frame::new(opcode::USEREXIT, SELF, Vec::new()),
            ByteOrder::Little,
        );

        assert_eq!(
            state.users.get(&SELF).map(|user| user.name.clone()),
            Some(name),
            "a room-change self exit must not blank the name USERNAME answers"
        );
        assert!(
            !state.room_users.contains(&SELF),
            "the room-scoped list still drops us"
        );
    }

    /// PalaceChat answers `WHONAME` from `currentRoom.getUserById`
    /// (`PalaceController.as:379`), and that room keeps the signed-in user even
    /// while its user list is being replaced on a room change. A room change in
    /// this client clears the room-scoped list and leaves our own record naming
    /// the old room, so `users_in_room` dropped us and `WHONAME(self)` answered
    /// an empty string during the destination's `ON ENTER`.
    #[test]
    fn the_signed_in_user_stays_in_the_room_across_a_room_change() {
        let mut state = room_state();
        add_user(&mut state, SELF);
        state.room_users.clear();
        state.current_room = Some(RoomInfo {
            id: 32009,
            name: "TEST - ipt palace".to_string(),
            users: 0,
            flags: 0,
        });

        let ids: Vec<i32> = state.users_in_room().iter().map(|user| user.id).collect();
        assert!(
            ids.contains(&SELF),
            "the signed-in user is always in the current room: {ids:?}"
        );
    }

    #[test]
    fn a_new_user_records_a_room_level_userenter() {
        let mut state = room_state();
        let mut w = Writer::new(ByteOrder::Little);
        user_rec(21, "Rico").encode(&mut w);
        let applied = state.apply(
            &Frame::new(opcode::USERNEW, 21, w.into_vec()),
            ByteOrder::Little,
        );
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::UserEnter,
                spot: None,
            }],
            "a new user runs the room's ON USERENTER"
        );
    }

    #[test]
    fn our_own_entry_records_no_userenter() {
        let mut state = room_state();
        let mut w = Writer::new(ByteOrder::Little);
        user_rec(SELF, "Me").encode(&mut w);
        let applied = state.apply(
            &Frame::new(opcode::USERNEW, SELF, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(
            applied.scripts.is_empty(),
            "our own entry must not run ON USERENTER"
        );
    }

    #[test]
    fn a_worn_prop_change_records_a_propchange() {
        let mut state = room_state();
        add_user(&mut state, 21);
        let frame = messages::UserProp {
            user_id: 21,
            props: vec![AssetSpec { id: 5, crc: 0 }],
        }
        .frame(ByteOrder::Little)
        .expect("a prop frame encodes");
        let applied = state.apply(&frame, ByteOrder::Little);
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::PropChange,
                spot: None,
            }],
            "a worn prop change runs the room's ON PROPCHANGE"
        );
    }

    #[test]
    fn a_server_down_records_a_signoff() {
        let mut state = room_state();
        let applied = state.apply(
            &Frame::new(opcode::SERVERDOWN, 0, Vec::new()),
            ByteOrder::Little,
        );
        assert_eq!(
            applied.scripts,
            vec![ScriptStimulus {
                event: ScriptEvent::SignOff,
                spot: None,
            }],
            "losing the connection runs ON SIGNOFF"
        );
    }

    #[test]
    fn a_lock_aimed_at_another_room_leaves_this_room_alone() {
        let mut state = room_state();

        let applied = state.apply(
            &Frame::new(opcode::DOORLOCK, 0, door_body(999, 7)),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "a foreign room's lock changes nothing here"
        );
        assert_eq!(hotspot_state(&state, 7), HS_UNLOCK);

        assert!(
            state
                .apply(
                    &Frame::new(opcode::DOORLOCK, 0, door_body(86, 7)),
                    ByteOrder::Little
                )
                .render
        );
        let applied = state.apply(
            &Frame::new(opcode::DOORUNLOCK, 0, door_body(999, 7)),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "a foreign room's unlock must not open our door"
        );
        assert_eq!(hotspot_state(&state, 7), HS_LOCK);

        let applied = state.apply(
            &Frame::new(opcode::SPOTSTATE, 0, spot_state_body(999, 105, 1)),
            ByteOrder::Little,
        );
        assert!(!applied.render, "a foreign room's spot state is ignored");
        assert_eq!(hotspot_state(&state, 105), HS_UNLOCK);
    }

    #[test]
    fn a_lock_for_a_hotspot_this_room_does_not_have_changes_nothing() {
        let mut state = room_state();
        let applied = state.apply(
            &Frame::new(opcode::DOORLOCK, 0, door_body(86, 32000)),
            ByteOrder::Little,
        );
        assert!(!applied.render);
    }

    fn room_spot_point_body(room_id: i16, spot_id: i16, point: Point) -> Vec<u8> {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(room_id);
        w.write_i16(spot_id);
        point.encode(&mut w);
        w.into_vec()
    }

    fn hotspot_loc(state: &SessionState, id: i16) -> Point {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .hotspots
            .iter()
            .find(|hotspot| hotspot.id == id)
            .expect("the hotspot is in the room")
            .loc
    }

    fn pic_loc(state: &SessionState, id: i16, index: usize) -> Point {
        state
            .room_desc
            .as_ref()
            .expect("a room")
            .hotspots
            .iter()
            .find(|hotspot| hotspot.id == id)
            .expect("the hotspot is in the room")
            .states[index]
            .pic_loc
    }

    #[test]
    fn spot_move_sets_the_absolute_location_in_this_room_only() {
        let mut state = room_state();
        let before = hotspot_loc(&state, 105);

        let applied = state.apply(
            &Frame::new(
                opcode::SPOTMOVE,
                0,
                room_spot_point_body(999, 105, Point::new(1, 2)),
            ),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "a foreign room's move changes nothing here"
        );
        assert_eq!(hotspot_loc(&state, 105), before);

        let applied = state.apply(
            &Frame::new(
                opcode::SPOTMOVE,
                0,
                room_spot_point_body(86, 105, Point::new(240, 120)),
            ),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert_eq!(
            hotspot_loc(&state, 105),
            Point::new(240, 120),
            "v is y, h is x, and the position is the whole new location"
        );

        let applied = state.apply(
            &Frame::new(
                opcode::SPOTMOVE,
                0,
                room_spot_point_body(86, 32000, Point::new(1, 2)),
            ),
            ByteOrder::Little,
        );
        assert!(!applied.render, "a hotspot this room lacks cannot move");
    }

    #[test]
    fn pict_move_sets_the_current_states_picture_offset_in_this_room_only() {
        let mut state = room_state();
        {
            let room = state.room_desc.as_mut().expect("a room");
            let hotspot = room
                .hotspots
                .iter_mut()
                .find(|hotspot| hotspot.id == 105)
                .expect("the fixture hotspot");
            hotspot.state = 0;
            hotspot.states = vec![palace_room::HotspotState::default()];
        }

        let applied = state.apply(
            &Frame::new(
                opcode::PICTMOVE,
                0,
                room_spot_point_body(999, 105, Point::new(1, 2)),
            ),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "a foreign room's picture move changes nothing here"
        );

        let applied = state.apply(
            &Frame::new(
                opcode::PICTMOVE,
                0,
                room_spot_point_body(86, 105, Point::new(300, 400)),
            ),
            ByteOrder::Little,
        );
        assert!(applied.render);
        assert_eq!(
            pic_loc(&state, 105, 0),
            Point::new(300, 400),
            "the hotspot's current state picture offset is set absolutely"
        );

        {
            let room = state.room_desc.as_mut().expect("a room");
            let door = room
                .hotspots
                .iter_mut()
                .find(|hotspot| hotspot.id == 7)
                .expect("the fixture door");
            door.state = 0;
            door.states.clear();
        }
        let applied = state.apply(
            &Frame::new(
                opcode::PICTMOVE,
                0,
                room_spot_point_body(86, 7, Point::new(1, 2)),
            ),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "a hotspot with no state picture has nowhere to put the offset"
        );
    }

    #[test]
    fn spot_del_removes_the_hotspot_from_this_room() {
        let mut state = room_state();
        let before = state.room_desc.as_ref().expect("a room").hotspots.len();

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(105);
        let applied = state.apply(
            &Frame::new(opcode::SPOTDEL, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(applied.render);
        let hotspots = &state.room_desc.as_ref().expect("a room").hotspots;
        assert_eq!(hotspots.len(), before - 1);
        assert!(!hotspots.iter().any(|hotspot| hotspot.id == 105));

        let mut w = Writer::new(ByteOrder::Little);
        w.write_i16(105);
        let applied = state.apply(
            &Frame::new(opcode::SPOTDEL, 0, w.into_vec()),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "deleting an absent hotspot changes nothing"
        );
    }

    fn room_frame(fixture: &str) -> Frame {
        let body = match fixture {
            "86" => include_bytes!("../../../fixtures/rooms/86.bin").to_vec(),
            "887" => include_bytes!("../../../fixtures/rooms/887.bin").to_vec(),
            other => panic!("no room fixture named {other}"),
        };
        Frame::new(opcode::ROOMDESC, 0, body)
    }

    fn lifecycle() -> Vec<ScriptStimulus> {
        vec![
            ScriptStimulus {
                event: ScriptEvent::RoomLoad,
                spot: None,
            },
            ScriptStimulus {
                event: ScriptEvent::Enter,
                spot: None,
            },
            ScriptStimulus {
                event: ScriptEvent::RoomReady,
                spot: None,
            },
        ]
    }

    #[test]
    fn a_room_arrival_records_roomload_enter_and_roomready_in_order() {
        let mut state = SessionState::new("test", 1);
        let applied = state.apply(&room_frame("86"), ByteOrder::Little);
        assert!(
            applied.room_entered,
            "the arrival is reported to the runtime"
        );
        assert_eq!(
            applied.scripts,
            lifecycle(),
            "the reference order is ROOMLOAD, ENTER, ROOMREADY"
        );
    }

    #[test]
    fn redescribing_the_same_room_records_no_lifecycle() {
        let mut state = SessionState::new("test", 1);
        state.apply(&room_frame("86"), ByteOrder::Little);
        let again = state.apply(&room_frame("86"), ByteOrder::Little);
        assert!(again.room_entered, "the description still arrives");
        assert!(
            again.scripts.is_empty(),
            "a repeated description must not re-run the lifecycle"
        );
    }

    #[test]
    fn arriving_in_a_second_room_records_a_fresh_lifecycle() {
        let mut state = SessionState::new("test", 1);
        assert_eq!(
            state.apply(&room_frame("86"), ByteOrder::Little).scripts,
            lifecycle()
        );
        state.begin_room_change();
        assert_eq!(
            state.apply(&room_frame("887"), ByteOrder::Little).scripts,
            lifecycle(),
            "a different room is a new arrival"
        );
    }

    #[test]
    fn re_entering_the_same_room_after_leaving_records_a_fresh_lifecycle() {
        let mut state = SessionState::new("test", 1);
        assert_eq!(
            state.apply(&room_frame("86"), ByteOrder::Little).scripts,
            lifecycle()
        );
        state.begin_room_change();
        assert_eq!(
            state.apply(&room_frame("86"), ByteOrder::Little).scripts,
            lifecycle(),
            "leaving clears the room, so coming back is an arrival"
        );
    }

    #[test]
    fn spot_new_carries_no_geometry_and_leaves_the_room_alone() {
        let mut state = room_state();
        let before = state.room_desc.as_ref().expect("a room").hotspots.len();

        let applied = state.apply(
            &Frame::new(opcode::SPOTNEW, 0, Vec::new()),
            ByteOrder::Little,
        );
        assert!(
            !applied.render,
            "an empty body describes nothing to add to the room"
        );
        assert_eq!(
            state.room_desc.as_ref().expect("a room").hotspots.len(),
            before
        );
        assert!(
            applied.chat.iter().any(|line| line.text.contains("stale")),
            "the stale room is reported instead of silently ignored"
        );
    }

    #[test]
    fn a_truncated_draw_record_is_ignored_without_desyncing_the_session() {
        let mut state = room_state();

        let empty = state.apply(&Frame::new(opcode::DRAW, 0, Vec::new()), ByteOrder::Little);
        assert!(!empty.render, "a body with no command has nothing to paint");
        assert!(
            state.draw.is_empty(),
            "an empty record must not leave a phantom command behind"
        );

        state.apply(
            &Frame::new(opcode::DRAW, 0, vec![0u8; 4]),
            ByteOrder::Little,
        );
        assert!(
            state.draw.is_empty(),
            "half a header names no command either"
        );

        state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert_eq!(state.draw.back().len(), 1, "a formed record still lands");
        state.apply(
            &draw_frame(palace_room::draw_cmd::DELETE, 0, &[]),
            ByteOrder::Little,
        );
        assert!(
            state.draw.is_empty(),
            "DELETE must pop the real stroke, not a phantom from a bad record"
        );
    }

    #[test]
    fn a_draw_message_appends_to_its_layer_and_delete_pops_that_layer() {
        let mut state = room_state();

        let back = state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert!(back.render, "a received DRAW asks for a recompose");
        assert_eq!(state.draw.back().len(), 1);
        assert!(state.draw.front().is_empty());

        state.apply(
            &draw_frame(
                palace_room::draw_cmd::PATH,
                palace_room::draw_flags::LAYER_FRONT,
                &[(40, 5), (0, 10)],
            ),
            ByteOrder::Little,
        );
        assert_eq!(state.draw.front().len(), 1, "the front flag selects front");
        assert_eq!(state.draw.back().len(), 1, "and leaves the back list alone");

        state.apply(
            &draw_frame(palace_room::draw_cmd::DELETE, 0, &[]),
            ByteOrder::Little,
        );
        assert!(
            state.draw.front().is_empty(),
            "DELETE pops the most recent command, which was on the front layer"
        );
        assert_eq!(state.draw.back().len(), 1, "the back layer is untouched");

        state.apply(
            &draw_frame(palace_room::draw_cmd::DETONATE, 0, &[]),
            ByteOrder::Little,
        );
        assert!(state.draw.is_empty(), "DETONATE clears both layers");
        assert!(state.draw.undo().is_none(), "and the history with them");
    }

    #[test]
    fn a_stroke_does_not_survive_arrival_in_another_room() {
        let mut state = room_state();
        state.apply(&room_frame("887"), ByteOrder::Little);
        assert_eq!(
            state.current_room.as_ref().map(|room| room.id),
            Some(887),
            "a room description is an arrival"
        );

        state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert_eq!(state.draw.len(), 1, "the stroke is on the current room");

        let applied = state.apply(&room_frame("86"), ByteOrder::Little);
        assert!(applied.room_entered, "the description is for another room");
        assert!(applied.render, "the new room recomposes");
        assert!(
            state.draw.is_empty(),
            "the previous room's stroke must not carry over: {:?}",
            state.draw
        );
    }

    #[test]
    fn leaving_clears_the_draw_list() {
        let mut state = room_state();
        state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert!(!state.draw.is_empty());

        state.begin_room_change();

        assert!(
            state.draw.is_empty(),
            "leaving drops the room's paint with the room itself"
        );
    }

    #[test]
    fn load_room_draw_seeds_the_list_from_the_rooms_own_commands() {
        let mut state = room_state();
        state.apply(
            &draw_frame(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
            ByteOrder::Little,
        );
        assert_eq!(state.draw.len(), 1);

        let mut room = state.room_desc.clone().expect("a room");
        room.draw_cmds = vec![
            palace_room::decode_draw_record(
                &draw_body(palace_room::draw_cmd::PATH, 0, &[(5, 5), (0, 10)]),
                ByteOrder::Little,
            )
            .0,
            palace_room::decode_draw_record(
                &draw_body(
                    palace_room::draw_cmd::PATH,
                    palace_room::draw_flags::LAYER_FRONT,
                    &[(40, 5), (0, 10)],
                ),
                ByteOrder::Little,
            )
            .0,
        ];
        state.load_room_draw(&room);

        assert_eq!(
            state.draw.back().len(),
            1,
            "the room's own back command seeds the back list"
        );
        assert_eq!(
            state.draw.front().len(),
            1,
            "and its front command the front list"
        );
    }
}

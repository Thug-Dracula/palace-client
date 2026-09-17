//! Turning effects into protocol frames.
//!
//! The byte layouts are ported from OpenPalace's `PalaceClient.as` senders
//! (`move`, `setSpotState`, `lockDoor`, `unlockDoor`, `addLooseProp`,
//! `moveLooseProp`, `removeLooseProp`, `updateUserProps`, `setFace`,
//! `setColor`, `sendDrawPacket`) and from the 1999 protocol reference. Every
//! encoder is pinned by a byte-exact test in `tests/wire.rs`.

use palace_wire::byteorder::{ByteOrder, Writer};
use palace_wire::frame::{navr_frame, Frame};
use palace_wire::messages::{Point, Talk, Whisper};
use palace_wire::opcode;

use crate::effect::Effect;
use crate::host::PenState;

/// The session facts an effect needs to become a frame.
#[derive(Debug, Clone, Copy)]
pub struct WireContext {
    /// Session byte order.
    pub byte_order: ByteOrder,
    /// Our user id, used as the `refNum` of user-scoped messages.
    pub user_id: i32,
    /// The room we are in.
    pub room_id: i32,
    /// Room width, for the movement clamp.
    pub room_width: i32,
    /// Room height, for the movement clamp.
    pub room_height: i32,
    /// Our current position, for relative moves.
    pub self_pos: (i32, i32),
    /// Pen state, for relative strokes.
    pub pen: PenState,
}

/// Encode one effect as a frame, or `None` when it has no wire form.
///
/// Local effects (`SOUND`, `GOTOURL`, `STATUSMSG`, …) deliberately return
/// `None`: the runtime reports them to the user instead.
#[must_use]
pub fn effect_frame(effect: &Effect, ctx: &WireContext) -> Option<Frame> {
    let order = ctx.byte_order;
    match effect {
        Effect::Say { text } | Effect::RoomMessage { text } | Effect::GlobalMessage { text } => {
            let mut w = Writer::new(order);
            Talk {
                user_id: ctx.user_id,
                text: text.clone(),
            }
            .encode(&mut w);
            Some(Frame::new(opcode::TALK, ctx.user_id, w.into_vec()))
        }
        Effect::SayAt { text, x, y } => {
            let mut w = Writer::new(order);
            Talk {
                user_id: ctx.user_id,
                text: format!("@{x},{y} {text}"),
            }
            .encode(&mut w);
            Some(Frame::new(opcode::TALK, ctx.user_id, w.into_vec()))
        }
        Effect::PrivateMessage { user, text } => {
            let mut w = Writer::new(order);
            Whisper {
                user_id: ctx.user_id,
                target_id: *user,
                text: text.clone(),
            }
            .encode(&mut w);
            Some(Frame::new(opcode::WHISPER, ctx.user_id, w.into_vec()))
        }
        Effect::GotoRoom { room } => {
            // RoomID is 16-bit on the wire (`typedef sint16 RoomID`), so an id
            // above 65535 narrows to its low 16 bits. Truncate, never clamp:
            // clamping would send a different, non-existent room (73251 would
            // become 65535) instead of the room the script means (7715).
            let room = *room as u16;
            Some(navr_frame(room, ctx.user_id, order))
        }
        Effect::MoveUserAbs { .. } | Effect::MoveUserRel { .. } => {
            let (x, y) = move_target(effect, ctx)?;
            let mut w = Writer::new(order);
            Point::new(y as i16, x as i16).encode(&mut w);
            Some(Frame::new(opcode::USERMOVE, ctx.user_id, w.into_vec()))
        }
        Effect::SetColor { color } => {
            let mut w = Writer::new(order);
            w.write_i16((*color).clamp(0, 15) as i16);
            Some(Frame::new(opcode::USERCOLOR, ctx.user_id, w.into_vec()))
        }
        Effect::SetFace { face } => {
            let mut w = Writer::new(order);
            w.write_i16((*face).clamp(0, 15) as i16);
            Some(Frame::new(opcode::USERFACE, ctx.user_id, w.into_vec()))
        }
        Effect::SetUserName { name } => Some(palace_wire::frame::user_name_frame(
            name,
            ctx.user_id,
            order,
        )),
        Effect::SetProps { props } => {
            let mut w = Writer::new(order);
            let worn: Vec<i64> = props.iter().copied().filter(|p| *p != 0).take(9).collect();
            w.write_i32(worn.len() as i32);
            for prop in worn {
                w.write_i32(prop as i32);
                w.write_u32(0);
            }
            Some(Frame::new(opcode::USERPROP, ctx.user_id, w.into_vec()))
        }
        Effect::SetSpotState { spot, state } => {
            let mut w = Writer::new(order);
            w.write_i16(ctx.room_id as i16);
            w.write_i16(*spot as i16);
            w.write_i16(*state as i16);
            Some(Frame::new(opcode::SPOTSTATE, 0, w.into_vec()))
        }
        Effect::Lock { spot } => {
            let mut w = Writer::new(order);
            w.write_i16(ctx.room_id as i16);
            w.write_i16(*spot as i16);
            Some(Frame::new(opcode::DOORLOCK, 0, w.into_vec()))
        }
        Effect::Unlock { spot } => {
            let mut w = Writer::new(order);
            w.write_i16(ctx.room_id as i16);
            w.write_i16(*spot as i16);
            Some(Frame::new(opcode::DOORUNLOCK, 0, w.into_vec()))
        }
        Effect::AddLooseProp { prop, x, y } => {
            let mut w = Writer::new(order);
            w.write_i32(*prop as i32);
            w.write_u32(0);
            w.write_i16(*y as i16);
            w.write_i16(*x as i16);
            Some(Frame::new(opcode::PROPNEW, 0, w.into_vec()))
        }
        Effect::RemoveLooseProp { index } => {
            let mut w = Writer::new(order);
            w.write_i32(*index);
            Some(Frame::new(opcode::PROPDEL, 0, w.into_vec()))
        }
        Effect::MoveLooseProp { index, x, y } => {
            let mut w = Writer::new(order);
            w.write_i32(*index);
            w.write_i16(*y as i16);
            w.write_i16(*x as i16);
            Some(Frame::new(opcode::PROPMOVE, 0, w.into_vec()))
        }
        Effect::DrawLine { x1, y1, x2, y2 } => {
            let body = draw_path(ctx, &[(*x1, *y1), (*x2, *y2)], ctx.pen.front);
            Some(Frame::new(opcode::DRAW, 0, body))
        }
        Effect::DrawLineRel { x1, y1, x2, y2 } => {
            let body = draw_path(ctx, &[(*x1, *y1), (*x2, *y2)], ctx.pen.front);
            Some(Frame::new(opcode::DRAW, 0, body))
        }
        Effect::PaintClear => Some(Frame::new(opcode::DRAW, 0, draw_detonate(order))),
        _ => None,
    }
}

/// The room position a scripted user move comes to rest at.
///
/// `SETPOS` names the position outright; `MOVE` nudges from
/// [`WireContext::self_pos`]. Both are clamped by the same rule
/// [`effect_frame`] encodes, so the frame a script sends and the position the
/// client applies locally are the same by construction — there is one
/// computation, not two that can drift. `None` for every effect that is not a
/// user move.
///
/// The clamp here is [`clamp_position`], which is *not* identical to
/// `palace_render::clamp_avatar_position` for rooms at or below 44 px: this one
/// lets the coordinate reach the room edge (and pins to the room size when it is
/// under 22 px), while the renderer pins to the 22 px avatar margin. They agree
/// for every room larger than 44 px, which is every real room; the divergence is
/// left in place because the renderer's clamp is tied to how avatars are
/// anchored.
#[must_use]
pub fn move_target(effect: &Effect, ctx: &WireContext) -> Option<(i32, i32)> {
    match effect {
        Effect::MoveUserAbs { x, y } => Some(clamp_position(*x, *y, ctx)),
        Effect::MoveUserRel { dx, dy } => Some(clamp_position(
            ctx.self_pos.0 + *dx,
            ctx.self_pos.1 + *dy,
            ctx,
        )),
        _ => None,
    }
}

fn clamp_position(x: i32, y: i32, ctx: &WireContext) -> (i32, i32) {
    let max_x = if ctx.room_width > 44 {
        ctx.room_width - 22
    } else {
        ctx.room_width
    };
    let max_y = if ctx.room_height > 44 {
        ctx.room_height - 22
    } else {
        ctx.room_height
    };
    (x.clamp(22.min(max_x), max_x), y.clamp(22.min(max_y), max_y))
}

/// One `DC_Path` record: a polyline with `points.len() - 1` segments.
fn draw_path(ctx: &WireContext, points: &[(i32, i32)], front: bool) -> Vec<u8> {
    let mut data = Writer::new(ctx.byte_order);
    data.write_i16(ctx.pen.size.clamp(0, i16::MAX as i32) as i16);
    data.write_i16((points.len() as i32 - 1).max(0) as i16);
    let (r, g, b) = ctx.pen.rgb;
    for channel in [r, g, b] {
        data.write_u8(channel.clamp(0, 255) as u8);
        data.write_u8(channel.clamp(0, 255) as u8);
    }
    for (x, y) in points {
        data.write_i16(*y as i16);
        data.write_i16(*x as i16);
    }
    draw_header(ctx.byte_order, 0, front, data.into_vec())
}

/// A `DC_Detonate` record: delete every draw command.
fn draw_detonate(order: ByteOrder) -> Vec<u8> {
    draw_header(order, 3, false, Vec::new())
}

fn draw_header(order: ByteOrder, command: u8, front: bool, data: Vec<u8>) -> Vec<u8> {
    let mut w = Writer::new(order);
    w.write_i16(0);
    w.write_i16(0);
    let flags: u8 = if front { 0x80 } else { 0x00 };
    w.write_u16(u16::from(command) | (u16::from(flags) << 8));
    w.write_u16(data.len() as u16);
    w.write_i16(0);
    w.write_bytes(&data);
    w.into_vec()
}

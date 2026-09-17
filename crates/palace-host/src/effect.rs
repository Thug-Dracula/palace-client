//! What a script asked the client to do.
//!
//! A [`ScriptHost`](crate::ScriptHost) never touches the socket, the UI or the
//! clock. Every Palace command an IPTSCRAE script invokes is recorded here as an
//! [`Effect`]; the runtime then decides how to honour it — protocol frame, local
//! UI change, or a refusal. That split is what makes dispatch testable without a
//! server: the same fixtures produce the same effect list.
//!
//! The naming follows the IPTSCRAE command that produced the effect, so a
//! transcript can be read against the language guide.

use std::fmt;

/// One requested effect.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `SAY` / `CHAT` — speak in the room.
    Say { text: String },
    /// `SAYAT` — speak anchored at a screen position.
    SayAt { text: String, x: i32, y: i32 },
    /// `PRIVATEMSG` — whisper to one user.
    PrivateMessage { user: i32, text: String },
    /// `ROOMMSG` — room-wide system message.
    RoomMessage { text: String },
    /// `GLOBALMSG` — server-wide system message.
    GlobalMessage { text: String },
    /// `LOCALMSG` — a line only you see.
    LocalMessage { text: String },
    /// `SUSRMSG` — page the owner/operator.
    SuperUserMessage { text: String },
    /// `STATUSMSG` — text in the status bar.
    StatusMessage { text: String },
    /// `LOGMSG` — text in the log window.
    LogMessage { text: String },
    /// `ERRORMSG` — an error reported to the user.
    ErrorMessage { text: String },
    /// `GOTOROOM` / `NETGOTO`-style room change.
    GotoRoom { room: i32 },
    /// `GOTOURL` / `NETGOTO` — open a URL.
    GotoUrl { url: String },
    /// `LAUNCHAPP` — start a Palace plugin.
    LaunchApp { app: String },
    /// `SETPOS` — absolute teleport.
    MoveUserAbs { x: i32, y: i32 },
    /// `MOVE` — relative nudge.
    MoveUserRel { dx: i32, dy: i32 },
    /// `SETCOLOR` — roundhead colour index.
    SetColor { color: i32 },
    /// `SETFACE` — roundhead expression index.
    SetFace { face: i32 },
    /// `SETUSERNAME` — change your screen name.
    SetUserName { name: String },
    /// `SETPROPS` — replace the worn prop list.
    SetProps { props: Vec<i64> },
    /// `DONPROP` — wear a prop by id.
    DonProp { prop: i64 },
    /// `DOFFPROP` — remove the most recently worn prop.
    DoffProp,
    /// `REMOVEPROP` — remove a specific prop.
    RemoveProp { prop: i64 },
    /// `NAKED` / `CLEARPROPS` — wear nothing.
    Naked,
    /// `SETSPOTSTATE` — set a spot's state for everyone.
    SetSpotState { spot: i32, state: i32 },
    /// `SETSPOTSTATELOCAL` — set a spot's state for you alone.
    SetSpotStateLocal { spot: i32, state: i32 },
    /// `SETLOC` — move a spot for everyone.
    MoveSpot { spot: i32, dx: i32, dy: i32 },
    /// `SETLOCLOCAL` — move a spot for you alone.
    MoveSpotLocal { spot: i32, dx: i32, dy: i32 },
    /// `SETPICLOC` — move a spot's picture for everyone.
    SetPicOffset { spot: i32, dx: i32, dy: i32 },
    /// `SETPICLOCLOCAL` — move a spot's picture for you alone.
    SetPicOffsetLocal {
        spot: i32,
        state: i32,
        dx: i32,
        dy: i32,
    },
    /// `SETPICOPACITY` — fade a spot's picture.
    SetPicOpacity { spot: i32, state: i32, opacity: f64 },
    /// `LOCK` — lock a door.
    Lock { spot: i32 },
    /// `UNLOCK` — unlock a door.
    Unlock { spot: i32 },
    /// `SELECT` — fire another spot's `ON SELECT`.
    SelectSpot { spot: i32 },
    /// `ADDLOOSEPROP` — place a prop on the floor.
    AddLooseProp { prop: i64, x: i32, y: i32 },
    /// `REMOVELOOSEPROP` — remove a loose prop by index.
    RemoveLooseProp { index: i32 },
    /// `MOVELOOSEPROP` — move a loose prop by index.
    MoveLooseProp { index: i32, x: i32, y: i32 },
    /// `CLEARLOOSEPROPS` — remove every loose prop.
    ClearLooseProps,
    /// `DROPPROP` — drop your last prop at a position.
    DropProp { x: i32, y: i32 },
    /// `DIMROOM` — dim the room.
    DimRoom { percent: i32 },
    /// `SOUND` — play a `.wav`.
    PlaySound { name: String },
    /// `MIDIPLAY` — play a `.mid` once.
    MidiPlay { name: String },
    /// `MIDILOOP` — loop a `.mid`.
    MidiLoop { name: String, loops: i32 },
    /// `MIDISTOP` — stop MIDI playback.
    MidiStop,
    /// `BEEP` — a system beep.
    Beep,
    /// `LINE` — a painted segment in absolute room coordinates.
    DrawLine { x1: i32, y1: i32, x2: i32, y2: i32 },
    /// `LINETO` — a painted segment, resolved to absolute room coordinates by
    /// the host (which owns the pen).
    DrawLineRel { x1: i32, y1: i32, x2: i32, y2: i32 },
    /// `PENPOS` / `PENTO` — move the pen without painting.
    MovePen { x: i32, y: i32 },
    /// `PENCOLOR` — set the pen colour.
    SetPenColor { r: i32, g: i32, b: i32 },
    /// `PENSIZE` — set the pen width.
    SetPenSize { size: i32 },
    /// `PENFRONT` / `PENBACK` — choose the layer new strokes go to.
    PaintLayer { front: bool },
    /// `PAINTCLEAR` — erase every painted stroke.
    PaintClear,
    /// `PAINTUNDO` — erase the most recent stroke.
    PaintUndo,
    /// `HIDEAVATARS`.
    HideAvatars,
    /// `SHOWAVATARS`.
    ShowAvatars,
    /// `MACRO` — run one of your avatar macros.
    Macro { index: i32 },
    /// `SETALARM` — fire a spot's `ON ALARM` later.
    SetSpotAlarm { spot: i32, ticks: i32 },
    /// `CHATSTR` was rewritten by an `ON INCHAT`/`ON OUTCHAT` handler.
    SetChatString { text: String },
    /// `LOADSCRIPT` / `HTTPGET` — fetch a URL on the media base.
    ///
    /// Sparky scopes the response event to the executing hotspot, so the spot
    /// rides along from the handler that asked.
    FetchScript {
        /// The URL as the script spelled it (absolute, or relative to the media base).
        url: String,
        /// The executing hotspot; `0` for room-level scripts.
        spot: i32,
    },
    /// A command this host does not implement. Recorded so a run can report it
    /// rather than silently dropping it.
    Unsupported { command: String },
}

impl Effect {
    /// The IPTSCRAE command this effect came from.
    #[must_use]
    pub fn command(&self) -> &str {
        match self {
            Effect::Say { .. } => "SAY",
            Effect::SayAt { .. } => "SAYAT",
            Effect::PrivateMessage { .. } => "PRIVATEMSG",
            Effect::RoomMessage { .. } => "ROOMMSG",
            Effect::GlobalMessage { .. } => "GLOBALMSG",
            Effect::LocalMessage { .. } => "LOCALMSG",
            Effect::SuperUserMessage { .. } => "SUSRMSG",
            Effect::StatusMessage { .. } => "STATUSMSG",
            Effect::LogMessage { .. } => "LOGMSG",
            Effect::ErrorMessage { .. } => "ERRORMSG",
            Effect::GotoRoom { .. } => "GOTOROOM",
            Effect::GotoUrl { .. } => "GOTOURL",
            Effect::FetchScript { url: _, .. } => "LOADSCRIPT",
            Effect::LaunchApp { .. } => "LAUNCHAPP",
            Effect::MoveUserAbs { .. } => "SETPOS",
            Effect::MoveUserRel { .. } => "MOVE",
            Effect::SetColor { .. } => "SETCOLOR",
            Effect::SetFace { .. } => "SETFACE",
            Effect::SetUserName { .. } => "SETUSERNAME",
            Effect::SetProps { .. } => "SETPROPS",
            Effect::DonProp { .. } => "DONPROP",
            Effect::DoffProp => "DOFFPROP",
            Effect::RemoveProp { .. } => "REMOVEPROP",
            Effect::Naked => "NAKED",
            Effect::SetSpotState { .. } => "SETSPOTSTATE",
            Effect::SetSpotStateLocal { .. } => "SETSPOTSTATELOCAL",
            Effect::MoveSpot { .. } => "SETLOC",
            Effect::MoveSpotLocal { .. } => "SETLOCLOCAL",
            Effect::SetPicOffset { .. } => "SETPICLOC",
            Effect::SetPicOffsetLocal { .. } => "SETPICLOCLOCAL",
            Effect::SetPicOpacity { .. } => "SETPICOPACITY",
            Effect::Lock { .. } => "LOCK",
            Effect::Unlock { .. } => "UNLOCK",
            Effect::SelectSpot { .. } => "SELECT",
            Effect::AddLooseProp { .. } => "ADDLOOSEPROP",
            Effect::RemoveLooseProp { .. } => "REMOVELOOSEPROP",
            Effect::MoveLooseProp { .. } => "MOVELOOSEPROP",
            Effect::ClearLooseProps => "CLEARLOOSEPROPS",
            Effect::DropProp { .. } => "DROPPROP",
            Effect::DimRoom { .. } => "DIMROOM",
            Effect::PlaySound { .. } => "SOUND",
            Effect::MidiPlay { .. } => "MIDIPLAY",
            Effect::MidiLoop { .. } => "MIDILOOP",
            Effect::MidiStop => "MIDISTOP",
            Effect::Beep => "BEEP",
            Effect::DrawLine { .. } => "LINE",
            Effect::DrawLineRel { .. } => "LINETO",
            Effect::MovePen { .. } => "PENPOS",
            Effect::SetPenColor { .. } => "PENCOLOR",
            Effect::SetPenSize { .. } => "PENSIZE",
            Effect::PaintLayer { .. } => "PENFRONT",
            Effect::PaintClear => "PAINTCLEAR",
            Effect::PaintUndo => "PAINTUNDO",
            Effect::HideAvatars => "HIDEAVATARS",
            Effect::ShowAvatars => "SHOWAVATARS",
            Effect::Macro { .. } => "MACRO",
            Effect::SetSpotAlarm { .. } => "SETALARM",
            Effect::SetChatString { .. } => "CHATSTR",
            Effect::Unsupported { command } => command,
        }
    }

    /// Whether the runtime can put this effect on the wire.
    #[must_use]
    pub fn is_wire_effect(&self) -> bool {
        matches!(
            self,
            Effect::Say { .. }
                | Effect::SayAt { .. }
                | Effect::PrivateMessage { .. }
                | Effect::GotoRoom { .. }
                | Effect::MoveUserAbs { .. }
                | Effect::MoveUserRel { .. }
                | Effect::SetColor { .. }
                | Effect::SetFace { .. }
                | Effect::SetUserName { .. }
                | Effect::SetProps { .. }
                | Effect::SetSpotState { .. }
                | Effect::Lock { .. }
                | Effect::Unlock { .. }
                | Effect::AddLooseProp { .. }
                | Effect::RemoveLooseProp { .. }
                | Effect::MoveLooseProp { .. }
                | Effect::ClearLooseProps
                | Effect::DrawLine { .. }
                | Effect::DrawLineRel { .. }
                | Effect::PaintClear
        )
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Say { text }
            | Effect::RoomMessage { text }
            | Effect::GlobalMessage { text }
            | Effect::LocalMessage { text }
            | Effect::SuperUserMessage { text }
            | Effect::StatusMessage { text }
            | Effect::LogMessage { text }
            | Effect::ErrorMessage { text } => write!(f, "{} {:?}", self.command(), text),
            Effect::SayAt { text, x, y } => {
                write!(f, "SAYAT ({x},{y}) {text:?}")
            }
            Effect::PrivateMessage { user, text } => {
                write!(f, "PRIVATEMSG -> {user} {text:?}")
            }
            Effect::GotoRoom { room } => write!(f, "GOTOROOM {room}"),
            Effect::GotoUrl { url } => write!(f, "GOTOURL {url:?}"),
            Effect::FetchScript { url, spot } => write!(f, "FETCHSCRIPT spot={spot} {url:?}"),
            Effect::LaunchApp { app } => write!(f, "LAUNCHAPP {app:?}"),
            Effect::MoveUserAbs { x, y } => write!(f, "SETPOS ({x},{y})"),
            Effect::MoveUserRel { dx, dy } => write!(f, "MOVE ({dx},{dy})"),
            Effect::SetColor { color } => write!(f, "SETCOLOR {color}"),
            Effect::SetFace { face } => write!(f, "SETFACE {face}"),
            Effect::SetUserName { name } => write!(f, "SETUSERNAME {name:?}"),
            Effect::SetProps { props } => write!(f, "SETPROPS {props:?}"),
            Effect::DonProp { prop } => write!(f, "DONPROP {prop}"),
            Effect::DoffProp => write!(f, "DOFFPROP"),
            Effect::RemoveProp { prop } => write!(f, "REMOVEPROP {prop}"),
            Effect::Naked => write!(f, "NAKED"),
            Effect::SetSpotState { spot, state } => {
                write!(f, "SETSPOTSTATE spot={spot} state={state}")
            }
            Effect::SetSpotStateLocal { spot, state } => {
                write!(f, "SETSPOTSTATELOCAL spot={spot} state={state}")
            }
            Effect::MoveSpot { spot, dx, dy } => {
                write!(f, "SETLOC spot={spot} d=({dx},{dy})")
            }
            Effect::MoveSpotLocal { spot, dx, dy } => {
                write!(f, "SETLOCLOCAL spot={spot} d=({dx},{dy})")
            }
            Effect::SetPicOffset { spot, dx, dy } => {
                write!(f, "SETPICLOC spot={spot} d=({dx},{dy})")
            }
            Effect::SetPicOffsetLocal {
                spot,
                state,
                dx,
                dy,
            } => write!(f, "SETPICLOCLOCAL spot={spot} state={state} d=({dx},{dy})"),
            Effect::SetPicOpacity {
                spot,
                state,
                opacity,
            } => write!(f, "SETPICOPACITY spot={spot} state={state} {opacity:.2}"),
            Effect::Lock { spot } => write!(f, "LOCK {spot}"),
            Effect::Unlock { spot } => write!(f, "UNLOCK {spot}"),
            Effect::SelectSpot { spot } => write!(f, "SELECT {spot}"),
            Effect::AddLooseProp { prop, x, y } => {
                write!(f, "ADDLOOSEPROP {prop} at ({x},{y})")
            }
            Effect::RemoveLooseProp { index } => write!(f, "REMOVELOOSEPROP {index}"),
            Effect::MoveLooseProp { index, x, y } => {
                write!(f, "MOVELOOSEPROP {index} to ({x},{y})")
            }
            Effect::ClearLooseProps => write!(f, "CLEARLOOSEPROPS"),
            Effect::DropProp { x, y } => write!(f, "DROPPROP ({x},{y})"),
            Effect::DimRoom { percent } => write!(f, "DIMROOM {percent}"),
            Effect::PlaySound { name } => write!(f, "SOUND {name:?}"),
            Effect::MidiPlay { name } => write!(f, "MIDIPLAY {name:?}"),
            Effect::MidiLoop { name, loops } => write!(f, "MIDILOOP {name:?} x{loops}"),
            Effect::MidiStop => write!(f, "MIDISTOP"),
            Effect::Beep => write!(f, "BEEP"),
            Effect::DrawLine { x1, y1, x2, y2 } => {
                write!(f, "LINE ({x1},{y1})-({x2},{y2})")
            }
            Effect::DrawLineRel { x1, y1, x2, y2 } => {
                write!(f, "LINETO ({x1},{y1})-({x2},{y2})")
            }
            Effect::MovePen { x, y } => write!(f, "PENPOS ({x},{y})"),
            Effect::SetPenColor { r, g, b } => write!(f, "PENCOLOR ({r},{g},{b})"),
            Effect::SetPenSize { size } => write!(f, "PENSIZE {size}"),
            Effect::PaintLayer { front } if *front => write!(f, "PENFRONT"),
            Effect::PaintLayer { .. } => write!(f, "PENBACK"),
            Effect::PaintClear => write!(f, "PAINTCLEAR"),
            Effect::PaintUndo => write!(f, "PAINTUNDO"),
            Effect::HideAvatars => write!(f, "HIDEAVATARS"),
            Effect::ShowAvatars => write!(f, "SHOWAVATARS"),
            Effect::Macro { index } => write!(f, "MACRO {index}"),
            Effect::SetSpotAlarm { spot, ticks } => {
                write!(f, "SETALARM spot={spot} ticks={ticks}")
            }
            Effect::SetChatString { text } => write!(f, "CHATSTR={text:?}"),
            Effect::Unsupported { command } => write!(f, "unsupported {command}"),
        }
    }
}

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
    /// `GOTOURLFRAME` — open a URL in a named browser frame.
    GotoUrlFrame { url: String, frame: String },
    /// `LAUNCHAPP` — start a Palace plugin.
    LaunchApp { app: String },
    /// `LAUNCHEVENT` — fire a named Palace event.
    LaunchEvent { event: String },
    /// `LAUNCHPPA` — start a Palace plugin action.
    LaunchPpa { ppa: String },
    /// `LOADJAVA` — request a Java module.
    LoadJava { url: String },
    /// `TALKPPA` — send text to a Palace plugin.
    TalkPpa { text: String },
    /// `SHELLCMD` — a shell command a script asked to run. Recorded so the
    /// request is visible; the runtime refuses it and never executes it.
    ShellCommand { command: String },
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
    /// `LOADPROPS` — preload prop assets so a later `DONPROP` does not stall.
    LoadProps { props: Vec<i64> },
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
    /// `SETSPOTNAMELOCAL` — rename a spot for you alone.
    SetSpotNameLocal { spot: i32, name: String },
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
    /// `SETPICBRIGHTNESS` — set a spot state's brightness filter.
    SetPicBrightness { spot: i32, state: i32, value: i32 },
    /// `SETPICSATURATION` — set a spot state's saturation filter.
    SetPicSaturation { spot: i32, state: i32, value: i32 },
    /// `ADDSPOT` — append a polygon hotspot to the room and answer its new id.
    ///
    /// Local-only: the reference mutates its own hotspot store in place and
    /// there is no documented client→server body, so [`Effect::is_wire_effect`]
    /// omits it. The runtime appends it, so a later hit-test can select it.
    AddSpot {
        /// The id the host allocated for it and returned to the script.
        id: i32,
        /// Polygon outline as `(x, y)` room coordinates, in wire order.
        points: Vec<(i32, i32)>,
        /// The hotspot's nominal location `(x, y)`.
        x: i32,
        y: i32,
    },
    /// `ADDPIC` — attach a picture file to a hotspot as one more state.
    ///
    /// Local-only, like [`Effect::AddSpot`]: the reference mutates its own
    /// picture and state lists and no wire body exists for it.
    AddPic {
        /// Target hotspot id.
        spot: i32,
        /// Picture file name.
        name: String,
    },
    /// `SETSPOTOPTIONS` — patch a hotspot's type and flags.
    ///
    /// Local-only, like [`Effect::AddSpot`]. `flags` is the raw flag word the
    /// script passed and `top_layer` is the separate `topLayer` operand; the
    /// runtime merges the latter into the pictures-above-all bit, because the
    /// wire model has only `flags` where the reference keeps both fields.
    SetSpotOptions {
        /// Target hotspot id.
        spot: i32,
        /// New `HS_*` type.
        hotspot_type: i32,
        /// Raw `HS_*` flag word before the top-layer bit is merged.
        flags: i32,
        /// Whether `topLayer` was positive.
        top_layer: bool,
    },
    /// `SETSPOTSCRIPT` — attach an `ON <EVENT> { ... }` handler to a hotspot.
    ///
    /// Local-only, like [`Effect::AddSpot`]: the reference merges the block into
    /// its own hotspot script and there is no documented client→server body, so
    /// [`Effect::is_wire_effect`] omits it.
    SetSpotScript {
        /// Target hotspot id.
        spot: i32,
        /// The event name, upper-cased as the reference merges it.
        event: String,
        /// The block's inner source text, without the braces.
        script: String,
    },
    /// `SETTOOLTIP` — show `text` as the hover tooltip.
    ///
    /// Local-only, like [`Effect::AddSpot`]: the reference keeps the tooltip in
    /// its own UI state and no wire body exists for it.
    SetTooltip { text: String },
    /// `CLEARTOOLTIP` — hide the hover tooltip.
    ///
    /// Local-only, like [`Effect::SetTooltip`].
    ClearTooltip,
    /// `HIDESMILEYS` — suppress smiley rendering locally.
    HideSmileys,
    /// `LOCKUSERPROPS` — stop other users changing your props.
    LockUserProps,
    /// `AUTOUSERLAYER` — set the automatic user-layer flag.
    AutoUserLayer { on: bool },
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
    /// `REMOVEPIC` — remove the picture at an index from a spot's state list.
    RemovePic { spot: i32, picture: i32 },
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
    /// `CONFIRMBOX` — ask the user to confirm something before continuing.
    Confirm { text: String },
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
    /// `DRAWTEXT` — text on the paint layer at `(x, y)` with the pen's current
    /// text style (`sparky/index.js` line 7, `e1` @56542).
    DrawText { text: String, x: i32, y: i32 },
    /// `OVAL` — a filled/outlined ellipse on the paint layer (`Qw` @56534).
    DrawOval { x: i32, y: i32, w: i32, h: i32 },
    /// `POLYGON` — a closed shape from a flat `[x y x y …]` outline (`qw` @56496).
    DrawPolygon { points: Vec<(i32, i32)> },
    /// `PENFONT` — face name new text strokes use (`t1` @56554).
    SetPenFont { name: String },
    /// `PENBOLD` — bold on/off for new text strokes (`n1` @56565).
    SetPenBold { on: bool },
    /// `PENITALIC` — italic on/off for new text strokes (`s1` @56592).
    SetPenItalic { on: bool },
    /// `PENUNDERLINE` — underline on/off for new text strokes (`o1` @56576).
    SetPenUnderline { on: bool },
    /// `PENSHADOW` — shadow on/off for new strokes (`r1` @56605).
    SetPenShadow { on: bool },
    /// `PENOPACITY` — paint opacity for new strokes (`jw` @56448).
    SetPenOpacity { value: i32 },
    /// `PENFILLCOLOR` — fill colour for closed shapes (`Xw` @56462).
    SetPenFillColor { r: i32, g: i32, b: i32 },
    /// `PENFILLOPACITY` — fill opacity for closed shapes (`Kw` @56478).
    SetPenFillOpacity { value: i32 },
    /// `REMOVESPOT` — drop a script-created hotspot (`v1` @56800).
    RemoveSpot { spot: i32 },
    /// `SETSPOTLOC` — move a script-created hotspot (`T1` @56814).
    SetSpotLoc { spot: i32, x: i32, y: i32 },
    /// `SETSPOTDEST` — set a door hotspot's destination (`E1` @56828).
    SetSpotDest { spot: i32, dest: i32 },
    /// `SETSPOTPOINTS` — replace a hotspot's outline (`Eb` @48267).
    SetSpotPoints {
        spot: i32,
        x: i32,
        y: i32,
        points: Vec<(i32, i32)>,
    },
    /// `SETSPOTPICMODE` — how a hotspot's picture scales (`Ub` @57862).
    SetSpotPicMode { spot: i32, mode: i32 },
    /// `SETSPOTSTYLE` — local colour/border override for a hotspot (`Ob` @57800).
    SetSpotStyle {
        spot: i32,
        color: String,
        border: i32,
        size: i32,
    },
    /// `ALERTBOX` — show a modal alert (`iS` @58282).
    Alert { text: String },
    /// `PROMPT` — ask for a line of text, seeded with `default` (`lS` @58308).
    Prompt { label: String, default: String },
    /// `SETCURSOR` — choose a stock cursor by index (`gS` @58401).
    SetCursor { index: i32 },
    /// `SETCURSORPIC` — use a picture as the cursor (`yS` @58414).
    SetCursorPic { name: String, x: i32, y: i32 },
    /// `WEBEMBED` — point a hotspot's embedded web view at `url`; an empty
    /// string tears it down (`uS` @58333).
    WebEmbed { spot: i32, url: String },
    /// `WEBSCRIPT` — run JavaScript inside a hotspot's embedded page (`fS` @58372).
    WebScript { spot: i32, script: String },
    /// `HTTPCANCEL` — abort every in-flight HTTP request (`f1` @56686).
    HttpCancel,
    /// An explicit, traced refusal.
    ///
    /// The command is recognised and its operands are consumed, but this host
    /// does not carry the action out. Unlike [`Effect::Unsupported`], a
    /// refusal is a documented decision that keeps the corpus
    /// "reached but not implemented" tally at zero; the reason travels with it
    /// so a run report names *why* rather than showing a silent drop.
    Refused { command: String, reason: String },
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
            Effect::GotoUrlFrame { .. } => "GOTOURLFRAME",
            Effect::FetchScript { .. } => "LOADSCRIPT",
            Effect::LaunchApp { .. } => "LAUNCHAPP",
            Effect::LaunchEvent { .. } => "LAUNCHEVENT",
            Effect::LaunchPpa { .. } => "LAUNCHPPA",
            Effect::LoadJava { .. } => "LOADJAVA",
            Effect::TalkPpa { .. } => "TALKPPA",
            Effect::ShellCommand { .. } => "SHELLCMD",
            Effect::MoveUserAbs { .. } => "SETPOS",
            Effect::MoveUserRel { .. } => "MOVE",
            Effect::SetColor { .. } => "SETCOLOR",
            Effect::SetFace { .. } => "SETFACE",
            Effect::SetUserName { .. } => "SETUSERNAME",
            Effect::SetProps { .. } => "SETPROPS",
            Effect::LoadProps { .. } => "LOADPROPS",
            Effect::DonProp { .. } => "DONPROP",
            Effect::DoffProp => "DOFFPROP",
            Effect::RemoveProp { .. } => "REMOVEPROP",
            Effect::RemovePic { .. } => "REMOVEPIC",
            Effect::Naked => "NAKED",
            Effect::SetSpotState { .. } => "SETSPOTSTATE",
            Effect::SetSpotStateLocal { .. } => "SETSPOTSTATELOCAL",
            Effect::SetSpotNameLocal { .. } => "SETSPOTNAMELOCAL",
            Effect::MoveSpot { .. } => "SETLOC",
            Effect::MoveSpotLocal { .. } => "SETLOCLOCAL",
            Effect::SetPicOffset { .. } => "SETPICLOC",
            Effect::SetPicOffsetLocal { .. } => "SETPICLOCLOCAL",
            Effect::SetPicOpacity { .. } => "SETPICOPACITY",
            Effect::SetPicBrightness { .. } => "SETPICBRIGHTNESS",
            Effect::SetPicSaturation { .. } => "SETPICSATURATION",
            Effect::AddSpot { .. } => "ADDSPOT",
            Effect::AddPic { .. } => "ADDPIC",
            Effect::SetSpotOptions { .. } => "SETSPOTOPTIONS",
            Effect::SetSpotScript { .. } => "SETSPOTSCRIPT",
            Effect::SetTooltip { .. } => "SETTOOLTIP",
            Effect::ClearTooltip => "CLEARTOOLTIP",
            Effect::HideSmileys => "HIDESMILEYS",
            Effect::LockUserProps => "LOCKUSERPROPS",
            Effect::AutoUserLayer { .. } => "AUTOUSERLAYER",
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
            Effect::Confirm { .. } => "CONFIRMBOX",
            Effect::DrawText { .. } => "DRAWTEXT",
            Effect::DrawOval { .. } => "OVAL",
            Effect::DrawPolygon { .. } => "POLYGON",
            Effect::SetPenFont { .. } => "PENFONT",
            Effect::SetPenBold { .. } => "PENBOLD",
            Effect::SetPenItalic { .. } => "PENITALIC",
            Effect::SetPenUnderline { .. } => "PENUNDERLINE",
            Effect::SetPenShadow { .. } => "PENSHADOW",
            Effect::SetPenOpacity { .. } => "PENOPACITY",
            Effect::SetPenFillColor { .. } => "PENFILLCOLOR",
            Effect::SetPenFillOpacity { .. } => "PENFILLOPACITY",
            Effect::RemoveSpot { .. } => "REMOVESPOT",
            Effect::SetSpotLoc { .. } => "SETSPOTLOC",
            Effect::SetSpotDest { .. } => "SETSPOTDEST",
            Effect::SetSpotPoints { .. } => "SETSPOTPOINTS",
            Effect::SetSpotPicMode { .. } => "SETSPOTPICMODE",
            Effect::SetSpotStyle { .. } => "SETSPOTSTYLE",
            Effect::Alert { .. } => "ALERTBOX",
            Effect::Prompt { .. } => "PROMPT",
            Effect::SetCursor { .. } => "SETCURSOR",
            Effect::SetCursorPic { .. } => "SETCURSORPIC",
            Effect::WebEmbed { .. } => "WEBEMBED",
            Effect::WebScript { .. } => "WEBSCRIPT",
            Effect::HttpCancel => "HTTPCANCEL",
            Effect::Refused { command, .. } => command,
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
            Effect::GotoUrlFrame { url, frame } => {
                write!(f, "GOTOURLFRAME {url:?} frame={frame:?}")
            }
            Effect::FetchScript { url, spot } => write!(f, "FETCHSCRIPT spot={spot} {url:?}"),
            Effect::LaunchApp { app } => write!(f, "LAUNCHAPP {app:?}"),
            Effect::LaunchEvent { event } => write!(f, "LAUNCHEVENT {event:?}"),
            Effect::LaunchPpa { ppa } => write!(f, "LAUNCHPPA {ppa:?}"),
            Effect::LoadJava { url } => write!(f, "LOADJAVA {url:?}"),
            Effect::TalkPpa { text } => write!(f, "TALKPPA {text:?}"),
            Effect::ShellCommand { command } => write!(f, "SHELLCMD {command:?}"),
            Effect::MoveUserAbs { x, y } => write!(f, "SETPOS ({x},{y})"),
            Effect::MoveUserRel { dx, dy } => write!(f, "MOVE ({dx},{dy})"),
            Effect::SetColor { color } => write!(f, "SETCOLOR {color}"),
            Effect::SetFace { face } => write!(f, "SETFACE {face}"),
            Effect::SetUserName { name } => write!(f, "SETUSERNAME {name:?}"),
            Effect::SetProps { props } => write!(f, "SETPROPS {props:?}"),
            Effect::LoadProps { props } => write!(f, "LOADPROPS {props:?}"),
            Effect::DonProp { prop } => write!(f, "DONPROP {prop}"),
            Effect::DoffProp => write!(f, "DOFFPROP"),
            Effect::RemoveProp { prop } => write!(f, "REMOVEPROP {prop}"),
            Effect::RemovePic { spot, picture } => {
                write!(f, "REMOVEPIC spot={spot} picture={picture}")
            }
            Effect::Naked => write!(f, "NAKED"),
            Effect::SetSpotState { spot, state } => {
                write!(f, "SETSPOTSTATE spot={spot} state={state}")
            }
            Effect::SetSpotStateLocal { spot, state } => {
                write!(f, "SETSPOTSTATELOCAL spot={spot} state={state}")
            }
            Effect::SetSpotNameLocal { spot, name } => {
                write!(f, "SETSPOTNAMELOCAL spot={spot} {name:?}")
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
            Effect::SetPicBrightness { spot, state, value } => {
                write!(f, "SETPICBRIGHTNESS spot={spot} state={state} {value}")
            }
            Effect::SetPicSaturation { spot, state, value } => {
                write!(f, "SETPICSATURATION spot={spot} state={state} {value}")
            }
            Effect::AddSpot { id, points, x, y } => {
                write!(f, "ADDSPOT id={id} at ({x},{y}) [{} points]", points.len())
            }
            Effect::AddPic { spot, name } => write!(f, "ADDPIC spot={spot} {name:?}"),
            Effect::SetSpotOptions {
                spot,
                hotspot_type,
                flags,
                top_layer,
            } => write!(
                f,
                "SETSPOTOPTIONS spot={spot} type={hotspot_type} flags={flags} top_layer={top_layer}"
            ),
            Effect::SetSpotScript {
                spot,
                event,
                script,
            } => write!(
                f,
                "SETSPOTSCRIPT spot={spot} event={event} script={script:?}"
            ),
            Effect::SetTooltip { text } => write!(f, "SETTOOLTIP {text:?}"),
            Effect::ClearTooltip => write!(f, "CLEARTOOLTIP"),
            Effect::HideSmileys => write!(f, "HIDESMILEYS"),
            Effect::LockUserProps => write!(f, "LOCKUSERPROPS"),
            Effect::AutoUserLayer { on } => write!(f, "AUTOUSERLAYER {on}"),
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
            Effect::Confirm { text } => write!(f, "CONFIRMBOX {text:?}"),
            Effect::DrawText { text, x, y } => write!(f, "DRAWTEXT ({x},{y}) {text:?}"),
            Effect::DrawOval { x, y, w, h } => write!(f, "OVAL ({x},{y}) {w}x{h}"),
            Effect::DrawPolygon { points } => write!(f, "POLYGON [{} points]", points.len()),
            Effect::SetPenFont { name } => write!(f, "PENFONT {name:?}"),
            Effect::SetPenBold { on } => write!(f, "PENBOLD {on}"),
            Effect::SetPenItalic { on } => write!(f, "PENITALIC {on}"),
            Effect::SetPenUnderline { on } => write!(f, "PENUNDERLINE {on}"),
            Effect::SetPenShadow { on } => write!(f, "PENSHADOW {on}"),
            Effect::SetPenOpacity { value } => write!(f, "PENOPACITY {value}"),
            Effect::SetPenFillColor { r, g, b } => write!(f, "PENFILLCOLOR ({r},{g},{b})"),
            Effect::SetPenFillOpacity { value } => write!(f, "PENFILLOPACITY {value}"),
            Effect::RemoveSpot { spot } => write!(f, "REMOVESPOT {spot}"),
            Effect::SetSpotLoc { spot, x, y } => write!(f, "SETSPOTLOC {spot} ({x},{y})"),
            Effect::SetSpotDest { spot, dest } => write!(f, "SETSPOTDEST {spot} -> {dest}"),
            Effect::SetSpotPoints { spot, x, y, points } => {
                write!(
                    f,
                    "SETSPOTPOINTS {spot} ({x},{y}) [{} points]",
                    points.len()
                )
            }
            Effect::SetSpotPicMode { spot, mode } => {
                write!(f, "SETSPOTPICMODE {spot} mode={mode}")
            }
            Effect::SetSpotStyle {
                spot,
                color,
                border,
                size,
            } => write!(
                f,
                "SETSPOTSTYLE {spot} {color:?} border={border} size={size}"
            ),
            Effect::Alert { text } => write!(f, "ALERTBOX {text:?}"),
            Effect::Prompt { label, default } => {
                write!(f, "PROMPT {label:?} default={default:?}")
            }
            Effect::SetCursor { index } => write!(f, "SETCURSOR {index}"),
            Effect::SetCursorPic { name, x, y } => {
                write!(f, "SETCURSORPIC {name:?} ({x},{y})")
            }
            Effect::WebEmbed { spot, url } => write!(f, "WEBEMBED {spot} {url:?}"),
            Effect::WebScript { spot, script } => write!(f, "WEBSCRIPT {spot} {script:?}"),
            Effect::HttpCancel => write!(f, "HTTPCANCEL"),
            Effect::Refused { command, reason } => write!(f, "refused {command}: {reason}"),
            Effect::Unsupported { command } => write!(f, "unsupported {command}"),
        }
    }
}

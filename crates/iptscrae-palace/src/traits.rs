//! The Palace capability trait.
//!
//! This is the layer above [`iptscrae::Host`]: every Palace command is written
//! against one or more of these methods, and nothing else. It mirrors
//! OpenPalace's `IPalaceController` — the ~90-method host interface — so the
//! command surface can be ported command-for-command.
//!
//! Every method has a default, so a host implements only what it can honestly
//! support: getters return `0`/an empty string, and actions return
//! [`IptError::CommandUnavailable`]. A cyborg-only or headless host is therefore
//! a few lines, and a script that asks for something unsupported fails that
//! command rather than silently doing the wrong thing.
//!
//! The trait deliberately exposes no sockets, files or clocks. `goto_room` and
//! `goto_url` are *requests* the host may refuse; `chat` is text the host may
//! drop. That keeps the security boundary in one place: the trait.
//!
//! [`iptscrae::Host`]: iptscrae::Host
//! [`IptError::CommandUnavailable`]: iptscrae::IptError::CommandUnavailable

use iptscrae::error::{IptError, Result};
use iptscrae::Host;

/// A Palace client as seen by a script.
///
/// Method names follow the reference `IPalaceController` interface; the
/// doc-comments name the IPTSCRAE command(s) each one backs. Coordinates are
/// screen pixels in the 512×384 viewing area unless stated otherwise.
pub trait PalaceHost: Host {
    // ------------------------------------------------------------- identity
    /// Your own user id (`USERID`, `WHOME`).
    fn get_self_user_id(&self) -> i64 {
        0
    }
    /// Your own screen name (`USERNAME`).
    fn get_self_user_name(&self) -> String {
        String::new()
    }
    /// A user's screen name (`WHONAME`).
    fn get_user_name(&self, _user: i64) -> String {
        String::new()
    }
    /// A user's id from their name (`WHOPOS "name"`).
    fn get_user_by_name(&self, _name: &str) -> i64 {
        0
    }
    /// Whether you are a guest (`ISGUEST`).
    fn is_guest(&self) -> bool {
        false
    }
    /// Whether you are an operator (`ISWIZARD`).
    fn is_wizard(&self) -> bool {
        false
    }
    /// Whether you own the server (`ISGOD`).
    fn is_god(&self) -> bool {
        false
    }
    /// The client name (`CLIENTTYPE`).
    fn client_type(&self) -> String {
        "OPENPALACE".to_owned()
    }

    // ----------------------------------------------------------------- room
    /// The current room id (`ROOMID`).
    fn get_room_id(&self) -> i64 {
        0
    }
    /// The current room's name (`ROOMNAME`).
    fn get_room_name(&self) -> String {
        String::new()
    }
    /// The server's name (`SERVERNAME`).
    fn get_server_name(&self) -> String {
        String::new()
    }
    /// Viewing-area width (`ROOMWIDTH`).
    fn get_room_width(&self) -> i64 {
        512
    }
    /// Viewing-area height (`ROOMHEIGHT`).
    fn get_room_height(&self) -> i64 {
        384
    }
    /// How many users are in the room (`NBRROOMUSERS`).
    fn get_num_room_users(&self) -> i64 {
        0
    }
    /// Nth user in the room (`ROOMUSER`).
    fn get_room_user_id_by_index(&self, _index: i64) -> i64 {
        0
    }
    /// How many spots and doors the room has (`NBRSPOTS`, `NBRDOORS`).
    fn get_num_spots(&self) -> i64 {
        0
    }
    /// Nth spot id (`SPOTIDX`).
    fn get_spot_id_by_index(&self, _index: i64) -> i64 {
        0
    }
    /// Nth door id (`DOORIDX`).
    fn get_door_id_by_index(&self, _index: i64) -> i64 {
        0
    }
    /// Move you to another room (`GOTOROOM`).
    fn goto_room(&mut self, _room: i64) -> Result<()> {
        Err(unavailable("GOTOROOM"))
    }
    /// Open a URL in the user's browser (`NETGOTO`, `GOTOURL`).
    fn goto_url(&mut self, _url: &str) -> Result<()> {
        Err(unavailable("GOTOURL"))
    }
    /// Launch a Palace plugin (`LAUNCHAPP`).
    fn launch_app(&mut self, _app: &str) -> Result<()> {
        Err(unavailable("LAUNCHAPP"))
    }
    /// Dim the room to `percent` (`DIMROOM`).
    fn dim_room(&mut self, _percent: i64) -> Result<()> {
        Err(unavailable("DIMROOM"))
    }

    // ------------------------------------------------------------- position
    /// Your x position (`POSX`).
    fn get_self_pos_x(&self) -> i64 {
        0
    }
    /// Your y position (`POSY`).
    fn get_self_pos_y(&self) -> i64 {
        0
    }
    /// A user's x position (`WHOPOS`).
    fn get_pos_x(&self, _user: i64) -> i64 {
        0
    }
    /// A user's y position (`WHOPOS`).
    fn get_pos_y(&self, _user: i64) -> i64 {
        0
    }
    /// Mouse x (`MOUSEPOS`).
    fn get_mouse_x(&self) -> i64 {
        0
    }
    /// Mouse y (`MOUSEPOS`).
    fn get_mouse_y(&self) -> i64 {
        0
    }
    /// Teleport you to absolute `(x, y)` (`SETPOS`).
    fn move_user_abs(&mut self, _x: i64, _y: i64) -> Result<()> {
        Err(unavailable("SETPOS"))
    }
    /// Nudge you by `(dx, dy)` (`MOVE`).
    fn move_user_rel(&mut self, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("MOVE"))
    }

    // ------------------------------------------------------------- spots
    /// A spot's current state (`GETSPOTSTATE`).
    fn get_spot_state(&self, _spot: i64) -> i64 {
        0
    }
    /// A door's destination room (`SPOTDEST`, `DEST`).
    fn get_spot_dest(&self, _spot: i64) -> i64 {
        0
    }
    /// A spot's name (`SPOTNAME`).
    fn get_spot_name(&self, _spot: i64) -> String {
        String::new()
    }
    /// A spot's outline origin (`GETSPOTLOC`).
    fn get_spot_location(&self, _spot: i64) -> (i64, i64) {
        (0, 0)
    }
    /// A spot's picture offset for a state (`GETPICLOC`).
    fn get_pic_offset(&self, _spot: i64, _state: i64) -> (i64, i64) {
        (0, 0)
    }
    /// A spot's picture size for a state (`GETPICDIMENSIONS`).
    fn get_pic_dimensions(&self, _spot: i64, _state: i64) -> (i64, i64) {
        (0, 0)
    }
    /// Set a spot's state for everyone (`SETSPOTSTATE`).
    fn set_spot_state(&mut self, _spot: i64, _state: i64) -> Result<()> {
        Err(unavailable("SETSPOTSTATE"))
    }
    /// Set a spot's state for you alone (`SETSPOTSTATELOCAL`).
    fn set_spot_state_local(&mut self, _spot: i64, _state: i64) -> Result<()> {
        Err(unavailable("SETSPOTSTATELOCAL"))
    }
    /// Rename a spot for you alone (`SETSPOTNAMELOCAL`).
    fn set_spot_name_local(&mut self, _spot: i64, _name: &str) -> Result<()> {
        Err(unavailable("SETSPOTNAMELOCAL"))
    }
    /// Move a spot for everyone (`SETLOC`).
    fn move_spot(&mut self, _spot: i64, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("SETLOC"))
    }
    /// Move a spot for you alone (`SETLOCLOCAL`).
    fn move_spot_local(&mut self, _spot: i64, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("SETLOCLOCAL"))
    }
    /// Move a spot's picture for everyone (`SETPICLOC`).
    fn set_pic_offset(&mut self, _spot: i64, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("SETPICLOC"))
    }
    /// Move a spot's picture for you alone (`SETPICLOCLOCAL`).
    fn set_pic_offset_local(&mut self, _spot: i64, _state: i64, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("SETPICLOCLOCAL"))
    }
    /// Set a spot picture's opacity (`SETPICOPACITY`).
    fn set_pic_opacity(&mut self, _spot: i64, _state: i64, _opacity: f64) -> Result<()> {
        Err(unavailable("SETPICOPACITY"))
    }
    /// Whether a door is locked (`ISLOCKED`).
    fn is_locked(&self, _door: i64) -> bool {
        false
    }
    /// Whether you are inside a spot (`INSPOT`).
    fn in_spot(&self, _spot: i64) -> bool {
        false
    }
    /// Lock a door (`LOCK`).
    fn lock(&mut self, _door: i64) -> Result<()> {
        Err(unavailable("LOCK"))
    }
    /// Unlock a door (`UNLOCK`).
    fn unlock(&mut self, _door: i64) -> Result<()> {
        Err(unavailable("UNLOCK"))
    }
    /// Fire a spot's `ON SELECT` (`SELECT`).
    fn select_hot_spot(&mut self, _spot: i64) -> Result<()> {
        Err(unavailable("SELECT"))
    }
    /// Schedule a spot's `ON ALARM` (`SETALARM`).
    fn set_spot_alarm(&mut self, _spot: i64, _future_ticks: i64) -> Result<()> {
        Err(unavailable("SETALARM"))
    }

    // -------------------------------------------------------------- props
    /// The prop you are wearing on top (`TOPPROP`).
    fn get_top_prop(&self) -> i64 {
        0
    }
    /// Nth worn prop (`USERPROP`).
    fn get_user_prop(&self, _index: i64) -> i64 {
        0
    }
    /// How many props you wear (`NBRUSERPROPS`).
    fn get_num_user_props(&self) -> i64 {
        0
    }
    /// Resolve a prop name to an id (`DONPROP "name"`).
    fn get_prop_id_by_name(&self, _name: &str) -> i64 {
        0
    }
    /// Whether you wear a prop id (`HASPROP`).
    fn has_prop_by_id(&self, _prop: i64) -> bool {
        false
    }
    /// Whether you wear a prop name (`HASPROP`).
    fn has_prop_by_name(&self, _name: &str) -> bool {
        false
    }
    /// Wear a prop by id (`DONPROP`).
    fn don_prop_by_id(&mut self, _prop: i64) -> Result<()> {
        Err(unavailable("DONPROP"))
    }
    /// Wear a prop by name (`DONPROP`).
    fn don_prop_by_name(&mut self, _name: &str) -> Result<()> {
        Err(unavailable("DONPROP"))
    }
    /// Wear a list of props (`SETPROPS`).
    fn set_props(&mut self, _props: &[i64]) -> Result<()> {
        Err(unavailable("SETPROPS"))
    }
    /// Remove the last prop you put on (`DOFFPROP`).
    fn doff_prop(&mut self) -> Result<()> {
        Err(unavailable("DOFFPROP"))
    }
    /// Remove a prop by id (`REMOVEPROP`).
    fn doff_prop_by_id(&mut self, _prop: i64) -> Result<()> {
        Err(unavailable("REMOVEPROP"))
    }
    /// Remove a prop by name (`REMOVEPROP`).
    fn doff_prop_by_name(&mut self, _name: &str) -> Result<()> {
        Err(unavailable("REMOVEPROP"))
    }
    /// Remove every prop (`NAKED`, `CLEARPROPS`).
    fn naked(&mut self) -> Result<()> {
        Err(unavailable("NAKED"))
    }
    /// Preload props into the cache (`LOADPROPS`).
    fn load_props(&mut self, _props: &[i64]) -> Result<()> {
        Err(unavailable("LOADPROPS"))
    }

    // -------------------------------------------------------- loose props
    /// How many loose props the room has (`NBRLOOSEPROPS`).
    fn get_num_loose_props(&self) -> i64 {
        0
    }
    /// Nth loose prop's id (`LOOSEPROP`).
    fn get_loose_prop_id_by_index(&self, _index: i64) -> i64 {
        0
    }
    /// A loose prop's index from its id (`LOOSEPROPIDX`).
    fn get_loose_prop_index_by_id(&self, _prop: i64) -> i64 {
        -1
    }
    /// A loose prop's position (`LOOSEPROPPOS`).
    fn get_loose_prop_pos(&self, _index: i64) -> (i64, i64) {
        (0, 0)
    }
    /// Place a loose prop (`ADDLOOSEPROP`).
    fn add_loose_prop(&mut self, _prop: i64, _x: i64, _y: i64) -> Result<()> {
        Err(unavailable("ADDLOOSEPROP"))
    }
    /// Remove a loose prop by index (`REMOVELOOSEPROP`).
    fn remove_loose_prop(&mut self, _index: i64) -> Result<()> {
        Err(unavailable("REMOVELOOSEPROP"))
    }
    /// Move a loose prop (`MOVELOOSEPROP`).
    fn move_loose_prop(&mut self, _index: i64, _x: i64, _y: i64) -> Result<()> {
        Err(unavailable("MOVELOOSEPROP"))
    }
    /// Drop your last prop at a position (`DROPPROP`).
    fn drop_prop(&mut self, _x: i64, _y: i64) -> Result<()> {
        Err(unavailable("DROPPROP"))
    }
    /// Remove every loose prop (`CLEARLOOSEPROPS`).
    fn clear_loose_props(&mut self) -> Result<()> {
        Err(unavailable("CLEARLOOSEPROPS"))
    }
    /// List loose props in the log (`SHOWLOOSEPROPS`).
    fn show_loose_props(&mut self) -> Result<()> {
        Err(unavailable("SHOWLOOSEPROPS"))
    }

    // ------------------------------------------------------------- talking
    /// Speak in a balloon (`SAY`, `CHAT`, `SAYAT`).
    fn chat(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("SAY"))
    }
    /// Message everyone on the server (`GLOBALMSG`).
    fn send_global_message(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("GLOBALMSG"))
    }
    /// Message everyone in the room (`ROOMMSG`).
    fn send_room_message(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("ROOMMSG"))
    }
    /// Message yourself (`LOCALMSG`).
    fn send_local_msg(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("LOCALMSG"))
    }
    /// Page the owner/operator (`SUSRMSG`).
    fn send_susr_message(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("SUSRMSG"))
    }
    /// Whisper to one user (`PRIVATEMSG`).
    fn send_private_message(&mut self, _user: i64, _text: &str) -> Result<()> {
        Err(unavailable("PRIVATEMSG"))
    }
    /// Put text in the status bar (`STATUSMSG`).
    fn status_message(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("STATUSMSG"))
    }
    /// Put text in the log window (`LOGMSG`).
    fn log_message(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("LOGMSG"))
    }
    /// Report an error to the user (`ERRORMSG`, `LOGERR`).
    fn log_error(&mut self, _text: &str) -> Result<()> {
        Err(unavailable("ERRORMSG"))
    }
    /// The user being chatted about (`WHOCHAT`).
    fn get_who_chat(&self) -> i64 {
        0
    }
    /// The whisper target (`WHOTARGET`).
    fn get_who_target(&self) -> i64 {
        0
    }

    // ------------------------------------------------------------- avatar
    /// Set the roundhead colour (`SETCOLOR`).
    fn change_color(&mut self, _colour: i64) -> Result<()> {
        Err(unavailable("SETCOLOR"))
    }
    /// Set the roundhead expression (`SETFACE`).
    fn set_face(&mut self, _face: i64) -> Result<()> {
        Err(unavailable("SETFACE"))
    }
    /// Run one of the user's avatar macros (`MACRO`).
    fn do_macro(&mut self, _macro: i64) -> Result<()> {
        Err(unavailable("MACRO"))
    }
    /// Kick a user (`KILLUSER`).
    fn kill_user(&mut self, _user: i64) -> Result<()> {
        Err(unavailable("KILLUSER"))
    }
    /// Hide everyone's avatar (`HIDEAVATARS`).
    fn hide_avatars(&mut self) -> Result<()> {
        Err(unavailable("HIDEAVATARS"))
    }
    /// Show everyone's avatar (`SHOWAVATARS`).
    fn show_avatars(&mut self) -> Result<()> {
        Err(unavailable("SHOWAVATARS"))
    }

    // -------------------------------------------------------------- sound
    /// Play a `.wav` (`SOUND`).
    fn play_sound(&mut self, _name: &str) -> Result<()> {
        Err(unavailable("SOUND"))
    }
    /// Play a `.mid` (`MIDIPLAY`).
    fn midi_play(&mut self, _name: &str) -> Result<()> {
        Err(unavailable("MIDIPLAY"))
    }
    /// Loop a `.mid` (`MIDILOOP`).
    fn midi_loop(&mut self, _name: &str, _loops: i64) -> Result<()> {
        Err(unavailable("MIDILOOP"))
    }
    /// Stop MIDI (`MIDISTOP`).
    fn midi_stop(&mut self) -> Result<()> {
        Err(unavailable("MIDISTOP"))
    }
    /// Beep (`BEEP`).
    fn beep(&mut self) -> Result<()> {
        Err(unavailable("BEEP"))
    }

    // -------------------------------------------------------------- paint
    /// Draw a line in absolute coordinates (`LINE`).
    fn draw_line_abs(&mut self, _x1: i64, _y1: i64, _x2: i64, _y2: i64) -> Result<()> {
        Err(unavailable("LINE"))
    }
    /// Draw a line relative to the pen (`LINETO`).
    fn draw_line_rel(&mut self, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("LINETO"))
    }
    /// Move the pen without drawing (`PENPOS`).
    fn move_pen_abs(&mut self, _x: i64, _y: i64) -> Result<()> {
        Err(unavailable("PENPOS"))
    }
    /// Move the pen relative without drawing (`PENTO`).
    fn move_pen_rel(&mut self, _dx: i64, _dy: i64) -> Result<()> {
        Err(unavailable("PENTO"))
    }
    /// Set the pen colour (`PENCOLOR`).
    fn set_pen_color(&mut self, _r: i64, _g: i64, _b: i64) -> Result<()> {
        Err(unavailable("PENCOLOR"))
    }
    /// Set the pen width (`PENSIZE`).
    fn set_pen_size(&mut self, _size: i64) -> Result<()> {
        Err(unavailable("PENSIZE"))
    }
    /// Paint behind avatars (`PENBACK`).
    fn paint_back_layer(&mut self) -> Result<()> {
        Err(unavailable("PENBACK"))
    }
    /// Paint in front of avatars (`PENFRONT`).
    fn paint_front_layer(&mut self) -> Result<()> {
        Err(unavailable("PENFRONT"))
    }
    /// Erase all painting (`PAINTCLEAR`).
    fn paint_clear(&mut self) -> Result<()> {
        Err(unavailable("PAINTCLEAR"))
    }
    /// Undo the last paint stroke (`PAINTUNDO`).
    fn paint_undo(&mut self) -> Result<()> {
        Err(unavailable("PAINTUNDO"))
    }

    // -------------------------------------------------------------- misc
    /// Cancel every pending alarm (`STOPALARMS`).
    fn clear_alarms(&mut self) -> Result<()> {
        Err(unavailable("STOPALARMS"))
    }
    /// Block the client for a while (`DELAY`).
    fn delay_ticks(&mut self, _ticks: i64) -> Result<()> {
        Ok(())
    }
    /// Set the incoming/outgoing chat text (`CHATSTR`).
    fn set_chat_string(&mut self, _text: &str) -> Result<()> {
        Ok(())
    }
    /// Read the current chat text (`CHATSTR`).
    fn get_chat_string(&self) -> String {
        String::new()
    }
}

fn unavailable(command: &str) -> IptError {
    IptError::CommandUnavailable {
        command: command.to_owned(),
    }
}

//! The `PalaceHost` refusal contract.
//!
//! Every method on [`PalaceHost`] has a default. This file pins what those
//! defaults *are*: getters return a neutral value, and every action refuses with
//! [`IptError::CommandUnavailable`] naming the command it backs. That refusal is
//! load-bearing — a script that asks a headless or cyborg host for something it
//! cannot do fails that command instead of silently corrupting the stack or
//! doing the wrong thing — so it is asserted command by command rather than
//! spot-checked.
//!
//! The second half proves the other direction: a host that *does* override a
//! method gets its own value out, for both getters and actions.

use iptscrae::{Host, IptError, Result};
use iptscrae_palace::PalaceHost;

/// A host that implements nothing at all, so every Palace method takes its
/// default.
#[derive(Default)]
struct Bare;

impl Host for Bare {}
impl PalaceHost for Bare {}

/// Assert that an action default refused, and that the refusal names `expected`.
fn refused(result: Result<()>, expected: &str) {
    match result {
        Err(IptError::CommandUnavailable { command }) => {
            assert_eq!(command, expected, "refusal must name the command")
        }
        Err(other) => panic!("{expected}: expected a CommandUnavailable, got {other:?}"),
        Ok(()) => panic!("{expected}: expected a refusal, got Ok(())"),
    }
}

#[test]
fn identity_getters_default_to_neutral_values() {
    let host = Bare;
    assert_eq!(host.get_self_user_id(), 0, "USERID/WHOME");
    assert_eq!(host.get_self_user_name(), "", "USERNAME");
    assert_eq!(host.get_user_name(4242), "", "WHONAME is empty for nobody");
    assert_eq!(host.get_user_by_name("nobody"), 0, "WHOPOS finds nobody");
    assert!(!host.is_guest(), "ISGUEST");
    assert!(!host.is_wizard(), "ISWIZARD");
    assert!(!host.is_god(), "ISGOD");
    assert_eq!(
        host.client_type(),
        "OPENPALACE",
        "CLIENTTYPE is the one getter with a meaningful non-empty default"
    );
}

#[test]
fn room_and_geometry_getters_default_to_neutral_values() {
    let host = Bare;
    assert_eq!(host.get_room_id(), 0, "ROOMID");
    assert_eq!(host.get_room_name(), "", "ROOMNAME");
    assert_eq!(host.get_server_name(), "", "SERVERNAME");
    assert_eq!(host.get_room_width(), 512, "ROOMWIDTH is the viewing area");
    assert_eq!(
        host.get_room_height(),
        384,
        "ROOMHEIGHT is the viewing area"
    );
    assert_eq!(host.get_num_room_users(), 0, "NBRROOMUSERS");
    assert_eq!(host.get_room_user_id_by_index(0), 0, "ROOMUSER");
    assert_eq!(
        host.get_room_user_id_by_index(99),
        0,
        "ROOMUSER out of range"
    );
    assert_eq!(host.get_num_spots(), 0, "NBRSPOTS/NBRDOORS");
    assert_eq!(host.get_spot_id_by_index(3), 0, "SPOTIDX");
    assert_eq!(host.get_door_id_by_index(3), 0, "DOORIDX");
}

#[test]
fn position_getters_default_to_zero() {
    let host = Bare;
    assert_eq!(host.get_self_pos_x(), 0, "POSX");
    assert_eq!(host.get_self_pos_y(), 0, "POSY");
    assert_eq!(host.get_pos_x(7), 0, "WHOPOS");
    assert_eq!(host.get_pos_y(7), 0, "WHOPOS");
    assert_eq!(host.get_mouse_x(), 0, "MOUSEPOS");
    assert_eq!(host.get_mouse_y(), 0, "MOUSEPOS");
}

#[test]
fn spot_getters_default_to_zero_false_and_empty() {
    let host = Bare;
    assert_eq!(host.get_spot_state(3), 0, "GETSPOTSTATE");
    assert_eq!(host.get_spot_dest(3), 0, "SPOTDEST/DEST");
    assert_eq!(host.get_spot_name(3), "", "SPOTNAME");
    assert_eq!(host.get_spot_location(3), (0, 0), "GETSPOTLOC");
    assert_eq!(host.get_pic_offset(3, 1), (0, 0), "GETPICLOC");
    assert_eq!(host.get_pic_dimensions(3, 1), (0, 0), "GETPICDIMENSIONS");
    assert!(!host.is_locked(3), "ISLOCKED");
    assert!(!host.in_spot(3), "INSPOT");
}

#[test]
fn prop_getters_default_to_zero_false_and_empty() {
    let host = Bare;
    assert_eq!(host.get_top_prop(), 0, "TOPPROP");
    assert_eq!(host.get_user_prop(0), 0, "USERPROP");
    assert_eq!(host.get_num_user_props(), 0, "NBRUSERPROPS");
    assert_eq!(host.get_prop_id_by_name("hat"), 0, "DONPROP by name");
    assert!(!host.has_prop_by_id(3), "HASPROP");
    assert!(!host.has_prop_by_name("hat"), "HASPROP");
}

#[test]
fn loose_prop_getters_default_to_neutral_values() {
    let host = Bare;
    assert_eq!(host.get_num_loose_props(), 0, "NBRLOOSEPROPS");
    assert_eq!(host.get_loose_prop_id_by_index(0), 0, "LOOSEPROP");
    assert_eq!(
        host.get_loose_prop_index_by_id(3),
        -1,
        "LOOSEPROPIDX is -1 for a prop that is not loose"
    );
    assert_eq!(host.get_loose_prop_pos(0), (0, 0), "LOOSEPROPPOS");
}

#[test]
fn talking_getters_default_to_zero_and_empty() {
    let host = Bare;
    assert_eq!(host.get_who_chat(), 0, "WHOCHAT");
    assert_eq!(host.get_who_target(), 0, "WHOTARGET");
    assert_eq!(host.get_chat_string(), "", "CHATSTR");
}

#[test]
fn room_and_movement_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.goto_room(817), "GOTOROOM");
    refused(host.goto_url("https://example.invalid/"), "GOTOURL");
    refused(host.launch_app("calc.exe"), "LAUNCHAPP");
    refused(host.dim_room(50), "DIMROOM");
    refused(host.move_user_abs(10, 20), "SETPOS");
    refused(host.move_user_rel(1, -1), "MOVE");
}

#[test]
fn spot_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.set_spot_state(1, 2), "SETSPOTSTATE");
    refused(host.set_spot_state_local(1, 2), "SETSPOTSTATELOCAL");
    refused(host.set_spot_name_local(1, "name"), "SETSPOTNAMELOCAL");
    refused(host.move_spot(1, 2, 3), "SETLOC");
    refused(host.move_spot_local(1, 2, 3), "SETLOCLOCAL");
    refused(host.set_pic_offset(1, 2, 3), "SETPICLOC");
    refused(host.set_pic_offset_local(1, 2, 3, 4), "SETPICLOCLOCAL");
    refused(host.set_pic_opacity(1, 0, 0.5), "SETPICOPACITY");
    refused(host.lock(1), "LOCK");
    refused(host.unlock(1), "UNLOCK");
    refused(host.select_hot_spot(1), "SELECT");
    refused(host.set_spot_alarm(1, 60), "SETALARM");
}

#[test]
fn prop_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.don_prop_by_id(3), "DONPROP");
    refused(host.don_prop_by_name("hat"), "DONPROP");
    refused(host.set_props(&[1, 2, 3]), "SETPROPS");
    refused(host.doff_prop(), "DOFFPROP");
    refused(host.doff_prop_by_id(3), "REMOVEPROP");
    refused(host.doff_prop_by_name("hat"), "REMOVEPROP");
    refused(host.naked(), "NAKED");
    refused(host.load_props(&[1, 2]), "LOADPROPS");
}

#[test]
fn loose_prop_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.add_loose_prop(3, 1, 2), "ADDLOOSEPROP");
    refused(host.remove_loose_prop(0), "REMOVELOOSEPROP");
    refused(host.move_loose_prop(0, 1, 2), "MOVELOOSEPROP");
    refused(host.drop_prop(1, 2), "DROPPROP");
    refused(host.clear_loose_props(), "CLEARLOOSEPROPS");
    refused(host.show_loose_props(), "SHOWLOOSEPROPS");
}

#[test]
fn talking_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.chat("hello"), "SAY");
    refused(host.send_global_message("hello"), "GLOBALMSG");
    refused(host.send_room_message("hello"), "ROOMMSG");
    refused(host.send_local_msg("hello"), "LOCALMSG");
    refused(host.send_susr_message("hello"), "SUSRMSG");
    refused(host.send_private_message(7, "psst"), "PRIVATEMSG");
    refused(host.status_message("hello"), "STATUSMSG");
    refused(host.log_message("hello"), "LOGMSG");
    refused(host.log_error("bad"), "ERRORMSG");
}

#[test]
fn avatar_sound_and_paint_actions_refuse_with_their_command_name() {
    let mut host = Bare;
    refused(host.change_color(1), "SETCOLOR");
    refused(host.set_face(1), "SETFACE");
    refused(host.do_macro(0), "MACRO");
    refused(host.kill_user(7), "KILLUSER");
    refused(host.hide_avatars(), "HIDEAVATARS");
    refused(host.show_avatars(), "SHOWAVATARS");

    refused(host.play_sound("beep.wav"), "SOUND");
    refused(host.midi_play("song.mid"), "MIDIPLAY");
    refused(host.midi_loop("song.mid", 3), "MIDILOOP");
    refused(host.midi_stop(), "MIDISTOP");
    refused(host.beep(), "BEEP");

    refused(host.draw_line_abs(0, 0, 1, 1), "LINE");
    refused(host.draw_line_rel(1, 1), "LINETO");
    refused(host.move_pen_abs(1, 1), "PENPOS");
    refused(host.move_pen_rel(1, 1), "PENTO");
    refused(host.set_pen_color(1, 2, 3), "PENCOLOR");
    refused(host.set_pen_size(2), "PENSIZE");
    refused(host.paint_back_layer(), "PENBACK");
    refused(host.paint_front_layer(), "PENFRONT");
    refused(host.paint_clear(), "PAINTCLEAR");
    refused(host.paint_undo(), "PAINTUNDO");

    refused(host.clear_alarms(), "STOPALARMS");
}

#[test]
fn delay_and_chat_string_are_accepted_no_ops_rather_than_refusals() {
    // These two "actions" are deliberately inert in the default host: a delay
    // that the VM already budgeted must not fault, and an unset CHATSTR is a
    // value the script may write, not a capability it is denied.
    let mut host = Bare;
    assert_eq!(host.delay_ticks(30), Ok(()), "DELAY must not fault");
    assert_eq!(host.set_chat_string("typed"), Ok(()), "CHATSTR is writable");
}

// ---------------------------------------------------------------------------
// A host that overrides methods gets its own values out.
// ---------------------------------------------------------------------------

/// A cyborg-ish host overriding one getter in every section.
struct Custom;

impl Host for Custom {}
impl PalaceHost for Custom {
    fn get_self_user_id(&self) -> i64 {
        4242
    }
    fn get_self_user_name(&self) -> String {
        "Nyx".to_owned()
    }
    fn get_user_name(&self, user: i64) -> String {
        format!("user{user}")
    }
    fn get_user_by_name(&self, name: &str) -> i64 {
        name.len() as i64
    }
    fn is_guest(&self) -> bool {
        true
    }
    fn is_wizard(&self) -> bool {
        true
    }
    fn is_god(&self) -> bool {
        true
    }
    fn client_type(&self) -> String {
        "cyborg".to_owned()
    }
    fn get_room_id(&self) -> i64 {
        77
    }
    fn get_room_name(&self) -> String {
        "Atrium".to_owned()
    }
    fn get_server_name(&self) -> String {
        "Palace".to_owned()
    }
    fn get_room_width(&self) -> i64 {
        800
    }
    fn get_room_height(&self) -> i64 {
        600
    }
    fn get_num_room_users(&self) -> i64 {
        3
    }
    fn get_self_pos_x(&self) -> i64 {
        11
    }
    fn get_self_pos_y(&self) -> i64 {
        22
    }
    fn get_mouse_x(&self) -> i64 {
        33
    }
    fn get_mouse_y(&self) -> i64 {
        44
    }
    fn get_spot_state(&self, spot: i64) -> i64 {
        spot + 1
    }
    fn get_spot_name(&self, spot: i64) -> String {
        format!("spot{spot}")
    }
    fn get_spot_location(&self, spot: i64) -> (i64, i64) {
        (spot, -spot)
    }
    fn get_top_prop(&self) -> i64 {
        5
    }
    fn has_prop_by_id(&self, prop: i64) -> bool {
        prop == 5
    }
    fn get_num_loose_props(&self) -> i64 {
        9
    }
    fn get_loose_prop_index_by_id(&self, prop: i64) -> i64 {
        prop * 2
    }
    fn get_who_chat(&self) -> i64 {
        8
    }
    fn get_who_target(&self) -> i64 {
        9
    }
    fn get_chat_string(&self) -> String {
        "typed".to_owned()
    }
}

#[test]
fn an_overriding_host_sees_its_own_getter_values() {
    let host = Custom;
    assert_eq!(host.get_self_user_id(), 4242);
    assert_eq!(host.get_self_user_name(), "Nyx");
    assert_eq!(host.get_user_name(7), "user7");
    assert_eq!(host.get_user_by_name("abcd"), 4);
    assert!(host.is_guest() && host.is_wizard() && host.is_god());
    assert_eq!(host.client_type(), "cyborg");
    assert_eq!(host.get_room_id(), 77);
    assert_eq!(host.get_room_name(), "Atrium");
    assert_eq!(host.get_server_name(), "Palace");
    assert_eq!((host.get_room_width(), host.get_room_height()), (800, 600));
    assert_eq!(host.get_num_room_users(), 3);
    assert_eq!((host.get_self_pos_x(), host.get_self_pos_y()), (11, 22));
    assert_eq!((host.get_mouse_x(), host.get_mouse_y()), (33, 44));
    assert_eq!(host.get_spot_state(4), 5);
    assert_eq!(host.get_spot_name(4), "spot4");
    assert_eq!(host.get_spot_location(4), (4, -4));
    assert_eq!(host.get_top_prop(), 5);
    assert!(host.has_prop_by_id(5) && !host.has_prop_by_id(6));
    assert_eq!(host.get_num_loose_props(), 9);
    assert_eq!(host.get_loose_prop_index_by_id(3), 6);
    assert_eq!((host.get_who_chat(), host.get_who_target()), (8, 9));
    assert_eq!(host.get_chat_string(), "typed");
}

/// A host that records the actions it was asked to perform, so the override is
/// observable rather than just returning `Ok(())`.
#[derive(Default)]
struct Recorder {
    calls: Vec<String>,
}

impl Host for Recorder {}
impl PalaceHost for Recorder {
    fn chat(&mut self, text: &str) -> Result<()> {
        self.calls.push(format!("chat:{text}"));
        Ok(())
    }
    fn send_private_message(&mut self, user: i64, text: &str) -> Result<()> {
        self.calls.push(format!("private:{user}:{text}"));
        Ok(())
    }
    fn move_user_abs(&mut self, x: i64, y: i64) -> Result<()> {
        self.calls.push(format!("abs:{x},{y}"));
        Ok(())
    }
    fn goto_room(&mut self, room: i64) -> Result<()> {
        self.calls.push(format!("room:{room}"));
        Ok(())
    }
    fn delay_ticks(&mut self, ticks: i64) -> Result<()> {
        self.calls.push(format!("delay:{ticks}"));
        Ok(())
    }
    fn set_chat_string(&mut self, text: &str) -> Result<()> {
        self.calls.push(format!("chatstr:{text}"));
        Ok(())
    }
}

#[test]
fn an_overriding_host_replaces_the_refusal_with_its_own_action() {
    let mut host = Recorder::default();
    host.chat("hi").unwrap();
    host.send_private_message(7, "psst").unwrap();
    host.move_user_abs(1, 2).unwrap();
    host.goto_room(817).unwrap();
    host.delay_ticks(30).unwrap();
    host.set_chat_string("typed").unwrap();
    assert_eq!(
        host.calls,
        vec![
            "chat:hi".to_owned(),
            "private:7:psst".to_owned(),
            "abs:1,2".to_owned(),
            "room:817".to_owned(),
            "delay:30".to_owned(),
            "chatstr:typed".to_owned(),
        ]
    );
}

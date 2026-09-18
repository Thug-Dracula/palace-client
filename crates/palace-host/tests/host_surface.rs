//! Direct `PalaceHost` capability calls — the surface the dispatch table wraps.
//!
//! `Host::command` is one caller of these methods; the runtime is another (it
//! applies effects without going through a script). This file pins the trait
//! methods themselves, including the handful (`get_user_by_name`,
//! `get_prop_id_by_name`, `has_prop_by_name`, `kill_user`, `log_error`) that no
//! IPTSCRAE command reaches.

mod common;

use common::*;
use iptscrae::value::Value;
use iptscrae::{Chunk, Host, IptError};
use iptscrae_palace::PalaceHost;
use palace_host::{AlarmKind, Effect, PendingAlarm, ScriptHost};

// ------------------------------------------------------------- host primitives

#[test]
fn host_primitives_are_seeded_bounded_and_deterministic() {
    assert_eq!(ScriptHost::default().random(0), 0);
    assert_eq!(ScriptHost::default().random(-5), 0);

    let mut a = ScriptHost::default();
    let mut b = ScriptHost::default();
    let seq_a: Vec<i64> = (0..5).map(|_| a.random(1000)).collect();
    let seq_b: Vec<i64> = (0..5).map(|_| b.random(1000)).collect();
    assert_eq!(seq_a, seq_b, "the host rng is seeded, so runs reproduce");
    assert!(seq_a.iter().all(|v| (0..1000).contains(v)), "{seq_a:?}");

    let mut host = populated_host();
    host.tick = 120;
    assert_eq!(host.ticks(), 120);
    assert!(host.datetime() > 1_600_000_000, "a real wall clock");
    host.trace("one");
    host.trace("two");
    assert_eq!(host.trace, vec!["one".to_owned(), "two".to_owned()]);

    assert_eq!(
        host.grep_match("^a", "abc").unwrap(),
        Some(vec!["a".to_owned()])
    );
    assert_eq!(host.grep_match("zzz", "abc").unwrap(), None);
    assert_eq!(
        host.grep_match("", "abc").unwrap(),
        Some(vec![String::new()])
    );

    assert_eq!(
        host.initial_variables(),
        vec![("CHATSTR".to_owned(), Value::str("hi"))]
    );

    assert_eq!(host.command_pops("SAY"), 1);
    assert_eq!(host.command_pops("SAYAT"), 3);
    assert_eq!(host.command_pops("MIDILOOP"), 2);
    assert_eq!(host.command_pops("NOPE"), 0);
}

// -------------------------------------------------------- read-only capability

#[test]
fn resolvers_no_command_reaches_still_answer() {
    let host = populated_host();
    assert_eq!(host.get_user_by_name("squall"), 7, "case-insensitive");
    assert_eq!(host.get_user_by_name("SQUALL"), 7);
    assert_eq!(host.get_user_by_name("nobody"), 0);
    assert_eq!(host.get_prop_id_by_name("hat"), 0);
    assert!(!host.has_prop_by_name("hat"));
    assert!(host.has_prop_by_id(7));
    assert!(!host.has_prop_by_id(99));
    assert_eq!(host.get_pic_dimensions(5, 0), (10, 20));
    assert_eq!(host.get_pic_dimensions(5, -1), (30, 40));
    assert_eq!(host.get_pic_dimensions(99, 0), (0, 0));
    assert_eq!(host.get_prop_dimensions(7), (100, 50));
    assert_eq!(host.get_prop_offsets(7), (18, 3));
    assert_eq!(host.get_prop_dimensions(12345), (0, 0));
    assert_eq!(host.get_prop_offsets(12345), (0, 0));
    assert_eq!(host.get_num_spots(), 3);
}

// ----------------------------------------------------------- action abilities

#[test]
fn room_spot_and_picture_actions_record_effects() {
    let mut host = populated_host();
    host.goto_room(817).unwrap();
    host.goto_url("http://u").unwrap();
    host.launch_app("app").unwrap();
    host.dim_room(30).unwrap();
    host.move_user_abs(1, 2).unwrap();
    host.move_user_rel(3, 4).unwrap();
    host.set_spot_state(5, 1).unwrap();
    host.set_spot_state_local(5, 1).unwrap();
    host.set_spot_name_local(5, "New").unwrap();
    host.move_spot(5, 1, 2).unwrap();
    host.move_spot_local(5, 1, 2).unwrap();
    host.set_pic_offset(5, 1, 2).unwrap();
    host.set_pic_offset_local(5, 1, 3, 4).unwrap();
    host.set_pic_opacity(5, 1, 0.5).unwrap();
    host.lock(5).unwrap();
    host.unlock(5).unwrap();
    host.select_hot_spot(5).unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::GotoRoom { room: 817 },
            Effect::GotoUrl {
                url: "http://u".to_owned()
            },
            Effect::LaunchApp {
                app: "app".to_owned()
            },
            Effect::DimRoom { percent: 30 },
            Effect::MoveUserAbs { x: 1, y: 2 },
            Effect::MoveUserRel { dx: 3, dy: 4 },
            Effect::SetSpotState { spot: 5, state: 1 },
            Effect::SetSpotStateLocal { spot: 5, state: 1 },
            Effect::Unsupported {
                command: "SETSPOTNAMELOCAL(5,New)".to_owned()
            },
            Effect::MoveSpot {
                spot: 5,
                dx: 1,
                dy: 2
            },
            Effect::MoveSpotLocal {
                spot: 5,
                dx: 1,
                dy: 2
            },
            Effect::SetPicOffset {
                spot: 5,
                dx: 1,
                dy: 2
            },
            Effect::SetPicOffsetLocal {
                spot: 5,
                state: 1,
                dx: 3,
                dy: 4
            },
            Effect::SetPicOpacity {
                spot: 5,
                state: 1,
                opacity: 0.5
            },
            Effect::Lock { spot: 5 },
            Effect::Unlock { spot: 5 },
            Effect::SelectSpot { spot: 5 },
        ]
    );
}

#[test]
fn prop_loose_prop_and_avatar_actions_record_effects() {
    let mut host = populated_host();
    host.don_prop_by_id(7).unwrap();
    host.don_prop_by_name("hat").unwrap();
    host.set_props(&[7, 9]).unwrap();
    host.doff_prop().unwrap();
    host.doff_prop_by_id(7).unwrap();
    host.doff_prop_by_name("hat").unwrap();
    host.naked().unwrap();
    host.load_props(&[7]).unwrap(); // cache warm-up: no effect
    host.add_loose_prop(7, 10, 20).unwrap();
    host.remove_loose_prop(3).unwrap();
    host.move_loose_prop(3, 10, 20).unwrap();
    host.drop_prop(10, 20).unwrap();
    host.clear_loose_props().unwrap();
    host.show_loose_props().unwrap(); // log-only: no effect
    host.log_message("hello").unwrap();
    host.log_error("bad").unwrap();
    host.change_color(3).unwrap();
    host.set_face(4).unwrap();
    host.do_macro(1).unwrap(); // macros are GUI-only: no effect
    host.hide_avatars().unwrap();
    host.show_avatars().unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::DonProp { prop: 7 },
            Effect::Unsupported {
                command: "DONPROP(\"hat\")".to_owned()
            },
            Effect::SetProps { props: vec![7, 9] },
            Effect::DoffProp,
            Effect::RemoveProp { prop: 7 },
            Effect::Unsupported {
                command: "REMOVEPROP(\"hat\")".to_owned()
            },
            Effect::Naked,
            Effect::AddLooseProp {
                prop: 7,
                x: 10,
                y: 20
            },
            Effect::RemoveLooseProp { index: 3 },
            Effect::MoveLooseProp {
                index: 3,
                x: 10,
                y: 20
            },
            Effect::DropProp { x: 10, y: 20 },
            Effect::ClearLooseProps,
            Effect::LogMessage {
                text: "hello".to_owned()
            },
            Effect::ErrorMessage {
                text: "bad".to_owned()
            },
            Effect::SetColor { color: 3 },
            Effect::SetFace { face: 4 },
            Effect::HideAvatars,
            Effect::ShowAvatars,
        ]
    );
}

#[test]
fn sound_actions_record_effects() {
    let mut host = populated_host();
    host.play_sound("garden").unwrap();
    host.midi_play("g.mid").unwrap();
    host.midi_loop("g.mid", 99).unwrap();
    host.midi_stop().unwrap();
    host.beep().unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::PlaySound {
                name: "garden".to_owned()
            },
            Effect::MidiPlay {
                name: "g.mid".to_owned()
            },
            Effect::MidiLoop {
                name: "g.mid".to_owned(),
                loops: 99
            },
            Effect::MidiStop,
            Effect::Beep,
        ]
    );
}

#[test]
fn paint_actions_keep_the_pen_state_consistent() {
    let mut host = populated_host();
    host.draw_line_abs(1, 2, 3, 4).unwrap();
    assert_eq!(host.pen.pos, (3, 4));
    host.draw_line_rel(5, 6).unwrap();
    assert_eq!(host.pen.pos, (8, 10));
    host.move_pen_abs(10, 20).unwrap();
    host.move_pen_rel(5, 6).unwrap();
    assert_eq!(host.pen.pos, (15, 26));
    host.set_pen_color(1, 2, 3).unwrap();
    host.set_pen_size(2).unwrap();
    host.paint_back_layer().unwrap();
    host.paint_front_layer().unwrap();
    host.paint_clear().unwrap();
    host.paint_undo().unwrap();
    assert_eq!(host.pen.rgb, (1, 2, 3));
    assert_eq!(host.pen.size, 2);
    assert!(host.pen.front, "PENFRONT was the last layer command");
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::DrawLine {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4
            },
            Effect::DrawLineRel {
                x1: 3,
                y1: 4,
                x2: 8,
                y2: 10
            },
            Effect::MovePen { x: 10, y: 20 },
            Effect::MovePen { x: 15, y: 26 },
            Effect::SetPenColor { r: 1, g: 2, b: 3 },
            Effect::SetPenSize { size: 2 },
            Effect::PaintLayer { front: false },
            Effect::PaintLayer { front: true },
            Effect::PaintClear,
            Effect::PaintUndo,
        ]
    );
}

#[test]
fn messaging_helpers_record_their_effects() {
    let mut host = populated_host();
    host.chat("a").unwrap();
    host.send_global_message("b").unwrap();
    host.send_room_message("c").unwrap();
    host.send_local_msg("d").unwrap();
    host.send_susr_message("e").unwrap();
    host.send_private_message(7, "f").unwrap();
    host.status_message("g").unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::Say {
                text: "a".to_owned()
            },
            Effect::GlobalMessage {
                text: "b".to_owned()
            },
            Effect::RoomMessage {
                text: "c".to_owned()
            },
            Effect::LocalMessage {
                text: "d".to_owned()
            },
            Effect::SuperUserMessage {
                text: "e".to_owned()
            },
            Effect::PrivateMessage {
                user: 7,
                text: "f".to_owned()
            },
            Effect::StatusMessage {
                text: "g".to_owned()
            },
        ]
    );
}

#[test]
fn chat_string_is_read_then_written() {
    let mut host = populated_host();
    assert_eq!(host.get_chat_string(), "hi");
    host.delay_ticks(5).unwrap();
    assert!(host.effects.is_empty(), "DELAY blocks, it is not an effect");
    host.set_chat_string("new").unwrap();
    assert_eq!(host.view.chat_string, "new");
    assert_eq!(host.get_chat_string(), "new");
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetChatString {
            text: "new".to_owned()
        }]
    );
}

// ---------------------------------------------------------------------- alarms

#[test]
fn body_and_spot_alarms_are_recorded_and_clearable() {
    let mut host = populated_host();
    host.current_spot = 7;
    let body = Chunk::empty();
    // ALARMEXEC with spot 0 belongs to the executing hotspot.
    host.schedule_alarm(30, body.clone(), 0).unwrap();
    // A negative delay is clamped to 0; an explicit spot wins.
    host.schedule_alarm(-5, body.clone(), 5).unwrap();
    host.schedule_alarm(10, body.clone(), 0).unwrap();
    // SETALARM always makes a spot alarm, also defaulting to the current spot.
    host.set_spot_alarm(5, 60).unwrap();
    host.set_spot_alarm(0, 90).unwrap();
    host.set_spot_alarm(5, -1).unwrap();
    assert_eq!(
        host.take_alarms(),
        vec![
            PendingAlarm {
                ticks: 30,
                kind: AlarmKind::Body(body.clone()),
                spot: 7
            },
            PendingAlarm {
                ticks: 0,
                kind: AlarmKind::Body(body.clone()),
                spot: 5
            },
            PendingAlarm {
                ticks: 10,
                kind: AlarmKind::Body(body.clone()),
                spot: 7
            },
            PendingAlarm {
                ticks: 60,
                kind: AlarmKind::Spot,
                spot: 5
            },
            PendingAlarm {
                ticks: 90,
                kind: AlarmKind::Spot,
                spot: 7
            },
            PendingAlarm {
                ticks: 0,
                kind: AlarmKind::Spot,
                spot: 5
            },
        ]
    );

    host.set_spot_alarm(5, 60).unwrap();
    assert_eq!(host.alarms.len(), 1);
    host.clear_alarms().unwrap();
    assert!(host.alarms.is_empty());
    assert!(host.take_alarms().is_empty());
}

// -------------------------------------------------------------------- refusal

#[test]
fn kill_user_is_refused_by_the_trait() {
    let mut host = populated_host();
    assert_eq!(
        host.kill_user(7),
        Err(IptError::CommandUnavailable {
            command: "KILLUSER".to_owned()
        })
    );
    assert!(host.effects.is_empty(), "a refusal records nothing");
}

#[test]
fn take_effects_and_alarms_drain_without_disturbing_the_view() {
    let mut host = populated_host();
    host.chat("one").unwrap();
    let drained = host.take_effects();
    assert_eq!(drained.len(), 1);
    assert!(host.effects.is_empty(), "the buffer is left empty");
    assert_eq!(host.view.self_name, "RustProbe", "the view is untouched");
    // A second take returns nothing new.
    assert!(host.take_effects().is_empty());
}

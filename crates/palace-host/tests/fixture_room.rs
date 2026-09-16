//! Dispatch driven by a recorded server frame — Balamb Garden (room 901).
//!
//! `fixtures/logon-run1/frames/0007-server-room.bin` is a real `MSG_ROOMDESC`
//! captured from `localhost:9998`. Every hotspot in it carries a script, so
//! this is a full round trip with no network: bytes → `palace-room` → IPTSCRAE →
//! effects → protocol frames.

use std::path::PathBuf;

use palace_host::{effect_frame, Effect, HostView, ScriptEngine, ScriptEvent, WireContext};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;
use palace_wire::opcode;

fn room_901() -> palace_room::RoomDesc {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logon-run1/frames/0007-server-room.bin");
    let bytes = std::fs::read(&path).expect("recorded room frame");
    let frame = Frame::decode_from(&bytes, ByteOrder::Little).expect("frame decodes");
    assert_eq!(frame.opcode, opcode::ROOMDESC);
    palace_room::decode_payload(&frame.payload, ByteOrder::Little).expect("room decodes")
}

fn engine() -> ScriptEngine {
    let room = room_901();
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(HostView {
        self_id: 13,
        self_name: "RustProbe".to_string(),
        self_x: 256,
        self_y: 192,
        room_width: 512,
        room_height: 384,
        ..HostView::default()
    });
    engine.load_room(&room);
    engine
}

#[test]
fn every_hotspot_script_in_the_recorded_room_parses() {
    let room = room_901();
    assert_eq!(room.name, "Balamb Garden");
    assert_eq!(i32::from(room.header.room_id), 901);
    assert_eq!(room.hotspots.len(), 6);
    assert!(
        room.hotspots.iter().all(|h| h.script.is_some()),
        "the live room's hotspots all carry scripts"
    );

    let engine = engine();
    assert_eq!(engine.problems.len(), 0, "{:?}", engine.problems);
    assert_eq!(engine.scripts().len(), 6);
}

#[test]
fn entering_the_room_fires_its_on_enter_script() {
    let mut engine = engine();
    let report = engine.fire(ScriptEvent::Enter);
    assert!(report.fired());
    assert_eq!(
        report.effects,
        vec![
            Effect::MidiLoop {
                name: "garden".to_string(),
                loops: 99
            },
            Effect::PlaySound {
                name: "garden".to_string()
            },
        ]
    );
    assert_eq!(engine.pending_alarms(), 1, "the script armed a 1600-tick alarm");
}

#[test]
fn the_on_enter_alarm_runs_its_body_when_it_expires() {
    let mut engine = engine();
    engine.fire(ScriptEvent::Enter);
    assert!(engine.advance(1599).is_empty());
    let effects = engine.advance(1600);
    assert_eq!(
        effects,
        vec![
            Effect::MidiLoop {
                name: "garden".to_string(),
                loops: 99
            },
            Effect::PlaySound {
                name: "garden".to_string()
            },
        ]
    );
}

#[test]
fn clicking_the_music_notes_fires_their_select_scripts() {
    let mut engine = engine();
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(report.runs.len(), 4, "four hotspots handle SELECT");
    assert!(report.runs.iter().all(|r| r.error.is_none()), "{report:?}");

    assert!(report.effects.contains(&Effect::MidiStop));
    assert!(report.effects.iter().any(|e| matches!(
        e,
        Effect::MidiLoop { name, loops } if name == "garden" && *loops == 99
    )));
    assert!(report.effects.iter().any(|e| matches!(
        e,
        Effect::GotoUrl { url } if url.contains("psycrow.chatserve.com")
    )));
    assert!(report.effects.iter().any(|e| matches!(
        e,
        Effect::LocalMessage { text } if text.starts_with("Opening http")
    )));
    assert!(report.effects.iter().any(|e| matches!(
        e,
        Effect::LocalMessage { text } if text.starts_with("Click the music note")
    )));
}

#[test]
fn leaving_the_room_fires_its_on_leave_script() {
    let mut engine = engine();
    let report = engine.fire(ScriptEvent::Leave);
    assert!(report.fired());
    assert_eq!(
        report.effects,
        vec![Effect::PaintClear, Effect::ClearLooseProps],
        "0 users in the room is < 2"
    );
}

#[test]
fn a_click_is_hit_tested_then_dispatched() {
    let room = room_901();
    let mut view = HostView::default();
    view.apply_room(&room);

    let spot = view
        .spot_at(361, 114)
        .or_else(|| view.spot_at(view.spots[1].loc.0, view.spots[1].loc.1))
        .expect("a hotspot covers the note bar");
    assert!(view.spot(spot.id).is_some());

    let mut engine = engine();
    let report = engine.fire_spot(ScriptEvent::Select, spot.id);
    assert!(report.fired(), "the hit hotspot declares ON SELECT");
    assert!(!report.effects.is_empty());

    assert!(
        view.spot_at(-100, -100).is_none(),
        "a point outside the room hits nothing"
    );
}

#[test]
fn a_script_effect_becomes_the_protocol_frame_it_should() {
    let context = WireContext {
        byte_order: ByteOrder::Little,
        user_id: 13,
        room_id: 901,
        room_width: 512,
        room_height: 384,
        self_pos: (256, 192),
        pen: palace_host::PenState::default(),
    };
    let goto = effect_frame(&Effect::GotoRoom { room: 817 }, &context).expect("navR");
    assert_eq!(goto.opcode, opcode::ROOMGOTO);
    assert!(!goto.encode(ByteOrder::Little).expect("encodes").is_empty());

    assert!(
        effect_frame(
            &Effect::GotoUrl {
                url: "http://psycrow.chatserve.com/music.html".to_string()
            },
            &context
        )
        .is_none(),
        "GOTOURL stays a local effect, it is not a wire message"
    );
}

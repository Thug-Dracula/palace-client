//! Regression: a repeated room description must not cancel scheduled work.
//!
//! A pserver re-sends a room description whenever the room's state changes
//! (another user arrives, a spot changes, the room is edited). The client
//! runtime loads a room's scripts for every description, and the reference
//! client keeps already-armed timers across that re-description: the lifecycle
//! (`ON ENTER`) does not run again, so anything it scheduled with `ALARMEXEC` /
//! `SETALARM` must survive. A real room depends on this — the Colosseum's gate
//! arms `{ c1hplay GLOBAL 2 c1hplay = } 10 ALARMEXEC` on entry and then updates a
//! spot, which makes the server re-describe the room within the alarm's window.

use palace_host::{Effect, ScriptEngine, ScriptEvent};
use palace_room::{Hotspot, RoomDesc};
use palace_wire::messages::RoomRec;

fn room(id: i16, scripts: &[(i16, &str)]) -> RoomDesc {
    let hotspots: Vec<Hotspot> = scripts
        .iter()
        .map(|(id, source)| Hotspot {
            id: *id,
            state: 0,
            script: Some((*source).to_string()),
            ..Hotspot::default()
        })
        .collect();
    RoomDesc {
        header: RoomRec {
            room_id: id,
            nbr_hotspots: hotspots.len() as i16,
            ..RoomRec::default()
        },
        name: format!("Room {id}"),
        picture: String::new(),
        artist: String::new(),
        password: String::new(),
        pictures: Vec::new(),
        hotspots,
        loose_props: Vec::new(),
        draw_cmds: Vec::new(),
        var_data: Vec::new(),
        trailing_len: 0,
        warnings: Vec::new(),
    }
}

/// An `ALARMEXEC` body armed in `ON ENTER` runs even after the server
/// re-describes the room it was armed in.
#[test]
fn a_redescription_keeps_an_alarmexec_body_armed() {
    let desc = room(32000, &[(1, "ON ENTER { { \"later\" SAY } 30 ALARMEXEC }")]);
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&desc);

    assert!(engine.fire(ScriptEvent::Enter).effects.is_empty());
    assert_eq!(
        engine.pending_alarms(),
        1,
        "ON ENTER armed one ALARMEXEC body"
    );

    // The server re-sends the room description: same room, no re-ENTRY.
    engine.load_room(&desc);
    assert_eq!(
        engine.pending_alarms(),
        1,
        "a repeated description of the same room must not cancel the alarm"
    );

    let effects = engine.advance(30);
    assert_eq!(
        effects,
        vec![Effect::Say {
            text: "later".to_string()
        }],
        "the armed body still ran when the clock reached it"
    );
    assert_eq!(engine.pending_alarms(), 0);
}

/// The same protection covers `SETALARM`, which arms a hotspot's `ON ALARM`.
#[test]
fn a_redescription_keeps_a_spot_alarm_armed() {
    let desc = room(
        32000,
        &[
            (1, "ON ENTER { 60 2 SETALARM }"),
            (2, "ON ALARM { \"alarm\" SAY }"),
        ],
    );
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&desc);
    engine.fire(ScriptEvent::Enter);
    assert_eq!(engine.pending_alarms(), 1);

    engine.load_room(&desc);
    assert_eq!(
        engine.pending_alarms(),
        1,
        "a repeated description must not cancel the spot alarm"
    );

    let effects = engine.advance(60);
    assert_eq!(
        effects,
        vec![Effect::Say {
            text: "alarm".to_string()
        }]
    );
}

/// Entering a *different* room makes the old room's timers meaningless, so they
/// are still dropped.
#[test]
fn a_new_room_still_clears_the_old_rooms_alarms() {
    let first = room(32000, &[(1, "ON ENTER { { \"old\" SAY } 30 ALARMEXEC }")]);
    let second = room(32001, &[(1, "ON ENTER { \"new\" STATUSMSG }")]);

    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&first);
    engine.fire(ScriptEvent::Enter);
    assert_eq!(engine.pending_alarms(), 1);

    engine.load_room(&second);
    assert_eq!(
        engine.pending_alarms(),
        0,
        "leaving the room discards its pending alarms"
    );
    assert!(engine.advance(30).is_empty());
}

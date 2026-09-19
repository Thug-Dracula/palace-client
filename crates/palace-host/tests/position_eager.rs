//! What a script command changes must be visible to a later read in the same
//! handler, exactly as the reference client applies it.
//!
//! PalaceChat applies a user move to `currentUser` before returning from
//! `PalaceClient.move` (`OpenPalace/PalaceClient/src/net/codecomposer/palace/rpc/PalaceClient.as:548-568`),
//! so a command later in the same handler that reads `POSX`/`POSY` sees the new
//! coordinates. By contrast `SETSPOTSTATE` only sends to the server
//! (`PalaceClient.setSpotState`, `rpc/PalaceClient.as:689-699`); the local
//! hotspot does not change until the server broadcast arrives, so a read later
//! in the handler still sees the old state. These tests pin both against a
//! synthetic room, with no network and no server.

use palace_host::{Effect, HostView, ScriptEngine, ScriptEvent, SpotView};
use palace_room::{Hotspot, RoomDesc};
use palace_wire::messages::RoomRec;

fn room_with(scripts: &[(i16, &str)]) -> RoomDesc {
    let hotspots: Vec<Hotspot> = scripts
        .iter()
        .map(|(id, source)| Hotspot {
            id: *id,
            script: Some((*source).to_string()),
            ..Hotspot::default()
        })
        .collect();
    RoomDesc {
        header: RoomRec {
            room_id: 32001,
            nbr_hotspots: hotspots.len() as i16,
            ..RoomRec::default()
        },
        name: "Test Room".to_string(),
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

/// The same 512x384 view the smoke room reports, so the movement clamp is the
/// one the wire path uses.
fn engine_for(scripts: &[(i16, &str)]) -> ScriptEngine {
    let room = room_with(scripts);
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(HostView {
        self_id: 13,
        self_name: "Smoke".to_string(),
        self_x: 0,
        self_y: 0,
        room_id: 32001,
        room_name: "Test Room".to_string(),
        room_width: 512,
        room_height: 384,
        spots: room.hotspots.iter().map(SpotView::from_hotspot).collect(),
        ..HostView::default()
    });
    engine.load_room(&room);
    engine
}

/// `200 150 SETPOS` then `POSX`/`POSY`: the reads see the new position, and the
/// move is still recorded for the wire.
#[test]
fn setpos_is_visible_to_a_read_in_the_same_handler() {
    let mut engine = engine_for(&[(
        7,
        "ON SELECT { 200 150 SETPOS \
         \"POSX=\" POSX ITOA & SAY \"POSY=\" POSY ITOA & SAY }",
    )]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![
            Effect::MoveUserAbs { x: 200, y: 150 },
            Effect::Say {
                text: "POSX=200".to_string()
            },
            Effect::Say {
                text: "POSY=150".to_string()
            },
        ],
        "the position read back inside the handler that set it"
    );
}

/// `MOVE` is relative to the position a `SETPOS` in the same handler left
/// (`PalaceController.as:286-289`), so the read sees the sum.
#[test]
fn move_is_relative_to_the_setpos_in_the_same_handler() {
    let mut engine = engine_for(&[(
        7,
        "ON SELECT { 200 150 SETPOS 10 20 MOVE \"POSX=\" POSX ITOA & SAY }",
    )]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![
            Effect::MoveUserAbs { x: 200, y: 150 },
            Effect::MoveUserRel { dx: 10, dy: 20 },
            Effect::Say {
                text: "POSX=210".to_string()
            },
        ],
        "the relative move starts from the position just set"
    );
}

/// The eager snapshot is clamped exactly as the wire frame is, so the value a
/// script reads is the value the move comes to rest at.
#[test]
fn the_eager_position_is_clamped_like_the_frame() {
    let mut engine = engine_for(&[(7, "ON SELECT { 9000 -50 SETPOS \"POSX=\" POSX ITOA & SAY }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![
            Effect::MoveUserAbs { x: 9000, y: -50 },
            Effect::Say {
                text: "POSX=490".to_string()
            },
        ],
        "the read reports the clamped resting position, while the effect stays raw"
    );
}

/// `SETSPOTSTATE` is send-only in the reference, so a `GETSPOTSTATE` later in the
/// same handler still sees the pre-set state. This mirrors the fresh-room
/// `IPT|GETSPOTSTATE|1 initial|0` / `IPT|SETSPOTSTATE|7 1 now|0` reference lines:
/// both read the room's state, neither is pushed by the command.
#[test]
fn setspotstate_does_not_change_the_local_state_seen_in_the_same_handler() {
    let mut engine = engine_for(&[(
        7,
        "ON SELECT { 7 1 SETSPOTSTATE \"now=\" 1 GETSPOTSTATE ITOA & SAY }",
    )]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![
            Effect::SetSpotState { spot: 1, state: 7 },
            Effect::Say {
                text: "now=0".to_string()
            },
        ],
        "the command sends to the server but leaves the local state unchanged"
    );
}

/// `SETSPOTSTATELOCAL` is the opposite: `PalaceController.setSpotStateLocal`
/// calls `hotspot.changeState(state)` immediately (`iptscrae/PalaceController.as:329-337`)
/// and `getSpotState` reads that same `hotspot.state`
/// (`iptscrae/PalaceController.as:291-297`), so a `GETSPOTSTATE` later in the
/// same handler returns the state just set. This is the room 32009 reference
/// line `IPT|SETSPOTSTATELOCAL|7 1|7`.
#[test]
fn setspotstatelocal_is_visible_to_a_read_in_the_same_handler() {
    let mut engine = engine_for(&[(
        1,
        "ON SELECT { 7 1 SETSPOTSTATELOCAL \"local=\" 1 GETSPOTSTATE ITOA & SAY }",
    )]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![
            Effect::SetSpotStateLocal { spot: 1, state: 7 },
            Effect::Say {
                text: "local=7".to_string()
            },
        ],
        "the local spot state is readable before the handler returns"
    );
}

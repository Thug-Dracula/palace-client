//! Hotspot `ON SELECT` handlers that pick a destination from the pointer x.
//!
//! Colosseum 7774's arena menu is the reference case: `x GLOBAL MOUSEPOS SWAP x
//! =` stores the click's horizontal room coordinate in the global `x`, and a
//! chain of `x <lo> > x <hi> < AND IF` branches turns it into a `GOTOROOM`.
//!
//! These tests pin the two halves of that idiom with the real host:
//!
//! * `MOUSEPOS` pushes x then y, so the `SWAP` leaves x on top and `x =` stores
//!   the horizontal coordinate (not y, and not zero);
//! * an ungated chain reaches its band, while the live room's
//!   `USERNAME cname == in69 0 == NOT AND` guard keeps the chain from running
//!   until the player is enrolled, falling back to `wrongo.wav` instead.

use palace_host::{HostView, ScriptEngine, ScriptEvent};
use palace_room::{Hotspot, RoomDesc};
use palace_wire::messages::RoomRec;

fn room_with(script: &str) -> RoomDesc {
    let hotspot = Hotspot {
        id: 102,
        script: Some(script.to_string()),
        ..Hotspot::default()
    };
    RoomDesc {
        header: RoomRec {
            room_id: 7774,
            nbr_hotspots: 1,
            ..RoomRec::default()
        },
        name: "Selection".to_string(),
        picture: String::new(),
        artist: String::new(),
        password: String::new(),
        pictures: Vec::new(),
        hotspots: vec![hotspot],
        loose_props: Vec::new(),
        draw_cmds: Vec::new(),
        var_data: Vec::new(),
        trailing_len: 0,
        warnings: Vec::new(),
    }
}

fn engine_for(mouse: (i32, i32), script: &str) -> ScriptEngine {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(HostView {
        self_id: 13,
        self_name: "ArenaTester".to_string(),
        room_id: 7774,
        room_width: 512,
        room_height: 384,
        mouse,
        ..HostView::default()
    });
    engine.load_room(&room_with(script));
    engine
}

fn effects(mouse: (i32, i32), seed: Option<&str>, script: &str) -> Vec<String> {
    let mut engine = engine_for(mouse, script);
    if let Some(seed) = seed {
        engine.run_source(seed).expect("the seed runs");
    }
    engine
        .fire_spot(ScriptEvent::Select, 102)
        .effects
        .iter()
        .map(|effect| effect.to_string())
        .collect()
}

const MENU_FROM_POINTER: &str = r#"ON SELECT { x GLOBAL MOUSEPOS SWAP x = x ITOA SAY }"#;

#[test]
fn mousepos_swap_assign_stores_the_horizontal_coordinate() {
    assert_eq!(
        effects((278, 375), None, MENU_FROM_POINTER),
        vec!["SAY \"278\"".to_string()],
        "x must be the click's x, not its y"
    );
    assert_eq!(
        effects((137, 368), None, MENU_FROM_POINTER),
        vec!["SAY \"137\"".to_string()]
    );
    assert_eq!(
        effects((0, 0), None, MENU_FROM_POINTER),
        vec!["SAY \"0\"".to_string()],
        "a zero pointer must reach x as zero, not as a missing value"
    );
}

const UNGATED_CHAIN: &str = r#"ON SELECT {
x GLOBAL MOUSEPOS SWAP x =
{ 31743 GOTOROOM } x 78 < IF
{ 31741 GOTOROOM } x 78 > x 120 < AND IF
{ 31746 GOTOROOM } x 120 > x 170 < AND IF
{ 31747 GOTOROOM } x 179 > x 219 < AND IF
{ 31748 GOTOROOM } x 219 > x 271 < AND IF
{ 31000 GOTOROOM } x 271 > x 320 < AND IF
{ 31001 GOTOROOM } x 320 > x 383 < AND IF
}"#;

#[test]
fn an_ungated_region_chain_reaches_its_band() {
    assert_eq!(
        effects((278, 375), None, UNGATED_CHAIN),
        vec!["GOTOROOM 31000".to_string()],
        "x 271..320 is the 31000 band"
    );
    assert_eq!(
        effects((295, 373), None, UNGATED_CHAIN),
        vec!["GOTOROOM 31000".to_string()]
    );
}

const LIVE_GUARDED_CHAIN: &str = r#"ON SELECT {
cname GLOBAL
in69 GLOBAL
x GLOBAL MOUSEPOS SWAP x =
{ 31743 GOTOROOM } x 94 < IF
{
x GLOBAL MOUSEPOS SWAP x =
{ 31741 GOTOROOM } x 93 > x 178 < AND IF
{ 31746 GOTOROOM } x 177 > x 259 < AND IF
{ 31747 GOTOROOM } x 258 > x 342 < AND IF
{ 31748 GOTOROOM } x 341 > x 421 < AND IF
{ 31749 GOTOROOM } x 420 > x 511 < AND IF
}
{ "wrongo.wav" SOUND } USERNAME cname == in69 0 == NOT AND IFELSE
}"#;

#[test]
fn the_enrolled_player_reaches_the_band() {
    let seed = "USERNAME cname GLOBAL cname = 1 in69 GLOBAL in69 =";
    assert_eq!(
        effects((278, 375), Some(seed), LIVE_GUARDED_CHAIN),
        vec!["GOTOROOM 31747".to_string()]
    );
    assert_eq!(
        effects((137, 368), Some(seed), LIVE_GUARDED_CHAIN),
        vec!["GOTOROOM 31741".to_string()]
    );
}

#[test]
fn an_unenrolled_clicker_gets_the_rooms_fallback() {
    for mouse in [(278, 375), (295, 373), (137, 368)] {
        assert_eq!(
            effects(mouse, None, LIVE_GUARDED_CHAIN),
            vec!["SOUND \"wrongo.wav\"".to_string()],
            "the region chain is unreachable until cname/in69 are set"
        );
    }
}

/// Room 31743's Audience hotspot: `cname GLOBAL USERNAME cname = 1 in69 GLOBAL in69 =`.
const ENROLL_ON_LEAVE: &str =
    r#"ON LEAVE { CLEARLOOSEPROPS cname GLOBAL USERNAME cname = 1 in69 GLOBAL in69 = }"#;

fn enrollment_room() -> RoomDesc {
    let hotspot = Hotspot {
        id: 1,
        script: Some(ENROLL_ON_LEAVE.to_string()),
        ..Hotspot::default()
    };
    RoomDesc {
        header: RoomRec {
            room_id: 31743,
            nbr_hotspots: 1,
            ..RoomRec::default()
        },
        name: "<[ Colosseum Menu ]>".to_string(),
        picture: String::new(),
        artist: String::new(),
        password: String::new(),
        pictures: Vec::new(),
        hotspots: vec![hotspot],
        loose_props: Vec::new(),
        draw_cmds: Vec::new(),
        var_data: Vec::new(),
        trailing_len: 0,
        warnings: Vec::new(),
    }
}

fn view_for(room_id: i32, mouse: (i32, i32)) -> HostView {
    HostView {
        self_id: 13,
        self_name: "ArenaTester".to_string(),
        room_id,
        room_width: 512,
        room_height: 384,
        mouse,
        ..HostView::default()
    }
}

#[test]
fn a_global_set_in_room_a_is_readable_after_moving_to_room_b() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(view_for(31743, (0, 0)));
    engine.load_room(&enrollment_room());
    let leave = engine.fire(ScriptEvent::Leave);
    assert!(
        leave.runs.iter().all(|run| run.error.is_none()),
        "room 31743's LEAVE enrolls cleanly: {:?}",
        leave.runs
    );

    engine.set_view(view_for(7774, (0, 0)));
    engine.load_room(&room_with(r#"ON SELECT { cname GLOBAL cname SAY }"#));
    assert_eq!(
        effects_after(&mut engine, ScriptEvent::Select),
        vec!["SAY \"ArenaTester\"".to_string()],
        "the global cname survives the room change"
    );
}

/// The exact arena failure: enroll in room 31743, load and fetch in room 7774,
/// then run the guarded `ON SELECT`. The fetched body must not drop the globals.
#[test]
fn enrollment_in_room_a_survives_room_b_and_its_fetched_interface() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(view_for(31743, (0, 0)));
    engine.load_room(&enrollment_room());
    let leave = engine.fire(ScriptEvent::Leave);
    assert!(
        leave.runs.iter().all(|run| run.error.is_none()),
        "room 31743's LEAVE enrolls cleanly: {:?}",
        leave.runs
    );

    engine.set_view(view_for(7774, (278, 375)));
    engine.load_room(&room_with(LIVE_GUARDED_CHAIN));
    let fetched =
        engine.execute_fetched_source("but1 GLOBAL but2 GLOBAL \"cust.gif\" but2 =", 7774);
    assert!(
        fetched.error.is_none(),
        "room 7774's fetched interface runs: {:?}",
        fetched.error
    );

    assert_eq!(
        effects_after(&mut engine, ScriptEvent::Select),
        vec!["GOTOROOM 31747".to_string()],
        "the enrolled player reaches the band, not the room's fallback"
    );
}

fn effects_after(engine: &mut ScriptEngine, event: ScriptEvent) -> Vec<String> {
    engine
        .fire_spot(event, 102)
        .effects
        .iter()
        .map(|effect| effect.to_string())
        .collect()
}

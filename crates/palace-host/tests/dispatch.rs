//! Event dispatch on synthetic rooms — no network, no server.

use palace_host::{Effect, HostView, ScriptEngine, ScriptEvent};
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
            room_id: 901,
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

fn engine_for(scripts: &[(i16, &str)]) -> ScriptEngine {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.set_view(HostView {
        self_id: 13,
        self_name: "RustProbe".to_string(),
        room_id: 901,
        room_name: "Test Room".to_string(),
        room_width: 512,
        room_height: 384,
        ..HostView::default()
    });
    engine.load_room(&room_with(scripts));
    engine
}

#[test]
fn a_select_handler_fires_and_says() {
    let mut engine = engine_for(&[(7, "ON SELECT { \"hello\" SAY }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert!(report.fired());
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].spot, 7);
    assert!(report.runs[0].error.is_none());
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "hello".to_string()
        }]
    );
}

#[test]
fn a_handler_for_another_event_does_not_fire() {
    let mut engine = engine_for(&[(7, "ON SELECT { \"hello\" SAY }")]);
    let report = engine.fire(ScriptEvent::Enter);
    assert!(!report.fired());
    assert!(report.effects.is_empty());
}

#[test]
fn outchat_can_clear_the_outgoing_text() {
    let mut engine = engine_for(&[(1, "ON OUTCHAT { \"\" CHATSTR = }")]);
    engine.host_mut().view.chat_string = "secret".to_string();
    let report = engine.fire(ScriptEvent::OutChat);
    assert_eq!(report.chat_string.as_deref(), Some(""));
}

#[test]
fn inchat_can_rewrite_the_incoming_text() {
    let mut engine = engine_for(&[(1, "ON INCHAT { CHATSTR \"!\" + CHATSTR = }")]);
    engine.host_mut().view.chat_string = "hi".to_string();
    let report = engine.fire(ScriptEvent::InChat);
    assert_eq!(report.chat_string.as_deref(), Some("hi!"));
}

#[test]
fn every_script_with_the_handler_runs() {
    let mut engine = engine_for(&[
        (1, "ON ENTER { 1 x GLOBAL x = }"),
        (2, "ON ENTER { 2 y GLOBAL y = }"),
        (3, "ON SELECT { 3 z GLOBAL z = }"),
    ]);
    let report = engine.fire(ScriptEvent::Enter);
    assert_eq!(report.runs.len(), 2, "{:?}", report.runs);
    assert!(report.runs.iter().all(|r| r.error.is_none()));
}

#[test]
fn dest_reads_the_executing_hotspot() {
    let mut engine = ScriptEngine::with_palace_limits();
    let mut room = room_with(&[(5, "ON SELECT { DEST SELECT }")]);
    room.hotspots[0].dest = 42;
    engine.set_view(HostView {
        room_id: 901,
        ..HostView::default()
    });
    engine.load_room(&room);
    let report = engine.fire_spot(ScriptEvent::Select, 5);
    assert_eq!(report.effects, vec![Effect::SelectSpot { spot: 42 }]);
}

#[test]
fn a_clicked_doors_script_reaches_its_destination_room() {
    let mut engine = ScriptEngine::with_palace_limits();
    let mut room = room_with(&[(5, "ON SELECT { DEST GOTOROOM }")]);
    room.hotspots[0].hotspot_type = 1;
    room.hotspots[0].dest = 161;
    engine.set_view(HostView {
        room_id: 901,
        ..HostView::default()
    });
    engine.load_room(&room);
    let report = engine.fire_spot(ScriptEvent::Select, 5);
    assert_eq!(
        report.effects,
        vec![Effect::GotoRoom { room: 161 }],
        "the clicked door's DEST must be its destination room"
    );
}

#[test]
fn alarmexec_schedules_a_body_that_runs_later() {
    let mut engine = engine_for(&[(1, "ON ENTER { { \"later\" SAY } 30 ALARMEXEC }")]);
    let enter = engine.fire(ScriptEvent::Enter);
    assert!(enter.effects.is_empty(), "the alarm has not expired");
    assert_eq!(engine.pending_alarms(), 1);

    assert!(engine.advance(29).is_empty());
    assert_eq!(engine.pending_alarms(), 1);

    let effects = engine.advance(30);
    assert_eq!(
        effects,
        vec![Effect::Say {
            text: "later".to_string()
        }]
    );
    assert_eq!(engine.pending_alarms(), 0);
}

#[test]
fn setalarm_fires_the_named_spots_alarm_handler() {
    let mut engine = engine_for(&[
        (1, "ON ENTER { 60 2 SETALARM }"),
        (2, "ON ALARM { \"alarm\" SAY }"),
    ]);
    engine.fire(ScriptEvent::Enter);
    assert_eq!(engine.pending_alarms(), 1);
    let effects = engine.advance(60);
    assert_eq!(
        effects,
        vec![Effect::Say {
            text: "alarm".to_string()
        }]
    );
}

#[test]
fn a_faulting_handler_is_reported_not_swallowed() {
    let mut engine = engine_for(&[(1, "ON SELECT { 1 0 / POP }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert!(report.fired());
    assert!(
        report.runs[0].error.is_none(),
        "division by zero is defined"
    );

    let mut engine = engine_for(&[(1, "ON SELECT { 1 2 & }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.errors().len(),
        1,
        "ampersand on two numbers must fault: {:?}",
        report.runs
    );
    assert!(report.errors()[0].error.is_some());
}

#[test]
fn an_unparseable_script_is_reported() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with(&[(9, "ON SELECT { $1 }")]));
    assert_eq!(engine.problems.len(), 1);
    assert_eq!(engine.problems[0].spot, 9);
    assert!(engine.scripts().is_empty());
}

#[test]
fn unimplemented_commands_are_recorded_not_dropped() {
    let mut engine = engine_for(&[(1, "ON SELECT { PING }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert!(report.fired());
    assert!(
        report.effects.iter().any(|effect| matches!(
            effect,
            Effect::Unsupported { command } if command == "PING"
        )),
        "{:?}",
        report.effects
    );
}

#[test]
fn globals_survive_between_handlers() {
    let mut engine = engine_for(&[
        (1, "ON ENTER { 41 answer GLOBAL answer = }"),
        (2, "ON SELECT { answer GLOBAL answer 1 + ITOA SAY }"),
    ]);
    engine.fire(ScriptEvent::Enter);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "42".to_string()
        }]
    );
}

#[test]
fn a_hostile_script_cannot_hang_dispatch() {
    let mut engine = engine_for(&[(1, "ON SELECT { { 1 } { 1 } WHILE }")]);
    let report = engine.fire(ScriptEvent::Select);
    assert_eq!(report.errors().len(), 1, "the loop cap must stop it");
}

#[test]
fn spot_hit_testing_uses_the_decoded_room() {
    let mut room = room_with(&[(7, "ON SELECT { \"hit\" SAY }")]);
    room.hotspots[0].loc = palace_wire::messages::Point::new(100, 200);
    room.hotspots[0].points = vec![
        palace_wire::messages::Point::new(80, 180),
        palace_wire::messages::Point::new(80, 220),
        palace_wire::messages::Point::new(120, 220),
        palace_wire::messages::Point::new(120, 180),
    ];
    let mut view = HostView::default();
    view.apply_room(&room);
    assert_eq!(view.spot_at(200, 100).map(|s| s.id), Some(7), "inside");
    assert_eq!(view.spot_at(0, 0).map(|s| s.id), None, "outside");
}

// ------------------------------------------------------- LOADSCRIPT / HTTPGET

#[test]
fn loadscript_records_a_fetch_scoped_to_the_executing_spot() {
    let mut engine = engine_for(&[(7, "ON SELECT { \"custo2.txt\" LOADSCRIPT }")]);
    let report = engine.fire_spot(ScriptEvent::Select, 7);
    assert!(report.runs[0].error.is_none());
    assert_eq!(
        report.effects,
        vec![Effect::FetchScript {
            url: "custo2.txt".to_string(),
            spot: 7,
        }]
    );
}

#[test]
fn httpget_from_a_room_level_handler_is_scoped_to_room_zero() {
    let mut engine = engine_for(&[(0, "ON ROOMREADY { \"ludo/\" HTTPGET }")]);
    let report = engine.fire(ScriptEvent::RoomReady);
    assert!(report.fired());
    assert_eq!(
        report.effects,
        vec![Effect::FetchScript {
            url: "ludo/".to_string(),
            spot: 0,
        }]
    );
}

#[test]
fn a_fetched_source_executes_and_its_definitions_answer_httpreceived() {
    let mut engine = engine_for(&[(
        9,
        "ON HTTPRECEIVED { cname GOTPROPS == { \"defined\" SAY } { \"undefined\" SAY } IF }",
    )]);
    let run = engine.execute_fetched_source(
        "\"Gogo\" cname = { 14109 GOTOROOM } cname \"Cyan\" == IF",
        9,
    );
    assert!(
        run.error.is_none(),
        "fetched source runs clean: {:?}",
        run.error
    );
    let report = engine.fire_spot(ScriptEvent::HttpReceived, 9);
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "defined".to_string()
        }],
        "the fetched definition is callable from the response handler"
    );
}

#[test]
fn a_fetched_global_survives_until_the_response_dispatch() {
    let mut engine = engine_for(&[(
        9,
        "ON HTTPRECEIVED { gotprops 1 == { \"defined\" SAY } { \"undefined\" SAY } IF }",
    )]);
    let run = engine.execute_fetched_source("\"Gogo\" cname = 1 gotprops =", 9);
    assert!(
        run.error.is_none(),
        "fetched source runs clean: {:?}",
        run.error
    );
    let report = engine.fire_spot(ScriptEvent::HttpReceived, 9);
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "defined".to_string()
        }],
        "a global the fetched body assigned is readable in the response dispatch"
    );
}

#[test]
fn a_malformed_fetched_source_faults_inside_the_run_report() {
    let mut engine = engine_for(&[(3, "ON HTTPRECEIVED { }")]);
    let run = engine.execute_fetched_source("{ this is not iptscrae", 3);
    assert!(run.error.is_some(), "the parse failure is reported");
    assert_eq!(run.spot, 3);
    let report = engine.fire_spot(ScriptEvent::HttpReceived, 3);
    assert!(
        !report.fired() || report.runs.iter().all(|r| r.error.is_none()),
        "the engine is still alive afterwards"
    );
}

#[test]
fn a_hostile_fetched_source_is_stopped_by_the_budget() {
    let mut engine = engine_for(&[(3, "ON HTTPRECEIVED { }")]);
    let run = engine.execute_fetched_source("{ 1 } { 1 } WHILE", 3);
    assert!(
        run.error.is_some(),
        "the step budget must stop an infinite fetched loop"
    );
}

#[test]
fn an_httperror_handler_receives_the_failure_dispatch() {
    let mut engine = engine_for(&[(5, "ON HTTPERROR { \"handled\" SAY }")]);
    let report = engine.fire_spot(ScriptEvent::HttpError, 5);
    assert!(report.fired(), "the spot-scoped error handler runs");
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "handled".to_string()
        }]
    );
    let other = engine.fire_spot(ScriptEvent::HttpError, 6);
    assert!(!other.fired(), "a different spot's handler does not run");
}

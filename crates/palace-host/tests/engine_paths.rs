//! Engine paths the event-dispatch tests do not reach: the script box
//! (`run_source`), cyborg loading, room teardown, and the alarm queue cap.

mod common;

use common::*;
use palace_host::{Effect, ScriptEngine, ScriptEvent, MAX_PENDING_ALARMS};

#[test]
fn run_source_returns_effects_and_reports_syntax_errors() {
    let mut engine = ScriptEngine::with_palace_limits();
    let run = engine.run_source("\"hi\" SAY").expect("a bare body parses");
    assert_eq!(
        run.effects,
        vec![Effect::Say {
            text: "hi".to_owned()
        }]
    );
    assert_eq!(run.spot, 0, "the script box runs as cyborg");
    assert!(run.error.is_none());

    let error = engine.run_source("$1").expect_err("$ is not a token");
    assert!(!error.is_empty());

    let fault = engine
        .run_source("1 2 &")
        .expect_err("two non-strings fault at run time");
    assert!(!fault.is_empty());
}

#[test]
fn host_accessor_exposes_the_session_host() {
    let engine = ScriptEngine::with_palace_limits();
    assert_eq!(engine.host().tick, 0);
    assert_eq!(engine.host().view.self_id, 0);
}

#[test]
fn clear_room_forgets_scripts_and_handlers() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[(1, Some("ON SELECT { 1 NOPE }"))]));
    assert!(engine.has_handler(ScriptEvent::Select));
    assert!(!engine.has_handler(ScriptEvent::Enter));
    assert_eq!(engine.scripts().len(), 1);
    engine.clear_room();
    assert!(engine.scripts().is_empty());
    assert!(!engine.has_handler(ScriptEvent::Select));
}

#[test]
fn load_cyborg_adds_a_spotless_script_or_a_problem() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_cyborg("ON ENTER { 1 NOPE }");
    assert_eq!(engine.scripts().len(), 1);
    assert_eq!(
        engine.scripts()[0].spot,
        0,
        "a cyborg script has no hotspot"
    );
    assert!(engine.problems.is_empty());

    engine.load_cyborg("1 2 +");
    assert_eq!(engine.problems.len(), 1);
    assert_eq!(engine.problems[0].spot, 0);
}

#[test]
fn set_spot_script_reparses_one_hotspot_and_its_handler_fires() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[(1, Some("ON ENTER { \"old\" SAY }"))]));
    assert!(!engine.has_handler(ScriptEvent::Select));

    let problem = engine.set_spot_script(1, "ON SELECT { \"hi\" SAY }");
    assert!(problem.is_none(), "the merged source parses");
    assert!(engine.has_handler(ScriptEvent::Select));

    let report = engine.fire_spot(ScriptEvent::Select, 1);
    assert_eq!(report.runs.len(), 1, "the merged handler ran");
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "hi".to_owned()
        }]
    );
}

#[test]
fn set_spot_script_replaces_the_existing_script_for_the_same_spot() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[(
        1,
        Some("ON SELECT { \"old\" SAY }"),
    )]));

    let problem = engine.set_spot_script(1, "ON SELECT { \"new\" SAY }");
    assert!(problem.is_none());
    assert_eq!(
        engine.scripts().len(),
        1,
        "the spot's script is replaced, not appended"
    );
    let report = engine.fire_spot(ScriptEvent::Select, 1);
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "new".to_owned()
        }]
    );
}

#[test]
fn set_spot_script_reports_a_source_with_no_handlers() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[(1, Some("ON ENTER { 1 NOPE }"))]));

    let problem = engine.set_spot_script(1, "1 2 +");
    assert_eq!(
        problem.map(|problem| problem.spot),
        Some(1),
        "a bare body is reported against the spot and leaves its script alone"
    );
    assert!(
        engine.has_handler(ScriptEvent::Enter),
        "the previous script for the spot is kept"
    );
}

#[test]
fn the_alarm_queue_is_capped_and_counts_what_it_drops() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[(
        1,
        Some("ON ENTER { 0 i = { { 1 } 10 ALARMEXEC i ++ } { i 300 < } WHILE }"),
    )]));
    engine.fire(ScriptEvent::Enter);
    assert_eq!(engine.pending_alarms(), MAX_PENDING_ALARMS);
    assert_eq!(
        engine.alarms_dropped,
        300 - MAX_PENDING_ALARMS as u64,
        "the excess is reported, not silently discarded"
    );
}

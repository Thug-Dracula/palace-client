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
        .run_source("1 \"x\" &")
        .expect_err("mixed operands fault at run time");
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

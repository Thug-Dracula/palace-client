//! Dispatch semantics matched against the reference client.

mod common;

use common::room_with_scripts;
use palace_host::{Effect, ScriptEngine, ScriptEvent};

#[test]
fn hotspots_run_first_to_last_then_the_cyborg() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[
        (1, Some("ON ENTER { 1 POP }")),
        (2, Some("ON ENTER { 2 POP }")),
        (3, Some("ON ENTER { 3 POP }")),
    ]));
    engine.load_cyborg("ON ENTER { 0 POP }");
    let report = engine.fire(ScriptEvent::Enter);
    let spots: Vec<i32> = report.runs.iter().map(|run| run.spot).collect();
    assert_eq!(
        spots,
        vec![1, 2, 3, 0],
        "PalaceChat's triggerHotspotEvents walks hotSpots first to last, then the cyborg: {report:?}"
    );
}

#[test]
fn a_later_handler_sees_the_state_an_earlier_one_set() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[
        (
            1,
            Some("ON ENTER { cplace GLOBAL 1 cplace = \"placed\" SAY }"),
        ),
        (
            69,
            Some("ON ENTER { cplace GLOBAL { \"audience\" SAY } cplace 0 == IF }"),
        ),
    ]));
    let report = engine.fire(ScriptEvent::Enter);
    let spots: Vec<i32> = report.runs.iter().map(|run| run.spot).collect();
    assert_eq!(
        spots,
        vec![1, 69],
        "hotspots run in room order, so the gate's state is set before the audience check reads it: {report:?}"
    );
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "placed".to_owned()
        }],
        "the audience check must see the state the team gate set: {report:?}"
    );
}

#[test]
fn a_fault_aborts_the_rest_of_the_dispatch() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[
        (1, Some("ON ENTER { \"one\" SAY }")),
        (2, Some("ON ENTER { 1 2 3 GET }")),
        (3, Some("ON ENTER { \"three\" SAY }")),
    ]));
    let report = engine.fire(ScriptEvent::Enter);
    let spots: Vec<i32> = report.runs.iter().map(|run| run.spot).collect();
    assert_eq!(spots, vec![1, 2], "the fault aborts the rest: {report:?}");
    assert_eq!(report.errors().len(), 1);
    assert_eq!(
        report.effects,
        vec![Effect::Say {
            text: "one".to_owned()
        }]
    );
}

#[test]
fn alarms_queued_before_a_fault_survive() {
    let mut engine = ScriptEngine::with_palace_limits();
    engine.load_room(&room_with_scripts(&[
        (1, Some("ON ENTER { { \"later\" SAY } 30 ALARMEXEC }")),
        (2, Some("ON ENTER { 1 2 3 GET }")),
    ]));
    let report = engine.fire(ScriptEvent::Enter);
    assert_eq!(report.errors().len(), 1);
    assert_eq!(
        engine.pending_alarms(),
        1,
        "the reference faults through IptManager.step, which does not clear alarms"
    );
}

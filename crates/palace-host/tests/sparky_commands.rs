//! Behaviour of the PalaceChat (Sparky) commands the harvested corpus calls.
//!
//! The registration test in `iptscrae-palace` proves every Sparky GS name
//! resolves to a command rather than a variable. These cases prove the
//! corpus-observed group actually *does* something when it is reached: it
//! records a named effect, pushes a defined answer, schedules or cancels an
//! alarm, and never lands in the `unsupported` gap tally.

mod common;

use common::*;
use iptscrae::value::{Chunk, Value};
use iptscrae::{Host, IptError};
use palace_host::view::SpotView;
use palace_host::{AlarmKind, Effect, ScriptHost};

fn host() -> ScriptHost {
    let mut host = ScriptHost::new(populated_view());
    host.view.spots.push(SpotView {
        id: 7,
        kind: 0,
        name: "Outline".to_owned(),
        points: vec![(1, 2), (3, 4), (5, 6)],
        loc: (10, 20),
        ..SpotView::default()
    });
    host
}

fn run(host: &mut ScriptHost, name: &str, args: &[Value]) -> Vec<Value> {
    host.command(name, args).expect("command runs")
}

#[test]
fn gettimezone_pushes_a_numeric_utc_offset() {
    let mut host = host();
    let pushed = run(&mut host, "GETTIMEZONE", &[]);
    assert_eq!(pushed.len(), 1, "GETTIMEZONE pushes one value");
    match pushed[0] {
        Value::Int(offset) => assert!(
            (-12..=14).contains(&offset),
            "a UTC offset in whole hours, got {offset}"
        ),
        ref other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn fileexists_answers_not_found_rather_than_faulting() {
    let mut host = host();
    assert_eq!(
        run(&mut host, "FILEEXISTS", &[Value::str("city.gif")]),
        ints(&[0])
    );
    assert!(
        host.unsupported.is_empty(),
        "FILEEXISTS is handled: {:?}",
        host.unsupported
    );
}

#[test]
fn getspotpoints_returns_the_flat_outline() {
    let mut host = host();
    let pushed = run(&mut host, "GETSPOTPOINTS", &[Value::Int(7)]);
    assert_eq!(pushed.len(), 1);
    match &pushed[0] {
        Value::Array(items) => {
            let items = items.borrow();
            assert_eq!(
                items.as_slice(),
                ints(&[1, 2, 3, 4, 5, 6]).as_slice(),
                "a flat x y list, in room order"
            );
        }
        other => panic!("expected an array, got {other:?}"),
    }
}

#[test]
fn setspotpoints_records_the_new_outline() {
    let mut host = host();
    run(
        &mut host,
        "SETSPOTPOINTS",
        &[
            Value::array(ints(&[9, 8, 7, 6])),
            Value::Int(1),
            Value::Int(2),
            Value::Int(7),
        ],
    );
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetSpotPoints {
            spot: 7,
            x: 1,
            y: 2,
            points: vec![(9, 8), (7, 6)],
        }]
    );
}

#[test]
fn removespot_records_the_spot() {
    let mut host = host();
    run(&mut host, "REMOVESPOT", &[Value::Int(7)]);
    assert_eq!(host.take_effects(), vec![Effect::RemoveSpot { spot: 7 }]);
}

#[test]
fn setspotpicmode_records_mode_and_spot() {
    let mut host = host();
    run(&mut host, "SETSPOTPICMODE", &[Value::Int(5), Value::Int(7)]);
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetSpotPicMode { spot: 7, mode: 5 }]
    );
}

#[test]
fn setspotstyle_records_colour_and_spot() {
    let mut host = host();
    run(
        &mut host,
        "SETSPOTSTYLE",
        &[
            Value::str("&h26ffffff"),
            Value::Int(0),
            Value::Int(0),
            Value::Int(7),
        ],
    );
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetSpotStyle {
            spot: 7,
            color: "&h26ffffff".to_owned(),
            border: 0,
            size: 0,
        }]
    );
}

#[test]
fn webembed_and_webscript_record_their_targets() {
    let mut host = host();
    run(
        &mut host,
        "WEBEMBED",
        &[Value::str("https://example.invalid/player"), Value::Int(7)],
    );
    assert_eq!(
        host.take_effects(),
        vec![Effect::WebEmbed {
            spot: 7,
            url: "https://example.invalid/player".to_owned(),
        }]
    );
    host.take_effects();
    run(
        &mut host,
        "WEBSCRIPT",
        &[Value::str("disconnectSocket()"), Value::Int(7)],
    );
    assert_eq!(
        host.take_effects(),
        vec![Effect::WebScript {
            spot: 7,
            script: "disconnectSocket()".to_owned(),
        }]
    );
}

#[test]
fn weblocation_answers_empty_without_faulting() {
    let mut host = host();
    assert_eq!(run(&mut host, "WEBLOCATION", &[Value::Int(7)]), strs(&[""]));
}

#[test]
fn httpcancel_records_a_cancel() {
    let mut host = host();
    run(&mut host, "HTTPCANCEL", &[]);
    assert_eq!(host.take_effects(), vec![Effect::HttpCancel]);
}

#[test]
fn drawtext_and_the_pen_style_commands_record_effects() {
    let mut host = host();
    run(
        &mut host,
        "DRAWTEXT",
        &[Value::str("hello"), Value::Int(10), Value::Int(20)],
    );
    assert_eq!(
        host.take_effects(),
        vec![Effect::DrawText {
            text: "hello".to_owned(),
            x: 10,
            y: 20,
        }]
    );
    run(&mut host, "PENFONT", &[Value::str("Tahoma")]);
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetPenFont {
            name: "Tahoma".to_owned()
        }]
    );
    run(&mut host, "PENBOLD", &[Value::Int(1)]);
    assert_eq!(host.take_effects(), vec![Effect::SetPenBold { on: true }]);
    run(&mut host, "PENITALIC", &[Value::Int(0)]);
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetPenItalic { on: false }]
    );
    run(&mut host, "PENUNDERLINE", &[Value::Int(1)]);
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetPenUnderline { on: true }]
    );
}

#[test]
fn prompt_answers_the_default_text() {
    let mut host = host();
    let pushed = run(
        &mut host,
        "PROMPT",
        &[Value::str("Password?"), Value::str("guest")],
    );
    assert_eq!(pushed, strs(&["guest"]));
    assert_eq!(
        host.take_effects(),
        vec![Effect::Prompt {
            label: "Password?".to_owned(),
            default: "guest".to_owned(),
        }]
    );
}

#[test]
fn timerexec_schedules_its_body_as_an_alarm() {
    let mut host = host();
    host.current_spot = 7;
    run(
        &mut host,
        "TIMEREXEC",
        &[Value::Chunk(Chunk::empty()), Value::Int(160)],
    );
    assert_eq!(host.alarms.len(), 1);
    assert_eq!(host.alarms[0].ticks, 160);
    assert_eq!(host.alarms[0].spot, 7);
    assert!(matches!(host.alarms[0].kind, AlarmKind::Body(_)));
}

#[test]
fn stopalarms_clears_every_pending_alarm() {
    let mut host = host();
    run(&mut host, "SETALARM", &[Value::Int(60), Value::Int(7)]);
    run(&mut host, "SETALARM", &[Value::Int(60), Value::Int(0)]);
    assert_eq!(host.alarms.len(), 2);
    run(&mut host, "STOPALARMS", &[]);
    assert!(host.alarms.is_empty(), "STOPALARMS clears the queue");
}

#[test]
fn stopalarm_removes_only_that_spot() {
    let mut host = host();
    run(&mut host, "SETALARM", &[Value::Int(60), Value::Int(7)]);
    run(&mut host, "SETALARM", &[Value::Int(60), Value::Int(0)]);
    run(&mut host, "STOPALARM", &[Value::Int(7)]);
    assert_eq!(host.alarms.len(), 1);
    assert_eq!(host.alarms[0].spot, 0);
}

#[test]
fn updatelater_is_an_explicit_refusal_not_a_silent_drop() {
    let mut host = host();
    run(&mut host, "UPDATELATER", &[]);
    assert!(matches!(
        host.effects.as_slice(),
        [Effect::Refused { command, .. }] if command == "UPDATELATER"
    ));
    assert!(
        host.unsupported.is_empty(),
        "a documented refusal is not an unimplemented gap: {:?}",
        host.unsupported
    );
}

#[test]
fn setspotfont_is_refused_with_a_reason_and_declares_ten_operands() {
    assert_eq!(
        iptscrae_palace::commands::command_spec("SETSPOTFONT").map(|spec| spec.pops),
        Some(10),
        "Ab pops nine typed values plus one qn colour"
    );
    let mut host = host();
    let args = vec![
        Value::Int(1),
        Value::Int(2),
        Value::Int(3),
        Value::Int(4),
        Value::Int(5),
        Value::Int(6),
        Value::Int(7),
        Value::str("Arial"),
        Value::Int(9),
        Value::Int(10),
    ];
    run(&mut host, "SETSPOTFONT", &args);
    match host.effects.as_slice() {
        [Effect::Refused { command, reason }] => {
            assert_eq!(command, "SETSPOTFONT");
            assert!(!reason.is_empty(), "a refusal names its reason");
        }
        other => panic!("expected one refusal, got {other:?}"),
    }
    assert!(host.unsupported.is_empty());
}

#[test]
fn setspotclip_is_an_explicit_refusal_because_the_reference_only_pops() {
    let mut host = host();
    run(&mut host, "SETSPOTCLIP", &[Value::Int(1), Value::Int(7)]);
    assert!(matches!(
        host.effects.as_slice(),
        [Effect::Refused { command, .. }] if command == "SETSPOTCLIP"
    ));
    assert!(host.unsupported.is_empty());
}

#[test]
fn mute_and_unmute_whisper_the_admin_command_to_the_server() {
    let mut god = ScriptHost::new(palace_host::HostView {
        is_god: true,
        ..populated_view()
    });
    run(&mut god, "MUTE", &[Value::str("bob")]);
    assert_eq!(
        god.take_effects(),
        vec![Effect::PrivateMessage {
            user: 0,
            text: "`mute bob".to_owned(),
        }]
    );
    run(&mut god, "UNMUTE", &[Value::str("bob")]);
    assert_eq!(
        god.take_effects(),
        vec![Effect::PrivateMessage {
            user: 0,
            text: "`unmute bob".to_owned(),
        }]
    );
    let mut guest = ScriptHost::new(palace_host::HostView {
        is_wizard: false,
        is_god: false,
        ..populated_view()
    });
    run(&mut guest, "MUTE", &[Value::str("bob")]);
    assert!(guest.effects.is_empty(), "a guest cannot mute");
}

#[test]
fn a_wrong_operand_type_still_reports_a_type_mismatch() {
    let mut host = host();
    assert_eq!(
        host.command("WEBEMBED", &[Value::Int(1), Value::Int(7)]),
        Err(IptError::TypeMismatch {
            expected: "string operand",
            found: "number",
        })
    );
}

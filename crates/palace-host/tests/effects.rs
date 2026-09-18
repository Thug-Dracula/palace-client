//! The effect model: one row per [`Effect`] variant.
//!
//! [`Effect`] is the whole contract between a script and the runtime: each
//! variant names the IPTSCRAE command that produced it (`command()`), whether
//! the runtime can put it on the wire (`is_wire_effect()`) and the transcript
//! line a run report shows (`Display`). The table below lists every variant with
//! all three answers, so a new variant that forgets one is a compile error (the
//! `match` in `command()`/`Display`) or a failing test here.

use palace_host::Effect;

/// `(effect, command(), is_wire_effect(), Display)`.
///
/// The wire column is `true` only for the twenty effects `wire::effect_frame`
/// encodes; everything else is a local UI/refusal action.
fn every_effect() -> Vec<(Effect, &'static str, bool, &'static str)> {
    vec![
        (
            Effect::Say {
                text: "hi".to_owned(),
            },
            "SAY",
            true,
            "SAY \"hi\"",
        ),
        (
            Effect::SayAt {
                text: "hi".to_owned(),
                x: 1,
                y: 2,
            },
            "SAYAT",
            true,
            "SAYAT (1,2) \"hi\"",
        ),
        (
            Effect::PrivateMessage {
                user: 7,
                text: "psst".to_owned(),
            },
            "PRIVATEMSG",
            true,
            "PRIVATEMSG -> 7 \"psst\"",
        ),
        (
            Effect::RoomMessage {
                text: "hi".to_owned(),
            },
            "ROOMMSG",
            false,
            "ROOMMSG \"hi\"",
        ),
        (
            Effect::GlobalMessage {
                text: "hi".to_owned(),
            },
            "GLOBALMSG",
            false,
            "GLOBALMSG \"hi\"",
        ),
        (
            Effect::LocalMessage {
                text: "hi".to_owned(),
            },
            "LOCALMSG",
            false,
            "LOCALMSG \"hi\"",
        ),
        (
            Effect::SuperUserMessage {
                text: "hi".to_owned(),
            },
            "SUSRMSG",
            false,
            "SUSRMSG \"hi\"",
        ),
        (
            Effect::StatusMessage {
                text: "hi".to_owned(),
            },
            "STATUSMSG",
            false,
            "STATUSMSG \"hi\"",
        ),
        (
            Effect::LogMessage {
                text: "hi".to_owned(),
            },
            "LOGMSG",
            false,
            "LOGMSG \"hi\"",
        ),
        (
            Effect::ErrorMessage {
                text: "hi".to_owned(),
            },
            "ERRORMSG",
            false,
            "ERRORMSG \"hi\"",
        ),
        (
            Effect::GotoRoom { room: 817 },
            "GOTOROOM",
            true,
            "GOTOROOM 817",
        ),
        (
            Effect::GotoUrl {
                url: "http://x".to_owned(),
            },
            "GOTOURL",
            false,
            "GOTOURL \"http://x\"",
        ),
        (
            Effect::LaunchApp {
                app: "app".to_owned(),
            },
            "LAUNCHAPP",
            false,
            "LAUNCHAPP \"app\"",
        ),
        (
            Effect::MoveUserAbs { x: 1, y: 2 },
            "SETPOS",
            true,
            "SETPOS (1,2)",
        ),
        (
            Effect::MoveUserRel { dx: 1, dy: 2 },
            "MOVE",
            true,
            "MOVE (1,2)",
        ),
        (
            Effect::SetColor { color: 3 },
            "SETCOLOR",
            true,
            "SETCOLOR 3",
        ),
        (Effect::SetFace { face: 4 }, "SETFACE", true, "SETFACE 4"),
        (
            Effect::SetUserName {
                name: "bob".to_owned(),
            },
            "SETUSERNAME",
            true,
            "SETUSERNAME \"bob\"",
        ),
        (
            Effect::SetProps { props: vec![7, 9] },
            "SETPROPS",
            true,
            "SETPROPS [7, 9]",
        ),
        (Effect::DonProp { prop: 7 }, "DONPROP", false, "DONPROP 7"),
        (Effect::DoffProp, "DOFFPROP", false, "DOFFPROP"),
        (
            Effect::RemoveProp { prop: 7 },
            "REMOVEPROP",
            false,
            "REMOVEPROP 7",
        ),
        (Effect::Naked, "NAKED", false, "NAKED"),
        (
            Effect::SetSpotState { spot: 2, state: 1 },
            "SETSPOTSTATE",
            true,
            "SETSPOTSTATE spot=2 state=1",
        ),
        (
            Effect::SetSpotStateLocal { spot: 2, state: 1 },
            "SETSPOTSTATELOCAL",
            false,
            "SETSPOTSTATELOCAL spot=2 state=1",
        ),
        (
            Effect::MoveSpot {
                spot: 2,
                dx: 1,
                dy: 2,
            },
            "SETLOC",
            false,
            "SETLOC spot=2 d=(1,2)",
        ),
        (
            Effect::MoveSpotLocal {
                spot: 2,
                dx: 1,
                dy: 2,
            },
            "SETLOCLOCAL",
            false,
            "SETLOCLOCAL spot=2 d=(1,2)",
        ),
        (
            Effect::SetPicOffset {
                spot: 2,
                dx: 1,
                dy: 2,
            },
            "SETPICLOC",
            false,
            "SETPICLOC spot=2 d=(1,2)",
        ),
        (
            Effect::SetPicOffsetLocal {
                spot: 2,
                state: 1,
                dx: 3,
                dy: 4,
            },
            "SETPICLOCLOCAL",
            false,
            "SETPICLOCLOCAL spot=2 state=1 d=(3,4)",
        ),
        (
            Effect::SetPicOpacity {
                spot: 2,
                state: 1,
                opacity: 0.5,
            },
            "SETPICOPACITY",
            false,
            "SETPICOPACITY spot=2 state=1 0.50",
        ),
        (
            Effect::AddSpot {
                id: 6,
                points: vec![(0, 0), (10, 0), (10, 10)],
                x: 20,
                y: 30,
            },
            "ADDSPOT",
            false,
            "ADDSPOT id=6 at (20,30) [3 points]",
        ),
        (
            Effect::AddPic {
                spot: 2,
                name: "stage.png".to_owned(),
            },
            "ADDPIC",
            false,
            "ADDPIC spot=2 \"stage.png\"",
        ),
        (
            Effect::SetSpotOptions {
                spot: 5,
                hotspot_type: 3,
                flags: 0x40,
                top_layer: true,
            },
            "SETSPOTOPTIONS",
            false,
            "SETSPOTOPTIONS spot=5 type=3 flags=64 top_layer=true",
        ),
        (Effect::Lock { spot: 4 }, "LOCK", true, "LOCK 4"),
        (Effect::Unlock { spot: 4 }, "UNLOCK", true, "UNLOCK 4"),
        (Effect::SelectSpot { spot: 5 }, "SELECT", false, "SELECT 5"),
        (
            Effect::AddLooseProp {
                prop: 7,
                x: 10,
                y: 20,
            },
            "ADDLOOSEPROP",
            true,
            "ADDLOOSEPROP 7 at (10,20)",
        ),
        (
            Effect::RemoveLooseProp { index: 3 },
            "REMOVELOOSEPROP",
            true,
            "REMOVELOOSEPROP 3",
        ),
        (
            Effect::MoveLooseProp {
                index: 3,
                x: 10,
                y: 20,
            },
            "MOVELOOSEPROP",
            true,
            "MOVELOOSEPROP 3 to (10,20)",
        ),
        (
            Effect::ClearLooseProps,
            "CLEARLOOSEPROPS",
            false,
            "CLEARLOOSEPROPS",
        ),
        (
            Effect::DropProp { x: 10, y: 20 },
            "DROPPROP",
            false,
            "DROPPROP (10,20)",
        ),
        (
            Effect::DimRoom { percent: 50 },
            "DIMROOM",
            false,
            "DIMROOM 50",
        ),
        (
            Effect::PlaySound {
                name: "garden".to_owned(),
            },
            "SOUND",
            false,
            "SOUND \"garden\"",
        ),
        (
            Effect::MidiPlay {
                name: "garden".to_owned(),
            },
            "MIDIPLAY",
            false,
            "MIDIPLAY \"garden\"",
        ),
        (
            Effect::MidiLoop {
                name: "garden".to_owned(),
                loops: 99,
            },
            "MIDILOOP",
            false,
            "MIDILOOP \"garden\" x99",
        ),
        (Effect::MidiStop, "MIDISTOP", false, "MIDISTOP"),
        (Effect::Beep, "BEEP", false, "BEEP"),
        (
            Effect::DrawLine {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4,
            },
            "LINE",
            true,
            "LINE (1,2)-(3,4)",
        ),
        (
            Effect::DrawLineRel {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4,
            },
            "LINETO",
            true,
            "LINETO (1,2)-(3,4)",
        ),
        (
            Effect::MovePen { x: 1, y: 2 },
            "PENPOS",
            false,
            "PENPOS (1,2)",
        ),
        (
            Effect::SetPenColor { r: 1, g: 2, b: 3 },
            "PENCOLOR",
            false,
            "PENCOLOR (1,2,3)",
        ),
        (
            Effect::SetPenSize { size: 2 },
            "PENSIZE",
            false,
            "PENSIZE 2",
        ),
        (
            Effect::PaintLayer { front: true },
            "PENFRONT",
            false,
            "PENFRONT",
        ),
        (
            Effect::PaintLayer { front: false },
            // `command()` cannot see the layer (both map to the PEN family);
            // `Display` is where PENBACK is distinguished.
            "PENFRONT",
            false,
            "PENBACK",
        ),
        (Effect::PaintClear, "PAINTCLEAR", true, "PAINTCLEAR"),
        (Effect::PaintUndo, "PAINTUNDO", false, "PAINTUNDO"),
        (Effect::HideAvatars, "HIDEAVATARS", false, "HIDEAVATARS"),
        (Effect::ShowAvatars, "SHOWAVATARS", false, "SHOWAVATARS"),
        (Effect::Macro { index: 1 }, "MACRO", false, "MACRO 1"),
        (
            Effect::SetSpotAlarm { spot: 2, ticks: 60 },
            "SETALARM",
            false,
            "SETALARM spot=2 ticks=60",
        ),
        (
            Effect::SetChatString {
                text: "hi".to_owned(),
            },
            "CHATSTR",
            false,
            "CHATSTR=\"hi\"",
        ),
        (
            Effect::Unsupported {
                command: "WEIRD".to_owned(),
            },
            "WEIRD",
            false,
            "unsupported WEIRD",
        ),
    ]
}

#[test]
fn every_effect_reports_the_command_that_produced_it() {
    for (effect, command, _, _) in every_effect() {
        assert_eq!(effect.command(), command, "command for {effect:?}");
    }
}

#[test]
fn wire_classification_is_exact() {
    for (effect, _, wire, _) in every_effect() {
        assert_eq!(effect.is_wire_effect(), wire, "wire flag for {effect:?}");
    }
}

#[test]
fn every_effect_renders_a_transcript_line() {
    for (effect, _, _, display) in every_effect() {
        assert_eq!(effect.to_string(), display, "display for {effect:?}");
    }
}

#[test]
fn an_unsupported_effect_keeps_the_offending_command() {
    let effect = Effect::Unsupported {
        command: "SETSPOTNAMELOCAL(5,New)".to_owned(),
    };
    assert_eq!(effect.command(), "SETSPOTNAMELOCAL(5,New)");
    assert_eq!(effect.to_string(), "unsupported SETSPOTNAMELOCAL(5,New)");
    assert!(!effect.is_wire_effect());
}

#[test]
fn the_wire_set_is_exactly_the_nineteen_encodable_effects() {
    let wire: Vec<String> = every_effect()
        .into_iter()
        .filter(|(effect, _, _, _)| effect.is_wire_effect())
        .map(|(effect, _, _, _)| effect.command().to_owned())
        .collect();
    assert_eq!(wire.len(), 19, "wire effects: {wire:?}");
    for command in [
        "SAY",
        "SAYAT",
        "PRIVATEMSG",
        "GOTOROOM",
        "SETPOS",
        "MOVE",
        "SETCOLOR",
        "SETFACE",
        "SETUSERNAME",
        "SETPROPS",
        "SETSPOTSTATE",
        "LOCK",
        "UNLOCK",
        "ADDLOOSEPROP",
        "REMOVELOOSEPROP",
        "MOVELOOSEPROP",
        "LINE",
        "LINETO",
        "PAINTCLEAR",
    ] {
        assert!(
            wire.iter().any(|c| c == command),
            "{command} must be a wire effect"
        );
    }
}

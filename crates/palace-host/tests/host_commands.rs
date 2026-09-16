//! The `Host::command` dispatch table, command by command.
//!
//! Every Palace command name the corpus can emit is sent through
//! [`ScriptHost`](palace_host::ScriptHost) with the operands the reference
//! clients push, and the recorded [`Effect`] (or pushed value) is asserted
//! exactly. Read this file next to `iptscrae-palace`'s `PALACE_COMMANDS` table:
//! the operands here are in **push order** (`args[0]` deepest), which is why
//! e.g. `PRIVATEMSG` is `(text, user)` and not `(user, text)`.

mod common;

use common::*;
use iptscrae::value::Value;
use iptscrae::{Chunk, Host, IptError};
use palace_host::{AlarmKind, Effect, PendingAlarm};

// --------------------------------------------------------------- state getters

#[test]
fn identity_and_room_getters_answer_from_the_snapshot() {
    for name in ["ME", "ID", "USERID", "WHOME"] {
        assert_eq!(pushed(name, &[]), ints(&[13]), "{name}");
    }
    assert_eq!(pushed("USERNAME", &[]), vec![Value::str("RustProbe")]);
    assert_eq!(pushed("SERVERNAME", &[]), vec![Value::str("TestServer")]);
    assert_eq!(pushed("CLIENTTYPE", &[]), vec![Value::str("OPENPALACE")]);
    assert_eq!(pushed("ROOMID", &[]), ints(&[901]));
    assert_eq!(pushed("ROOMNAME", &[]), vec![Value::str("Balamb Garden")]);
    assert_eq!(pushed("ROOMWIDTH", &[]), ints(&[512]));
    assert_eq!(pushed("ROOMHEIGHT", &[]), ints(&[384]));
    assert_eq!(pushed("POSX", &[]), ints(&[256]));
    assert_eq!(pushed("POSY", &[]), ints(&[192]));
    assert_eq!(pushed("MOUSEPOS", &[]), ints(&[100, 200]));
    assert_eq!(pushed("MOUSEX", &[]), ints(&[100]));
    assert_eq!(pushed("MOUSEY", &[]), ints(&[200]));
    assert_eq!(pushed("WHOCHAT", &[]), ints(&[7]));
    assert_eq!(pushed("WHOTARGET", &[]), ints(&[9]));
    assert_eq!(pushed("LASTNAME", &[]), vec![Value::str("")]);
    assert_eq!(pushed("HTTPRECEIVED", &[]), ints(&[0]));
    // Version banners the corpus scripts probe.
    assert_eq!(pushed("OPENPALACE", &[]), ints(&[1]));
    assert_eq!(pushed("PALACECHAT", &[]), ints(&[1]));
    assert_eq!(pushed("IPTVERSION", &[]), ints(&[2]));
}

#[test]
fn predicate_and_counter_commands_read_the_view() {
    assert_eq!(pushed("ISGOD", &[]), ints(&[1]));
    assert_eq!(pushed("ISGUEST", &[]), ints(&[0]));
    assert_eq!(pushed("ISWIZARD", &[]), ints(&[1]));
    assert_eq!(pushed("ISRIGHTCLICK", &[]), ints(&[1]));
    assert_eq!(pushed("NBRROOMUSERS", &[]), ints(&[2]));
    assert_eq!(pushed("NBRSPOTS", &[]), ints(&[3]));
    assert_eq!(pushed("NBRDOORS", &[]), ints(&[1]), "only spot 5 is kind 1");
    assert_eq!(pushed("NBRUSERPROPS", &[]), ints(&[2]));
    assert_eq!(pushed("NBRLOOSEPROPS", &[]), ints(&[2]));
    assert_eq!(pushed("TOPPROP", &[]), ints(&[9]));
}

#[test]
fn dest_reads_the_executing_spot() {
    let mut host = populated_host();
    host.current_spot = 5;
    assert_eq!(host.command("DEST", &[]).unwrap(), ints(&[817]));
    host.current_spot = 0;
    assert_eq!(host.command("DEST", &[]).unwrap(), ints(&[0]));
    host.current_spot = 2;
    assert_eq!(host.command("DEST", &[]).unwrap(), ints(&[0]));
}

#[test]
fn lookups_clamp_bad_indices_instead_of_panicking() {
    // Spot ids by index: 0 -> 5, 1 -> 2, 2 -> 3, past the end -> 0, negative -> clamped to 0.
    assert_eq!(pushed("SPOTIDX", &ints(&[0])), ints(&[5]));
    assert_eq!(pushed("SPOTIDX", &ints(&[1])), ints(&[2]));
    assert_eq!(pushed("SPOTIDX", &ints(&[8])), ints(&[0]));
    assert_eq!(pushed("SPOTIDX", &ints(&[-1])), ints(&[5]));
    // Door ids are the kind-1 subset of spots.
    assert_eq!(pushed("DOORIDX", &ints(&[0])), ints(&[5]));
    assert_eq!(pushed("DOORIDX", &ints(&[1])), ints(&[0]));
    assert_eq!(pushed("DOORIDX", &ints(&[-1])), ints(&[5]));
    // Room user ids by index.
    assert_eq!(pushed("ROOMUSER", &ints(&[0])), ints(&[13]));
    assert_eq!(pushed("ROOMUSER", &ints(&[1])), ints(&[7]));
    assert_eq!(pushed("ROOMUSER", &ints(&[9])), ints(&[0]));
    assert_eq!(pushed("ROOMUSER", &ints(&[-3])), ints(&[13]));
    // Named lookups return empty/zero for an unknown id.
    assert_eq!(pushed("WHONAME", &ints(&[7])), vec![Value::str("Squall")]);
    assert_eq!(pushed("WHONAME", &ints(&[99])), vec![Value::str("")]);
    assert_eq!(pushed("WHOPOS", &ints(&[7])), ints(&[10, 20]));
    assert_eq!(pushed("WHOPOS", &ints(&[99])), ints(&[0, 0]));
    assert_eq!(pushed("SPOTNAME", &ints(&[5])), vec![Value::str("Door")]);
    assert_eq!(pushed("SPOTNAME", &ints(&[99])), vec![Value::str("")]);
    assert_eq!(pushed("GETSPOTLOC", &ints(&[5])), ints(&[50, 60]));
    assert_eq!(pushed("GETSPOTLOC", &ints(&[99])), ints(&[0, 0]));
    assert_eq!(pushed("GETSPOTSTATE", &ints(&[5])), ints(&[1]));
    assert_eq!(pushed("GETSPOTSTATE", &ints(&[99])), ints(&[0]));
    assert_eq!(pushed("SPOTDEST", &ints(&[5])), ints(&[817]));
    assert_eq!(pushed("GETPICLOC", &ints(&[5, 0])), ints(&[2, 3]));
    assert_eq!(pushed("GETPICLOC", &ints(&[5, 9])), ints(&[0, 0]));
    assert_eq!(pushed("GETPICLOC", &ints(&[99, 0])), ints(&[0, 0]));
    assert_eq!(pushed("GETPICDIMENSIONS", &ints(&[5, 0])), ints(&[0, 0]));
    // Props.
    assert_eq!(pushed("USERPROP", &ints(&[0])), ints(&[7]));
    assert_eq!(pushed("USERPROP", &ints(&[1])), ints(&[9]));
    assert_eq!(pushed("USERPROP", &ints(&[9])), ints(&[0]));
    assert_eq!(pushed("HASPROP", &ints(&[7])), ints(&[1]));
    assert_eq!(pushed("HASPROP", &ints(&[99])), ints(&[0]));
    // Loose props.
    assert_eq!(pushed("LOOSEPROP", &ints(&[0])), ints(&[0x4000_0001]));
    assert_eq!(pushed("LOOSEPROP", &ints(&[9])), ints(&[0]));
    assert_eq!(pushed("LOOSEPROPIDX", &ints(&[0x4000_0001])), ints(&[0]));
    assert_eq!(pushed("LOOSEPROPIDX", &ints(&[123])), ints(&[-1]));
    assert_eq!(pushed("LOOSEPROPPOS", &ints(&[0])), ints(&[10, 20]));
    assert_eq!(pushed("LOOSEPROPPOS", &ints(&[9])), ints(&[0, 0]));
    // Hit testing and door lock state.
    assert_eq!(
        pushed("INSPOT", &ints(&[3])),
        ints(&[1]),
        "self stands in spot 3"
    );
    assert_eq!(pushed("INSPOT", &ints(&[2])), ints(&[0]));
    assert_eq!(pushed("INSPOT", &ints(&[99])), ints(&[0]));
    assert_eq!(pushed("ISLOCKED", &ints(&[5])), ints(&[0]));
}

#[test]
fn prop_dimensions_and_offsets_are_stubbed_with_two_numbers() {
    assert_eq!(pushed("PROPDIMENSIONS", &ints(&[7])), ints(&[0, 0]));
    assert_eq!(pushed("PROPOFFSETS", &ints(&[7])), ints(&[0, 0]));
}

#[test]
fn str_converts_numbers_strings_and_everything_else() {
    assert_eq!(pushed("STR", &ints(&[42])), vec![Value::str("42")]);
    assert_eq!(
        pushed("STR", &strs(&["already"])),
        vec![Value::str("already")]
    );
    assert_eq!(
        pushed("STR", &[Value::Chunk(Chunk::empty())]),
        vec![Value::str("")]
    );
    assert_eq!(pushed("STR", &[]), vec![Value::str("")]);
}

// ------------------------------------------------------------------- messaging

#[test]
fn messaging_commands_record_their_effects() {
    assert_eq!(
        effect_of("SAY", &strs(&["hello"])),
        Effect::Say {
            text: "hello".to_owned()
        }
    );
    assert_eq!(
        effect_of("CHAT", &strs(&["hi"])),
        Effect::Say {
            text: "hi".to_owned()
        }
    );
    assert_eq!(
        effect_of("SAYAT", &[Value::str("hi"), Value::Int(10), Value::Int(20)]),
        Effect::SayAt {
            text: "hi".to_owned(),
            x: 10,
            y: 20
        }
    );
    assert_eq!(
        effect_of("PRIVATEMSG", &[Value::str("psst"), Value::Int(7)]),
        Effect::PrivateMessage {
            user: 7,
            text: "psst".to_owned()
        }
    );
    assert_eq!(
        effect_of("GLOBALMSG", &strs(&["g"])),
        Effect::GlobalMessage {
            text: "g".to_owned()
        }
    );
    assert_eq!(
        effect_of("ROOMMSG", &strs(&["r"])),
        Effect::RoomMessage {
            text: "r".to_owned()
        }
    );
    assert_eq!(
        effect_of("LOCALMSG", &strs(&["l"])),
        Effect::LocalMessage {
            text: "l".to_owned()
        }
    );
    assert_eq!(
        effect_of("SUSRMSG", &strs(&["s"])),
        Effect::SuperUserMessage {
            text: "s".to_owned()
        }
    );
    assert_eq!(
        effect_of("STATUSMSG", &strs(&["st"])),
        Effect::StatusMessage {
            text: "st".to_owned()
        }
    );
    assert_eq!(
        effect_of("LOGMSG", &strs(&["lg"])),
        Effect::LogMessage {
            text: "lg".to_owned()
        }
    );
}

// ------------------------------------------------------------------ spot state

#[test]
fn spot_mutation_commands_record_their_effects() {
    assert_eq!(
        effect_of("SETSPOTSTATE", &ints(&[1, 5])),
        Effect::SetSpotState { spot: 5, state: 1 }
    );
    assert_eq!(
        effect_of("SETSPOTSTATELOCAL", &ints(&[1, 5])),
        Effect::SetSpotStateLocal { spot: 5, state: 1 }
    );
    assert_eq!(
        effect_of("SETSPOTNAMELOCAL", &[Value::str("New"), Value::Int(5)]),
        Effect::Unsupported {
            command: "SETSPOTNAMELOCAL(5,New)".to_owned()
        }
    );
    assert_eq!(
        effect_of("SETLOC", &ints(&[1, 2, 5])),
        Effect::MoveSpot {
            spot: 5,
            dx: 1,
            dy: 2
        }
    );
    assert_eq!(
        effect_of("SETLOCLOCAL", &ints(&[1, 2, 5])),
        Effect::MoveSpotLocal {
            spot: 5,
            dx: 1,
            dy: 2
        }
    );
    assert_eq!(
        effect_of("SETPICLOC", &ints(&[1, 2, 5])),
        Effect::SetPicOffset {
            spot: 5,
            dx: 1,
            dy: 2
        }
    );
    assert_eq!(
        effect_of("SETPICLOCLOCAL", &ints(&[1, 2, 3, 5])),
        Effect::SetPicOffsetLocal {
            spot: 5,
            state: 3,
            dx: 1,
            dy: 2
        }
    );
    assert_eq!(
        effect_of("SETPICOPACITY", &ints(&[50, 1, 5])),
        Effect::SetPicOpacity {
            spot: 5,
            state: 1,
            opacity: 0.5
        }
    );
    assert_eq!(effect_of("LOCK", &ints(&[5])), Effect::Lock { spot: 5 });
    assert_eq!(effect_of("UNLOCK", &ints(&[5])), Effect::Unlock { spot: 5 });
    assert_eq!(
        effect_of("SELECT", &ints(&[5])),
        Effect::SelectSpot { spot: 5 }
    );
}

// --------------------------------------------------------------------- props

#[test]
fn prop_commands_record_their_effects() {
    assert_eq!(
        effect_of("DONPROP", &ints(&[7])),
        Effect::DonProp { prop: 7 }
    );
    assert_eq!(
        effect_of("REMOVEPROP", &ints(&[7])),
        Effect::RemoveProp { prop: 7 }
    );
    assert_eq!(effect_of("DOFFPROP", &[]), Effect::DoffProp);
    assert_eq!(effect_of("NAKED", &[]), Effect::Naked);
    assert_eq!(effect_of("CLEARPROPS", &[]), Effect::Naked);
    // LOADPROPS is a cache warm-up: it records nothing.
    let mut host = populated_host();
    assert_eq!(
        host.command("LOADPROPS", &ints(&[7, 9])).unwrap(),
        Vec::new()
    );
    assert!(host.effects.is_empty());
}

#[test]
fn prop_ids_accept_numbers_and_quoted_numbers() {
    assert_eq!(
        effect_of("DONPROP", &ints(&[7])),
        Effect::DonProp { prop: 7 }
    );
    assert_eq!(
        effect_of("DONPROP", &strs(&[" 7 "])),
        Effect::DonProp { prop: 7 }
    );
    assert_eq!(
        effect_of("DONPROP", &strs(&["hat"])),
        Effect::DonProp { prop: 0 }
    );
    assert_eq!(effect_of("DONPROP", &[]), Effect::DonProp { prop: 0 });
    assert_eq!(
        effect_of("REMOVEPROP", &strs(&["9"])),
        Effect::RemoveProp { prop: 9 }
    );
}

#[test]
fn set_props_accepts_arrays_singletons_and_junk() {
    let mut host = populated_host();
    host.command(
        "SETPROPS",
        &[Value::array(vec![Value::Int(7), Value::Int(9)])],
    )
    .unwrap();
    host.command("SETPROPS", &[Value::Int(5)]).unwrap();
    host.command("SETPROPS", &[Value::str("junk")]).unwrap();
    host.command("SETPROPS", &[]).unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::SetProps { props: vec![7, 9] },
            Effect::SetProps { props: vec![5] },
            Effect::SetProps { props: vec![] },
            Effect::SetProps { props: vec![] },
        ]
    );
}

#[test]
fn set_props_on_a_borrowed_array_falls_back_to_empty() {
    // An array that is mutably borrowed while the command runs cannot be read;
    // the host must record an empty list rather than panic.
    let array = Value::array(vec![Value::Int(7)]);
    let cell = match &array {
        Value::Array(cell) => cell.clone(),
        other => panic!("expected an array, got {other:?}"),
    };
    let held = cell.borrow_mut();
    let mut host = populated_host();
    host.command("SETPROPS", std::slice::from_ref(&array))
        .unwrap();
    drop(held);
    assert_eq!(
        host.take_effects(),
        vec![Effect::SetProps { props: vec![] }]
    );
}

// ------------------------------------------------------------------ movement

#[test]
fn movement_commands_record_their_effects() {
    assert_eq!(
        effect_of("SETPOS", &ints(&[1, 2])),
        Effect::MoveUserAbs { x: 1, y: 2 }
    );
    assert_eq!(
        effect_of("MOVE", &ints(&[3, 4])),
        Effect::MoveUserRel { dx: 3, dy: 4 }
    );
    assert_eq!(
        effect_of("GOTOROOM", &ints(&[817])),
        Effect::GotoRoom { room: 817 }
    );
    assert_eq!(
        effect_of("ROOMGOTO", &ints(&[817])),
        Effect::GotoRoom { room: 817 }
    );
    assert_eq!(
        effect_of("NETGOTO", &strs(&["http://u"])),
        Effect::GotoUrl {
            url: "http://u".to_owned()
        }
    );
    assert_eq!(
        effect_of("GOTOURL", &strs(&["http://u"])),
        Effect::GotoUrl {
            url: "http://u".to_owned()
        }
    );
    assert_eq!(
        effect_of("SETUSERNAME", &strs(&["bob"])),
        Effect::SetUserName {
            name: "bob".to_owned()
        }
    );
    assert_eq!(
        effect_of("SETCOLOR", &ints(&[3])),
        Effect::SetColor { color: 3 }
    );
    assert_eq!(
        effect_of("SETFACE", &ints(&[4])),
        Effect::SetFace { face: 4 }
    );
    assert_eq!(effect_of("MACRO", &ints(&[1])), Effect::Macro { index: 1 });
    assert_eq!(
        effect_of("DIMROOM", &ints(&[50])),
        Effect::DimRoom { percent: 50 }
    );
    assert_eq!(
        effect_of("LAUNCHAPP", &strs(&["app"])),
        Effect::LaunchApp {
            app: "app".to_owned()
        }
    );
    assert_eq!(effect_of("HIDEAVATARS", &[]), Effect::HideAvatars);
    assert_eq!(effect_of("SHOWAVATARS", &[]), Effect::ShowAvatars);
}

// ---------------------------------------------------------------------- sound

#[test]
fn sound_commands_record_their_effects() {
    assert_eq!(
        effect_of("SOUND", &strs(&["garden"])),
        Effect::PlaySound {
            name: "garden".to_owned()
        }
    );
    assert_eq!(
        effect_of("MIDIPLAY", &strs(&["g.mid"])),
        Effect::MidiPlay {
            name: "g.mid".to_owned()
        }
    );
    assert_eq!(
        effect_of("MIDILOOP", &[Value::Int(99), Value::str("g.mid")]),
        Effect::MidiLoop {
            name: "g.mid".to_owned(),
            loops: 99
        }
    );
    assert_eq!(effect_of("MIDISTOP", &[]), Effect::MidiStop);
    assert_eq!(effect_of("BEEP", &[]), Effect::Beep);
}

// ----------------------------------------------------------------- loose props

#[test]
fn loose_prop_commands_record_their_effects() {
    assert_eq!(
        effect_of("ADDLOOSEPROP", &ints(&[7, 10, 20])),
        Effect::AddLooseProp {
            prop: 7,
            x: 10,
            y: 20
        }
    );
    assert_eq!(
        effect_of("REMOVELOOSEPROP", &ints(&[3])),
        Effect::RemoveLooseProp { index: 3 }
    );
    assert_eq!(
        effect_of("MOVELOOSEPROP", &ints(&[10, 20, 3])),
        Effect::MoveLooseProp {
            index: 3,
            x: 10,
            y: 20
        }
    );
    assert_eq!(effect_of("CLEARLOOSEPROPS", &[]), Effect::ClearLooseProps);
    assert_eq!(
        effect_of("DROPPROP", &ints(&[10, 20])),
        Effect::DropProp { x: 10, y: 20 }
    );
    assert_eq!(
        effect_of("SHOWLOOSEPROPS", &[]),
        Effect::Unsupported {
            command: "SHOWLOOSEPROPS".to_owned()
        }
    );
}

// ---------------------------------------------------------------------- paint

#[test]
fn paint_commands_move_the_pen_and_record_strokes() {
    let mut host = populated_host();
    host.command("LINE", &ints(&[1, 2, 3, 4])).unwrap();
    // LINETO resolves against the current pen position and advances it.
    host.pen.pos = (10, 20);
    host.command("LINETO", &ints(&[5, 6])).unwrap();
    assert_eq!(host.pen.pos, (15, 26));
    // PENPOS / PENTO move without drawing.
    host.command("PENPOS", &ints(&[100, 100])).unwrap();
    host.command("PENTO", &ints(&[-4, 9])).unwrap();
    assert_eq!(host.pen.pos, (96, 109));
    // PENSIZE / PENCOLOR update the pen.
    host.command("PENSIZE", &ints(&[7])).unwrap();
    host.command("PENCOLOR", &ints(&[1, 2, 3])).unwrap();
    // Layer selection and erasure.
    host.command("PENFRONT", &[]).unwrap();
    host.command("PENBACK", &[]).unwrap();
    host.command("PAINTCLEAR", &[]).unwrap();
    host.command("PAINTUNDO", &[]).unwrap();

    assert_eq!(
        host.take_effects(),
        vec![
            Effect::DrawLine {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4
            },
            Effect::DrawLineRel {
                x1: 10,
                y1: 20,
                x2: 15,
                y2: 26
            },
            Effect::MovePen { x: 100, y: 100 },
            Effect::MovePen { x: 96, y: 109 },
            Effect::SetPenSize { size: 7 },
            Effect::SetPenColor { r: 1, g: 2, b: 3 },
            Effect::PaintLayer { front: true },
            Effect::PaintLayer { front: false },
            Effect::PaintClear,
            Effect::PaintUndo,
        ]
    );
    assert_eq!(host.pen.rgb, (1, 2, 3));
    assert_eq!(host.pen.size, 7);
    assert!(!host.pen.front, "PENBACK left the pen on the back layer");
}

// ----------------------------------------------------------------------- misc

#[test]
fn unimplemented_commands_are_tallied_and_recorded() {
    let mut host = populated_host();
    for _ in 0..2 {
        assert_eq!(host.command("PING", &[]).unwrap(), Vec::new());
    }
    host.command("NOPE", &[]).unwrap();
    host.command("KILLUSER", &ints(&[7])).unwrap();
    host.command("FLUSH", &[]).unwrap();
    assert_eq!(
        host.take_effects(),
        vec![
            Effect::Unsupported {
                command: "PING".to_owned()
            },
            Effect::Unsupported {
                command: "PING".to_owned()
            },
            Effect::Unsupported {
                command: "NOPE".to_owned()
            },
            Effect::Unsupported {
                command: "KILLUSER".to_owned()
            },
            Effect::Unsupported {
                command: "FLUSH".to_owned()
            },
        ]
    );
    assert_eq!(host.unsupported.get("PING"), Some(&2));
    assert_eq!(host.unsupported.get("NOPE"), Some(&1));
    assert_eq!(host.unsupported.get("KILLUSER"), Some(&1));
    assert_eq!(host.unsupported.get("FLUSH"), Some(&1));
}

#[test]
fn the_stubbed_spot_colour_commands_are_reported_not_dropped() {
    for name in [
        "SETPICBRIGHTNESS",
        "SETPICSATURATION",
        "SETPICDIM",
        "GOTOURLFRAME",
        "LAUNCHEVENT",
        "LAUNCHPPA",
        "LOADJAVA",
        "TALKPPA",
        "BAN",
        "KICK",
        "ROOMDESC",
        "OFFLINE",
        "ONLINE",
    ] {
        assert_eq!(
            effect_of(name, &[]),
            Effect::Unsupported {
                command: name.to_owned()
            },
            "{name}"
        );
    }
}

// -------------------------------------------------------------------- alarms

#[test]
fn setalarm_records_a_spot_alarm_and_defaults_to_the_current_spot() {
    let mut host = populated_host();
    host.current_spot = 2;
    host.command("SETALARM", &ints(&[60, 0])).unwrap();
    host.command("SETALARM", &ints(&[30, 5])).unwrap();
    host.command("SETALARM", &ints(&[-1, 5])).unwrap();
    assert_eq!(
        host.take_alarms(),
        vec![
            PendingAlarm {
                ticks: 60,
                kind: AlarmKind::Spot,
                spot: 2
            },
            PendingAlarm {
                ticks: 30,
                kind: AlarmKind::Spot,
                spot: 5
            },
            PendingAlarm {
                ticks: 0,
                kind: AlarmKind::Spot,
                spot: 5
            },
        ]
    );
    assert!(host.effects.is_empty(), "SETALARM does not push an effect");
}

// --------------------------------------------------------------- refusal paths

#[test]
fn bad_operands_are_refused_and_record_nothing() {
    let mut host = populated_host();
    // String where a number is required.
    assert_eq!(
        host.command("SETFACE", &[Value::str("x")]),
        Err(IptError::TypeMismatch {
            expected: "number operand",
            found: "string"
        })
    );
    // Number where a string is required.
    assert_eq!(
        host.command("SAY", &[Value::Int(1)]),
        Err(IptError::TypeMismatch {
            expected: "string operand",
            found: "number"
        })
    );
    // Missing operands.
    assert_eq!(
        host.command("SETFACE", &[]),
        Err(IptError::StackUnderflow {
            needed: 1,
            available: 0
        })
    );
    assert_eq!(
        host.command("SAY", &[]),
        Err(IptError::StackUnderflow {
            needed: 1,
            available: 0
        })
    );
    assert_eq!(
        host.command("SAYAT", &strs(&["hi"])),
        Err(IptError::StackUnderflow {
            needed: 2,
            available: 1
        })
    );
    assert_eq!(
        host.command("SETPOS", &ints(&[1])),
        Err(IptError::StackUnderflow {
            needed: 2,
            available: 1
        })
    );
    assert!(host.effects.is_empty(), "a refused command records nothing");
    assert!(host.alarms.is_empty());
    assert!(
        host.unsupported.is_empty(),
        "a refused command is not a gap: {:?}",
        host.unsupported
    );
    assert_eq!(host.pen, palace_host::PenState::default());
}

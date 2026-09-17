//! The Palace command registry and the `PalaceCommands` adapter.
//!
//! `PALACE_COMMANDS` is data; `PalaceCommands` is the code that turns a
//! [`PalaceHost`] into an [`iptscrae::Host`]. These cases exercise the adapter
//! directly — every implemented match arm, every operand-decoding failure, the
//! host-primitive forwarding, and the relationship between the `IMPLEMENTED`
//! list and the registry — rather than only through a running script.

use iptscrae::{Builtin, Chunk, CommandKind, CommandSet, Engine, Host, IptError, Result, Value};
use iptscrae_palace::commands::{
    command_spec, register_palace_commands, PalaceCommands, Push, IMPLEMENTED, PALACE_COMMANDS,
};
use iptscrae_palace::harness::SkeletonHost;
use iptscrae_palace::PalaceHost;

/// A host recording every effect the adapter forwards, overriding the `Host`
/// primitives as well as the Palace methods the adapter implements.
#[derive(Default)]
struct Featured {
    spoke: Vec<String>,
    private: Vec<(i64, String)>,
    moved: Option<(i64, i64)>,
    room: Option<i64>,
    traced: Vec<String>,
    alarm: Option<(i64, i64)>,
}

impl Host for Featured {
    fn random(&mut self, bound: i64) -> i64 {
        bound + 1
    }
    fn ticks(&self) -> i64 {
        77
    }
    fn datetime(&self) -> i64 {
        1234
    }
    fn trace(&mut self, message: &str) {
        self.traced.push(message.to_owned());
    }
    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        Ok(Some(vec![format!("{pattern}:{text}")]))
    }
    fn schedule_alarm(&mut self, ticks: i64, _body: Chunk, spot: i64) -> Result<()> {
        self.alarm = Some((ticks, spot));
        Ok(())
    }
}

impl PalaceHost for Featured {
    fn chat(&mut self, text: &str) -> Result<()> {
        self.spoke.push(text.to_owned());
        Ok(())
    }
    fn send_private_message(&mut self, user: i64, text: &str) -> Result<()> {
        self.private.push((user, text.to_owned()));
        Ok(())
    }
    fn move_user_abs(&mut self, x: i64, y: i64) -> Result<()> {
        self.moved = Some((x, y));
        Ok(())
    }
    fn goto_room(&mut self, room: i64) -> Result<()> {
        self.room = Some(room);
        Ok(())
    }
    fn get_self_user_id(&self) -> i64 {
        99
    }
    fn get_self_user_name(&self) -> String {
        "Nova".to_owned()
    }
}

fn adapter() -> PalaceCommands<Featured> {
    PalaceCommands::new(Featured::default())
}

#[test]
fn the_implemented_commands_forward_their_operands_to_the_trait() {
    let mut host = adapter();

    assert!(
        host.command("SAY", &[Value::str("hello")])
            .unwrap()
            .is_empty(),
        "SAY pushes nothing"
    );
    assert!(
        host.command("CHAT", &[Value::str("again")])
            .unwrap()
            .is_empty(),
        "CHAT is the SAY arm"
    );
    host.command("SAYAT", &[Value::str("hi"), Value::Int(10), Value::Int(20)])
        .unwrap();
    host.command("PRIVATEMSG", &[Value::str("psst"), Value::Int(7)])
        .unwrap();
    host.command("SETPOS", &[Value::Int(3), Value::Int(4)])
        .unwrap();
    host.command("GOTOROOM", &[Value::Int(817)]).unwrap();

    let inner = host.inner;
    assert_eq!(
        inner.spoke,
        vec![
            "hello".to_owned(),
            "again".to_owned(),
            "@10,20 hi".to_owned()
        ],
        "SAYAT formats the balloon position the way the reference does"
    );
    assert_eq!(inner.private, vec![(7, "psst".to_owned())]);
    assert_eq!(inner.moved, Some((3, 4)));
    assert_eq!(inner.room, Some(817));
}

#[test]
fn the_identity_commands_read_through_the_trait() {
    let mut host = adapter();
    assert_eq!(
        host.command("USERNAME", &[]).unwrap(),
        vec![Value::str("Nova")]
    );
    assert_eq!(host.command("USERID", &[]).unwrap(), vec![Value::Int(99)]);
    assert_eq!(
        host.command("WHOME", &[]).unwrap(),
        vec![Value::Int(99)],
        "WHOME shares the USERID arm"
    );
}

#[test]
fn a_wrong_operand_type_is_a_type_mismatch_naming_the_kind_it_wanted() {
    let mut host = adapter();
    assert_eq!(
        host.command("SAY", &[Value::Int(1)]).unwrap_err(),
        IptError::TypeMismatch {
            expected: "string operand",
            found: "number",
        }
    );
    assert_eq!(
        host.command("SETPOS", &[Value::str("x"), Value::Int(1)])
            .unwrap_err(),
        IptError::TypeMismatch {
            expected: "number operand",
            found: "string",
        }
    );
}

#[test]
fn a_missing_operand_is_a_stack_underflow_reporting_what_was_needed() {
    let mut host = adapter();
    assert_eq!(
        host.command("SAY", &[]).unwrap_err(),
        IptError::StackUnderflow {
            needed: 1,
            available: 0,
        }
    );
    assert_eq!(
        host.command("SAYAT", &[Value::str("hi"), Value::Int(1)])
            .unwrap_err(),
        IptError::StackUnderflow {
            needed: 3,
            available: 2,
        },
        "SAYAT needs three operands; two are present"
    );
}

#[test]
fn an_unimplemented_name_is_refused_by_the_adapter() {
    let mut host = adapter();
    assert_eq!(
        host.command("PENSIZE", &[]).unwrap_err(),
        IptError::CommandUnavailable {
            command: "PENSIZE".to_owned(),
        },
        "the adapter refuses names the trait cannot serve rather than ignoring them"
    );
}

#[test]
fn every_implemented_command_is_served_by_the_adapter_not_refused() {
    for name in IMPLEMENTED {
        let mut host = adapter();
        let result = host.command(name, &[]);
        assert!(
            !matches!(result, Err(IptError::CommandUnavailable { .. })),
            "{name} is advertised as implemented but the adapter refuses it: {result:?}"
        );
    }
}

#[test]
fn command_pops_reports_the_registrys_operand_count_for_implemented_names() {
    let host = adapter();
    for name in IMPLEMENTED {
        let spec = command_spec(name).expect("an implemented command must be registered");
        assert_eq!(
            host.command_pops(name),
            usize::from(spec.pops),
            "{name} pops disagree with the registry"
        );
    }
    assert_eq!(
        host.command_pops("PENSIZE"),
        0,
        "an unimplemented name consumes nothing, so a fault surfaces cleanly"
    );
    assert_eq!(host.command_pops("not_a_command"), 0);
}

#[test]
fn the_adapter_forwards_the_host_primitives_to_the_wrapped_host() {
    let mut host = adapter();
    assert_eq!(host.random(9), 10);
    assert_eq!(host.ticks(), 77);
    assert_eq!(host.datetime(), 1234);
    host.trace("note");
    assert_eq!(
        host.grep_match("p", "t").unwrap(),
        Some(vec!["p:t".to_owned()])
    );
    host.schedule_alarm(5, Chunk::empty(), 3).unwrap();

    let inner = host.inner;
    assert_eq!(inner.traced, vec!["note".to_owned()]);
    assert_eq!(inner.alarm, Some((5, 3)));
}

#[test]
fn new_into_inner_and_default_expose_the_wrapped_host() {
    let host = PalaceCommands::new(Featured::default());
    assert_eq!(host.into_inner().get_self_user_id(), 99);

    let host: PalaceCommands<Featured> = PalaceCommands::default();
    assert!(host.inner.spoke.is_empty());
    assert_eq!(host.inner.get_self_user_name(), "Nova");
}

#[test]
fn command_spec_lookup_is_case_insensitive_and_rejects_unknown_names() {
    assert_eq!(command_spec("SAY").map(|s| s.name), Some("SAY"));
    assert_eq!(command_spec("say").map(|s| s.name), Some("SAY"));
    assert_eq!(command_spec("SaY").map(|s| s.name), Some("SAY"));
    assert!(command_spec("NOT A PALACE COMMAND").is_none());
}

#[test]
fn register_palace_commands_installs_every_name_and_is_idempotent() {
    let mut set = CommandSet::core();
    let core = CommandSet::core();
    assert!(!set.contains("SGLOBAL"), "SGLOBAL is a Palace addition");

    let added = register_palace_commands(&mut set);
    assert_eq!(PALACE_COMMANDS.len(), 121 + 12, "12 sourced additions");
    assert_eq!(
        added,
        120 + 12,
        "ALARMEXEC/IPTVERSION overlap core; SGLOBAL adds one"
    );
    assert_eq!(
        set.get("SGLOBAL"),
        Some(CommandKind::Builtin(Builtin::Global)),
        "SGLOBAL is the spot-scope alias of GLOBAL"
    );
    for spec in PALACE_COMMANDS {
        let kind = set.get(spec.name);
        if core.contains(spec.name) && spec.name != "IPTVERSION" {
            assert_ne!(kind, Some(CommandKind::Host), "core {} must win", spec.name);
        } else {
            assert_eq!(kind, Some(CommandKind::Host), "{} must register", spec.name);
        }
    }

    // Names already present in the core dictionary gain nothing.
    let expected = usize::from(!core.contains("SGLOBAL"))
        + PALACE_COMMANDS
            .iter()
            .filter(|spec| !core.contains(spec.name))
            .count();
    assert_eq!(
        added, expected,
        "added count must match the newly registered names"
    );

    assert_eq!(
        register_palace_commands(&mut set),
        0,
        "a second registration adds nothing"
    );
    assert_eq!(
        set.get("GREPSTR"),
        Some(CommandKind::Builtin(Builtin::GrepStr)),
        "a core command keeps its core binding"
    );
}

#[test]
fn extended_signatures_match_the_sparky_gs_handlers() {
    for (name, pops, pushes, push) in [
        ("AUTOUSERLAYER", 1, 0, Push::None),
        ("SETTOOLTIP", 1, 0, Push::None),
        ("CLEARTOOLTIP", 0, 0, Push::None),
        ("SETSPOTOPTIONS", 4, 0, Push::None),
        ("ADDPIC", 2, 0, Push::None),
        ("REMOVEPIC", 2, 0, Push::None),
        ("ADDSPOT", 3, 1, Push::Int),
        ("SETSPOTSCRIPT", 3, 0, Push::None),
        ("LOADSCRIPT", 1, 0, Push::None),
        ("HTTPGET", 1, 0, Push::None),
        ("BAN", 1, 0, Push::None),
        ("KICK", 1, 0, Push::None),
        ("SHELLCMD", 1, 0, Push::None),
    ] {
        let spec = command_spec(name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(
            (spec.pops, spec.pushes, spec.push),
            (pops, pushes, push),
            "{name}"
        );
    }
}

fn assert_script_dispatches(name: &str, operands: &str, results: &[Value]) {
    let source = format!("777 {operands} {name}");
    let mut engine = SkeletonHost::engine();
    let stack = engine.run_source_resolved(&source).unwrap();
    assert_eq!(
        engine.host.invocations.get(name),
        Some(&1),
        "{name} must dispatch, not resolve as a variable; stack: {stack:?}"
    );
    let mut expected = vec![Value::Int(777)];
    expected.extend_from_slice(results);
    assert_eq!(stack, expected, "{name} must preserve the stack sentinel");

    let mut unavailable = Engine::new(adapter()).with_commands(SkeletonHost::command_set());
    assert_eq!(
        unavailable.run_source(&source).unwrap_err(),
        IptError::CommandUnavailable {
            command: name.to_owned()
        }
        .in_command(name),
        "{name} must report unsupported rather than silently become a variable"
    );
}

#[test]
fn newly_registered_addpic_dispatches() {
    assert_script_dispatches("ADDPIC", "\"picture.png\" 7", &[]);
}

#[test]
fn newly_registered_httpget_dispatches() {
    assert_script_dispatches("HTTPGET", "\"https://example.invalid/data\"", &[]);
}

#[test]
fn newly_registered_settooltip_dispatches() {
    assert_script_dispatches("SETTOOLTIP", "\"hint\"", &[]);
}

#[test]
fn newly_registered_addspot_dispatches() {
    assert_script_dispatches("ADDSPOT", "[0 0 10 0 10 10] 20 30", &[Value::Int(0)]);
}

#[test]
fn newly_registered_cleartooltip_dispatches() {
    assert_script_dispatches("CLEARTOOLTIP", "", &[]);
}

#[test]
fn shellcmd_already_dispatches_but_remains_unavailable() {
    assert_script_dispatches("SHELLCMD", "\"not executed\"", &[]);
}

#[test]
fn registry_specs_are_self_consistent_about_pushes() {
    for spec in PALACE_COMMANDS {
        if spec.pushes == 0 {
            assert_eq!(
                spec.push,
                Push::None,
                "{} pushes nothing but declares a push kind",
                spec.name
            );
        } else {
            assert_ne!(
                spec.push,
                Push::None,
                "{} pushes {} value(s) but declares no kind",
                spec.name,
                spec.pushes
            );
        }
    }
}

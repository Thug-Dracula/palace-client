//! The Palace command surface: the capability trait, the command registry and
//! the harness adapter.
//!
//! The pure-language conformance suite lives in `iptscrae/tests/conformance.rs`;
//! these cases exercise the layer that turns [`PalaceHost`] methods into
//! IPTSCRAE commands.

use iptscrae::{Builtin, CommandKind, CommandSet, Engine, Limits, Op, Value};
use iptscrae_palace::commands::PALACE_COMMANDS;
use iptscrae_palace::harness::SkeletonHost;
use iptscrae_palace::{command_spec, register_palace_commands, PalaceCommands, PalaceHost};

#[test]
fn a_palace_command_reaches_the_capability_trait() {
    #[derive(Default)]
    struct Client {
        spoke: Vec<String>,
        room: i64,
    }
    impl iptscrae::Host for Client {}
    impl PalaceHost for Client {
        fn chat(&mut self, text: &str) -> iptscrae::Result<()> {
            self.spoke.push(text.to_owned());
            Ok(())
        }
        fn goto_room(&mut self, room: i64) -> iptscrae::Result<()> {
            self.room = room;
            Ok(())
        }
    }

    let mut engine = Engine::new(PalaceCommands::new(Client::default()))
        .with_commands(SkeletonHost::command_set());
    engine.run_source("\"hello\" SAY 817 GOTOROOM").unwrap();
    assert_eq!(engine.host.inner.spoke, vec!["hello".to_owned()]);
    assert_eq!(engine.host.inner.room, 817);
}

#[test]
fn sayat_formats_the_balloon_position_the_way_the_reference_does() {
    #[derive(Default)]
    struct Client {
        spoke: Vec<String>,
    }
    impl iptscrae::Host for Client {}
    impl PalaceHost for Client {
        fn chat(&mut self, text: &str) -> iptscrae::Result<()> {
            self.spoke.push(text.to_owned());
            Ok(())
        }
    }

    let mut engine = Engine::new(PalaceCommands::new(Client::default()))
        .with_commands(SkeletonHost::command_set());
    engine
        .run_source("\"hello\" 10 40 SAYAT \"plain\" SAY")
        .unwrap();
    assert_eq!(
        engine.host.inner.spoke,
        vec!["@10,40 hello".to_owned(), "plain".to_owned()]
    );
}

#[test]
fn userid_and_username_read_through_the_trait() {
    struct Client;
    impl iptscrae::Host for Client {}
    impl PalaceHost for Client {
        fn get_self_user_id(&self) -> i64 {
            4242
        }
        fn get_self_user_name(&self) -> String {
            "Ensi".to_owned()
        }
    }

    let mut engine =
        Engine::new(PalaceCommands::new(Client)).with_commands(SkeletonHost::command_set());
    assert_eq!(
        engine.run_source_resolved("WHOME").unwrap(),
        vec![Value::Int(4242)]
    );
    assert_eq!(
        engine.run_source_resolved("USERNAME").unwrap(),
        vec![Value::str("Ensi")]
    );
}

#[test]
fn unimplemented_palace_commands_are_refused_not_ignored() {
    struct Client;
    impl iptscrae::Host for Client {}
    impl PalaceHost for Client {}

    let mut engine =
        Engine::new(PalaceCommands::new(Client)).with_commands(SkeletonHost::command_set());
    let error = engine.run_source("\"x\" PENSIZE").unwrap_err();
    assert_eq!(error.category(), "host");
    assert!(format!("{error}").contains("PENSIZE"), "{error}");
}

#[test]
fn palace_command_names_lex_as_commands_not_variables() {
    let commands = SkeletonHost::command_set();
    let chunk = iptscrae::parse_body("SAY SETPOS GOTOROOM", &commands, &Limits::default()).unwrap();
    assert_eq!(chunk.ops().len(), 3);
    for op in chunk.ops() {
        assert!(matches!(op, Op::Host(_)), "{op:?} should be a command");
    }
}

#[test]
fn chatstr_is_host_provided_as_a_string() {
    let mut engine = SkeletonHost::engine();
    assert_eq!(
        engine.run_source_resolved("CHATSTR TOPTYPE").unwrap(),
        vec![Value::str(""), Value::Int(2)],
        "TOPTYPE sees the variable reference, as in the reference VM"
    );
    assert_eq!(
        engine.run_source_resolved("CHATSTR VARTYPE").unwrap(),
        vec![Value::str(""), Value::Int(4)],
        "VARTYPE dereferences it, and it is a string rather than a zero"
    );
}

#[test]
fn sglobal_is_the_spot_scope_alias_of_global() {
    let mut engine = SkeletonHost::engine();
    assert_eq!(
        engine
            .run_source_resolved("5 v = { 1 } v DUP SGLOBAL IF")
            .unwrap(),
        vec![Value::Int(1)],
        "SGLOBAL consumes one copy of the variable and leaves the other as a condition"
    );

    engine.run_source("9 v = v DUP SGLOBAL").unwrap();
    assert_eq!(
        engine.run_source_resolved("v GLOBAL v").unwrap(),
        vec![Value::Int(9)],
        "SGLOBAL promoted the variable the same way GLOBAL does"
    );
}

#[test]
fn every_registered_palace_command_has_a_unique_upper_case_name() {
    let mut seen = std::collections::BTreeSet::new();
    for cmd in PALACE_COMMANDS {
        assert!(
            seen.insert(cmd.name),
            "duplicate registered name {}",
            cmd.name
        );
        assert_eq!(cmd.name, cmd.name.to_ascii_uppercase());
        assert!(command_spec(&cmd.name.to_ascii_lowercase()).is_some());
    }
}

#[test]
fn implemented_commands_are_a_subset_of_the_registered_ones() {
    for name in iptscrae_palace::commands::IMPLEMENTED {
        assert!(
            command_spec(name).is_some(),
            "{name} is implemented but not registered"
        );
    }
}

#[test]
fn registration_is_idempotent_and_leaves_core_commands_alone() {
    let mut commands = CommandSet::core();
    let first = register_palace_commands(&mut commands);
    let second = register_palace_commands(&mut commands);
    assert!(first > 0);
    assert_eq!(second, 0, "nothing new the second time");
    assert_eq!(
        commands.get("GREPSTR"),
        Some(CommandKind::Builtin(Builtin::GrepStr)),
        "a core command keeps its core binding"
    );
}

#[test]
fn the_skeleton_stubs_consume_the_documented_operand_count() {
    let mut engine = SkeletonHost::engine();
    // SETSPOTSTATE is `state spotId SETSPOTSTATE`: two operands, no result.
    assert_eq!(
        engine.run_source_resolved("3 4 SETSPOTSTATE 99").unwrap(),
        vec![Value::Int(99)]
    );
    // MOUSEPOS pushes a pair.
    assert_eq!(
        engine.run_source_resolved("MOUSEPOS").unwrap(),
        vec![Value::Int(0), Value::Int(0)]
    );
}

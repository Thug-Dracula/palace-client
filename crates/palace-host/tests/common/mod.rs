//! Shared fixtures for the `palace-host` integration tests.
//!
//! Every helper here is deterministic: the recorded room comes from the capture
//! on disk, and the synthetic view/room builders use fixed ids and positions, so
//! a test can assert an exact effect list rather than "something happened".

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use iptscrae::{Host, Value};
use palace_host::{
    AssetFacts, Effect, HostView, LoosePropView, PropFacts, ScriptHost, SpotView, UserView,
};
use palace_room::{Hotspot, RoomDesc, RoomRec};
use palace_wire::byteorder::ByteOrder;
use palace_wire::{frame::Frame, opcode};

/// The real recorded `MSG_ROOMDESC` for Balamb Garden (room 901).
///
/// Same capture `tests/fixture_room.rs` uses, so host commands are exercised
/// against real decoded room data (six scripted hotspots, loose props, states).
pub fn room_901() -> RoomDesc {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/logon-run1/frames/0007-server-room.bin");
    let bytes = std::fs::read(&path).expect("recorded room frame");
    let frame = Frame::decode_from(&bytes, ByteOrder::Little).expect("frame decodes");
    assert_eq!(frame.opcode, opcode::ROOMDESC);
    palace_room::decode_payload(&frame.payload, ByteOrder::Little).expect("room decodes")
}

/// A host over the recorded room, with the same self user the wire tests use.
pub fn room_901_host() -> ScriptHost {
    let room = room_901();
    let mut view = HostView {
        self_id: 13,
        self_name: "RustProbe".to_owned(),
        self_x: 256,
        self_y: 192,
        room_id: 901,
        room_name: "Balamb Garden".to_owned(),
        room_width: 512,
        room_height: 384,
        ..HostView::default()
    };
    view.apply_room(&room);
    ScriptHost::new(view)
}

/// A fully populated view: two users, three spots (one door), two loose props.
///
/// The values are chosen so every getter has both a hit and a miss: ids `5`/`2`/
/// `3` exist, `99` does not; the door is `5`; self stands inside spot `3`.
pub fn populated_view() -> HostView {
    HostView {
        self_id: 13,
        self_name: "RustProbe".to_owned(),
        self_x: 256,
        self_y: 192,
        self_props: vec![7, 9],
        room_id: 901,
        room_name: "Balamb Garden".to_owned(),
        server_name: "TestServer".to_owned(),
        room_width: 512,
        room_height: 384,
        is_guest: false,
        is_wizard: true,
        is_god: true,
        mouse: (100, 200),
        right_click: true,
        chat_string: "hi".to_owned(),
        who_chat: 7,
        who_target: 9,
        face: 3,
        color: 4,
        users: vec![
            UserView {
                id: 13,
                name: "RustProbe".to_owned(),
                x: 256,
                y: 192,
                props: vec![7, 9],
            },
            UserView {
                id: 7,
                name: "Squall".to_owned(),
                x: 10,
                y: 20,
                props: vec![3],
            },
        ],
        spots: vec![
            SpotView {
                id: 5,
                kind: 1,
                dest: 817,
                name: "Door".to_owned(),
                state: 1,
                nbr_states: 2,
                state_pics: vec![(1, 2, 3), (2, 4, 5)],
                loc: (50, 60),
                ..SpotView::default()
            },
            SpotView {
                id: 2,
                kind: 0,
                name: "Note".to_owned(),
                nbr_states: 1,
                loc: (100, 100),
                ..SpotView::default()
            },
            SpotView {
                id: 3,
                kind: 0,
                name: "Center".to_owned(),
                nbr_states: 1,
                loc: (256, 192),
                ..SpotView::default()
            },
        ],
        loose_props: vec![
            LoosePropView {
                id: 0x4000_0001,
                x: 10,
                y: 20,
            },
            LoosePropView {
                id: 99,
                x: 30,
                y: 40,
            },
        ],
        assets: Arc::new(AssetFacts {
            pic_dims: BTreeMap::from([(1, (10, 20)), (2, (30, 40))]),
            prop_facts: BTreeMap::from([(
                7,
                PropFacts {
                    width: 100,
                    height: 50,
                    h_offset: 40,
                    v_offset: 25,
                },
            )]),
        }),
    }
}

/// A host over [`populated_view`].
pub fn populated_host() -> ScriptHost {
    ScriptHost::new(populated_view())
}

/// Run one command that must record exactly one effect and push nothing.
pub fn effect_of(name: &str, args: &[Value]) -> Effect {
    let mut host = populated_host();
    let pushed = host.command(name, args).expect("command succeeds");
    assert!(
        pushed.is_empty(),
        "{name} must push nothing, got {pushed:?}"
    );
    let mut effects = host.take_effects();
    assert_eq!(effects.len(), 1, "{name} records one effect: {effects:?}");
    effects.pop().expect("one effect")
}

/// Run one command and return the values it pushed; asserts no effect recorded.
pub fn pushed(name: &str, args: &[Value]) -> Vec<Value> {
    let mut host = populated_host();
    let pushed = host.command(name, args).expect("command succeeds");
    assert!(
        host.effects.is_empty(),
        "{name} must not record effects: {:?}",
        host.effects
    );
    pushed
}

/// `Value::Int` for each element.
pub fn ints(ns: &[i32]) -> Vec<Value> {
    ns.iter().map(|n| Value::Int(i64::from(*n))).collect()
}

/// `Value::str` for each element.
pub fn strs(ss: &[&str]) -> Vec<Value> {
    ss.iter().map(|s| Value::str(*s)).collect()
}

/// A synthetic room with one hotspot per entry. `None` means "no script text";
/// `Some("")` means an empty script.
pub fn room_with_scripts(scripts: &[(i16, Option<&str>)]) -> RoomDesc {
    let hotspots: Vec<Hotspot> = scripts
        .iter()
        .map(|(id, source)| Hotspot {
            id: *id,
            script: source.map(str::to_owned),
            ..Hotspot::default()
        })
        .collect();
    RoomDesc {
        header: RoomRec {
            room_id: 901,
            nbr_hotspots: hotspots.len() as i16,
            ..RoomRec::default()
        },
        name: "Test Room".to_owned(),
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

/// A command set with the Palace names registered, as dispatch uses.
pub fn palace_commands() -> iptscrae::registry::CommandSet {
    let mut set = iptscrae::registry::CommandSet::core();
    iptscrae_palace::commands::register_palace_commands(&mut set);
    set
}

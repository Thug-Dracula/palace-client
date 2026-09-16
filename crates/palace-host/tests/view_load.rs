//! `view` lookups and `load` parsing — the parts of the crate outside the
//! command table.
//!
//! These are the seams the host sits on: `HostView` decides what a script can
//! see, and `load` decides which hotspot scripts become runnable handlers. Both
//! are tested against the recorded room and against synthetic rows built to hit
//! the "empty" and "malformed" branches.

mod common;

use common::*;
use iptscrae::budget::Limits;
use palace_host::{
    cyborg_script, point_in_polygon, scripts_from_room, HostView, LoadProblem, LoosePropView,
    SpotView,
};
use palace_room::{Hotspot, HotspotState, LooseProp, LoosePropSpec, Point};

// -------------------------------------------------------------------- lookups

#[test]
fn spot_lookup_and_index() {
    let view = populated_view();
    assert_eq!(view.spot(5).map(|s| s.name.as_str()), Some("Door"));
    assert!(view.spot(99).is_none());
    assert_eq!(view.spot_index(5), 0);
    assert_eq!(view.spot_index(2), 1);
    assert_eq!(view.spot_index(99), -1);
}

#[test]
fn user_lookup_by_id_and_by_name_is_case_insensitive() {
    let view = populated_view();
    assert_eq!(view.user(13).map(|u| u.name.as_str()), Some("RustProbe"));
    assert!(view.user(99).is_none());
    assert_eq!(view.user_by_name("squall"), 7);
    assert_eq!(view.user_by_name("SQUALL"), 7);
    assert_eq!(view.user_by_name("nobody"), 0);
}

#[test]
fn loose_prop_index_lookup() {
    let view = populated_view();
    assert_eq!(view.loose_prop_index(0x4000_0001), 0);
    assert_eq!(view.loose_prop_index(99), 1);
    assert_eq!(view.loose_prop_index(123), -1);
}

#[test]
fn self_in_spot_uses_hotspot_containment() {
    let view = populated_view();
    assert!(view.self_in_spot(3), "self stands on spot 3's loc");
    assert!(!view.self_in_spot(2), "spot 2 is far away");
    assert!(!view.self_in_spot(99), "no such spot");
}

#[test]
fn sole_door_is_only_reported_when_there_is_exactly_one() {
    assert_eq!(populated_view().sole_door(), Some(5));

    let no_doors = HostView {
        spots: vec![SpotView {
            id: 1,
            kind: 0,
            ..SpotView::default()
        }],
        ..HostView::default()
    };
    assert_eq!(no_doors.sole_door(), None);

    let shuttable = HostView {
        spots: vec![SpotView {
            id: 1,
            kind: 2,
            ..SpotView::default()
        }],
        ..HostView::default()
    };
    assert_eq!(
        shuttable.sole_door(),
        None,
        "a shuttable door is not a door"
    );

    let two_doors = HostView {
        spots: vec![
            SpotView {
                id: 1,
                kind: 1,
                ..SpotView::default()
            },
            SpotView {
                id: 2,
                kind: 1,
                ..SpotView::default()
            },
        ],
        ..HostView::default()
    };
    assert_eq!(two_doors.sole_door(), None);
}

// ------------------------------------------------------------------ geometry

#[test]
fn point_in_polygon_rejects_outlines_that_are_not_polygons() {
    assert!(!point_in_polygon(0, 0, &[]));
    assert!(!point_in_polygon(0, 0, &[(0, 0)]));
    assert!(!point_in_polygon(0, 0, &[(0, 0), (10, 10)]));
    let square = [(0, 0), (10, 0), (10, 10), (0, 10)];
    assert!(point_in_polygon(5, 5, &square));
    assert!(!point_in_polygon(15, 5, &square));
}

#[test]
fn a_pointed_hotspot_falls_back_to_the_loc_box() {
    // The outline is a tiny triangle a thousand pixels away; neither the
    // absolute nor the loc-relative reading contains the test point, so the
    // 44x44 avatar box around `loc` is what answers.
    let spot = SpotView {
        id: 9,
        points: vec![(1000, 1000), (1010, 1000), (1000, 1010)],
        loc: (10, 10),
        ..SpotView::default()
    };
    assert!(spot.contains(10, 10));
    assert!(spot.contains(20, 20));
    assert!(!spot.contains(100, 100));
}

#[test]
fn from_hotspot_copies_every_script_visible_field() {
    let hotspot = Hotspot {
        id: 5,
        dest: 817,
        hotspot_type: 1,
        state: 2,
        nbr_states: 3,
        flags: 0x12,
        group_id: 4,
        // Point::new(v, h) — the wire names are vertical-then-horizontal.
        loc: Point::new(60, 50),
        points: vec![Point::new(2, 1), Point::new(4, 3)],
        states: vec![HotspotState {
            pict_id: 9,
            reserved: 0,
            pic_loc: Point::new(7, 8),
        }],
        name: Some("Door".to_owned()),
        ..Hotspot::default()
    };
    let spot = SpotView::from_hotspot(&hotspot);
    assert_eq!(spot.id, 5);
    assert_eq!(spot.kind, 1);
    assert_eq!(spot.dest, 817);
    assert_eq!(spot.name, "Door");
    assert_eq!(spot.state, 2);
    assert_eq!(spot.nbr_states, 3);
    assert_eq!(spot.flags, 0x12);
    assert_eq!(spot.group_id, 4);
    assert_eq!(spot.loc, (50, 60));
    assert_eq!(spot.points, vec![(1, 2), (3, 4)]);
    assert_eq!(spot.state_pics, vec![(9, 8, 7)]);

    // An absent name becomes the empty string.
    let unnamed = SpotView::from_hotspot(&Hotspot {
        id: 6,
        ..Hotspot::default()
    });
    assert_eq!(unnamed.name, "");
    assert!(unnamed.points.is_empty());
    assert!(unnamed.state_pics.is_empty());
}

#[test]
fn apply_room_maps_the_recorded_room_row_for_row() {
    let room = room_901();
    let mut view = HostView::default();
    view.apply_room(&room);
    assert_eq!(view.room_id, 901);
    assert_eq!(view.room_name, "Balamb Garden");
    assert_eq!(view.spots.len(), room.hotspots.len());
    assert_eq!(view.loose_props.len(), room.loose_props.len());
    for (spot_view, hotspot) in view.spots.iter().zip(&room.hotspots) {
        assert_eq!(spot_view.id, i32::from(hotspot.id));
        assert_eq!(spot_view.kind, i32::from(hotspot.hotspot_type));
        assert_eq!(spot_view.dest, i32::from(hotspot.dest));
    }
    for (prop_view, prop) in view.loose_props.iter().zip(&room.loose_props) {
        assert_eq!(prop_view.id, i64::from(prop.spec.id));
        assert_eq!(prop_view.x, i32::from(prop.loc.h));
        assert_eq!(prop_view.y, i32::from(prop.loc.v));
    }
}

#[test]
fn apply_room_maps_loose_props() {
    // The recorded room has no loose props, so the mapping is exercised with
    // synthetic rows.
    let mut room = room_with_scripts(&[]);
    room.loose_props = vec![
        LooseProp {
            spec: LoosePropSpec {
                id: 0x4000_0001,
                crc: 0,
            },
            loc: Point::new(20, 10),
            ..LooseProp::default()
        },
        LooseProp {
            spec: LoosePropSpec { id: 77, crc: 0 },
            loc: Point::new(40, 30),
            ..LooseProp::default()
        },
    ];
    let mut view = HostView::default();
    view.apply_room(&room);
    assert_eq!(
        view.loose_props,
        vec![
            LoosePropView {
                id: 0x4000_0001,
                x: 10,
                y: 20
            },
            LoosePropView {
                id: 77,
                x: 30,
                y: 40
            },
        ]
    );
}

// ----------------------------------------------------------------------- load

#[test]
fn scripts_from_room_keeps_parsed_scripts_and_reports_the_rest() {
    let commands = palace_commands();
    let limits = Limits::default();
    let room = room_with_scripts(&[
        (1, Some("ON SELECT { \"hi\" SAY }")),
        (2, None),                     // no script text at all: skipped, not a problem
        (3, Some("   ")),              // blank script: skipped
        (4, Some("1 2 +")),            // non-empty text with no handlers: a problem
        (5, Some("ON SELECT { $1 }")), // a real syntax error
    ]);
    let (loaded, problems) = scripts_from_room(&room, &commands, &limits);
    assert_eq!(loaded.len(), 1, "only hotspot 1 is runnable");
    assert_eq!(loaded[0].spot, 1);
    assert_eq!(loaded[0].source, "ON SELECT { \"hi\" SAY }");
    assert!(loaded[0].script.handler("SELECT").is_some());
    assert_eq!(problems.len(), 2);
    assert_eq!(
        problems[0],
        LoadProblem {
            spot: 4,
            error: "no ON handlers in script text".to_owned()
        }
    );
    assert_eq!(problems[1].spot, 5);
    assert!(!problems[1].error.is_empty(), "{:?}", problems[1]);
}

#[test]
fn cyborg_script_uses_spot_zero_and_the_same_problem_rules() {
    let commands = palace_commands();
    let limits = Limits::default();

    let ok = cyborg_script("ON ENTER { \"hi\" SAY }", &commands, &limits)
        .expect("a cyborg script with a handler parses");
    assert_eq!(ok.spot, 0);
    assert!(ok.script.handler("ENTER").is_some());

    let empty = cyborg_script("1 2 +", &commands, &limits).expect_err("no handlers");
    assert_eq!(
        empty,
        LoadProblem {
            spot: 0,
            error: "no ON handlers in cyborg script".to_owned()
        }
    );

    let bad = cyborg_script("ON ENTER { $1 }", &commands, &limits).expect_err("syntax error");
    assert_eq!(bad.spot, 0);
    assert!(!bad.error.is_empty());
}

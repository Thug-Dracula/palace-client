//! Regression fixture for the `comma_separator` corpus extension.
//!
//! The live Colosseum server serves bodies that separate values with commas —
//! `media_custo2.txt`'s arena line. The reference `IptParser.as` has no comma
//! branch, so this is a recorded production-tokenizer extension, not reference
//! behaviour. The fixture is committed so the guarantee holds without the
//! harvested corpus.

use iptscrae::budget::Limits;
use iptscrae::value::Op;
use iptscrae_palace::harness::SkeletonHost;

const ARENA: &str = include_str!("fixtures/media_custo2_arena.txt");

#[test]
fn parse_body_accepts_the_arena_lines() {
    let ops = iptscrae::parse_body(ARENA, &SkeletonHost::command_set(), &Limits::default())
        .expect("the committed arena fixture lexes")
        .ops()
        .to_vec();

    let ints: Vec<i64> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Int(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(
        ints,
        vec![1000, 0, 537, 0, 537, 363, 933, 363, 933, 396, 1000, 396, 0, 0, 768, 198, 0, 2, 1, 1],
        "the comma-separated polygon points, spot x,y, picture x,y, and SETSPOTOPTIONS operands"
    );
    for name in ["ADDSPOT", "ADDPIC", "SETPICLOCLOCAL", "SETSPOTOPTIONS"] {
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Host(n) if &**n == name)),
            "{name} must resolve to a host command, not be swallowed by a comma"
        );
    }
}

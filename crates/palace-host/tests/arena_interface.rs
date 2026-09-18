//! End-to-end acceptance for the Colosseum arena's fetched interface script.
//!
//! Room 7774's `ON ENTER` runs `"http://chat.animanic.de/media/custo2.txt"
//! LOADSCRIPT`. The server serves that body with `,` separators (the
//! `comma_separator` corpus extension), and its top-level statements are what
//! build the room's arena panel: `ADDSPOT`, `ADDPIC`, `SETPICLOCLOCAL` and
//! `SETSPOTOPTIONS`.
//!
//! This pins the whole chain rather than the pieces: the served text must lex,
//! run as a bare body through the host, and emit the effects that create the
//! panel. Before the lexer accepted commas it died at the first `,`; before the
//! dynamic-content effects existed it reported every one of those commands
//! unsupported.
//!
//! The two-line fixture is the hermetic case. The full harvest is exercised
//! when it is present, because it is the genuine article and it carries the
//! remainder of the interface (the `DEF`s, the character menu, and the
//! `SETSPOTSCRIPT` calls that attach the menu's handlers). Those calls must
//! yield [`Effect::SetSpotScript`], not `Effect::Unsupported`: the arena panel
//! is dead until `ON MOUSEMOVE`/`ON ROLLOUT`/`ON SELECT` exist for it.

use palace_host::{Effect, ScriptEngine};

/// The served lines that build the panel, verbatim from `media_custo2.txt`
/// lines 1-4 (the blank line between them is not significant).
const ARENA_INTERFACE: &str = "\
but1 GLOBAL but2 GLOBAL
[1000,0 537,0 537,363 933,363 933,396 1000,396] 0,0 ADDSPOT but1 = \"cust.gif\" but1 ADDPIC 768,198 0 but1 SETPICLOCLOCAL
2 1 1 but1 SETSPOTOPTIONS
";

const ARENA_HARVEST: &str = "$CORPUS/http_harvest/media_custo2.txt";

struct Panel {
    polygon: Vec<(i32, i32)>,
    picture: Option<String>,
    offset: Option<(i32, i32)>,
    options: bool,
}

fn interface_effects(effects: &[Effect]) -> Panel {
    let polygon = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::AddSpot { points, .. } => Some(points.clone()),
            _ => None,
        })
        .expect("ADDSPOT ran");
    let picture = effects.iter().find_map(|effect| match effect {
        Effect::AddPic { name, .. } => Some(name.clone()),
        _ => None,
    });
    let offset = effects.iter().find_map(|effect| match effect {
        Effect::SetPicOffsetLocal { dx, dy, .. } => Some((*dx, *dy)),
        _ => None,
    });
    let options = effects
        .iter()
        .any(|effect| matches!(effect, Effect::SetSpotOptions { .. }));
    Panel {
        polygon,
        picture,
        offset,
        options,
    }
}

#[test]
fn the_arena_interface_lines_build_the_panel_through_the_host() {
    let mut engine = ScriptEngine::with_palace_limits();
    let run = engine.execute_fetched_source(ARENA_INTERFACE, 7774);
    assert!(
        run.error.is_none(),
        "the served script must run, not fail: {:?}",
        run.error
    );

    let panel = interface_effects(&run.effects);
    assert_eq!(
        panel.polygon.len(),
        6,
        "the panel outline is six x,y pairs, so a comma must separate tokens"
    );
    assert_eq!(
        (panel.polygon[0], panel.polygon[5]),
        ((1000, 0), (1000, 396)),
        "the polygon is in x,y order"
    );
    assert_eq!(
        panel.picture.as_deref(),
        Some("cust.gif"),
        "ADDPIC appended a picture"
    );
    assert_eq!(panel.offset, Some((768, 198)), "SETPICLOCLOCAL moved it");
    assert!(panel.options, "SETSPOTOPTIONS configured the panel");
}

/// The genuine 25,620-byte body, skipped when the harvest is absent.
///
/// The full script is the real acceptance case: it is what the server actually
/// serves, and it is longer than any fixture. It must run to completion and its
/// four `SETSPOTSCRIPT` calls must attach the panel's handlers, so this pins the
/// panel *and* the handlers that make it interactive.
#[test]
fn the_full_harvest_builds_the_panel_when_it_is_present() {
    let Ok(bytes) = std::fs::read(ARENA_HARVEST) else {
        eprintln!("{ARENA_HARVEST} is absent; skipping the full-harvest check");
        return;
    };
    let source = iptscrae::decode_source(&bytes);
    let mut engine = ScriptEngine::with_palace_limits();
    let run = engine.execute_fetched_source(&source, 7774);

    assert!(
        run.error.is_none(),
        "the real body runs to completion: {:?}",
        run.error
    );

    let panel = interface_effects(&run.effects);
    assert_eq!(
        panel.polygon.len(),
        6,
        "the real body has the same six-point outline"
    );
    assert_eq!(panel.picture.as_deref(), Some("cust.gif"));
    assert_eq!(panel.offset, Some((768, 198)));
    assert!(panel.options);

    let created = run
        .effects
        .iter()
        .find_map(|effect| match effect {
            Effect::AddSpot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("ADDSPOT ran");
    let attached: Vec<(i32, String)> = run
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::SetSpotScript { spot, event, .. } => Some((*spot, event.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        attached.len(),
        4,
        "the body attaches four handlers: {attached:?}"
    );
    for event in ["MOUSEMOVE", "ROLLOUT", "SELECT"] {
        assert!(
            attached
                .iter()
                .any(|(spot, name)| *spot == created && name == event),
            "hotspot {created} must get an ON {event} handler: {attached:?}"
        );
    }
    assert!(
        !run.effects.iter().any(|effect| matches!(
            effect,
            Effect::Unsupported { command } if command == "SETSPOTSCRIPT"
        )),
        "SETSPOTSCRIPT must no longer fall through to Unsupported"
    );
}

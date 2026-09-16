//! Event dispatch over real room data.
//!
//! For every hotspot script reachable from the harvested corpus, this looks the
//! handler up **by event name** and runs it — the dispatch path an interactive
//! client needs, exercised end to end without a server or a window.
//!
//! Skipped when the corpus is absent. Point `PALACE_SCRIPTS_CORPUS` at the
//! `colosseum` root to use a different corpus.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::{parse_script, Engine};
use iptscrae_palace::harness::SkeletonHost;
use palace_room::decode_stream;
use palace_wire::ByteOrder;

/// The event names the harvested corpus actually uses, including the corpus's
/// own spelling of `HTTPRECIEVED`.
const EVENTS: &[&str] = &[
    "ENTER",
    "SELECT",
    "LEAVE",
    "OUTCHAT",
    "ALARM",
    "INCHAT",
    "ROLLOVER",
    "ROLLOUT",
    "ROOMREADY",
    "NAMECHANGE",
    "ROOMLOAD",
    "KEYDOWN",
    "SERVERMSG",
    "LOCK",
    "UNLOCK",
    "MOUSEUP",
    "MOUSEDRAG",
    "USERLEAVE",
    "STATECHANGE",
    "SIGNON",
    "MOUSEMOVE",
    "HTTPRECEIVED",
    "HTTPRECIEVED",
    "HTTPERROR",
];

fn corpus_root() -> Option<PathBuf> {
    let root = std::env::var("PALACE_SCRIPTS_CORPUS")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join("colosseum"))
        })?;
    root.join("payloads_all").is_dir().then_some(root)
}

#[test]
fn hotspot_scripts_dispatch_by_event_name_over_the_corpus() {
    let Some(root) = corpus_root() else {
        eprintln!("corpus absent; skipping");
        return;
    };

    let limits = Limits::default().with_dialect(StackDialect::PalaceChat);
    let commands = SkeletonHost::command_set();
    let mut engine = Engine::new(SkeletonHost::seeded(11))
        .with_limits(limits)
        .with_commands(commands.clone());

    let mut payloads_with_scripts = 0usize;
    let mut scripts = 0usize;
    let mut unparsed = 0usize;
    let mut dispatched = 0usize;
    let mut failed = 0usize;
    let mut by_event: BTreeMap<&str, usize> = BTreeMap::new();

    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("payloads_all"))
        .expect("payload dir must be readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    files.sort();

    for path in files {
        let bytes = std::fs::read(&path).expect("payload must be readable");
        let mut touched = false;
        for record in decode_stream(&bytes, ByteOrder::Little).into_iter().flatten() {
            for spot in &record.hotspots {
                let Some(text) = spot.script.as_deref() else {
                    continue;
                };
                let Ok(script) = parse_script(text, &commands, &limits) else {
                    unparsed += 1;
                    continue;
                };
                scripts += 1;
                touched = true;
                for event in EVENTS {
                    let Some(chunk) = script.handler(event) else {
                        continue;
                    };
                    match engine.run_handler(chunk) {
                        Ok(_) => {
                            dispatched += 1;
                            *by_event.entry(event).or_insert(0) += 1;
                        }
                        Err(_) => failed += 1,
                    }
                }
            }
        }
        if touched {
            payloads_with_scripts += 1;
        }
    }

    println!("payloads with scripts: {payloads_with_scripts}");
    println!("hotspot scripts parsed: {scripts}  (unparsed: {unparsed})");
    println!("handlers dispatched by event name: {dispatched}  (errored: {failed})");
    for (event, n) in &by_event {
        println!("  {event:14} {n}");
    }

    assert!(payloads_with_scripts > 0, "no payload carried a hotspot script");
    assert!(
        unparsed <= 4,
        "{unparsed} scripts failed to re-parse; the corpus has 4 known-malformed sources"
    );
    assert!(dispatched > 0, "no handler was dispatched by event name");
}

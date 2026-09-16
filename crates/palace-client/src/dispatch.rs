//! Live events → IPTSCRAE script dispatch.
//!
//! Hotspot scripts declare handlers named after the events they answer
//! (`ON ENTER`, `ON SELECT`, `ON OUTCHAT`). The runtime emits [`ClientEvent`]s.
//! This module is the bridge between the two: it names the script event a
//! runtime event corresponds to, and runs that handler for every hotspot in the
//! room.
//!
//! The engine keeps its globals between dispatches, which is what the reference
//! client does — a script that sets a global in `ON ENTER` can read it back in
//! `ON SELECT`.

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::{parse_script, CommandSet, Engine};
use iptscrae_palace::harness::SkeletonHost;
use iptscrae_palace::{PalaceCommands, PalaceHost};
use palace_room::RoomDesc;

use crate::runtime::ClientEvent;
use crate::state::{ChatKind, ConnectionStatus, SessionState};

/// The script event a runtime event answers, or `None` when none applies.
///
/// Chat lines carry only the speaker's id, so `self_user_id` is what decides
/// between `OUTCHAT` (a line of ours) and `INCHAT` (anyone else's).
pub fn script_event(event: &ClientEvent, self_user_id: i32) -> Option<&'static str> {
    match event {
        ClientEvent::RoomEntered { .. } => Some("ENTER"),
        ClientEvent::Chat { line } => match line.kind {
            ChatKind::System | ChatKind::Error => Some("SERVERMSG"),
            ChatKind::Talk | ChatKind::Whisper => Some(if line.user_id == self_user_id {
                "OUTCHAT"
            } else {
                "INCHAT"
            }),
        },
        ClientEvent::Status { status, .. } => match status {
            ConnectionStatus::Connected => Some("SIGNON"),
            _ => None,
        },
        ClientEvent::Banner { .. }
        | ClientEvent::Rooms { .. }
        | ClientEvent::Users { .. }
        | ClientEvent::Screen { .. }
        | ClientEvent::Note { .. } => None,
    }
}

/// What one dispatch pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Hotspots that declared a handler for the event.
    pub matched: usize,
    /// Handlers that ran to completion.
    pub ran: usize,
    /// `(hotspot id, error)` for handlers that failed.
    pub errors: Vec<(i16, String)>,
}

impl Outcome {
    /// Whether no hotspot answered the event at all.
    pub fn is_empty(&self) -> bool {
        self.matched == 0
    }
}

/// Runs hotspot scripts against a capability host.
pub struct Dispatcher<H: PalaceHost> {
    engine: Engine<PalaceCommands<H>>,
    commands: CommandSet,
    limits: Limits,
}

impl<H: PalaceHost> Dispatcher<H> {
    /// Build a dispatcher over `host`.
    pub fn new(host: H) -> Self {
        let limits = Limits::default().with_dialect(StackDialect::PalaceChat);
        let commands = SkeletonHost::command_set();
        let engine = Engine::new(PalaceCommands::new(host))
            .with_limits(limits)
            .with_commands(commands.clone());
        Dispatcher {
            engine,
            commands,
            limits,
        }
    }

    /// The host, for reading back the effects a script produced.
    pub fn host(&self) -> &H {
        &self.engine.host.inner
    }

    /// The host, mutably.
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.engine.host.inner
    }

    /// Run `event` for every hotspot in `desc`.
    ///
    /// A hotspot whose script is absent, malformed, or declares no handler for
    /// this event is skipped, not an error — that is the normal case.
    pub fn dispatch_scripts(&mut self, desc: &RoomDesc, event: &str) -> Outcome {
        let mut outcome = Outcome::default();
        for spot in &desc.hotspots {
            let Some(text) = spot.script.as_deref() else {
                continue;
            };
            let Ok(script) = parse_script(text, &self.commands, &self.limits) else {
                continue;
            };
            let Some(chunk) = script.handler(event) else {
                continue;
            };
            outcome.matched += 1;
            match self.engine.run_handler_spot(chunk, i64::from(spot.id)) {
                Ok(_) => outcome.ran += 1,
                Err(e) => outcome.errors.push((spot.id, e.to_string())),
            }
        }
        outcome
    }

    /// Run whatever script event `event` maps to against the state's room.
    ///
    /// `None` when the event answers no script event, or no room is entered.
    pub fn dispatch(&mut self, state: &SessionState, event: &ClientEvent) -> Option<Outcome> {
        let name = script_event(event, state.banner.user_id)?;
        let desc = state.room_desc.as_ref()?;
        Some(self.dispatch_scripts(desc, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ChatLine, RoomInfo};
    use iptscrae::Host;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Recorder {
        said: Vec<String>,
        visited: Vec<i64>,
    }

    impl Host for Recorder {}

    impl PalaceHost for Recorder {
        fn chat(&mut self, text: &str) -> iptscrae::Result<()> {
            self.said.push(text.to_owned());
            Ok(())
        }
        fn goto_room(&mut self, room: i64) -> iptscrae::Result<()> {
            self.visited.push(room);
            Ok(())
        }
    }

    fn chat(user_id: i32, kind: ChatKind) -> ClientEvent {
        ClientEvent::Chat {
            line: ChatLine {
                seq: 0,
                user_id,
                name: "someone".to_owned(),
                text: "hello".to_owned(),
                kind,
            },
        }
    }

    #[test]
    fn runtime_events_map_onto_the_script_events_the_corpus_uses() {
        let entered = ClientEvent::RoomEntered {
            room: RoomInfo {
                id: 1,
                name: "Room".to_owned(),
                users: 0,
                flags: 0,
            },
        };
        assert_eq!(script_event(&entered, 7), Some("ENTER"));

        // The speaker's identity decides the direction, not the text.
        assert_eq!(script_event(&chat(7, ChatKind::Talk), 7), Some("OUTCHAT"));
        assert_eq!(script_event(&chat(9, ChatKind::Talk), 7), Some("INCHAT"));
        assert_eq!(script_event(&chat(7, ChatKind::Whisper), 7), Some("OUTCHAT"));

        // Server traffic is not conversation.
        assert_eq!(script_event(&chat(0, ChatKind::System), 7), Some("SERVERMSG"));
        assert_eq!(script_event(&chat(0, ChatKind::Error), 7), Some("SERVERMSG"));

        assert_eq!(
            script_event(
                &ClientEvent::Status {
                    status: ConnectionStatus::Connected,
                    message: None,
                },
                7
            ),
            Some("SIGNON")
        );
        assert_eq!(
            script_event(
                &ClientEvent::Status {
                    status: ConnectionStatus::Disconnected,
                    message: None,
                },
                7
            ),
            None
        );
    }

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

    fn first_room(path: &std::path::Path) -> Option<RoomDesc> {
        let bytes = std::fs::read(path).ok()?;
        palace_room::decode_stream(&bytes, palace_wire::ByteOrder::Little)
            .into_iter()
            .flatten()
            .next()
    }

    #[test]
    fn enter_handlers_run_through_the_bridge_and_reach_the_host() {
        let Some(root) = corpus_root() else {
            eprintln!("corpus absent; skipping");
            return;
        };
        let Ok(dir) = std::fs::read_dir(root.join("payloads_all")) else {
            eprintln!("corpus unreadable; skipping");
            return;
        };
        let mut files: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "bin"))
            .collect();
        files.sort();

        let mut dispatcher = Dispatcher::new(Recorder::default());
        let (mut rooms, mut matched, mut ran, mut errors) = (0usize, 0usize, 0usize, 0usize);
        // PalaceHost refuses an unimplemented command rather than ignoring it.
        let mut refused: BTreeMap<String, usize> = BTreeMap::new();
        let mut failing_rooms = 0usize;
        for path in &files {
            let Some(desc) = first_room(path) else {
                continue;
            };
            let outcome = dispatcher.dispatch_scripts(&desc, "ENTER");
            if outcome.matched > 0 {
                rooms += 1;
            }
            if !outcome.errors.is_empty() {
                failing_rooms += 1;
            }
            matched += outcome.matched;
            ran += outcome.ran;
            errors += outcome.errors.len();
            for (_, e) in outcome.errors {
                *refused.entry(e).or_insert(0) += 1;
            }
        }

        let mut ranked: Vec<(&String, &usize)> = refused.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        println!(
            "ENTER through the bridge: {rooms} rooms answered, {matched} handlers matched, \
             {ran} ran, {errors} errored across {failing_rooms} rooms"
        );
        println!(
            "  host effects: {} SAY(s), {} GOTOROOM(s)",
            dispatcher.host().said.len(),
            dispatcher.host().visited.len()
        );
        println!(
            "  a two-command host still owes {} distinct commands:",
            ranked.len()
        );
        for (message, count) in &ranked {
            println!("    {count:5}  {message}");
        }

        assert!(matched > 0, "no room answered ON ENTER");
        assert_eq!(
            matched,
            ran + errors,
            "every matched handler either ran or errored — none may be skipped silently"
        );
        assert!(
            !dispatcher.host().said.is_empty(),
            "no ENTER handler's effect reached the capability trait"
        );
    }
}

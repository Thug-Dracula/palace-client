//! Event dispatch: the engine that runs handlers and schedules alarms.
//!
//! One [`ScriptEngine`] is one client session. It holds the IPTSCRAE
//! [`Engine`] (globals survive between handlers), the scripts loaded for the
//! current room, and the alarm queue. [`ScriptEngine::fire`] runs every handler
//! a real event selects and returns what each one did.

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::registry::CommandSet;
use iptscrae::value::Chunk;
use iptscrae::{parse_script, Engine, Value};
use iptscrae_palace::commands::register_palace_commands;
use iptscrae_palace::ScriptEvent;
use palace_room::RoomDesc;

use crate::effect::Effect;
use crate::host::{AlarmKind, ScriptHost};
use crate::load::{cyborg_script, scripts_from_room, LoadProblem, LoadedScript};
use crate::view::HostView;

/// How many alarms may be queued at once.
pub const MAX_PENDING_ALARMS: usize = 256;

/// How many alarms may fire in one [`ScriptEngine::advance`] call.
pub const MAX_ALARMS_PER_ADVANCE: usize = 64;

/// An alarm with an absolute deadline.
#[derive(Debug, Clone)]
struct AbsoluteAlarm {
    due: i64,
    kind: AlarmKind,
    spot: i32,
}

/// What one handler run did.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerRun {
    /// Hotspot the script belongs to; `0` for cyborg.
    pub spot: i32,
    /// Instructions retired.
    pub steps: u64,
    /// Present when the handler faulted.
    pub error: Option<String>,
    /// Effects this handler recorded.
    pub effects: Vec<Effect>,
}

/// What one dispatch did.
#[derive(Debug, Clone, Default)]
pub struct DispatchReport {
    /// The handler name that was looked for.
    pub handler: String,
    /// One row per script that declared the handler.
    pub runs: Vec<HandlerRun>,
    /// Every effect across the runs, in execution order.
    pub effects: Vec<Effect>,
    /// The `CHATSTR` value after the run, for `ON INCHAT`/`ON OUTCHAT`.
    pub chat_string: Option<String>,
}

impl DispatchReport {
    /// Whether any script declared the handler.
    #[must_use]
    pub fn fired(&self) -> bool {
        !self.runs.is_empty()
    }

    /// Handlers that faulted.
    #[must_use]
    pub fn errors(&self) -> Vec<&HandlerRun> {
        self.runs.iter().filter(|r| r.error.is_some()).collect()
    }
}

/// The IPTSCRAE runtime for one session.
pub struct ScriptEngine {
    engine: Engine<ScriptHost>,
    scripts: Vec<LoadedScript>,
    alarms: Vec<AbsoluteAlarm>,
    /// Scripts in the current room that could not be parsed.
    pub problems: Vec<LoadProblem>,
    /// Alarms dropped because the queue was full.
    pub alarms_dropped: u64,
}

impl ScriptEngine {
    /// An engine with the core and Palace commands registered.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        let mut commands = CommandSet::core();
        register_palace_commands(&mut commands);
        let mut host = ScriptHost::new(HostView::default());
        host.limits = limits;
        let engine = Engine::new(host)
            .with_limits(limits)
            .with_commands(commands);
        ScriptEngine {
            engine,
            scripts: Vec::new(),
            alarms: Vec::new(),
            problems: Vec::new(),
            alarms_dropped: 0,
        }
    }

    /// An engine with the PalaceChat dialect budgets, which is what the
    /// harvested corpus needs.
    #[must_use]
    pub fn with_palace_limits() -> Self {
        Self::new(Limits::default().with_dialect(StackDialect::PalaceChat))
    }

    /// The underlying host, for inspection and tests.
    #[must_use]
    pub fn host(&self) -> &ScriptHost {
        &self.engine.host
    }

    /// The underlying host, mutably.
    pub fn host_mut(&mut self) -> &mut ScriptHost {
        &mut self.engine.host
    }

    /// The scripts loaded for the current room.
    #[must_use]
    pub fn scripts(&self) -> &[LoadedScript] {
        &self.scripts
    }

    /// Replace the view a script observes.
    pub fn set_view(&mut self, view: HostView) {
        self.engine.host.view = view;
    }

    /// Advance the host clock, in ticks (1/60 s).
    pub fn set_tick(&mut self, tick: i64) {
        self.engine.host.tick = tick;
    }

    /// Parse the room's hotspot scripts, replacing any previous room.
    pub fn load_room(&mut self, room: &RoomDesc) {
        let limits = self.engine.limits;
        let commands = self.engine.commands.clone();
        let (scripts, problems) = scripts_from_room(room, &commands, &limits);
        self.scripts = scripts;
        self.problems = problems;
        self.alarms.clear();
        self.engine.host.alarms.clear();
        self.engine.host.view.chat_string.clear();
        self.engine.host.view.apply_room(room);
    }

    /// Install a cyborg script (the client-wide, spotless script).
    pub fn load_cyborg(&mut self, source: &str) {
        let limits = self.engine.limits;
        let commands = self.engine.commands.clone();
        match cyborg_script(source, &commands, &limits) {
            Ok(script) => self.scripts.push(script),
            Err(problem) => self.problems.push(problem),
        }
    }

    /// Forget the room's scripts and pending alarms.
    pub fn clear_room(&mut self) {
        self.scripts.clear();
        self.alarms.clear();
        self.engine.host.alarms.clear();
    }

    /// Replace one hotspot's script from its current source text.
    ///
    /// `SETSPOTSCRIPT` merges a new handler into a hotspot's source at runtime,
    /// so the engine has to re-parse that one spot or the handler never fires.
    /// A source with no handlers (or a syntax error) leaves the spot's previous
    /// script in place and returns the [`LoadProblem`].
    pub fn set_spot_script(&mut self, spot: i32, source: &str) -> Option<LoadProblem> {
        let limits = self.engine.limits;
        let commands = self.engine.commands.clone();
        match parse_script(source, &commands, &limits) {
            Ok(script) if script.is_empty() => Some(LoadProblem {
                spot,
                error: "no ON handlers in script text".to_owned(),
            }),
            Ok(script) => {
                let loaded = LoadedScript {
                    spot,
                    script,
                    source: source.to_owned(),
                };
                match self
                    .scripts
                    .iter_mut()
                    .find(|existing| existing.spot == spot)
                {
                    Some(existing) => *existing = loaded,
                    None => self.scripts.push(loaded),
                }
                None
            }
            Err(error) => Some(LoadProblem {
                spot,
                error: error.to_string(),
            }),
        }
    }

    /// Whether any loaded script declares the handler for `event`.
    #[must_use]
    pub fn has_handler(&self, event: ScriptEvent) -> bool {
        let name = event.handler_name();
        self.scripts
            .iter()
            .any(|script| script.script.handler(&name).is_some())
    }

    /// Parse and run a bare instruction sequence (no `ON` handler).
    ///
    /// Used by the client's script box: the effects are returned in
    /// [`HandlerRun::effects`] for the caller to apply.
    pub fn run_source(&mut self, source: &str) -> Result<HandlerRun, String> {
        let chunk = iptscrae::lexer::parse_body(source, &self.engine.commands, &self.engine.limits)
            .map_err(|error| error.to_string())?;
        self.engine.host.current_spot = 0;
        let run = self.run_handler(0, &chunk, false);
        match &run.error {
            Some(error) => Err(error.clone()),
            None => Ok(run),
        }
    }

    /// Parse and run a fetched script body in the sandboxed VM.
    ///
    /// This is the same engine room scripts run through: the step budget, the
    /// variable caps and the command table all apply, so a hostile or malformed
    /// body faults inside the run report instead of breaking the session.
    /// Definitions land in the engine's shared globals, so the `HTTPRECEIVED`
    /// dispatch that follows sees what the fetch defined — sparky's
    /// `executeScriptSource` runs the body through the same runtime as room
    /// scripts for exactly that reason. `spot` is the fetching hotspot; `0`
    /// runs room-level.
    pub fn execute_fetched_source(&mut self, source: &str, spot: i32) -> HandlerRun {
        self.engine.host.current_spot = spot;
        let run =
            match iptscrae::lexer::parse_body(source, &self.engine.commands, &self.engine.limits) {
                Ok(chunk) => self.run_handler(spot, &chunk, false),
                Err(error) => HandlerRun {
                    spot,
                    steps: 0,
                    error: Some(error.to_string()),
                    effects: Vec::new(),
                },
            };
        self.engine.reset_globals(); // CUT-MARKER
        run
    }

    /// Fire an event at every script that handles it.
    ///
    /// For `ON INCHAT` / `ON OUTCHAT` the incoming text is planted in
    /// `CHATSTR`; whatever the handlers leave there comes back as
    /// [`DispatchReport::chat_string`], which is how a script rewrites or
    /// suppresses chat.
    pub fn fire(&mut self, event: ScriptEvent) -> DispatchReport {
        self.fire_inner(event, None)
    }

    /// Fire an event at one hotspot's script only (`SETALARM`, `SELECT`).
    pub fn fire_spot(&mut self, event: ScriptEvent, spot: i32) -> DispatchReport {
        self.fire_inner(event, Some(spot))
    }

    fn fire_inner(&mut self, event: ScriptEvent, only: Option<i32>) -> DispatchReport {
        let mut report = DispatchReport {
            handler: event.handler_name(),
            ..DispatchReport::default()
        };
        let handler = report.handler.clone();

        if matches!(event, ScriptEvent::InChat | ScriptEvent::OutChat) {
            let initial = self.engine.host.view.chat_string.clone();
            report.chat_string = Some(initial);
        }

        let capture_chat = report.chat_string.is_some();
        let scripts = self.scripts.clone();
        for script in &scripts {
            if only.is_some_and(|spot| spot != script.spot) {
                continue;
            }
            let Some(chunk) = script.script.handler(&handler) else {
                continue;
            };
            let chunk = chunk.clone();
            self.engine.host.current_spot = script.spot;
            let run = self.run_handler(script.spot, &chunk, capture_chat);
            if capture_chat {
                let text = self.engine.host.view.chat_string.clone();
                report.chat_string = Some(text);
            }
            report.effects.extend(run.effects.iter().cloned());
            report.runs.push(run);
        }

        self.absorb_alarms();
        report
    }

    /// Fire any alarm whose deadline has passed.
    ///
    /// Returns every effect the expired alarms produced. An alarm body may
    /// schedule more alarms; those are queued, not run, so this cannot recurse.
    pub fn advance(&mut self, tick: i64) -> Vec<Effect> {
        self.set_tick(tick);
        let mut out = Vec::new();
        for _ in 0..MAX_ALARMS_PER_ADVANCE {
            let Some(index) = self.alarms.iter().position(|alarm| alarm.due <= tick) else {
                break;
            };
            let alarm = self.alarms.remove(index);
            match alarm.kind {
                AlarmKind::Body(chunk) => {
                    self.engine.host.current_spot = alarm.spot;
                    let run = self.run_handler(alarm.spot, &chunk, false);
                    out.extend(run.effects);
                }
                AlarmKind::Spot => {
                    let report = self.fire_spot(ScriptEvent::Alarm, alarm.spot);
                    out.extend(report.effects);
                }
            }
            self.absorb_alarms();
        }
        self.alarms.retain(|alarm| alarm.due > tick);
        out
    }

    /// How many alarms are queued.
    #[must_use]
    pub fn pending_alarms(&self) -> usize {
        self.alarms.len()
    }

    fn run_handler(&mut self, spot: i32, chunk: &Chunk, capture_chat: bool) -> HandlerRun {
        self.engine.host.effects.clear();
        let capture = if capture_chat {
            vec!["CHATSTR"]
        } else {
            Vec::new()
        };
        let outcome = self
            .engine
            .run_handler_capture(chunk, i64::from(spot), &capture);
        match outcome {
            Ok(captured) => {
                if let Some(Some(Value::Str(text))) = captured.captured.first() {
                    self.engine.host.view.chat_string = text.to_string();
                }
                HandlerRun {
                    spot,
                    steps: captured.steps,
                    error: None,
                    effects: self.engine.host.take_effects(),
                }
            }
            Err(error) => HandlerRun {
                spot,
                steps: 0,
                error: Some(error.to_string()),
                effects: self.engine.host.take_effects(),
            },
        }
    }

    fn absorb_alarms(&mut self) {
        let pending = self.engine.host.take_alarms();
        let now = self.engine.host.tick;
        for alarm in pending {
            if self.alarms.len() >= MAX_PENDING_ALARMS {
                self.alarms_dropped += 1;
                continue;
            }
            self.alarms.push(AbsoluteAlarm {
                due: now + alarm.ticks,
                kind: alarm.kind,
                spot: alarm.spot,
            });
        }
    }
}

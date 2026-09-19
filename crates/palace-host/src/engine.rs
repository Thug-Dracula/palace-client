//! Event dispatch: the engine that runs handlers and schedules alarms.
//!
//! One [`ScriptEngine`] is one client session. It holds the IPTSCRAE
//! [`Engine`] (globals survive between handlers), the scripts loaded for the
//! current room, and the alarm queue. [`ScriptEngine::fire`] runs every handler
//! a real event selects and returns what each one did.

use iptscrae::budget::{Limits, StackDialect};
use iptscrae::error::IptError;
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
        // The reference command tables have no `MOUSEX`/`MOUSEY` (the tracked
        // pointer is read with `MOUSEPOS`: `iptService.js` table at line 69, only
        // `MOUSEPOS` at 2300; `PalaceIptscraeCommands.as` and `IptDefaultCommands.as`
        // likewise). Keep the words out of the parse dictionary whatever the host
        // table reserves, because the parser turns any registered name into a
        // command (`IptParser.parseSymbol`). A real corpus script uses them as
        // variables — `mousex GLOBAL` in 5308_hs1/hs5 globalises `MOUSEX` — and
        // were `mousex` a command it would push a number where `GLOBAL` needs an
        // `IptVariable` (`GLOBALCommand.as:10`). The dispatch table still answers
        // the words for a host that asks directly.
        commands.unregister("MOUSEX");
        commands.unregister("MOUSEY");
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

    /// The session's global variables rendered as text, for diagnostics.
    #[must_use]
    pub fn globals_snapshot(&self) -> Vec<(String, String)> {
        self.engine
            .globals_snapshot()
            .into_iter()
            .map(|(name, value)| (name, format!("{value:?}")))
            .collect()
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

    /// The command dictionary this engine parses and runs with.
    #[must_use]
    pub fn commands(&self) -> &CommandSet {
        &self.engine.commands
    }

    /// The resource limits this engine applies.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.engine.limits
    }

    /// Forget every global, isolating the next corpus file or session.
    pub fn reset_globals(&mut self) {
        self.engine.reset_globals();
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
    ///
    /// A repeated description of the room we are already in is not a room
    /// change: the server re-sends a description whenever the room's state
    /// changes, the lifecycle does not run again for it, and the reference
    /// keeps already-armed timers across it. Only a *different* room makes the
    /// old room's pending alarms meaningless, so only then are they cleared.
    pub fn load_room(&mut self, room: &RoomDesc) {
        let limits = self.engine.limits;
        let commands = self.engine.commands.clone();
        let (scripts, problems) = scripts_from_room(room, &commands, &limits);
        self.scripts = scripts;
        self.problems = problems;
        if self.engine.host.view.room_id != i32::from(room.header.room_id) {
            self.alarms.clear();
            self.engine.host.alarms.clear();
        }
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

    /// Run one parsed handler body against the live host.
    ///
    /// [`ScriptEngine::fire`] reduces a fault to a string for the session log.
    /// A corpus run needs the raw [`IptError`] so it can classify the failure
    /// the same way the skeleton-host harness does, so this returns it
    /// unchanged. `spot` is the hotspot the body belongs to; `0` runs
    /// room-level.
    pub fn run_chunk(&mut self, spot: i32, chunk: &Chunk) -> Result<HandlerRun, IptError> {
        self.engine.host.current_spot = spot;
        self.engine.host.effects.clear();
        let outcome = self.engine.run_handler_capture(chunk, i64::from(spot), &[]);
        let effects = self.engine.host.take_effects();
        match outcome {
            Ok(captured) => Ok(HandlerRun {
                spot,
                steps: captured.steps,
                error: None,
                effects,
            }),
            Err(error) => Err(error),
        }
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
        match iptscrae::lexer::parse_body(source, &self.engine.commands, &self.engine.limits) {
            Ok(chunk) => self.run_handler(spot, &chunk, false),
            Err(error) => HandlerRun {
                spot,
                steps: 0,
                error: Some(error.to_string()),
                effects: Vec::new(),
            },
        }
    }

    /// Fire an event at every script that handles it.
    ///
    /// For `ON INCHAT` / `ON OUTCHAT` the incoming text is planted in
    /// `CHATSTR`; whatever the handlers leave there comes back as
    /// [`DispatchReport::chat_string`], which is how a script rewrites or
    /// suppresses chat.
    ///
    /// A handler fault aborts the rest of the dispatch, matching the
    /// reference, where `IptManager.step` clears the call stack every queued
    /// handler shares; the fault is still reported in
    /// [`DispatchReport::runs`]. Alarms queued before the fault survive, as
    /// they do on the reference's `step` path.
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
        for index in Self::dispatch_order(&scripts) {
            let script = &scripts[index];
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
            let faulted = run.error.is_some();
            report.runs.push(run);
            if faulted {
                // Reference: `IptManager.step()` catches an `IptError` from a
                // handler and calls `clearCallStack()`. Every handler queued
                // for one event shares that stack, so the fault aborts the
                // rest of the dispatch, not just the faulting handler.
                break;
            }
        }

        self.absorb_alarms();
        report
    }

    /// The order handlers run in: hotspots first to last, then the cyborg.
    ///
    /// The PalaceChat client walks its hotspot collection forwards, index `0` up
    /// to `UBound` (disassembled from the shipped Xojo binary:
    /// `PalaceController.triggerHotspotEvents%b%o<PalaceController>s`), and fills
    /// that collection in room-description order. OpenPalace's
    /// `PalaceController.triggerHotspotEvents`
    /// (`OpenPalace/PalaceClient/.../iptscrae/PalaceController.as:81-93`) walks
    /// it backwards instead; copying that walk was wrong, because a later
    /// hotspot's handler then ran before an earlier one had set the state it
    /// reads (room 31741's team gate must run before spot 69's audience check).
    /// Scripts load in room order (`scripts_from_room`) with the cyborg appended
    /// (`load_cyborg`), so load order plus cyborg last matches PalaceChat.
    fn dispatch_order(scripts: &[LoadedScript]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..scripts.len())
            .filter(|&index| scripts[index].spot != 0)
            .collect();
        order.extend((0..scripts.len()).filter(|&index| scripts[index].spot == 0));
        order
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

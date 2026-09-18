//! `palace-host` — the IPTSCRAE host for a live Palace client.
//!
//! This is the crate that turns the two halves that already existed — the
//! `palace-room` hotspot decoder and the `iptscrae`/`iptscrae-palace` language —
//! into something that actually fires:
//!
//! ```text
//! RoomDesc.hotspots[i].script ──► load::scripts_from_room ──► Script { handlers }
//!                                                                    │
//! ScriptEvent (ENTER, SELECT, …) ──► engine::ScriptEngine::fire ──────┤
//!                                                                    ▼
//!                                            host::ScriptHost (PalaceHost)
//!                                                                    │
//!                                                          effect::Effect
//!                                                                    │
//!                                            wire::effect_frame ──► Frame
//! ```
//!
//! * [`load`] parses the `ON <NAME> { … }` blocks out of each hotspot's script
//!   text and reports the ones that fail.
//! * [`engine::ScriptEngine`] dispatches events, keeps globals across handlers,
//!   and runs `ALARMEXEC` / `SETALARM` timers.
//! * [`host::ScriptHost`] implements `PalaceHost` over a [`view::HostView`]
//!   snapshot, recording every requested effect. It never touches the socket.
//! * [`wire`] encodes those effects as protocol frames.
//!
//! ## Untrusted scripts
//!
//! Scripts arrive from the server. The host has no ambient authority: it cannot
//! open a socket, read a file or spawn a process, and every effect is a value
//! the runtime may refuse. The VM's budgets (stack, step count, loop cap,
//! nesting, string length) still apply — see `iptscrae::budget`.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod effect;
pub mod engine;
pub mod host;
pub mod load;
pub mod view;
pub mod wire;

pub use effect::Effect;
pub use engine::{DispatchReport, HandlerRun, ScriptEngine, MAX_PENDING_ALARMS};
pub use host::{AlarmKind, PenState, PendingAlarm, ScriptHost, PALACECHAT_VERSION};
pub use load::{cyborg_script, scripts_from_room, LoadProblem, LoadedScript};
pub use view::{
    point_in_polygon, AssetFacts, HostView, LoosePropView, PropFacts, SpotView, UserView,
};
pub use wire::{effect_frame, move_target, WireContext};

pub use iptscrae_palace::ScriptEvent;

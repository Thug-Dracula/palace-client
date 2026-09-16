//! Turning a decoded room into runnable scripts.
//!
//! A hotspot's IPTSCRAE source lives in `RoomDesc::hotspots[i].script`; the
//! decoder recovers it from the `scriptTextOfst` CString. (`nbrScripts` and
//! `scriptRecOfst` are zero in every corpus hotspot, so the handler set is
//! derived from the `ON <NAME> { … }` blocks in the text — which is what the
//! reference clients do too.) This module parses those blocks and reports the
//! ones that fail instead of dropping them.

use iptscrae::budget::Limits;
use iptscrae::registry::CommandSet;
use iptscrae::{parse_script, Script};
use palace_room::RoomDesc;

/// A hotspot script that parsed, ready to dispatch.
#[derive(Debug, Clone)]
pub struct LoadedScript {
    /// Hotspot id the script belongs to; `0` for a cyborg script.
    pub spot: i32,
    /// The parsed handlers.
    pub script: Script,
    /// The original source, kept for the run report.
    pub source: String,
}

/// A script that could not be turned into runnable handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProblem {
    /// Hotspot id, or `0` for a cyborg script.
    pub spot: i32,
    /// Why it failed.
    pub error: String,
}

/// Parse every hotspot script in `room`.
///
/// Returns the scripts that parsed and a problem for each one that did not. An
/// empty script (no handlers) is reported as a problem rather than dispatched,
/// so "nothing fired" is never silent.
#[must_use]
pub fn scripts_from_room(
    room: &RoomDesc,
    commands: &CommandSet,
    limits: &Limits,
) -> (Vec<LoadedScript>, Vec<LoadProblem>) {
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for hotspot in &room.hotspots {
        let Some(source) = hotspot.script.as_deref() else {
            continue;
        };
        if source.trim().is_empty() {
            continue;
        }
        let spot = i32::from(hotspot.id);
        match parse_script(source, commands, limits) {
            Ok(script) if script.is_empty() => problems.push(LoadProblem {
                spot,
                error: "no ON handlers in script text".to_owned(),
            }),
            Ok(script) => loaded.push(LoadedScript {
                spot,
                script,
                source: source.to_owned(),
            }),
            Err(error) => problems.push(LoadProblem {
                spot,
                error: error.to_string(),
            }),
        }
    }
    (loaded, problems)
}

/// Parse a cyborg script (the per-user script file, no hotspot).
pub fn cyborg_script(
    source: &str,
    commands: &CommandSet,
    limits: &Limits,
) -> Result<LoadedScript, LoadProblem> {
    match parse_script(source, commands, limits) {
        Ok(script) if script.is_empty() => Err(LoadProblem {
            spot: 0,
            error: "no ON handlers in cyborg script".to_owned(),
        }),
        Ok(script) => Ok(LoadedScript {
            spot: 0,
            script,
            source: source.to_owned(),
        }),
        Err(error) => Err(LoadProblem {
            spot: 0,
            error: error.to_string(),
        }),
    }
}

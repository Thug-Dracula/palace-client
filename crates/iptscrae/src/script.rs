//! A parsed script file: a set of named event handlers.
//!
//! A Palace script (a hotspot's `SCRIPT ... ENDSCRIPT` body, or a cyborg file)
//! is a sequence of `ON NAME { ... }` blocks. Parsing stops at the handler
//! level: which handler runs and when is the host's decision, which is why this
//! crate parses and executes but does not dispatch.

use std::collections::BTreeMap;

use crate::value::Chunk;

/// A parsed script file.
///
/// Handler names are kept exactly as written (`ON ENTER` → `"ENTER"`), and
/// lookup upper-cases the query so a lower-case `on enter` is still found. The
/// reference implementation keeps names case-sensitive; being forgiving here
/// cannot change the meaning of a well-formed script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    handlers: BTreeMap<String, Chunk>,
}

impl Script {
    /// Build a script from an ordered handler map.
    pub fn new(handlers: BTreeMap<String, Chunk>) -> Self {
        Self { handlers }
    }

    /// The handlers, keyed by name and ordered alphabetically.
    pub fn handlers(&self) -> &BTreeMap<String, Chunk> {
        &self.handlers
    }

    /// Look up a handler by name, case-insensitively.
    pub fn handler(&self, name: &str) -> Option<&Chunk> {
        if let Some(chunk) = self.handlers.get(name) {
            return Some(chunk);
        }
        let upper = name.to_ascii_uppercase();
        self.handlers
            .iter()
            .find(|(k, _)| k.to_ascii_uppercase() == upper)
            .map(|(_, v)| v)
    }

    /// The handler names, alphabetically.
    pub fn handler_names(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(|k| k.as_str())
    }

    /// How many handlers the file declares.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Whether the file declares no handlers at all.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

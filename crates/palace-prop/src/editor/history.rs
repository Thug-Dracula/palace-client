//! Bounded undo/redo over whole-document snapshots.
//!
//! This mirrors the reference editor's `PaintSnapshotPush` / `PaintUndo` shape:
//! before a mutation the caller pushes a snapshot of the current state
//! ([`History::push`]); a new edit discards the redo branch; undo swaps the
//! current state for the most recent snapshot and redo swaps it back. A snapshot
//! is a whole-document copy, which is what the reference's history events carry
//! (per-layer images for every layer touched).
//!
//! # Caps
//!
//! History is bounded on two axes, both enforced by evicting the oldest past
//! snapshots first:
//!
//! * [`MAX_UNDO_SNAPSHOTS`] — the depth cap. At 32, a 44x44 document retains at
//!   most 32 states, about 248 KiB of pixels in total.
//! * [`MAX_UNDO_BYTES`] — the memory cap, measured over the pixel bytes of every
//!   state held in both stacks. At 128 MiB it is the guard for large canvases:
//!   one full-resolution 4096x4096 RGBA frame is 64 MiB, so two such snapshots
//!   fit and a third evicts the oldest.
//!
//! The newest snapshot is always kept, even when it alone exceeds the byte cap,
//! so an oversized edit stays undoable as long as possible; older states are
//! dropped to make room. A depth cap of zero disables history entirely.

use std::collections::VecDeque;

use super::DocumentState;

/// Maximum number of retained snapshots (undo plus redo), oldest evicted first.
pub const MAX_UNDO_SNAPSHOTS: usize = 32;

/// Maximum retained snapshot pixel memory in bytes, oldest evicted first.
pub const MAX_UNDO_BYTES: usize = 128 * 1024 * 1024;

/// A bounded two-stack undo/redo history over [`DocumentState`] snapshots.
#[derive(Debug, Clone)]
pub struct History {
    undo: VecDeque<DocumentState>,
    redo: Vec<DocumentState>,
    depth_cap: usize,
    byte_cap: usize,
    bytes: usize,
}

impl Default for History {
    fn default() -> Self {
        History::new()
    }
}

impl History {
    /// The default history: [`MAX_UNDO_SNAPSHOTS`] deep and [`MAX_UNDO_BYTES`] large.
    #[must_use]
    pub fn new() -> Self {
        History::with_caps(MAX_UNDO_SNAPSHOTS, MAX_UNDO_BYTES)
    }

    /// A history with explicit caps. Used by tests and by callers that want a
    /// smaller bound; a `depth_cap` of zero disables undo.
    #[must_use]
    pub fn with_caps(depth_cap: usize, byte_cap: usize) -> Self {
        History {
            undo: VecDeque::new(),
            redo: Vec::new(),
            depth_cap,
            byte_cap,
            bytes: 0,
        }
    }

    /// The depth cap this history was built with.
    #[must_use]
    pub fn depth_cap(&self) -> usize {
        self.depth_cap
    }

    /// The memory cap this history was built with.
    #[must_use]
    pub fn byte_cap(&self) -> usize {
        self.byte_cap
    }

    /// Retained snapshots across both stacks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.undo.len() + self.redo.len()
    }

    /// Whether no snapshot is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }

    /// Snapshots available to undo.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Snapshots available to redo.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Pixel bytes held by every retained snapshot.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }

    /// Whether [`History::undo`] would return a state.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether [`History::redo`] would return a state.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Drop every snapshot.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
    }

    /// Push `state` as the state to return to, discarding the redo branch.
    ///
    /// This is `PaintSnapshotPush`: call it with the document's current state
    /// immediately before mutating it. The caps are then enforced by evicting the
    /// oldest past snapshots.
    pub fn push(&mut self, state: DocumentState) {
        // Any new edit invalidates the redo branch, matching the reference's
        // `redoEvents = []` on a new history event.
        self.bytes -= self.redo_bytes();
        self.redo.clear();
        self.bytes += state.memory_bytes();
        self.undo.push_back(state);
        self.trim();
    }

    /// Swap `current` for the most recent snapshot, returning that snapshot.
    ///
    /// `current` is moved onto the redo stack. Returns `None` when there is
    /// nothing to undo.
    pub fn undo(&mut self, current: DocumentState) -> Option<DocumentState> {
        let previous = self.undo.pop_back()?;
        self.bytes -= previous.memory_bytes();
        self.bytes += current.memory_bytes();
        self.redo.push(current);
        Some(previous)
    }

    /// Swap `current` for the most recently undone snapshot.
    ///
    /// `current` is moved back onto the undo stack. Returns `None` when there is
    /// nothing to redo.
    pub fn redo(&mut self, current: DocumentState) -> Option<DocumentState> {
        let next = self.redo.pop()?;
        self.bytes -= next.memory_bytes();
        self.bytes += current.memory_bytes();
        self.undo.push_back(current);
        Some(next)
    }

    /// Evict the oldest past snapshots until both caps hold.
    ///
    /// The newest snapshot is never dropped for the byte cap: when a single state
    /// is larger than the cap it stays, and older ones are removed around it.
    fn trim(&mut self) {
        while self.undo.len() > self.depth_cap
            || (self.bytes > self.byte_cap && self.undo.len() > 1)
        {
            match self.undo.pop_front() {
                Some(evicted) => self.bytes -= evicted.memory_bytes(),
                None => break,
            }
        }
    }

    /// Pixel bytes currently held by the redo stack.
    fn redo_bytes(&self) -> usize {
        self.redo.iter().map(DocumentState::memory_bytes).sum()
    }
}

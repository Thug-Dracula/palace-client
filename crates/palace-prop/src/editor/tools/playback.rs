//! Playback modes and baking for multi-frame documents.
//!
//! A multi-frame document can be previewed in any of the reference editor's
//! four playback modes, mirroring its `pb_once`, `pb_loop`, `pb_pingpong` and
//! `pb_reverse` controls. Playback is a **view**: [`PlaybackMode::frame_at`] is
//! a pure computation from a frame count and a tick to a frame index, so
//! advancing the preview cannot touch a document.
//!
//! # Modes
//!
//! Given `n` frames and the tick `t` since playback started:
//!
//! * [`PlaybackMode::Once`] — `0, 1, ..., n-1`, then playback stops (`None`).
//!   This is the only finite mode; after it stops the caller holds the last
//!   frame itself.
//! * [`PlaybackMode::Loop`] — `0, 1, ..., n-1`, repeating forever.
//! * [`PlaybackMode::PingPong`] — `0, 1, ..., n-1, n-2, ..., 1`, repeating, so
//!   neither endpoint frame is shown twice in a row.
//! * [`PlaybackMode::Reverse`] — `n-1, n-2, ..., 0`, repeating.
//!
//! A one-frame document plays frame 0 in every mode; a frame count of zero has
//! no frame to show, so every mode returns `None`.
//!
//! # Baking
//!
//! "Bake" turns a preview mode into explicit frames, using the document's own
//! frame operations. The reference bakes a ping-pong over `1,2,3` as
//! `1,2,3,2,1`, and [`bake_playback`] reproduces that: forward through every
//! frame, then back down to the first. Reverse baking reverses the frame order;
//! once and loop are already the document's own order, so baking them is an
//! identity that changes nothing.
//!
//! The reference caps a bake at 15 seconds of playback. Playback here is
//! measured in ticks, so the cap is [`MAX_BAKE_FRAMES`] — 15 seconds at the
//! documented preview rate of one tick per [`PLAYBACK_TICK_MS`] milliseconds.
//! A bake whose resulting sequence would be longer is refused, leaving the
//! document untouched: the caller confirms the action first, and a refusal is
//! reported as `None`, never as a silent truncation of the animation.
//!
//! A bake pushes an undo snapshot before it moves any frame, so the
//! confirmation the caller shows can always be undone.
//!
//! # Example
//!
//! ```
//! use palace_prop::editor::tools::playback::PlaybackMode;
//!
//! let mode = PlaybackMode::PingPong;
//! assert_eq!(mode.frame_at(3, 0), Some(0));
//! assert_eq!(mode.frame_at(3, 3), Some(1));
//! assert_eq!(mode.frame_at(3, 4), Some(0), "a new cycle starts");
//!
//! use palace_prop::editor::{tools::playback::bake_playback, EditorDocument};
//!
//! let mut document = EditorDocument::blank(2, 2).unwrap();
//! document.add_blank_frame().unwrap();
//! document.add_blank_frame().unwrap();
//! let outcome = bake_playback(&mut document, PlaybackMode::PingPong).unwrap();
//! assert_eq!((outcome.added, outcome.total), (2, 5), "1,2,3,2,1");
//! ```

use super::super::EditorDocument;

/// Milliseconds between preview ticks: playback advances ten frames a second.
pub const PLAYBACK_TICK_MS: u64 = 100;

/// The bake cap the reference editor applies, in seconds of playback.
pub const MAX_BAKE_SECONDS: u64 = 15;

/// The bake cap in frames: [`MAX_BAKE_SECONDS`] at [`PLAYBACK_TICK_MS`].
pub const MAX_BAKE_FRAMES: usize = (MAX_BAKE_SECONDS * 1000 / PLAYBACK_TICK_MS) as usize;

/// Which frame order a preview plays.
///
/// Every mode is a pure function of the frame count and the tick; see
/// [`PlaybackMode::frame_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaybackMode {
    /// Play forward once, then stop.
    Once,
    /// Play forward forever.
    Loop,
    /// Play forward then back, forever.
    PingPong,
    /// Play backward forever.
    Reverse,
}

impl PlaybackMode {
    /// The frame index this mode shows at `tick`.
    ///
    /// Returns `None` when there is no frame to show (`frame_count == 0`) or a
    /// [`PlaybackMode::Once`] run has finished. Every other mode answers for
    /// every tick, wrapping around its cycle.
    #[must_use]
    pub fn frame_at(self, frame_count: usize, tick: u64) -> Option<usize> {
        if frame_count == 0 {
            return None;
        }
        let count = frame_count as u64;
        match self {
            PlaybackMode::Once => (tick < count).then_some(tick as usize),
            PlaybackMode::Loop => Some((tick % count) as usize),
            PlaybackMode::PingPong => {
                if frame_count == 1 {
                    return Some(0);
                }
                // 0, 1, ..., n-1, n-2, ..., 1 has 2n-2 steps; the second half
                // mirrors the first, so `period - position` walks back down.
                let period = 2 * (count - 1);
                let position = tick % period;
                if position < count {
                    Some(position as usize)
                } else {
                    Some((period - position) as usize)
                }
            }
            PlaybackMode::Reverse => Some((count - 1 - (tick % count)) as usize),
        }
    }

    /// The indices this mode shows over `ticks` ticks, in order.
    ///
    /// [`PlaybackMode::Once`] stops at the end, so its result is shorter than
    /// `ticks`; the other modes return exactly one index per tick (fewer only
    /// when there are no frames at all).
    #[must_use]
    pub fn frames(self, frame_count: usize, ticks: u64) -> Vec<usize> {
        let limit = match self {
            PlaybackMode::Once => ticks.min(frame_count as u64),
            _ => ticks,
        };
        (0..limit)
            .filter_map(|tick| self.frame_at(frame_count, tick))
            .collect()
    }
}

/// The frame sequence a bake of `mode` over `frame_count` frames produces, as
/// source-frame indices.
///
/// This is the plan a confirmation UI shows before the caller invokes
/// [`bake_playback`]. Once and loop bake to the document's own order, reverse
/// bakes to the order backwards, and ping-pong bakes to the reference's
/// `1,2,3,2,1` shape: forward through every frame, then back down to the first.
#[must_use]
pub fn bake_order(frame_count: usize, mode: PlaybackMode) -> Vec<usize> {
    match mode {
        PlaybackMode::Once | PlaybackMode::Loop => (0..frame_count).collect(),
        PlaybackMode::Reverse => (0..frame_count).rev().collect(),
        PlaybackMode::PingPong => {
            let mut order: Vec<usize> = (0..frame_count).collect();
            if frame_count > 1 {
                // `n-2` down to `0`: the last frame is not repeated, the first
                // one is, giving the reference's `1,2,3,2,1` for three frames.
                order.extend((0..frame_count - 1).rev());
            }
            order
        }
    }
}

/// How many frames a bake of `mode` over `frame_count` frames would produce.
///
/// Saturating, so a frame count so large that doubling would overflow reports
/// `usize::MAX`, which is always over the cap.
#[must_use]
pub fn bake_len(frame_count: usize, mode: PlaybackMode) -> usize {
    match mode {
        PlaybackMode::PingPong => frame_count.saturating_mul(2).saturating_sub(1),
        PlaybackMode::Once | PlaybackMode::Loop | PlaybackMode::Reverse => frame_count,
    }
}

/// Whether baking `mode` over `frame_count` frames would exceed
/// [`MAX_BAKE_FRAMES`] and therefore be refused.
///
/// The confirmation UI calls this (or compares [`bake_len`] with the cap) to
/// warn before it offers the bake.
#[must_use]
pub fn bake_exceeds_cap(frame_count: usize, mode: PlaybackMode) -> bool {
    bake_len(frame_count, mode) > MAX_BAKE_FRAMES
}

/// What a successful bake did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeOutcome {
    /// Frames the bake appended. Zero when the mode's order already equals the
    /// document's — once, loop, and any single-frame bake.
    pub added: usize,
    /// The document's frame count after the bake.
    pub total: usize,
}

/// Expand `mode`'s playback into explicit frames on `document`.
///
/// The document's own frame operations do the work
/// ([`EditorDocument::move_frame`] to reorder and
/// [`EditorDocument::duplicate_frame`] to append copies), so layer identity and
/// the model's index bookkeeping stay theirs. An undo snapshot is pushed before
/// the first change, so one undo returns the pre-bake document — including its
/// selection.
///
/// Returns `None` without touching the document when the resulting sequence
/// would exceed [`MAX_BAKE_FRAMES`] (see [`bake_exceeds_cap`]). An order that
/// already matches the document returns `Some` with no added frames and pushes
/// no snapshot, so a caller cannot tidy the history by asking for a no-op.
pub fn bake_playback(document: &mut EditorDocument, mode: PlaybackMode) -> Option<BakeOutcome> {
    let source_count = document.frame_count();
    if source_count == 0 || bake_exceeds_cap(source_count, mode) {
        return None;
    }
    let order = bake_order(source_count, mode);
    let is_identity = order
        .iter()
        .enumerate()
        .all(|(index, &source)| index == source);
    if is_identity {
        return Some(BakeOutcome {
            added: 0,
            total: source_count,
        });
    }

    // The snapshot also covers the impossible early return below: whatever
    // happens after this point is undoable.
    document.push_snapshot();

    // `positions[source]` tracks where source frame `source` currently sits.
    // Frames are identified by their original index, never by comparing pixels:
    // two frames may be pixel-identical and are still distinct frames.
    let mut positions: Vec<usize> = (0..source_count).collect();

    // First bring the original frames into the order's leading block. For every
    // mode that block is a permutation of the source frames; ping-pong's is the
    // identity, reverse's is the reversal.
    for (target, &source) in order.iter().take(source_count).enumerate() {
        let from = positions.get(source).copied()?;
        if from != target {
            document.move_frame(from, target);
            shift_positions(&mut positions, from, target);
        }
    }

    // Then append each remaining entry as a copy at the end. Duplicating at
    // `from` and moving the copy off the end leaves every source frame where it
    // was, so the tracked positions stay valid from one step to the next.
    for &source in order.iter().skip(source_count) {
        let from = positions.get(source).copied()?;
        let copy = document.duplicate_frame(from)?;
        let last = document.frame_count().checked_sub(1)?;
        document.move_frame(copy, last);
    }

    Some(BakeOutcome {
        added: order.len() - source_count,
        total: document.frame_count(),
    })
}

/// Follow tracked frame positions through a `move_frame(from, to)`.
///
/// Mirrors the model's own move bookkeeping: the moved frame lands at `to`,
/// frames between the two indices shift by one, and everything outside the
/// range stands still.
fn shift_positions(positions: &mut [usize], from: usize, to: usize) {
    if from == to {
        return;
    }
    for position in positions.iter_mut() {
        if *position == from {
            *position = to;
        } else if from < *position && *position <= to {
            *position -= 1;
        } else if to <= *position && *position < from {
            *position += 1;
        }
    }
}

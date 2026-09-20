//! Acceptance tests for frame operations and playback (plan task 27).
//!
//! Playback is exercised through the pure frame-order functions, so the tests
//! can assert exact index sequences for every mode. The bake is exercised
//! through the document, using the model's own frame operations. Each test ends
//! by proving the document was either untouched (playback, refused bakes) or
//! undoable (successful bakes).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use palace_prop::editor::tools::playback::{
    bake_exceeds_cap, bake_len, bake_order, bake_playback, BakeOutcome, PlaybackMode,
    MAX_BAKE_FRAMES,
};
use palace_prop::editor::EditorDocument;

/// A stable-for-this-run digest of a document: frame order and pixels, the
/// selection, and whether an undo is available.
fn digest(document: &EditorDocument) -> u64 {
    let mut hasher = DefaultHasher::new();
    document.frame_count().hash(&mut hasher);
    document.current_index().hash(&mut hasher);
    document.can_undo().hash(&mut hasher);
    for frame in document.frames() {
        frame.width().hash(&mut hasher);
        frame.height().hash(&mut hasher);
        frame
            .composite()
            .expect("test frames composite")
            .as_rgba()
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Every frame's composited RGBA bytes, in order.
fn frame_pixels(document: &EditorDocument) -> Vec<Vec<u8>> {
    document
        .frames()
        .iter()
        .map(|frame| {
            frame
                .composite()
                .expect("test frames composite")
                .as_rgba()
                .to_vec()
        })
        .collect()
}

/// The composited bytes of a solid one-pixel frame of `colour`.
fn pixel(colour: [u8; 4]) -> Vec<u8> {
    colour.to_vec()
}

/// A one-pixel frame per colour, so every frame is distinct and cheap.
fn tagged_document(colours: &[[u8; 4]]) -> EditorDocument {
    let (first, rest) = colours.split_first().expect("at least one colour");
    let mut document = EditorDocument::from_rgba(1, 1, first.to_vec()).expect("1x1 rgba");
    for colour in rest {
        document
            .add_blank_frame()
            .expect("a blank frame is appendable");
        document
            .current_frame_mut()
            .expect("a document always has a current frame")
            .layer_mut(0)
            .expect("a frame always has a base layer")
            .set_pixel(0, 0, *colour)
            .expect("(0, 0) is inside a 1x1 canvas");
    }
    document
}

/// A document of `frames` blank one-pixel frames.
fn blank_document(frames: usize) -> EditorDocument {
    let mut document = EditorDocument::blank(1, 1).expect("1x1 canvas");
    while document.frame_count() < frames {
        document
            .add_blank_frame()
            .expect("a blank frame is appendable");
    }
    document
}

const ALL_MODES: [PlaybackMode; 4] = [
    PlaybackMode::Once,
    PlaybackMode::Loop,
    PlaybackMode::PingPong,
    PlaybackMode::Reverse,
];

// (a) frame operations copy, paste, delete and reorder into the right sequence.
#[test]
fn frame_operations_produce_the_expected_sequence() {
    let [a, b, c] = [[1, 0, 0, 255], [0, 2, 0, 255], [0, 0, 3, 255]];
    let mut document = tagged_document(&[a, b, c]);

    // Duplicate the first frame: [A, A, B, C], selection on the copy.
    assert_eq!(document.duplicate_frame(0), Some(1));
    assert_eq!(document.current_index(), 1);
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(a), pixel(b), pixel(c)]
    );

    // Copy B and paste it after the current frame: [A, A, B, B, C].
    assert!(document.copy_frame(2), "frame 2 is B");
    assert!(document.has_clipboard());
    assert_eq!(document.paste_after_current(), Some(2));
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(a), pixel(b), pixel(b), pixel(c)]
    );

    // Reorder: move the first A to the end: [A, B, B, C, A].
    assert!(document.move_frame(0, 4));
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(b), pixel(b), pixel(c), pixel(a)]
    );
    assert_eq!(document.current_index(), 1, "selection follows the paste");

    // Delete the pasted B at index 1: [A, B, C, A].
    assert!(document.delete_frame(1).is_some());
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(b), pixel(c), pixel(a)]
    );
}

#[test]
fn deleting_a_frame_leaves_the_others_byte_identical() {
    let a = [10, 0, 0, 255];
    let b = [0, 20, 0, 255];
    let c = [0, 0, 30, 255];
    let mut document = tagged_document(&[a, b, c]);

    let removed = document.delete_frame(1).expect("frame 1 exists");
    assert_eq!(removed.layer_count(), 1);
    assert_eq!(document.frame_count(), 2);
    assert_eq!(frame_pixels(&document), [pixel(a), pixel(c)]);
}

// (b) all four playback modes compute the expected index sequence.
#[test]
fn playback_modes_produce_the_expected_frame_sequences() {
    // Three frames, two cycles where the mode cycles.
    assert_eq!(PlaybackMode::Once.frames(3, 6), [0, 1, 2]);
    assert_eq!(PlaybackMode::Loop.frames(3, 7), [0, 1, 2, 0, 1, 2, 0]);
    assert_eq!(
        PlaybackMode::PingPong.frames(3, 8),
        [0, 1, 2, 1, 0, 1, 2, 1]
    );
    assert_eq!(PlaybackMode::Reverse.frames(3, 6), [2, 1, 0, 2, 1, 0]);

    // The cycle boundaries, through `frame_at` directly.
    assert_eq!(PlaybackMode::Once.frame_at(3, 2), Some(2));
    assert_eq!(PlaybackMode::Once.frame_at(3, 3), None, "once stops");
    assert_eq!(PlaybackMode::Loop.frame_at(3, 3), Some(0));
    assert_eq!(PlaybackMode::PingPong.frame_at(3, 3), Some(1));
    assert_eq!(
        PlaybackMode::PingPong.frame_at(3, 4),
        Some(0),
        "a new cycle"
    );
    assert_eq!(PlaybackMode::Reverse.frame_at(3, 3), Some(2));

    // Two frames: ping-pong is just a two-step cycle.
    assert_eq!(PlaybackMode::PingPong.frames(2, 4), [0, 1, 0, 1]);

    // One frame plays itself in every mode, forever except once.
    assert_eq!(PlaybackMode::Once.frames(1, 4), [0]);
    assert_eq!(PlaybackMode::Loop.frames(1, 4), [0, 0, 0, 0]);
    assert_eq!(PlaybackMode::PingPong.frames(1, 4), [0, 0, 0, 0]);
    assert_eq!(PlaybackMode::Reverse.frames(1, 4), [0, 0, 0, 0]);

    // No frames, no frame index: a preview has nothing to show.
    for mode in ALL_MODES {
        assert_eq!(mode.frame_at(0, 0), None, "{mode:?} over no frames");
        assert!(mode.frames(0, 5).is_empty());
    }
}

// (c) playback never mutates the document.
#[test]
fn playback_does_not_mutate_the_document() {
    let a = [10, 0, 0, 255];
    let b = [0, 20, 0, 255];
    let c = [0, 0, 30, 255];
    let mut document = tagged_document(&[a, b, c]);
    assert!(document.set_current(1));
    let before = digest(&document);

    // Run a preview: many ticks of every mode, plus the bake plan queries the
    // confirmation UI would make.
    for mode in ALL_MODES {
        for tick in 0..64 {
            let _ = mode.frame_at(document.frame_count(), tick);
        }
        let _ = mode.frames(document.frame_count(), 64);
        let _ = bake_order(document.frame_count(), mode);
        let _ = bake_len(document.frame_count(), mode);
        let _ = bake_exceeds_cap(document.frame_count(), mode);
    }

    assert_eq!(digest(&document), before, "playback is a view operation");
    assert_eq!(document.frame_count(), 3);
    assert_eq!(document.current_index(), 1);
    assert!(!document.can_undo(), "playback pushes no snapshot");
}

// (d) baking ping-pong yields 1,2,3,2,1 and can be undone.
#[test]
fn bake_ping_pong_expands_the_sequence_like_the_reference() {
    let a = [10, 0, 0, 255];
    let b = [0, 20, 0, 255];
    let c = [0, 0, 30, 255];
    let mut document = tagged_document(&[a, b, c]);
    assert!(document.set_current(2));
    let before = digest(&document);

    // The plan the confirmation dialog shows.
    assert_eq!(bake_order(3, PlaybackMode::PingPong), [0, 1, 2, 1, 0]);
    assert_eq!(bake_len(3, PlaybackMode::PingPong), 5);
    assert!(!bake_exceeds_cap(3, PlaybackMode::PingPong));

    let outcome = bake_playback(&mut document, PlaybackMode::PingPong).expect("3 frames bake");
    assert_eq!(outcome, BakeOutcome { added: 2, total: 5 });
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(b), pixel(c), pixel(b), pixel(a)]
    );
    assert_ne!(digest(&document), before, "a bake changes the document");
    assert!(document.can_undo(), "a bake pushes a snapshot");

    assert!(document.undo(), "the bake must be undoable");
    assert_eq!(frame_pixels(&document), [pixel(a), pixel(b), pixel(c)]);
    assert_eq!(document.frame_count(), 3);
    assert_eq!(document.current_index(), 2, "undo restores the selection");
    assert_eq!(digest(&document), before);
}

// (e) baking reverse reverses the sequence, adding no frames.
#[test]
fn bake_reverse_reverses_the_frame_sequence() {
    let a = [1, 2, 3, 255];
    let b = [4, 5, 6, 255];
    let c = [7, 8, 9, 255];
    let d = [10, 11, 12, 255];
    let mut document = tagged_document(&[a, b, c, d]);

    assert_eq!(bake_order(4, PlaybackMode::Reverse), [3, 2, 1, 0]);
    let outcome = bake_playback(&mut document, PlaybackMode::Reverse).expect("4 frames bake");
    assert_eq!(outcome, BakeOutcome { added: 0, total: 4 });
    assert_eq!(
        frame_pixels(&document),
        [pixel(d), pixel(c), pixel(b), pixel(a)]
    );
    assert!(document.can_undo());
    assert!(document.undo());
    assert_eq!(
        frame_pixels(&document),
        [pixel(a), pixel(b), pixel(c), pixel(d)]
    );
}

// (f) once and loop bakes are identities: no frames, no history entry.
#[test]
fn bake_of_once_and_loop_is_an_identity() {
    let colours = [[1, 1, 1, 255], [2, 2, 2, 255], [3, 3, 3, 255]];
    for mode in [PlaybackMode::Once, PlaybackMode::Loop] {
        let mut document = tagged_document(&colours);
        let before = digest(&document);
        let outcome = bake_playback(&mut document, mode).expect("an identity bake succeeds");
        assert_eq!(outcome, BakeOutcome { added: 0, total: 3 });
        assert_eq!(digest(&document), before, "{mode:?} bake changes nothing");
        assert!(!document.can_undo(), "an identity bake pushes no snapshot");
    }

    // A one-frame document has nothing to bake in any mode.
    for mode in ALL_MODES {
        let mut document = tagged_document(&[[9, 9, 9, 255]]);
        let outcome = bake_playback(&mut document, mode).expect("a single frame bakes");
        assert_eq!(outcome.added, 0);
        assert_eq!(outcome.total, 1);
        assert!(!document.can_undo());
    }
}

// (g) the bake cap is 15 s of preview and refusals leave the document alone.
#[test]
fn bake_obeys_the_frame_cap() {
    assert_eq!(MAX_BAKE_FRAMES, 150, "15 s at ten ticks a second");

    // Ping-pong needs 2n-1 frames, so 75 frames bake to 149 and fit; 76 would
    // bake to 151 and must be refused.
    let fits = MAX_BAKE_FRAMES / 2;
    let over = fits + 1;
    assert_eq!(bake_len(fits, PlaybackMode::PingPong), 149);
    assert_eq!(bake_len(over, PlaybackMode::PingPong), 151);
    assert!(!bake_exceeds_cap(fits, PlaybackMode::PingPong));
    assert!(bake_exceeds_cap(over, PlaybackMode::PingPong));

    let mut refused = blank_document(over);
    let before = digest(&refused);
    assert_eq!(
        bake_playback(&mut refused, PlaybackMode::PingPong),
        None,
        "an over-cap bake is refused"
    );
    assert_eq!(digest(&refused), before, "a refusal changes nothing");
    assert!(!refused.can_undo(), "a refusal pushes no snapshot");

    let mut allowed = blank_document(fits);
    let outcome = bake_playback(&mut allowed, PlaybackMode::PingPong).expect("75 frames bake");
    assert_eq!(outcome.added, fits - 1);
    assert_eq!(outcome.total, 149);
    assert_eq!(allowed.frame_count(), MAX_BAKE_FRAMES - 1);
    assert!(allowed.can_undo());

    // Reverse adds no frames, so the cap bites only past 150 frames.
    let mut at_cap = blank_document(MAX_BAKE_FRAMES);
    assert!(bake_playback(&mut at_cap, PlaybackMode::Reverse).is_some());
    assert_eq!(at_cap.frame_count(), MAX_BAKE_FRAMES);

    let mut past_cap = blank_document(MAX_BAKE_FRAMES + 1);
    let before = digest(&past_cap);
    assert_eq!(bake_playback(&mut past_cap, PlaybackMode::Reverse), None);
    assert_eq!(digest(&past_cap), before);
}

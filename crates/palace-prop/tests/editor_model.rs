//! Acceptance tests for the editor document model (plan task 21).
//!
//! These exercise the model through its public surface only: no filesystem, no
//! network, no webview. The five required behaviours are undo/redo exactness, the
//! two history caps, frame add/duplicate/delete/reorder/copy/paste, import
//! dimensions and alpha, and deterministic compositing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use palace_prop::editor::{EditorDocument, History, Transform, MAX_UNDO_BYTES, MAX_UNDO_SNAPSHOTS};
use palace_prop::image::PropImage;
use palace_prop::{Prop, PropEndian, PropHeader};

/// A stable-for-this-run digest of an image's dimensions and pixels.
fn digest(image: &PropImage) -> u64 {
    let mut hasher = DefaultHasher::new();
    image.width().hash(&mut hasher);
    image.height().hash(&mut hasher);
    image.as_rgba().hash(&mut hasher);
    hasher.finish()
}

/// A deterministic overlay image, half of whose pixels are opaque.
fn overlay(width: u32, height: u32) -> PropImage {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (i, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        pixel[0] = ((i * 11) % 256) as u8;
        pixel[1] = ((i * 7) % 256) as u8;
        pixel[2] = ((i * 3) % 256) as u8;
        pixel[3] = if i % 2 == 0 { 255 } else { 128 };
    }
    PropImage::from_rgba(width, height, rgba).expect("overlay dimensions match its buffer")
}

// (a) blank -> draw -> undo -> redo returns the exact prior pixels.
#[test]
fn draw_undo_redo_returns_the_exact_prior_pixels() {
    let mut document = EditorDocument::blank(44, 44).unwrap();
    let before = digest(&document.composite_current().unwrap());

    document.push_snapshot();
    document
        .current_frame_mut()
        .unwrap()
        .layer_mut(0)
        .unwrap()
        .set_pixel(5, 5, [255, 0, 0, 255])
        .unwrap();
    let after = digest(&document.composite_current().unwrap());
    assert_ne!(before, after, "the stroke must change the frame");

    assert!(
        document.undo(),
        "an undo must be available after a snapshot"
    );
    assert_eq!(digest(&document.composite_current().unwrap()), before);
    // Exact bytes, not just equal digests.
    assert!(document
        .composite_current()
        .unwrap()
        .as_rgba()
        .iter()
        .all(|&b| b == 0));
    assert!(document.can_redo());

    assert!(document.redo(), "the undone state must be reapplied");
    assert_eq!(digest(&document.composite_current().unwrap()), after);
    assert_eq!(
        document.composite_current().unwrap().pixel(5, 5).unwrap(),
        [255, 0, 0, 255]
    );
}

// (b) the undo depth and memory caps are enforced, oldest evicted.
#[test]
fn the_undo_depth_cap_evicts_the_oldest_snapshots() {
    let mut document = EditorDocument::blank_with_history_caps(2, 2, 3, usize::MAX).unwrap();
    for _ in 0..20 {
        document.push_snapshot();
    }
    assert_eq!(document.history().undo_len(), 3, "depth cap holds");
    assert_eq!(document.history().memory_bytes(), 3 * 16, "2x2x4 per state");

    let mut undos = 0;
    while document.undo() {
        undos += 1;
    }
    assert_eq!(undos, 3, "only the cap's worth of snapshots remains");
    assert!(!document.can_undo());
}

#[test]
fn the_undo_memory_cap_evicts_the_oldest_snapshots() {
    let mut document = EditorDocument::blank_with_history_caps(10, 10, usize::MAX, 1000).unwrap();
    for _ in 0..5 {
        document.push_snapshot();
    }
    // 10x10x4 = 400 bytes per state, so only two fit under a 1000-byte cap.
    assert_eq!(document.history().undo_len(), 2);
    assert!(document.history().memory_bytes() <= 1000);

    let mut undos = 0;
    while document.undo() {
        undos += 1;
    }
    assert_eq!(undos, 2, "oldest states were evicted, newest kept");
    assert!(document.history().memory_bytes() <= 1000);
}

#[test]
fn the_default_caps_are_the_documented_values() {
    let history = History::new();
    assert_eq!(history.depth_cap(), MAX_UNDO_SNAPSHOTS);
    assert_eq!(history.byte_cap(), MAX_UNDO_BYTES);
    assert_eq!(MAX_UNDO_SNAPSHOTS, 32);
    assert_eq!(MAX_UNDO_BYTES, 128 * 1024 * 1024);
}

// (c) frame add/duplicate/delete/reorder preserves order and content.
#[test]
fn frame_operations_preserve_order_and_content() {
    let mut document = EditorDocument::blank(4, 4).unwrap();
    document
        .current_frame_mut()
        .unwrap()
        .layer_mut(0)
        .unwrap()
        .set_pixel(0, 0, [9, 8, 7, 255])
        .unwrap();
    let tagged = digest(&document.composite_current().unwrap());

    // add blank
    let blank = document.add_blank_frame().unwrap();
    assert_eq!(blank, 1);
    assert_eq!(document.current_index(), 1);
    assert_ne!(digest(&document.composite_current().unwrap()), tagged);

    // duplicate frame 0, inserted at 1
    let duplicate = document.duplicate_frame(0).unwrap();
    assert_eq!(duplicate, 1);
    assert_eq!(document.frame_count(), 3);
    assert_eq!(
        digest(&document.frame(1).unwrap().composite().unwrap()),
        tagged
    );

    // copy frame 0, paste after current (1) -> index 2
    assert!(document.copy_frame(0));
    assert!(document.has_clipboard());
    let pasted = document.paste_after_current().unwrap();
    assert_eq!(pasted, 2);
    assert_eq!(document.frame_count(), 4);
    assert_eq!(
        digest(&document.frame(2).unwrap().composite().unwrap()),
        tagged
    );

    // reorder: [f0, dup0, pasted0, blank] -> move 0 to 3 -> [dup0, pasted0, blank, f0]
    assert!(document.move_frame(0, 3));
    assert_eq!(
        digest(&document.frame(3).unwrap().composite().unwrap()),
        tagged
    );
    assert_eq!(
        document.current_index(),
        1,
        "selection follows the moved frame"
    );

    // delete: remove the duplicate at 0
    assert!(document.delete_frame(0).is_some());
    assert_eq!(document.frame_count(), 3);
    assert_eq!(
        digest(&document.frame(2).unwrap().composite().unwrap()),
        tagged
    );

    // a document always keeps at least one frame
    while document.frame_count() > 1 {
        assert!(document.delete_frame(0).is_some());
    }
    assert_eq!(document.frame_count(), 1);
    assert!(document.delete_frame(0).is_none());

    // out-of-range operations are refused, not panics
    assert!(!document.move_frame(0, 5));
    assert!(!document.copy_frame(5));
    assert!(document.duplicate_frame(5).is_none());
}

// (d) an import yields correct dimensions and alpha.
#[test]
fn an_import_yields_correct_dimensions_and_alpha() {
    let rgba = vec![
        10, 20, 30, 0, // fully transparent
        40, 50, 60, 255, // opaque
        70, 80, 90, 128, // half transparent
    ];
    let document = EditorDocument::from_rgba(3, 1, rgba.clone()).unwrap();
    assert_eq!(document.frame_count(), 1);
    let frame = document.current_frame().unwrap();
    assert_eq!((frame.width(), frame.height()), (3, 1));

    let image = document.composite_current().unwrap();
    assert_eq!((image.width(), image.height()), (3, 1));
    assert_eq!(
        image.pixel(0, 0).unwrap()[3],
        0,
        "a transparent input stays transparent"
    );
    assert_eq!(
        image.pixel(1, 0).unwrap(),
        [40, 50, 60, 255],
        "an opaque input survives exactly"
    );
    let half = image.pixel(2, 0).unwrap();
    assert_eq!(half[3], 128, "a half-transparent input keeps its alpha");
    assert!((i32::from(half[0]) - 70).abs() <= 1, "colour is preserved");

    // From an already-decoded prop.
    let prop = Prop {
        header: PropHeader {
            width: 3,
            height: 1,
            h_offset: 0,
            v_offset: 0,
            script_offset: 0,
            flags: 0,
            endian: PropEndian::Little,
        },
        image: PropImage::from_rgba(3, 1, rgba).unwrap(),
    };
    let from_prop = EditorDocument::from_prop(&prop);
    let prop_frame = from_prop.current_frame().unwrap();
    assert_eq!((prop_frame.width(), prop_frame.height()), (3, 1));
    assert_eq!(
        from_prop.composite_current().unwrap().pixel(1, 0).unwrap()[3],
        255
    );

    // The default canvas is 44x44 and fully transparent.
    let blank = EditorDocument::default();
    let blank_image = blank.composite_current().unwrap();
    assert_eq!((blank_image.width(), blank_image.height()), (44, 44));
    assert!(blank_image.as_rgba().iter().all(|&b| b == 0));

    // Nonsensical sizes are refused, not panics.
    assert!(EditorDocument::blank(0, 0).is_err());
}

// (e) compositing two layers is deterministic.
#[test]
fn compositing_two_layers_is_deterministic() {
    let mut document = EditorDocument::blank(8, 8).unwrap();
    document
        .current_frame_mut()
        .unwrap()
        .layer_mut(0)
        .unwrap()
        .edit_pixels(|bytes| {
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = ((i * 37) % 256) as u8;
            }
        })
        .unwrap();
    let base_only = digest(&document.composite_current().unwrap());
    assert_eq!(document.current_frame().unwrap().layer_count(), 1);
    assert_eq!(document.current_frame().unwrap().overlay_count(), 0);

    document.add_layer("Overlay", overlay(8, 8)).unwrap();
    assert_eq!(document.current_frame().unwrap().layer_count(), 2);
    assert_eq!(document.current_frame().unwrap().overlay_count(), 1);
    {
        let layer = document.current_frame_mut().unwrap().layer_mut(1).unwrap();
        layer.opacity = 0.75;
        layer.transform = Transform {
            x: 1,
            y: -1,
            rotation: 15.0,
            scale: 1.25,
        };
    }

    let first = document.composite_current().unwrap();
    let second = document.composite_current().unwrap();
    assert_eq!(
        first, second,
        "compositing must be byte-for-byte repeatable"
    );
    assert_ne!(
        digest(&first),
        base_only,
        "the visible overlay must contribute"
    );
    assert!(
        first.as_rgba().chunks_exact(4).any(|pixel| pixel[3] > 0),
        "the overlay leaves visible pixels"
    );
}

//! Whole-corpus render pass: every captured room must composite without
//! panicking, at a sane size, with no asset assumed present.
//!
//! The test deliberately uses **empty** asset stores. That makes it fast (no
//! media index, no prop lookups) and is the hostile case: every room renders
//! backgroundless with placeholder props, which is exactly the path a room whose
//! assets are missing takes. Nothing here depends on the media mirror being
//! present, so it runs anywhere the payload corpus exists.
//!
//! It is skipped when the corpus is absent, so it never blocks a machine without
//! the reference data. Point `PALACE_ROOM_CORPUS` at another directory to run
//! against a different corpus.

use std::path::PathBuf;

use palace_render::{
    render, AnimationClock, MediaStore, PropStore, RenderOptions, RoomSource, SceneBuilder,
    MIN_ROOM_HEIGHT, MIN_ROOM_WIDTH,
};
use palace_wire::ByteOrder;

fn corpus_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PALACE_ROOM_CORPUS") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join("colosseum").join("payloads_all");
    dir.is_dir().then_some(dir)
}

#[test]
fn every_corpus_room_composites_without_panicking() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipping corpus render: no payload corpus found");
        return;
    };
    let source = RoomSource::new(dir.clone(), ByteOrder::Little);
    let rooms = source.all();
    assert!(!rooms.is_empty(), "corpus directory is empty");

    let builder = SceneBuilder::new(MediaStore::default(), PropStore::new());
    let mut max_size = (0u32, 0u32);
    let mut loose_props = 0usize;
    let mut overlays = 0usize;
    let mut missing = 0usize;

    for (path, room) in &rooms {
        let scene = builder.build(room, &[]);
        let (w, h) = scene.size;
        assert!(
            w >= MIN_ROOM_WIDTH as u32 && h >= MIN_ROOM_HEIGHT as u32,
            "{}: room {} rendered smaller than the floor: {w}x{h}",
            path.display(),
            room.header.room_id
        );
        assert!(
            w <= palace_render::MAX_ROOM_DIMENSION && h <= palace_render::MAX_ROOM_DIMENSION,
            "{}: room {} exceeded the safety cap: {w}x{h}",
            path.display(),
            room.header.room_id
        );
        max_size = (max_size.0.max(w), max_size.1.max(h));
        loose_props += scene.loose_props.len();
        overlays += scene.overlays_above_nothing.len()
            + scene.overlays_above_avatars.len()
            + scene.overlays_above_name_tags.len()
            + scene.overlays_above_everything.len();
        missing += scene.notes.len();

        let canvas = render(
            &scene,
            RenderOptions {
                dpr: 1.0,
                clock: AnimationClock::zero(),
            },
        );
        assert_eq!((canvas.width(), canvas.height()), (w, h));
        // The frame must be exactly width*height*4 RGBA bytes.
        assert_eq!(canvas.as_rgba().len(), (w as usize) * (h as usize) * 4);

        // With empty stores every prop must be reported, never silently skipped.
        assert_eq!(
            scene.loose_props.len(),
            scene
                .notes
                .iter()
                .filter(|n| matches!(n, palace_render::AssetNote::MissingProp { .. }))
                .count(),
            "{}: room {} loose props must each be reported",
            path.display(),
            room.header.room_id
        );
    }

    eprintln!(
        "corpus render: {} records, max room {max_size:?}, {loose_props} loose props, \
         {overlays} overlays, {missing} asset notes",
        rooms.len()
    );
}

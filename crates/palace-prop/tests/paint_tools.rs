//! Acceptance tests for the paint tools (plan task 23).
//!
//! These exercise the pure operations through their public surface only: no
//! filesystem, no webview, no mutation of the input images. The pinned
//! behaviours are brush coverage, eraser alpha, bounded flood fill, tolerance
//! matching, clear-frame transparency and line strokes; the Layer/Frame and
//! apply-to-all-frames wrappers are covered alongside them.

use palace_prop::editor::tools::paint::{
    brush_stroke, brush_stroke_all_frames, brush_stroke_frame, brush_stroke_layer,
    clear_all_frames, clear_frame, clear_pixels, erase_stroke, flood_fill, pick_color,
    pick_color_layer, MAX_BRUSH_SIZE, MIN_BRUSH_SIZE,
};
use palace_prop::editor::{Frame, Layer, LayerId};
use palace_prop::image::PropImage;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// An image whose every pixel is `rgba`.
fn solid(width: u32, height: u32, rgba: [u8; 4]) -> PropImage {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        bytes.extend_from_slice(&rgba);
    }
    PropImage::from_rgba(width, height, bytes).expect("solid buffer matches its dimensions")
}

/// Pixel at `(x, y)`, panicking with a clear message if the test is wrong.
fn pixel(image: &PropImage, x: u32, y: u32) -> [u8; 4] {
    image.pixel(x, y).expect("test coordinates are in bounds")
}

/// How many pixels carry any alpha.
fn opaque_count(image: &PropImage) -> usize {
    image
        .as_rgba()
        .chunks_exact(4)
        .filter(|pixel| pixel[3] != 0)
        .count()
}

/// Every pixel of the image, for whole-image transparency checks.
fn all_transparent(image: &PropImage) -> bool {
    image.as_rgba().iter().all(|&byte| byte == 0)
}

// Brush: the size-1 stamp paints exactly the point, the alpha is forced opaque,
// and the size is clamped into the supported 1..=48 range.
#[test]
fn brush_stroke_paints_opaque_pixels_of_the_given_size() {
    assert_eq!(MIN_BRUSH_SIZE, 1);
    assert_eq!(MAX_BRUSH_SIZE, 48);

    let blank = PropImage::transparent(9, 9);

    // A click with a translucent colour still paints opaque red, one pixel.
    let dot = brush_stroke(&blank, (4, 4), (4, 4), [255, 0, 0, 17], 1).unwrap();
    assert_eq!(pixel(&dot, 4, 4), RED, "alpha must be forced opaque");
    assert_eq!(opaque_count(&dot), 1, "a size-1 click is a single pixel");

    // A size-3 stamp centred on (4, 4) is the full 3x3 block.
    let block = brush_stroke(&blank, (4, 4), (4, 4), RED, 3).unwrap();
    for y in 3..=5 {
        for x in 3..=5 {
            assert_eq!(pixel(&block, x, y), RED, "stamp pixel ({x}, {y})");
        }
    }
    assert_eq!(opaque_count(&block), 9);
    assert_eq!(pixel(&block, 2, 4), CLEAR, "outside the 3x3 stamp");

    // A size-2 stamp is a 2x2 block anchored up and left of the centre, the
    // same convention as the render crate's square brush.
    let pair = brush_stroke(&blank, (4, 4), (4, 4), RED, 2).unwrap();
    assert_eq!(pixel(&pair, 3, 3), RED);
    assert_eq!(pixel(&pair, 3, 4), RED);
    assert_eq!(pixel(&pair, 4, 3), RED);
    assert_eq!(pixel(&pair, 4, 4), RED);
    assert_eq!(pixel(&pair, 5, 5), CLEAR);
    assert_eq!(opaque_count(&pair), 4);

    // Size 0 reads as size 1; an absurd size clamps to 48.
    let zero = brush_stroke(&blank, (4, 4), (4, 4), RED, 0).unwrap();
    assert_eq!(opaque_count(&zero), 1, "size 0 clamps up to 1");

    let wide = PropImage::transparent(64, 64);
    let biggest = brush_stroke(&wide, (32, 32), (32, 32), RED, 1000).unwrap();
    assert_eq!(opaque_count(&biggest), 48 * 48, "size clamps down to 48");
}

// A drag covers every pixel between the endpoints, straight or diagonal.
#[test]
fn a_stroke_between_two_points_covers_the_line() {
    let blank = PropImage::transparent(16, 16);

    let horizontal = brush_stroke(&blank, (1, 2), (10, 2), RED, 1).unwrap();
    for x in 1..=10 {
        assert_eq!(pixel(&horizontal, x, 2), RED, "line pixel x={x}");
    }
    assert_eq!(pixel(&horizontal, 0, 2), CLEAR, "before the start");
    assert_eq!(pixel(&horizontal, 11, 2), CLEAR, "after the end");
    assert_eq!(opaque_count(&horizontal), 10, "no overpainting");

    // A diagonal Bresenham line has exactly one pixel per step.
    let diagonal = brush_stroke(&blank, (0, 0), (5, 5), RED, 1).unwrap();
    for i in 0..=5 {
        assert_eq!(pixel(&diagonal, i, i), RED, "diagonal pixel {i}");
    }
    assert_eq!(opaque_count(&diagonal), 6);
    assert_eq!(pixel(&diagonal, 3, 4), CLEAR, "off-line pixel");

    // A stroke that leaves and re-enters the canvas is clipped, not refused.
    let clipped = brush_stroke(&blank, (-4, 8), (4, 8), RED, 1).unwrap();
    for x in 0..=4 {
        assert_eq!(pixel(&clipped, x, 8), RED, "clipped line x={x}");
    }
    assert_eq!(opaque_count(&clipped), 5);
}

// The eraser clears alpha; it does not paint a colour over the pixels.
#[test]
fn the_eraser_clears_alpha_instead_of_painting_over() {
    let opaque = solid(8, 8, [200, 100, 50, 255]);

    let erased = erase_stroke(&opaque, (4, 4), (4, 4), 3).unwrap();
    for y in 3..=5 {
        for x in 3..=5 {
            assert_eq!(pixel(&erased, x, y), CLEAR, "erased pixel ({x}, {y})");
        }
    }
    assert_eq!(
        pixel(&erased, 0, 0),
        [200, 100, 50, 255],
        "untouched pixels keep their colour"
    );
    assert_eq!(opaque_count(&erased), 8 * 8 - 9);

    // Erasing already-transparent pixels is a no-op, and a line erases a line.
    let stroke = erase_stroke(&opaque, (0, 0), (7, 0), 1).unwrap();
    for x in 0..8 {
        assert_eq!(pixel(&stroke, x, 0), CLEAR, "erased line x={x}");
    }
    assert_eq!(pixel(&stroke, 0, 1), [200, 100, 50, 255]);
}

// A fill stops at a colour boundary and never touches the far side.
#[test]
fn flood_fill_fills_a_bounded_region_and_leaves_the_outside_unchanged() {
    // 9x9 white canvas with a red 5x5 box whose interior is a 3x3 white well.
    let mut bytes = Vec::with_capacity(9 * 9 * 4);
    for y in 0..9u32 {
        for x in 0..9u32 {
            let on_border = (2..=6).contains(&x)
                && (2..=6).contains(&y)
                && (x == 2 || x == 6 || y == 2 || y == 6);
            if on_border {
                bytes.extend_from_slice(&RED);
            } else {
                bytes.extend_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    let boxed = PropImage::from_rgba(9, 9, bytes).expect("9x9 buffer");

    let filled = flood_fill(&boxed, 4, 4, BLUE, 0.0).unwrap();

    for y in 3..=5 {
        for x in 3..=5 {
            assert_eq!(pixel(&filled, x, y), BLUE, "interior pixel ({x}, {y})");
        }
    }
    for (x, y) in [(2, 4), (6, 4), (4, 2), (4, 6), (2, 2), (6, 6)] {
        assert_eq!(pixel(&filled, x, y), RED, "border pixel ({x}, {y})");
    }
    for (x, y) in [(0, 0), (8, 8), (1, 4), (4, 1), (7, 4), (4, 7)] {
        assert_eq!(
            pixel(&filled, x, y),
            [255, 255, 255, 255],
            "outside pixel ({x}, {y}) must stay white"
        );
    }
    assert_eq!(
        boxed.pixel(4, 4),
        Some([255, 255, 255, 255]),
        "the input image itself is untouched"
    );

    // Filling an untouched white area reaches the canvas edge, and a seed off
    // the image is a no-op.
    let edge = flood_fill(&boxed, 0, 0, BLUE, 0.0).unwrap();
    assert_eq!(pixel(&edge, 0, 0), BLUE);
    assert_eq!(pixel(&edge, 1, 1), BLUE);
    assert_eq!(pixel(&edge, 2, 4), RED, "still stopped by the border");
    let off = flood_fill(&boxed, 99, 99, BLUE, 0.0).unwrap();
    assert_eq!(off.as_rgba(), boxed.as_rgba(), "off-canvas seed is a no-op");
}

// Tolerance 0 is exact equality; any tolerance above 0 admits near matches.
#[test]
fn tolerance_zero_matches_exactly_and_higher_tolerance_includes_near_matches() {
    // Three adjacent pixels: the seed, a near match 3 levels away, and a far
    // pixel 40 levels away.
    let mut bytes = Vec::with_capacity(3 * 4);
    bytes.extend_from_slice(&[100, 100, 100, 255]);
    bytes.extend_from_slice(&[103, 103, 103, 255]);
    bytes.extend_from_slice(&[140, 140, 140, 255]);
    let ramp = PropImage::from_rgba(3, 1, bytes).expect("3x1 buffer");

    let exact = flood_fill(&ramp, 0, 0, BLUE, 0.0).unwrap();
    assert_eq!(pixel(&exact, 0, 0), BLUE, "the seed is filled");
    assert_eq!(
        pixel(&exact, 1, 0),
        [103, 103, 103, 255],
        "tolerance 0 rejects the near match"
    );
    assert_eq!(pixel(&exact, 2, 0), [140, 140, 140, 255]);

    // 0.05 * 255 = 12.75: the near match (3) joins, the far pixel (40) does not.
    let loose = flood_fill(&ramp, 0, 0, BLUE, 0.05).unwrap();
    assert_eq!(pixel(&loose, 0, 0), BLUE);
    assert_eq!(
        pixel(&loose, 1, 0),
        BLUE,
        "near match joins at tolerance 0.05"
    );
    assert_eq!(
        pixel(&loose, 2, 0),
        [140, 140, 140, 255],
        "far match stays out"
    );

    // Out-of-range and NaN tolerances are clamped, not panics.
    let clamped = flood_fill(&ramp, 0, 0, BLUE, 50.0).unwrap();
    assert_eq!(pixel(&clamped, 1, 0), BLUE);
    assert_eq!(pixel(&clamped, 2, 0), BLUE, "tolerance 50 clamps to 1.0");
    let nan = flood_fill(&ramp, 0, 0, BLUE, f32::NAN).unwrap();
    assert_eq!(pixel(&nan, 1, 0), [103, 103, 103, 255], "NaN reads as 0.0");

    // Alpha is part of the match: an opaque near-colour over a transparent
    // canvas is not swallowed by filling the transparency around it.
    let mixed =
        PropImage::from_rgba(2, 1, vec![0, 0, 0, 0, 100, 100, 100, 255]).expect("2x1 buffer");
    let over = flood_fill(&mixed, 0, 0, BLUE, 1.0).unwrap();
    assert_eq!(pixel(&over, 0, 0), BLUE);
    assert_eq!(pixel(&over, 1, 0), BLUE, "tolerance 1.0 matches everything");
}

// Clearing a frame empties every layer and keeps the frame's structure.
#[test]
fn clear_frame_makes_everything_transparent() {
    let mut frame = Frame::from_base(solid(4, 4, [9, 8, 7, 255]));
    frame.push_layer(
        Layer::new(LayerId::new(1), "Overlay", solid(4, 4, [1, 2, 3, 255])).with_opacity(0.5),
    );

    let cleared = clear_frame(&frame);
    assert_eq!(cleared.width(), 4);
    assert_eq!(cleared.height(), 4);
    assert_eq!(cleared.layer_count(), 2, "clearing keeps the layer stack");
    for layer in cleared.layers() {
        assert!(all_transparent(layer.image()), "layer must be transparent");
    }
    assert_eq!(cleared.layer(1).unwrap().name, "Overlay");
    assert_eq!(cleared.layer(1).unwrap().opacity, 0.5);
    assert!(
        !all_transparent(frame.base().unwrap().image()),
        "the source frame is untouched"
    );

    // The image-level helper and the multi-frame helper agree.
    assert!(all_transparent(&clear_pixels(
        frame.base().unwrap().image()
    )));
    let frames = vec![
        Frame::from_base(solid(4, 4, [9, 8, 7, 255])),
        Frame::from_base(solid(4, 4, [1, 2, 3, 255])),
    ];
    let cleared_all = clear_all_frames(&frames);
    assert_eq!(cleared_all.len(), 2);
    for frame in &cleared_all {
        assert!(all_transparent(frame.base().unwrap().image()));
    }
}

// The colour picker samples exactly the layer pixel at the point.
#[test]
fn the_colour_picker_samples_the_colour_at_a_point() {
    let image = solid(4, 4, [11, 22, 33, 44]);
    assert_eq!(pick_color(&image, 2, 2), Some([11, 22, 33, 44]));
    assert_eq!(pick_color(&image, 4, 0), None, "off the right edge");
    assert_eq!(pick_color(&image, 0, 4), None, "off the bottom edge");

    let layer = Layer::new(LayerId::new(3), "Paint", image);
    assert_eq!(pick_color_layer(&layer, 3, 3), Some([11, 22, 33, 44]));
    assert_eq!(pick_color_layer(&layer, 0, 9), None);
}

// Layer and frame wrappers keep identity/metadata and report missing layers.
#[test]
fn layer_and_frame_wrappers_preserve_metadata_and_report_missing_layers() {
    let layer =
        Layer::new(LayerId::new(7), "Paint", PropImage::transparent(8, 8)).with_opacity(0.25);
    let painted = brush_stroke_layer(&layer, (1, 1), (1, 1), RED, 1).unwrap();
    assert_eq!(painted.id, LayerId::new(7));
    assert_eq!(painted.name, "Paint");
    assert_eq!(painted.opacity, 0.25);
    assert_eq!(pixel(painted.image(), 1, 1), RED);

    // The source layer is unchanged: operations are pure.
    assert_eq!(pixel(layer.image(), 1, 1), CLEAR);

    let frame = Frame::from_base(PropImage::transparent(8, 8));
    let stroke = brush_stroke_frame(&frame, 0, (2, 3), (6, 3), RED, 1)
        .unwrap()
        .expect("layer 0 exists");
    for x in 2..=6 {
        assert_eq!(pixel(stroke.base().unwrap().image(), x, 3), RED);
    }
    assert!(brush_stroke_frame(&frame, 9, (2, 3), (6, 3), RED, 1)
        .unwrap()
        .is_none());
    assert_eq!(
        pixel(frame.base().unwrap().image(), 2, 3),
        CLEAR,
        "source frame"
    );
}

// "Apply to all frames" variants hit every frame and leave missing layers be.
#[test]
fn apply_to_all_frames_variants_paint_every_frame() {
    let f1 = Frame::from_base(PropImage::transparent(8, 8));
    let f2 = Frame::from_base(solid(8, 8, [50, 50, 50, 255]));

    let painted =
        brush_stroke_all_frames(&[f1.clone(), f2.clone()], 0, (1, 1), (1, 1), RED, 1).unwrap();
    assert_eq!(painted.len(), 2);
    assert_eq!(pixel(painted[0].base().unwrap().image(), 1, 1), RED);
    assert_eq!(pixel(painted[1].base().unwrap().image(), 1, 1), RED);
    assert_eq!(
        pixel(painted[1].base().unwrap().image(), 0, 0),
        [50, 50, 50, 255],
        "only the stroke changes"
    );

    // A frame without the requested layer comes back byte-for-byte unchanged.
    let skipped =
        brush_stroke_all_frames(std::slice::from_ref(&f1), 4, (1, 1), (1, 1), RED, 1).unwrap();
    assert_eq!(
        skipped[0].base().unwrap().image().as_rgba(),
        f1.base().unwrap().image().as_rgba()
    );
}

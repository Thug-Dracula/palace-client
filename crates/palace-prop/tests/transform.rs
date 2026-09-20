//! Acceptance tests for the transform tools (plan task T24).
//!
//! These exercise the public surface only: no filesystem, no network, no
//! webview. The required behaviours are flip twice == identity, four 90-degree
//! rotations == identity, centred square crop, aspect-preserving constrained
//! resize, over-limit requests refused with a clear error, and alpha preserved
//! through every operation.

use palace_prop::editor::tools::transform::{
    crop, crop_square, flip_horizontal, flip_vertical, resize, resize_to_fit, resize_to_height,
    resize_to_width, rotate_180, rotate_ccw, rotate_cw, rotate_frames, shape_crop,
    shape_crop_frames, transform_frame, CropRect, CropShape, QuarterTurn, Resample,
    ShapeCropOptions, SIZE_PRESETS,
};
use palace_prop::editor::{Frame, Layer, LayerId};
use palace_prop::{PropError, PropImage};

/// A deterministic, non-uniform test image. Colours differ per pixel (so a
/// mirror or rotation is detectable) and alpha is always in `32..=231` (so a
/// "lost alpha" bug cannot hide behind a zero).
fn pattern(width: u32, height: u32) -> PropImage {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        let x = index as u32 % width;
        let y = index as u32 / width;
        pixel[0] = (x * 17 + y * 3) as u8;
        pixel[1] = (x * 5 + y * 29) as u8;
        pixel[2] = (x * 11 + y * 7) as u8;
        pixel[3] = 32 + ((x * 37 + y * 61) % 200) as u8;
    }
    PropImage::from_rgba(width, height, rgba).unwrap()
}

/// `pattern` with one pixel overwritten by a marker colour nothing else uses.
fn marked(width: u32, height: u32, at: (u32, u32)) -> PropImage {
    let source = pattern(width, height);
    let mut rgba = source.as_rgba().to_vec();
    let index = ((at.1 * width + at.0) * 4) as usize;
    rgba[index..index + 4].copy_from_slice(&[255, 0, 255, 255]);
    PropImage::from_rgba(width, height, rgba).unwrap()
}

/// A zoom far above the shape-crop clamp; the request must not overflow.
const MAX_ZOOM_REQUEST: f32 = 1.0e9;

/// A shape crop with square mask, natural size, no rotation, nearest sampling.
fn plain_options() -> ShapeCropOptions {
    ShapeCropOptions {
        shape: CropShape::Square,
        zoom: 1.0,
        rotation: 0.0,
        resample: Resample::Nearest,
    }
}

// -- Flip -------------------------------------------------------------------

#[test]
fn flip_horizontal_twice_is_identity() {
    let image = pattern(7, 5);
    let once = flip_horizontal(&image).unwrap();
    assert_ne!(once, image, "a flip must change a non-symmetric image");
    let twice = flip_horizontal(&once).unwrap();
    assert_eq!(twice, image);
}

#[test]
fn flip_vertical_twice_is_identity() {
    let image = pattern(7, 5);
    let once = flip_vertical(&image).unwrap();
    assert_ne!(once, image);
    let twice = flip_vertical(&once).unwrap();
    assert_eq!(twice, image);
}

#[test]
fn flip_horizontal_then_vertical_is_rotate_180() {
    let image = pattern(7, 5);
    let both = flip_vertical(&flip_horizontal(&image).unwrap()).unwrap();
    assert_eq!(both, rotate_180(&image).unwrap());
}

// -- Rotate -----------------------------------------------------------------

#[test]
fn four_clockwise_quarter_turns_are_byte_exact_identity() {
    let image = pattern(7, 5);
    let mut turned = image.clone();
    for _ in 0..4 {
        turned = rotate_cw(&turned).unwrap();
    }
    assert_eq!(turned, image);
}

#[test]
fn four_counter_clockwise_quarter_turns_are_byte_exact_identity() {
    let image = pattern(7, 5);
    let mut turned = image.clone();
    for _ in 0..4 {
        turned = rotate_ccw(&turned).unwrap();
    }
    assert_eq!(turned, image);
}

#[test]
fn clockwise_then_counter_clockwise_is_identity() {
    let image = pattern(7, 5);
    assert_eq!(rotate_ccw(&rotate_cw(&image).unwrap()).unwrap(), image);
    assert_eq!(rotate_cw(&rotate_ccw(&image).unwrap()).unwrap(), image);
    assert_eq!(
        rotate_180(&rotate_180(&image).unwrap()).unwrap(),
        image,
        "two half turns are the identity"
    );
}

#[test]
fn quarter_turns_swap_dimensions_and_half_keeps_them() {
    let image = pattern(7, 5);
    let clockwise = rotate_cw(&image).unwrap();
    assert_eq!((clockwise.width(), clockwise.height()), (5, 7));
    let counter = rotate_ccw(&image).unwrap();
    assert_eq!((counter.width(), counter.height()), (5, 7));
    let half = rotate_180(&image).unwrap();
    assert_eq!((half.width(), half.height()), (7, 5));
}

#[test]
fn rotate_moves_pixels_the_way_the_labels_say() {
    // Marker one in from the top-left of a 5x3 image.
    let image = marked(5, 3, (1, 0));
    // Clockwise: (1, 0) -> (3 - 1 - 0, 1).
    let clockwise = rotate_cw(&image).unwrap();
    assert_eq!(clockwise.pixel(2, 1).unwrap(), [255, 0, 255, 255]);
    // Counter-clockwise: (1, 0) -> (0, 5 - 1 - 1).
    let counter = rotate_ccw(&image).unwrap();
    assert_eq!(counter.pixel(0, 3).unwrap(), [255, 0, 255, 255]);
    // Half: (1, 0) -> (5 - 1 - 1, 3 - 1 - 0).
    let half = rotate_180(&image).unwrap();
    assert_eq!(half.pixel(3, 2).unwrap(), [255, 0, 255, 255]);
}

// -- Crop -------------------------------------------------------------------

#[test]
fn square_crop_centres_the_largest_square() {
    // 5x3 leaves one column on each side, none vertically.
    let wide = pattern(5, 3);
    let square = crop_square(&wide).unwrap();
    assert_eq!((square.width(), square.height()), (3, 3));
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(square.pixel(x, y), wide.pixel(x + 1, y));
        }
    }

    // 3x7 leaves two rows above and below.
    let tall = pattern(3, 7);
    let square = crop_square(&tall).unwrap();
    assert_eq!((square.width(), square.height()), (3, 3));
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(square.pixel(x, y), tall.pixel(x, y + 2));
        }
    }
}

#[test]
fn free_crop_extracts_the_requested_rectangle() {
    let image = pattern(8, 6);
    let cut = crop(&image, CropRect::new(2, 1, 3, 4)).unwrap();
    assert_eq!((cut.width(), cut.height()), (3, 4));
    for y in 0..4 {
        for x in 0..3 {
            assert_eq!(cut.pixel(x, y), image.pixel(x + 2, y + 1));
        }
    }
}

#[test]
fn crop_clamps_a_rectangle_that_pokes_over_the_edge() {
    let image = pattern(8, 6);
    let cut = crop(&image, CropRect::new(6, 4, 10, 10)).unwrap();
    assert_eq!((cut.width(), cut.height()), (2, 2));
    for y in 0..2 {
        for x in 0..2 {
            assert_eq!(cut.pixel(x, y), image.pixel(x + 6, y + 4));
        }
    }
}

#[test]
fn empty_crop_is_refused() {
    let image = pattern(8, 6);
    // Entirely to the right of the image: the clamped selection is zero wide.
    let outside = crop(&image, CropRect::new(8, 0, 4, 4)).unwrap_err();
    assert_eq!(
        outside,
        PropError::ImplausibleDimensions {
            width: 0,
            height: 4
        }
    );
    assert_eq!(outside.to_string(), "implausible prop dimensions 0x4");
    // Zero-sized selection inside the image.
    assert!(crop(&image, CropRect::new(1, 1, 0, 5)).is_err());
    assert!(crop(&image, CropRect::new(1, 1, 5, 0)).is_err());
}

// -- Resize -----------------------------------------------------------------

#[test]
fn resize_stretches_to_the_exact_request() {
    let image = pattern(3, 2);
    let stretched = resize(&image, 7, 5, Resample::Nearest).unwrap();
    assert_eq!((stretched.width(), stretched.height()), (7, 5));
    assert_eq!(stretched.pixel(0, 0), image.pixel(0, 0));
}

#[test]
fn constrained_resize_preserves_aspect() {
    let image = pattern(44, 22);

    // Fit inside a box: the longer side reaches the box.
    let fitted = resize_to_fit(&image, 88, 88, Resample::Bilinear).unwrap();
    assert_eq!((fitted.width(), fitted.height()), (88, 44));

    // Exact width, derived height.
    let by_width = resize_to_width(&image, 88, Resample::Nearest).unwrap();
    assert_eq!((by_width.width(), by_width.height()), (88, 44));

    // Exact height, derived width.
    let by_height = resize_to_height(&image, 11, Resample::Nearest).unwrap();
    assert_eq!((by_height.width(), by_height.height()), (22, 11));

    // Aspect ratio of the source is kept (2:1), unlike a free stretch.
    let stretched = resize(&image, 88, 88, Resample::Nearest).unwrap();
    assert_eq!((stretched.width(), stretched.height()), (88, 88));
}

#[test]
fn preset_sizes_are_usable_and_within_the_crate_limits() {
    let image = pattern(44, 44);
    for &(width, height) in &SIZE_PRESETS {
        assert!(width <= palace_prop::MAX_DIMENSION as u32);
        assert!(u64::from(width) * u64::from(height) <= palace_prop::MAX_PIXELS);
        let fitted = resize_to_fit(&image, width, height, Resample::Bilinear).unwrap();
        assert_eq!((fitted.width(), fitted.height()), (width, height));
    }
}

#[test]
fn over_limit_resize_is_refused_with_a_clear_error() {
    let image = pattern(44, 44);

    let too_wide = resize(&image, 5000, 1, Resample::Nearest).unwrap_err();
    assert_eq!(
        too_wide,
        PropError::ImplausibleDimensions {
            width: 5000,
            height: 1
        }
    );
    assert_eq!(too_wide.to_string(), "implausible prop dimensions 5000x1");

    // A dimension above `i16` still names a plausible number, never a wrapped one.
    let enormous = resize(&image, u32::MAX, u32::MAX, Resample::Bilinear).unwrap_err();
    assert!(matches!(enormous, PropError::ImplausibleDimensions { .. }));
    assert!(enormous.to_string().contains("32767x32767"));

    // The largest dimension the crate allows is accepted.
    let edge = resize(&image, 4096, 1, Resample::Nearest).unwrap();
    assert_eq!((edge.width(), edge.height()), (4096, 1));

    // Zero-sized targets are refused, not clamped to 1x1 silently.
    assert!(resize(&image, 0, 4, Resample::Nearest).is_err());
    assert!(resize(&image, 4, 0, Resample::Nearest).is_err());
}

// -- Resampling and alpha ---------------------------------------------------

#[test]
fn nearest_copies_pixels_and_bilinear_smooths() {
    let image = pattern(2, 2);
    let nearest = resize(&image, 4, 4, Resample::Nearest).unwrap();
    let smooth = resize(&image, 4, 4, Resample::Bilinear).unwrap();

    assert_ne!(nearest, smooth, "the two filters must differ on an upscale");
    // Nearest repeats whole source pixels in blocks.
    assert_eq!(nearest.pixel(0, 0), image.pixel(0, 0));
    assert_eq!(nearest.pixel(1, 1), image.pixel(0, 0));
    assert_eq!(nearest.pixel(3, 3), image.pixel(1, 1));
}

#[test]
fn bilinear_upscale_keeps_alpha_and_avoids_dark_fringes() {
    // Opaque white next to fully transparent black.
    let source = PropImage::from_rgba(2, 1, vec![255, 255, 255, 255, 0, 0, 0, 0]).unwrap();
    let upscaled = resize(&source, 4, 1, Resample::Bilinear).unwrap();

    assert_eq!(upscaled.pixel(0, 0).unwrap()[3], 255);
    assert_eq!(upscaled.pixel(3, 0).unwrap()[3], 0);
    let middle = upscaled.pixel(1, 0).unwrap();
    let nearer_edge = upscaled.pixel(2, 0).unwrap();
    assert!(middle[3] < 255 && middle[3] > nearer_edge[3]);
    assert!(nearer_edge[3] > 0);
    // Where alpha survives, premultiplied blending keeps the colour white.
    assert_eq!([middle[0], middle[1], middle[2]], [255, 255, 255]);
    assert_eq!(
        [nearer_edge[0], nearer_edge[1], nearer_edge[2]],
        [255, 255, 255]
    );
}

#[test]
fn uniform_alpha_survives_every_operation_exactly() {
    let source = PropImage::from_rgba(
        2,
        2,
        vec![
            10, 20, 30, 77, 40, 50, 60, 77, 70, 80, 90, 77, 100, 110, 120, 77,
        ],
    )
    .unwrap();

    let nearest = resize(&source, 5, 5, Resample::Nearest).unwrap();
    assert!(nearest.as_rgba().chunks_exact(4).all(|p| p[3] == 77));
    let smooth = resize(&source, 3, 3, Resample::Bilinear).unwrap();
    assert!(smooth.as_rgba().chunks_exact(4).all(|p| p[3] == 77));
    let flipped = flip_horizontal(&source).unwrap();
    assert!(flipped.as_rgba().chunks_exact(4).all(|p| p[3] == 77));
    let rotated = rotate_cw(&source).unwrap();
    assert!(rotated.as_rgba().chunks_exact(4).all(|p| p[3] == 77));
    let cut = crop(&source, CropRect::new(0, 0, 1, 1)).unwrap();
    assert!(cut.as_rgba().chunks_exact(4).all(|p| p[3] == 77));
}

#[test]
fn alpha_follows_its_pixel_through_flips_rotations_and_crops() {
    let image = pattern(6, 4);
    let horizontal = flip_horizontal(&image).unwrap();
    let vertical = flip_vertical(&image).unwrap();
    let clockwise = rotate_cw(&image).unwrap();
    let counter = rotate_ccw(&image).unwrap();
    let half = rotate_180(&image).unwrap();
    let cut = crop(&image, CropRect::new(1, 1, 3, 2)).unwrap();

    for y in 0..4 {
        for x in 0..6 {
            let alpha = image.pixel(x, y).unwrap()[3];
            assert_eq!(horizontal.pixel(5 - x, y).unwrap()[3], alpha);
            assert_eq!(vertical.pixel(x, 3 - y).unwrap()[3], alpha);
            assert_eq!(half.pixel(5 - x, 3 - y).unwrap()[3], alpha);
            // Clockwise: source (x, y) lands on destination (3 - y, x).
            assert_eq!(clockwise.pixel(3 - y, x).unwrap()[3], alpha);
            // Counter-clockwise: source (x, y) lands on destination (y, 5 - x).
            assert_eq!(counter.pixel(y, 5 - x).unwrap()[3], alpha);
        }
    }
    for y in 0..2 {
        for x in 0..3 {
            assert_eq!(
                cut.pixel(x, y).unwrap()[3],
                image.pixel(x + 1, y + 1).unwrap()[3]
            );
        }
    }
}

// -- Shape crop --------------------------------------------------------------

#[test]
fn shape_crop_with_a_square_mask_matches_square_crop() {
    let image = pattern(7, 5);
    let shaped = shape_crop(&image, &ShapeCropOptions::default()).unwrap();
    let squared = crop_square(&image).unwrap();
    assert_eq!(shaped, squared);
}

#[test]
fn shape_crop_circle_masks_the_corners_only() {
    let image = pattern(8, 8);
    let options = ShapeCropOptions {
        shape: CropShape::Circle,
        ..plain_options()
    };
    let cropped = shape_crop(&image, &options).unwrap();
    assert_eq!((cropped.width(), cropped.height()), (8, 8));
    assert_eq!(cropped.pixel(0, 0).unwrap(), [0, 0, 0, 0]);
    assert_eq!(cropped.pixel(7, 0).unwrap(), [0, 0, 0, 0]);
    assert_eq!(cropped.pixel(0, 7).unwrap(), [0, 0, 0, 0]);
    assert_eq!(cropped.pixel(7, 7).unwrap(), [0, 0, 0, 0]);
    assert_eq!(cropped.pixel(4, 4), image.pixel(4, 4));
    assert_eq!(cropped.pixel(3, 3), image.pixel(3, 3));
    // The mask clears everything outside the shape, not just alpha.
    assert_eq!(cropped.pixel(0, 0).unwrap(), [0, 0, 0, 0]);
}

#[test]
fn shape_crop_rounded_masks_only_the_corners() {
    let image = pattern(8, 8);
    let options = ShapeCropOptions {
        shape: CropShape::Rounded { radius: 2 },
        ..plain_options()
    };
    let cropped = shape_crop(&image, &options).unwrap();
    // The extreme corner is outside the rounded corner...
    assert_eq!(cropped.pixel(0, 0).unwrap(), [0, 0, 0, 0]);
    assert_eq!(cropped.pixel(7, 7).unwrap(), [0, 0, 0, 0]);
    // ...but the straight edges and the centre are kept.
    assert_eq!(cropped.pixel(0, 3), image.pixel(0, 3));
    assert_eq!(cropped.pixel(3, 0), image.pixel(3, 0));
    assert_eq!(cropped.pixel(4, 4), image.pixel(4, 4));
}

#[test]
fn shape_crop_zoom_magnifies_about_the_centre() {
    let image = pattern(4, 4);
    let options = ShapeCropOptions {
        zoom: 2.0,
        ..plain_options()
    };
    let zoomed = shape_crop(&image, &options).unwrap();
    assert_eq!((zoomed.width(), zoomed.height()), (4, 4));
    // The centre maps to itself; the top-left corner now shows a source pixel
    // closer to the centre.
    assert_eq!(zoomed.pixel(2, 2), image.pixel(2, 2));
    assert_eq!(zoomed.pixel(0, 0), image.pixel(1, 1));
}

#[test]
fn shape_crop_rotation_is_inverse_mapped() {
    // Marker at source (2, 0), the top centre of a 4x4 image.
    let image = marked(4, 4, (2, 0));
    let options = ShapeCropOptions {
        rotation: 90.0,
        ..plain_options()
    };
    let rotated = shape_crop(&image, &options).unwrap();
    // A 90-degree clockwise turn moves the top centre to the right centre.
    assert_eq!(rotated.pixel(3, 2).unwrap(), [255, 0, 255, 255]);
    assert_ne!(rotated.pixel(2, 0).unwrap(), [255, 0, 255, 255]);
}

#[test]
fn shape_crop_sanitises_zoom_and_rotation() {
    let image = pattern(4, 4);

    // A missing zoom (zero, negative, NaN, infinite) still produces the
    // natural-size square instead of an error or an empty image.
    let identity = crop_square(&image).unwrap();
    for zoom in [f32::NAN, f32::INFINITY] {
        let options = ShapeCropOptions {
            zoom,
            ..plain_options()
        };
        assert_eq!(shape_crop(&image, &options).unwrap(), identity);
    }
    for zoom in [0.0, -3.0, MAX_ZOOM_REQUEST] {
        let options = ShapeCropOptions {
            zoom,
            ..plain_options()
        };
        let cropped = shape_crop(&image, &options).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (4, 4));
    }

    // Rotation is normalised: -90 is the same turn as 270.
    let negative = shape_crop(
        &image,
        &ShapeCropOptions {
            rotation: -90.0,
            ..plain_options()
        },
    )
    .unwrap();
    let full = shape_crop(
        &image,
        &ShapeCropOptions {
            rotation: 270.0,
            ..plain_options()
        },
    )
    .unwrap();
    assert_eq!(negative, full);
}

// -- Frame helpers -----------------------------------------------------------

#[test]
fn transform_frame_rebuilds_the_canvas_and_keeps_the_layer_stack() {
    let mut frame = Frame::from_base(pattern(4, 2));
    let overlay = Layer::new(LayerId::new(77), "Top", pattern(4, 2))
        .with_opacity(0.25)
        .with_visible(false);
    frame.push_layer(overlay);

    transform_frame(&mut frame, rotate_cw).unwrap();

    assert_eq!((frame.width(), frame.height()), (2, 4));
    assert_eq!(frame.layer_count(), 2);
    let base = frame.layer(0).unwrap();
    assert_eq!((base.image().width(), base.image().height()), (2, 4));
    assert_eq!(*base.image(), rotate_cw(&pattern(4, 2)).unwrap());
    let top = frame.layer(1).unwrap();
    assert_eq!(top.id, LayerId::new(77));
    assert_eq!(top.name, "Top");
    assert!((top.opacity - 0.25).abs() < f32::EPSILON);
    assert!(!top.visible);
    assert_eq!((top.image().width(), top.image().height()), (2, 4));
}

#[test]
fn transform_frame_leaves_the_frame_untouched_when_the_operation_fails() {
    let mut frame = Frame::from_base(pattern(4, 2));
    let before = frame.clone();
    let failure = transform_frame(&mut frame, |_| {
        Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        })
    });
    assert!(failure.is_err());
    assert_eq!(frame, before);
}

#[test]
fn rotate_frames_rotates_every_frame() {
    let mut frames = vec![
        Frame::from_base(pattern(4, 2)),
        Frame::from_base(pattern(6, 3)),
    ];
    let changed = rotate_frames(&mut frames, QuarterTurn::Clockwise).unwrap();
    assert_eq!(changed, 2);
    assert_eq!((frames[0].width(), frames[0].height()), (2, 4));
    assert_eq!((frames[1].width(), frames[1].height()), (3, 6));
}

#[test]
fn shape_crop_frames_applies_to_every_frame_when_asked() {
    let mut frames = vec![
        Frame::from_base(pattern(8, 8)),
        Frame::from_base(pattern(8, 8)),
    ];
    let options = ShapeCropOptions {
        shape: CropShape::Circle,
        ..plain_options()
    };
    let changed = shape_crop_frames(&mut frames, 0, &options, true).unwrap();
    assert_eq!(changed, 2);
    for frame in &frames {
        assert_eq!((frame.width(), frame.height()), (8, 8));
        let base = frame.base().unwrap().image();
        assert_eq!(base.pixel(0, 0).unwrap(), [0, 0, 0, 0]);
        assert_ne!(base.pixel(4, 4).unwrap()[3], 0);
    }
}

#[test]
fn shape_crop_frames_touches_only_the_selected_frame_when_not_asked() {
    let untouched = Frame::from_base(pattern(8, 8));
    let mut frames = vec![Frame::from_base(pattern(8, 8)), untouched.clone()];
    let options = ShapeCropOptions {
        shape: CropShape::Circle,
        ..plain_options()
    };

    let changed = shape_crop_frames(&mut frames, 0, &options, false).unwrap();
    assert_eq!(changed, 1);
    assert_eq!(frames[1], untouched, "the unselected frame must not change");
    assert_eq!(
        frames[0].base().unwrap().image().pixel(0, 0).unwrap(),
        [0, 0, 0, 0]
    );

    // An out-of-range selection changes nothing rather than panicking.
    let changed = shape_crop_frames(&mut frames, 9, &options, false).unwrap();
    assert_eq!(changed, 0);
    assert_eq!(frames[1], untouched);
}

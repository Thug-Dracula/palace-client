//! Acceptance tests for the editor's adjustment tools (plan task 25).
//!
//! These exercise the module through its public surface only: no filesystem, no
//! network, no webview. The required behaviours are exact reset, deterministic
//! saturation, solid-background chroma keying, tolerance/feather behaviour, and
//! a no-op adjustment being the identity. Alpha preservation and the
//! "apply to all frames" variants are covered here too.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use palace_prop::editor::tools::adjust::{
    apply_brightness_contrast, apply_brightness_contrast_all_frames, apply_colour_adjust,
    apply_colour_adjust_all_frames, remove_background, remove_background_all_frames,
    sample_key_colour, BrightnessContrast, ChromaKey, ColourAdjust,
};
use palace_prop::editor::Frame;
use palace_prop::image::PropImage;

/// A stable-for-this-run digest of an image's dimensions and pixels.
fn digest(image: &PropImage) -> u64 {
    let mut hasher = DefaultHasher::new();
    image.width().hash(&mut hasher);
    image.height().hash(&mut hasher);
    image.as_rgba().hash(&mut hasher);
    hasher.finish()
}

/// A one-row image from explicit pixels.
fn one_row(pixels: &[[u8; 4]]) -> PropImage {
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        bytes.extend_from_slice(pixel);
    }
    PropImage::from_rgba(pixels.len() as u32, 1, bytes).expect("a row matches its buffer")
}

/// An image every pixel of which is the same colour.
fn solid(width: u32, height: u32, rgba: [u8; 4]) -> PropImage {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        bytes.extend_from_slice(&rgba);
    }
    PropImage::from_rgba(width, height, bytes).expect("a solid image matches its buffer")
}

/// A deterministic, colourful, half-transparent image.
fn gradient(width: u32, height: u32) -> PropImage {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for i in 0..(width * height) as usize {
        bytes.extend_from_slice(&[
            ((i * 13 + 7) % 256) as u8,
            ((i * 29 + 3) % 256) as u8,
            ((i * 47 + 11) % 256) as u8,
            200,
        ]);
    }
    PropImage::from_rgba(width, height, bytes).expect("a gradient matches its buffer")
}

// (a) brightness then reset returns the original bytes exactly.
#[test]
fn brightness_then_reset_returns_the_original_bytes() {
    let original = gradient(8, 8);
    let before = digest(&original);
    let kept = original.clone();

    let edited = apply_brightness_contrast(
        &original,
        BrightnessContrast {
            brightness: 0.6,
            contrast: 0.25,
        },
    )
    .expect("same dimensions");
    assert_ne!(digest(&edited), before, "the edit must change pixels");
    assert_eq!(
        digest(&original),
        before,
        "the source image must not be mutated"
    );

    // Reset: the UI re-applies from the kept pre-edit pixels with neutral
    // parameters, and that must return the original byte for byte.
    let reset =
        apply_brightness_contrast(&kept, BrightnessContrast::IDENTITY).expect("same dimensions");
    assert_eq!(digest(&reset), before);
    assert_eq!(reset.as_rgba(), kept.as_rgba());
    assert_eq!(reset.as_rgba(), original.as_rgba());
}

// (b) saturation changes pixels, greys the image at -1, and is deterministic.
#[test]
fn saturation_changes_pixels_and_is_deterministic() {
    let original = gradient(6, 4);

    let grey = apply_colour_adjust(
        &original,
        ColourAdjust {
            saturation: -1.0,
            ..ColourAdjust::default()
        },
    )
    .expect("same dimensions");
    assert_ne!(
        digest(&grey),
        digest(&original),
        "saturation -1 must change the pixels"
    );
    for pixel in grey.as_rgba().chunks_exact(4) {
        assert_eq!(pixel[0], pixel[1], "fully desaturated pixels are grey");
        assert_eq!(pixel[1], pixel[2], "fully desaturated pixels are grey");
    }

    let lifted = apply_colour_adjust(
        &original,
        ColourAdjust {
            saturation: 0.5,
            ..ColourAdjust::default()
        },
    )
    .expect("same dimensions");
    assert_ne!(lifted.as_rgba(), original.as_rgba());
    let again = apply_colour_adjust(
        &original,
        ColourAdjust {
            saturation: 0.5,
            ..ColourAdjust::default()
        },
    )
    .expect("same dimensions");
    assert_eq!(lifted.as_rgba(), again.as_rgba(), "deterministic");
    assert_eq!(digest(&lifted), digest(&again));
}

// (c) a solid-colour background becomes alpha 0; the foreground stays opaque.
#[test]
fn chroma_key_makes_a_solid_background_transparent() {
    // 6x2: left half solid green, right half solid red.
    let mut bytes = Vec::with_capacity(6 * 2 * 4);
    for _ in 0..2 {
        for x in 0..6 {
            if x < 3 {
                bytes.extend_from_slice(&[0, 255, 0, 255]);
            } else {
                bytes.extend_from_slice(&[255, 0, 0, 255]);
            }
        }
    }
    let image = PropImage::from_rgba(6, 2, bytes).expect("matches its buffer");
    let before = image.as_rgba().to_vec();

    let cut = remove_background(
        &image,
        ChromaKey {
            key: [0, 255, 0],
            tolerance: 0.1,
            feather: 0.0,
        },
    )
    .expect("same dimensions");

    for y in 0..2 {
        for x in 0..6 {
            let pixel = cut.pixel(x, y).expect("inside the image");
            if x < 3 {
                assert_eq!(pixel, [0, 255, 0, 0], "background pixel is cut out");
            } else {
                assert_eq!(
                    pixel,
                    [255, 0, 0, 255],
                    "foreground pixel is opaque and unchanged"
                );
            }
        }
    }
    assert_eq!(
        image.as_rgba(),
        before.as_slice(),
        "the source image must not be mutated"
    );
}

// (d) tolerance widens the cut-out and feather softens its edge.
#[test]
fn tolerance_and_feather_change_the_cut_out() {
    // Distances from the key [0, 255, 0], as fractions of 255:
    // exact green 0.0, near green 20/255, mid green 60/255, red 255/255.
    let image = one_row(&[
        [0, 255, 0, 255],
        [20, 235, 20, 255],
        [60, 195, 60, 255],
        [255, 0, 0, 255],
    ]);
    let key = [0, 255, 0];

    let narrow = remove_background(&image, ChromaKey::new(key).with_tolerance(0.05))
        .expect("same dimensions");
    let alphas: Vec<u8> = (0..4)
        .map(|x| narrow.pixel(x, 0).expect("inside the image")[3])
        .collect();
    assert_eq!(alphas, [0, 255, 255, 255], "only the exact key is cut");

    let wide = remove_background(&image, ChromaKey::new(key).with_tolerance(0.1))
        .expect("same dimensions");
    let alphas: Vec<u8> = (0..4)
        .map(|x| wide.pixel(x, 0).expect("inside the image")[3])
        .collect();
    assert_eq!(
        alphas,
        [0, 0, 255, 255],
        "a wider tolerance also cuts the near-green pixel"
    );

    let feathered = remove_background(
        &image,
        ChromaKey::new(key).with_tolerance(0.1).with_feather(0.4),
    )
    .expect("same dimensions");
    let alphas: Vec<u8> = (0..4)
        .map(|x| feathered.pixel(x, 0).expect("inside the image")[3])
        .collect();
    assert_eq!(alphas[0], 0, "inside the tolerance is fully transparent");
    assert_eq!(alphas[1], 0, "inside the tolerance is fully transparent");
    assert!(
        (1..255).contains(&alphas[2]),
        "a pixel in the feather band gets partial alpha, got {}",
        alphas[2]
    );
    assert_eq!(alphas[3], 255, "beyond the feather band is untouched");
    // The ramp is monotone along increasing distance.
    assert!(alphas[0] <= alphas[1] && alphas[1] <= alphas[2] && alphas[2] <= alphas[3]);

    // With no feather the same tolerance leaves that pixel opaque.
    assert_eq!(wide.pixel(2, 0).expect("inside the image")[3], 255);
    // RGB survives the cut exactly, background pixel included.
    assert_eq!(
        narrow.pixel(0, 0).expect("inside the image")[..3],
        [0, 255, 0]
    );
    assert_eq!(
        feathered.pixel(2, 0).expect("inside the image")[..3],
        [60, 195, 60]
    );
}

// (e) a neutral adjustment is the identity, byte for byte.
#[test]
fn a_no_op_adjustment_is_identity() {
    let image = gradient(5, 5);

    let unaffected =
        apply_brightness_contrast(&image, BrightnessContrast::default()).expect("same dimensions");
    assert_eq!(unaffected.as_rgba(), image.as_rgba());

    let unaffected = apply_colour_adjust(&image, ColourAdjust::default()).expect("same dimensions");
    assert_eq!(unaffected.as_rgba(), image.as_rgba());

    // A key that is not present, with no tolerance and no feather, changes
    // nothing either.
    let flat = solid(4, 4, [10, 20, 30, 255]);
    let unaffected = remove_background(&flat, ChromaKey::new([1, 2, 3])).expect("same dimensions");
    assert_eq!(unaffected.as_rgba(), flat.as_rgba());
}

// (f) colour operations preserve alpha and leave invisible RGB alone.
#[test]
fn adjustments_preserve_alpha_and_skip_transparent_colour() {
    let image = one_row(&[[10, 20, 30, 0], [40, 50, 60, 128]]);
    let before = image.as_rgba().to_vec();

    let edited = apply_brightness_contrast(
        &image,
        BrightnessContrast {
            brightness: 0.4,
            ..BrightnessContrast::default()
        },
    )
    .expect("same dimensions");

    assert_eq!(
        edited.pixel(0, 0).expect("inside the image"),
        [10, 20, 30, 0],
        "a fully transparent pixel keeps its stored colour"
    );
    let half = edited.pixel(1, 0).expect("inside the image");
    assert_eq!(half[3], 128, "alpha is copied through exactly");
    assert_ne!(half[..3], [40, 50, 60], "the visible pixel was adjusted");
    assert_eq!(image.as_rgba(), before.as_slice(), "input is not mutated");
}

// (g) the sample helper reads visible pixels only.
#[test]
fn sample_key_colour_reads_visible_pixels() {
    let image = one_row(&[[40, 50, 60, 255], [70, 80, 90, 0]]);
    assert_eq!(sample_key_colour(&image, 0, 0), Some([40, 50, 60]));
    assert_eq!(sample_key_colour(&image, 1, 0), None, "transparent pixel");
    assert_eq!(sample_key_colour(&image, 9, 0), None, "off the image");
}

// (h) the all-frames variants adjust every frame and return fresh frames.
#[test]
fn all_frames_variants_adjust_every_frame() {
    let frames = vec![
        Frame::from_base(solid(4, 4, [10, 20, 30, 255])),
        Frame::from_base(solid(4, 4, [200, 100, 50, 255])),
    ];

    let edited = apply_brightness_contrast_all_frames(
        &frames,
        BrightnessContrast {
            brightness: 0.5,
            ..BrightnessContrast::default()
        },
    )
    .expect("same dimensions");
    assert_eq!(edited.len(), 2, "frame count is preserved");
    assert_eq!(edited[0].layer_count(), 1, "layer structure is preserved");
    assert_ne!(
        edited[0].base().expect("base layer").image(),
        frames[0].base().expect("base layer").image()
    );
    assert_ne!(
        edited[1].base().expect("base layer").image(),
        frames[1].base().expect("base layer").image()
    );

    let same =
        apply_colour_adjust_all_frames(&frames, ColourAdjust::default()).expect("same dimensions");
    assert_eq!(same, frames, "a neutral adjustment returns equal frames");

    let cut =
        remove_background_all_frames(&frames, ChromaKey::new([10, 20, 30]).with_tolerance(0.05))
            .expect("same dimensions");
    assert_eq!(
        cut[0]
            .base()
            .expect("base layer")
            .image()
            .pixel(0, 0)
            .expect("pixel")[3],
        0,
        "the first frame's background is cut"
    );
    assert_eq!(
        cut[1]
            .base()
            .expect("base layer")
            .image()
            .pixel(0, 0)
            .expect("pixel")[3],
        255,
        "the second frame is untouched by the first frame's key"
    );
}

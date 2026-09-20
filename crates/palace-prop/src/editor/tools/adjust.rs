//! Image adjustments: brightness/contrast, colour, and chroma-key background
//! removal.
//!
//! These are the pixel operations behind the editor's "Brightness/Contrast",
//! "Color Adjust" and "Background Removal" panels. Every function here is pure:
//! it reads a [`PropImage`] (or the layers of some [`Frame`]s) and returns new
//! pixels. Nothing is written back into the input, no file is touched, and the
//! same input always produces the same output.
//!
//! # Reset
//!
//! The panels are bounded by OK/Cancel/Reset, so the editor keeps the pre-edit
//! pixels and drops the adjusted copy on Cancel or Reset. Because the functions
//! never mutate their input, "reset" is exactly that: re-apply nothing and the
//! original bytes are still there. Neutral parameters are also short-circuited to
//! a byte-identical clone, so even an accidental "apply" of a zeroed slider
//! cannot drift a single byte.
//!
//! # Parameters
//!
//! Adjustment strengths are unitless `f32` values in `-1.0..=1.0` — the panels'
//! `-100..=100` sliders divided by 100 — where `0.0` is the identity:
//!
//! * `brightness` adds `brightness * 255` to each RGB channel.
//! * `contrast` scales each channel about mid-grey with factor `1 + contrast`.
//! * `saturation` scales each channel's distance from perceptual luma (Rec. 601)
//!   with factor `1 + saturation`; `-1.0` is fully greyscale.
//!
//! Chroma-key `tolerance` and `feather` are `0.0..=1.0` fractions of the 0–255
//! channel range. A pixel matches the key when **every** channel is within
//! `tolerance * 255` of the sampled colour; matching pixels are cut to fully
//! transparent. `feather` is the extra band beyond that envelope over which alpha
//! ramps linearly from `0` back to the pixel's original value, so `0.0` is a hard
//! cut and larger values soften the edge. This is the reference panels'
//! "sampled colour + tolerance + feather" model (the `BakeChromaKey` / `HueDeg`
//! pair on the Background Removal dialog) expressed as absolute per-channel
//! distance; [`sample_key_colour`] is the "click a pixel" half.
//!
//! # Alpha correctness
//!
//! Pixels are straight (non-premultiplied) RGBA. Colour operations change RGB
//! only: alpha is copied through exactly. They also skip fully transparent
//! pixels, whose RGB is undefined — rewriting it would create coloured halos when
//! the prop is filtered later. Background removal changes alpha only: RGB is
//! copied through exactly and alpha can only stay the same or fall.
//!
//! # Frames
//!
//! The `*_all_frames` variants map the same operation over every layer of every
//! frame and return fresh frames. They are the panel's "apply to all frames"
//! action; they add no second implementation and no new behaviour.
//!
//! # Example
//!
//! ```
//! use palace_prop::editor::tools::adjust::{remove_background, ChromaKey};
//! use palace_prop::image::PropImage;
//!
//! // Two pixels: solid green, then solid red.
//! let rgba = vec![0, 255, 0, 255, 255, 0, 0, 255];
//! let image = PropImage::from_rgba(2, 1, rgba).unwrap();
//! let cut = remove_background(
//!     &image,
//!     ChromaKey { key: [0, 255, 0], tolerance: 0.1, feather: 0.0 },
//! )
//! .unwrap();
//! assert_eq!(cut.pixel(0, 0).unwrap()[3], 0, "the green background is gone");
//! assert_eq!(cut.pixel(1, 0).unwrap()[3], 255, "the red foreground stays");
//! ```

use crate::editor::Frame;
use crate::error::Result;
use crate::image::PropImage;

/// Perceptual luma weights (Rec. 601), used by saturation.
const LUMA: [f32; 3] = [0.299, 0.587, 0.114];

/// Brightness and contrast, both `-1.0..=1.0` with `0.0` the identity.
///
/// This is the "Brightness/Contrast" panel's pair of sliders. Applying
/// [`apply_brightness_contrast`] with this setting changes RGB only.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BrightnessContrast {
    /// Additive brightness offset, `-1.0..=1.0`.
    pub brightness: f32,
    /// Contrast strength, `-1.0..=1.0`; `0.0` leaves contrast alone.
    pub contrast: f32,
}

impl BrightnessContrast {
    /// The neutral setting. Applying it returns the pixels byte for byte.
    pub const IDENTITY: Self = BrightnessContrast {
        brightness: 0.0,
        contrast: 0.0,
    };

    /// Whether this setting leaves pixels untouched.
    ///
    /// Non-finite values count as zero, matching how the operations sanitise
    /// their parameters.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        signed(self.brightness) == 0.0 && signed(self.contrast) == 0.0
    }
}

/// Brightness, contrast and saturation, all `-1.0..=1.0` with `0.0` the
/// identity.
///
/// This is the "Color Adjust" panel's three sliders. The channels are processed
/// in that order: brightness, then contrast, then saturation. Applying
/// [`apply_colour_adjust`] with this setting changes RGB only.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColourAdjust {
    /// Additive brightness offset, `-1.0..=1.0`.
    pub brightness: f32,
    /// Contrast strength, `-1.0..=1.0`; `0.0` leaves contrast alone.
    pub contrast: f32,
    /// Saturation strength, `-1.0..=1.0`; `-1.0` is greyscale.
    pub saturation: f32,
}

impl ColourAdjust {
    /// The neutral setting. Applying it returns the pixels byte for byte.
    pub const IDENTITY: Self = ColourAdjust {
        brightness: 0.0,
        contrast: 0.0,
        saturation: 0.0,
    };

    /// Whether this setting leaves pixels untouched.
    ///
    /// Non-finite values count as zero, matching how the operations sanitise
    /// their parameters.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        signed(self.brightness) == 0.0
            && signed(self.contrast) == 0.0
            && signed(self.saturation) == 0.0
    }
}

/// Chroma-key settings for [`remove_background`].
///
/// `key` is the sampled background colour (usually from
/// [`sample_key_colour`]). `tolerance` is the per-channel envelope around it
/// that is cut to transparent, and `feather` is the width of the alpha ramp
/// beyond that envelope. Both are `0.0..=1.0` and are clamped on use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromaKey {
    /// The background colour to remove, as `[r, g, b]`.
    pub key: [u8; 3],
    /// How far each channel may differ from `key` and still be removed,
    /// `0.0..=1.0` of the 0–255 range.
    pub tolerance: f32,
    /// Width of the soft alpha edge beyond `tolerance`, `0.0..=1.0`.
    /// `0.0` is a hard cut.
    pub feather: f32,
}

impl ChromaKey {
    /// A hard-cut key with the given colour.
    #[must_use]
    pub fn new(key: [u8; 3]) -> Self {
        ChromaKey {
            key,
            tolerance: 0.0,
            feather: 0.0,
        }
    }

    /// Set the tolerance, builder-style.
    #[must_use]
    pub fn with_tolerance(mut self, tolerance: f32) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Set the feather width, builder-style.
    #[must_use]
    pub fn with_feather(mut self, feather: f32) -> Self {
        self.feather = feather;
        self
    }
}

/// Apply brightness and contrast to an image, returning new pixels.
///
/// The input is not touched. Alpha is copied through exactly; fully transparent
/// pixels keep their stored RGB. Neutral parameters (or non-finite ones, which
/// count as zero) return a byte-identical clone.
pub fn apply_brightness_contrast(
    image: &PropImage,
    adjust: BrightnessContrast,
) -> Result<PropImage> {
    if adjust.is_identity() {
        return Ok(image.clone());
    }
    let brightness = signed(adjust.brightness);
    let contrast = 1.0 + signed(adjust.contrast);
    map_pixels(image, |pixel| {
        if pixel[3] == 0 {
            return;
        }
        for channel in pixel.iter_mut().take(3) {
            *channel = quantize(tone(*channel, brightness, contrast));
        }
    })
}

/// Apply brightness, contrast and saturation to an image, returning new pixels.
///
/// Saturation is applied last, as a scale about perceptual luma: `-1.0` collapses
/// every pixel to grey, `0.0` leaves colour alone, positive values push colour
/// away from grey. Alpha is copied through exactly; a neutral setting returns a
/// byte-identical clone.
pub fn apply_colour_adjust(image: &PropImage, adjust: ColourAdjust) -> Result<PropImage> {
    if adjust.is_identity() {
        return Ok(image.clone());
    }
    let brightness = signed(adjust.brightness);
    let contrast = 1.0 + signed(adjust.contrast);
    let saturation = 1.0 + signed(adjust.saturation);
    map_pixels(image, |pixel| {
        if pixel[3] == 0 {
            return;
        }
        let red = tone(pixel[0], brightness, contrast);
        let green = tone(pixel[1], brightness, contrast);
        let blue = tone(pixel[2], brightness, contrast);
        let luma = LUMA[0] * red + LUMA[1] * green + LUMA[2] * blue;
        pixel[0] = quantize(luma + (red - luma) * saturation);
        pixel[1] = quantize(luma + (green - luma) * saturation);
        pixel[2] = quantize(luma + (blue - luma) * saturation);
    })
}

/// Remove a chroma-key background from an image, returning new pixels.
///
/// Every pixel whose three channels each sit within `tolerance` of the key
/// colour becomes fully transparent. Pixels between `tolerance` and
/// `tolerance + feather` of the key get a partial alpha on a linear ramp; pixels
/// beyond the ramp keep their original alpha. RGB is copied through exactly, so
/// the cut-out itself cannot recolour the sprite.
pub fn remove_background(image: &PropImage, key: ChromaKey) -> Result<PropImage> {
    let tolerance = fraction(key.tolerance);
    let feather = fraction(key.feather);
    map_pixels(image, |pixel| {
        let distance = key_distance(pixel, &key.key);
        pixel[3] = keyed_alpha(pixel[3], distance, tolerance, feather);
    })
}

/// Sample a pixel's colour for use as a chroma key.
///
/// Returns `None` when `(x, y)` is off the image or the pixel is fully
/// transparent: an invisible pixel has no meaningful colour to remove.
#[must_use]
pub fn sample_key_colour(image: &PropImage, x: u32, y: u32) -> Option<[u8; 3]> {
    let pixel = image.pixel(x, y)?;
    if pixel[3] == 0 {
        return None;
    }
    Some([pixel[0], pixel[1], pixel[2]])
}

/// Apply brightness and contrast to every layer of every frame.
///
/// Returns fresh frames; the input slice is not touched.
pub fn apply_brightness_contrast_all_frames(
    frames: &[Frame],
    adjust: BrightnessContrast,
) -> Result<Vec<Frame>> {
    map_frames(frames, |image| apply_brightness_contrast(image, adjust))
}

/// Apply brightness, contrast and saturation to every layer of every frame.
///
/// Returns fresh frames; the input slice is not touched.
pub fn apply_colour_adjust_all_frames(
    frames: &[Frame],
    adjust: ColourAdjust,
) -> Result<Vec<Frame>> {
    map_frames(frames, |image| apply_colour_adjust(image, adjust))
}

/// Remove a chroma-key background from every layer of every frame.
///
/// Returns fresh frames; the input slice is not touched.
pub fn remove_background_all_frames(frames: &[Frame], key: ChromaKey) -> Result<Vec<Frame>> {
    map_frames(frames, |image| remove_background(image, key))
}

/// Rebuild an image from edited pixels. Dimensions cannot change, so the only
/// error is the invariant guard inside [`PropImage::from_rgba`].
fn map_pixels(image: &PropImage, mut edit: impl FnMut(&mut [u8])) -> Result<PropImage> {
    let mut bytes = image.as_rgba().to_vec();
    for pixel in bytes.chunks_exact_mut(4) {
        edit(pixel);
    }
    PropImage::from_rgba(image.width(), image.height(), bytes)
}

/// Apply an image operation to every layer of every frame.
fn map_frames(
    frames: &[Frame],
    mut operation: impl FnMut(&PropImage) -> Result<PropImage>,
) -> Result<Vec<Frame>> {
    let mut adjusted = Vec::with_capacity(frames.len());
    for frame in frames {
        let mut copy = frame.clone();
        for layer in copy.layers_mut() {
            layer.image = operation(&layer.image)?;
        }
        adjusted.push(copy);
    }
    Ok(adjusted)
}

/// Normalised tone of one channel after brightness and contrast.
fn tone(channel: u8, brightness: f32, contrast: f32) -> f32 {
    let value = f32::from(channel) / 255.0;
    (value + brightness - 0.5) * contrast + 0.5
}

/// A normalised `0.0..=1.0` value back to a byte.
fn quantize(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The chroma-key distance of a pixel: the largest per-channel difference from
/// the key, normalised to `0.0..=1.0`.
fn key_distance(pixel: &[u8], key: &[u8; 3]) -> f32 {
    let mut distance = 0.0_f32;
    for (channel, key_channel) in pixel.iter().take(3).zip(key) {
        let difference = (f32::from(*channel) - f32::from(*key_channel)).abs() / 255.0;
        if difference > distance {
            distance = difference;
        }
    }
    distance
}

/// Alpha for a pixel at `distance` from the key.
fn keyed_alpha(alpha: u8, distance: f32, tolerance: f32, feather: f32) -> u8 {
    if distance <= tolerance {
        return 0;
    }
    if feather > 0.0 && distance < tolerance + feather {
        let ramp = (distance - tolerance) / feather;
        return quantize(f32::from(alpha) / 255.0 * ramp);
    }
    alpha
}

/// Sanitise a signed adjustment parameter: finite values clamp to
/// `-1.0..=1.0`, and non-finite values become the identity.
fn signed(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Sanitise a `0.0..=1.0` parameter the same way.
fn fraction(value: f32) -> f32 {
    signed(value).max(0.0)
}

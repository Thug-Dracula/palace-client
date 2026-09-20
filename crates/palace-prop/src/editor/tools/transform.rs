//! Transform tools: flip, rotate, crop, square crop, resize and shape crop.
//!
//! Implemented by plan task T24. Every operation here is a pure function from
//! one [`PropImage`] to another: it touches no document, layer, history or
//! clipboard state, so a caller can preview a transform before deciding to
//! keep it. The frame helpers at the bottom ([`transform_frame`],
//! [`rotate_frames`], [`shape_crop_frame`], [`shape_crop_frames`]) extend the
//! same operations over a frame's layer stack — that is what the panel's
//! "apply to all frames" checkbox needs, without re-implementing pixel work in
//! the UI.
//!
//! The surface mirrors PalaceChat's transform menu: flip horizontal/vertical,
//! rotate clockwise/counter-clockwise/180, free crop, square crop, shape crop
//! (square/circle/rounded, with zoom and rotation) and image resize with its
//! size presets.
//!
//! # Limits
//!
//! Every function checks the size it is about to emit against
//! [`MAX_DIMENSION`] and [`MAX_PIXELS`] before it builds any pixels, and
//! refuses with [`PropError::ImplausibleDimensions`] or
//! [`PropError::ImageTooLarge`] rather than returning a prop size the crate
//! would reject. A crop rectangle is clamped to the image first, and a
//! rectangle that overlaps nothing is refused as a zero-size result. An
//! over-limit resize is refused, never silently shrunk or grown.
//!
//! # Alpha
//!
//! Every operation preserves the alpha channel:
//!
//! * Flips, 90-degree/180-degree rotations and crops are exact byte
//!   permutations: no pixel is re-sampled, so alpha survives one-to-one.
//! * [`Resample::Nearest`] copies whole pixels, alpha included, so a uniform
//!   alpha stays exactly that value.
//! * [`Resample::Bilinear`] interpolates in **premultiplied** alpha and
//!   unpremultiplies the result. Interpolating straight alpha drags the colour
//!   of transparent pixels into their opaque neighbours (the classic dark
//!   fringe); premultiplying removes it. A fully transparent result is written
//!   as `[0, 0, 0, 0]`.
//!
//! # Rotation interpolation
//!
//! Right-angle rotation is exact and lossless: it is a permutation of the
//! pixels, so four 90-degree turns return the original bytes and dimensions
//! exactly. Arbitrary-angle rotation is offered only by [`shape_crop`], and is
//! sampled by inverse mapping: each output pixel's centre is rotated back into
//! the source by `-rotation` and sampled with the chosen filter. Bilinear taps
//! clamp to the source edge when [`resize`] scales (no dark fringe on an
//! enlarged edge) and read transparent outside the source in [`shape_crop`]
//! (so a rotated image does not smear its border pixels outward).
//!
//! # Which resampling filter is the default
//!
//! [`Resample::Bilinear`] is the default, because the reference editors
//! resample smoothly: PalaceChat's image-size dialog resizes through Hermite
//! resampling and its new-prop import through bicubic. Bilinear is the cheapest
//! smooth filter that needs no new dependency. [`Resample::Nearest`] is opt-in
//! for pixel-art props, where hard edges are worth more than smoothness.
//!
//! # Example
//!
//! ```
//! use palace_prop::editor::tools::transform::{flip_horizontal, resize, Resample};
//! use palace_prop::PropImage;
//!
//! let image = PropImage::transparent(4, 2);
//! let flipped = flip_horizontal(&image)?;
//! assert_eq!((flipped.width(), flipped.height()), (4, 2));
//! let scaled = resize(&flipped, 8, 4, Resample::Nearest)?;
//! assert_eq!((scaled.width(), scaled.height()), (8, 4));
//! # Ok::<(), palace_prop::PropError>(())
//! ```

use crate::editor::{Frame, Layer};
use crate::error::{PropError, Result, MAX_DIMENSION, MAX_PIXELS};
use crate::image::PropImage;

/// Smallest shape-crop zoom we apply; a smaller request would shrink the image
/// to a sub-pixel speck, so it is clamped.
const MIN_ZOOM: f32 = 0.01;

/// Largest shape-crop zoom we apply; larger requests are clamped so the source
/// coordinates stay in `f32` range.
const MAX_ZOOM: f32 = 100.0;

/// The sizes the image-resize panel offers, in pixels.
///
/// `44x44` is the Palace avatar cell; the larger entries are its usual
/// multiple and the common power-of-two sprite sizes. A preset is a target
/// box: [`resize_to_fit`] keeps the aspect ratio inside it when the panel's
/// "constrain aspect" box is checked.
pub const SIZE_PRESETS: [(u32, u32); 8] = [
    (16, 16),
    (32, 32),
    (44, 44),
    (64, 64),
    (88, 88),
    (128, 128),
    (176, 176),
    (256, 256),
];

/// How a transform guesses colours between source pixels.
///
/// Used by scaling and by arbitrary-angle shape-crop rotation. The default is
/// [`Resample::Bilinear`]; see the module docs for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resample {
    /// Copy the nearest source pixel. Exact colours and alpha; hard edges.
    Nearest,
    /// Average the four surrounding source pixels, weighted by distance, in
    /// premultiplied alpha. Smooth; the default.
    #[default]
    Bilinear,
}

/// Which exact quarter turn [`rotate`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarterTurn {
    /// 90 degrees clockwise.
    Clockwise,
    /// 90 degrees counter-clockwise.
    CounterClockwise,
    /// 180 degrees; the same as two clockwise quarter turns.
    Half,
}

// -- Flip -------------------------------------------------------------------

/// Mirror the image left-to-right.
///
/// Exact: pixels are moved, never sampled.
pub fn flip_horizontal(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    check_dims(width, height)?;
    let src = image.as_rgba();
    let row = width as usize * 4;
    let mut out = vec![0u8; src.len()];
    for y in 0..height as usize {
        let src_row = &src[y * row..(y + 1) * row];
        let dst_row = &mut out[y * row..(y + 1) * row];
        for (x, pixel) in dst_row.chunks_exact_mut(4).enumerate() {
            let from = (width as usize - 1 - x) * 4;
            pixel.copy_from_slice(&src_row[from..from + 4]);
        }
    }
    PropImage::from_rgba(width, height, out)
}

/// Mirror the image top-to-bottom. Exact: pixels are moved, never sampled.
pub fn flip_vertical(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    check_dims(width, height)?;
    let src = image.as_rgba();
    let row = width as usize * 4;
    let mut out = vec![0u8; src.len()];
    for y in 0..height as usize {
        let from = (height as usize - 1 - y) * row;
        out[y * row..(y + 1) * row].copy_from_slice(&src[from..from + row]);
    }
    PropImage::from_rgba(width, height, out)
}

// -- Rotate -----------------------------------------------------------------

/// Rotate by an exact right angle. Lossless: pixels are permuted, never
/// sampled, so the alpha channel is untouched and four 90-degree turns return
/// the original image byte for byte.
pub fn rotate(image: &PropImage, turn: QuarterTurn) -> Result<PropImage> {
    match turn {
        QuarterTurn::Clockwise => rotate_cw(image),
        QuarterTurn::CounterClockwise => rotate_ccw(image),
        QuarterTurn::Half => rotate_180(image),
    }
}

/// Rotate 90 degrees clockwise. The result is the source transposed:
/// `height` x `width`.
pub fn rotate_cw(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    check_dims(height, width)?;
    let (sw, sh) = (width as usize, height as usize);
    let src = image.as_rgba();
    let mut out = vec![0u8; src.len()];
    for y in 0..sw {
        for x in 0..sh {
            let si = ((sh - 1 - x) * sw + y) * 4;
            let di = (y * sh + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    PropImage::from_rgba(height, width, out)
}

/// Rotate 90 degrees counter-clockwise. The result is `height` x `width`.
pub fn rotate_ccw(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    check_dims(height, width)?;
    let (sw, sh) = (width as usize, height as usize);
    let src = image.as_rgba();
    let mut out = vec![0u8; src.len()];
    for y in 0..sw {
        for x in 0..sh {
            let si = (x * sw + (sw - 1 - y)) * 4;
            let di = (y * sh + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    PropImage::from_rgba(height, width, out)
}

/// Rotate 180 degrees. The result keeps the source dimensions.
pub fn rotate_180(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    check_dims(width, height)?;
    let (sw, sh) = (width as usize, height as usize);
    let src = image.as_rgba();
    let mut out = vec![0u8; src.len()];
    for y in 0..sh {
        for x in 0..sw {
            let si = ((sh - 1 - y) * sw + (sw - 1 - x)) * 4;
            let di = (y * sw + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    PropImage::from_rgba(width, height, out)
}

// -- Crop -------------------------------------------------------------------

/// A pixel rectangle. `x`/`y` are the top-left corner; `width`/`height` are
/// the size. The right and bottom edges are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    /// Left edge, in pixels.
    pub x: u32,
    /// Top edge, in pixels.
    pub y: u32,
    /// Width, in pixels.
    pub width: u32,
    /// Height, in pixels.
    pub height: u32,
}

impl CropRect {
    /// A rectangle from a corner and a size.
    #[must_use]
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        CropRect {
            x,
            y,
            width,
            height,
        }
    }
}

/// Cut `rect` out of the image.
///
/// The rectangle is clamped to the image, so a selection that pokes over an
/// edge yields the part that is inside. A rectangle that overlaps nothing (or
/// has zero width/height) is refused with [`PropError::ImplausibleDimensions`]
/// — the operation never emits a zero-size prop.
pub fn crop(image: &PropImage, rect: CropRect) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    let x0 = rect.x.min(width);
    let y0 = rect.y.min(height);
    let x1 = rect.x.saturating_add(rect.width).min(width);
    let y1 = rect.y.saturating_add(rect.height).min(height);
    let cut_width = x1.saturating_sub(x0);
    let cut_height = y1.saturating_sub(y0);
    check_dims(cut_width, cut_height)?;

    let src = image.as_rgba();
    let source_width = width as usize;
    let row = cut_width as usize * 4;
    let mut out = vec![0u8; (cut_width as usize) * (cut_height as usize) * 4];
    for y in 0..cut_height as usize {
        let from = ((y0 as usize + y) * source_width + x0 as usize) * 4;
        let to = y * row;
        out[to..to + row].copy_from_slice(&src[from..from + row]);
    }
    PropImage::from_rgba(cut_width, cut_height, out)
}

/// Crop to the largest centred square.
///
/// An odd difference leaves the extra pixel on the right/bottom edge (integer
/// division), matching the reference editors' centred crop.
pub fn crop_square(image: &PropImage) -> Result<PropImage> {
    let (width, height) = (image.width(), image.height());
    let side = width.min(height);
    crop(
        image,
        CropRect::new((width - side) / 2, (height - side) / 2, side, side),
    )
}

// -- Resize -----------------------------------------------------------------

/// Resize to exactly `width` x `height`, stretching when the target aspect
/// ratio differs from the source's. Use [`resize_to_fit`],
/// [`resize_to_width`] or [`resize_to_height`] when the aspect ratio must be
/// kept (the panel's "constrain aspect" option).
///
/// Refused when the target is zero-sized, above [`MAX_DIMENSION`] or above
/// [`MAX_PIXELS`].
pub fn resize(image: &PropImage, width: u32, height: u32, resample: Resample) -> Result<PropImage> {
    check_dims(width, height)?;
    if image.width() == 0 || image.height() == 0 {
        return Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        });
    }
    let bytes = match resample {
        Resample::Nearest => resample_nearest(image, width, height),
        Resample::Bilinear => resample_bilinear(image, width, height),
    };
    PropImage::from_rgba(width, height, bytes)
}

/// Resize preserving the aspect ratio, so the result fits inside
/// `max_width` x `max_height`. This is the "constrain aspect" option against a
/// preset box: scale up or down and round to whole pixels, with the longer
/// side reaching the box.
pub fn resize_to_fit(
    image: &PropImage,
    max_width: u32,
    max_height: u32,
    resample: Resample,
) -> Result<PropImage> {
    check_dims(max_width, max_height)?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        });
    }
    let scale = f64::min(
        f64::from(max_width) / f64::from(width),
        f64::from(max_height) / f64::from(height),
    );
    let fitted_width = ((f64::from(width) * scale).round() as u32).max(1);
    let fitted_height = ((f64::from(height) * scale).round() as u32).max(1);
    resize(image, fitted_width, fitted_height, resample)
}

/// Resize preserving the aspect ratio with an exact width; the height is
/// derived and rounded to whole pixels.
pub fn resize_to_width(image: &PropImage, width: u32, resample: Resample) -> Result<PropImage> {
    let (source_width, source_height) = (image.width(), image.height());
    if source_width == 0 || source_height == 0 {
        return Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        });
    }
    let height = ((f64::from(width) * f64::from(source_height) / f64::from(source_width)).round()
        as u32)
        .max(1);
    resize(image, width, height, resample)
}

/// Resize preserving the aspect ratio with an exact height; the width is
/// derived and rounded to whole pixels.
pub fn resize_to_height(image: &PropImage, height: u32, resample: Resample) -> Result<PropImage> {
    let (source_width, source_height) = (image.width(), image.height());
    if source_width == 0 || source_height == 0 {
        return Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        });
    }
    let width = ((f64::from(height) * f64::from(source_width) / f64::from(source_height)).round()
        as u32)
        .max(1);
    resize(image, width, height, resample)
}

// -- Shape crop --------------------------------------------------------------

/// The mask a shape crop keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CropShape {
    /// The whole square canvas: a shape crop with this mask is exactly
    /// [`crop_square`] (plus any zoom and rotation).
    #[default]
    Square,
    /// The circle inscribed in the square canvas: everything outside it is
    /// cleared to transparent.
    Circle,
    /// The square with rounded corners; `radius` is in pixels and is clamped
    /// to half the side.
    Rounded {
        /// Corner radius in pixels.
        radius: u32,
    },
}

/// Shape-crop parameters, mirroring PalaceChat's PropEditorShapeCrop dialog.
///
/// The result is always the largest centred square of the source (`min(width,
/// height)`), with the mask applied on top. The image is scaled by `zoom` and
/// rotated by `rotation` about that square's centre before the mask is
/// applied; sampling reads transparent outside the source, so a rotated or
/// shrunk image does not smear its border pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeCropOptions {
    /// Which mask to keep.
    pub shape: CropShape,
    /// Magnification about the canvas centre; `1.0` keeps the natural size.
    /// Non-finite values fall back to `1.0`, and finite ones are clamped to
    /// `0.01..=100.0`.
    pub zoom: f32,
    /// Clockwise rotation in degrees about the canvas centre. Non-finite
    /// values fall back to `0.0`; finite ones are normalised into
    /// `0.0..360.0`.
    pub rotation: f32,
    /// Filter used when the sampling grid does not land exactly on a source
    /// pixel (zoom, rotation, or a bilinear mask edge).
    pub resample: Resample,
}

impl Default for ShapeCropOptions {
    fn default() -> Self {
        ShapeCropOptions {
            shape: CropShape::Square,
            zoom: 1.0,
            rotation: 0.0,
            resample: Resample::Bilinear,
        }
    }
}

/// Apply a shape mask, with zoom and rotation, to one image.
///
/// The operation masks only: pixels outside the shape (and outside the
/// source) become fully transparent `[0, 0, 0, 0]`; no shape outline or fill
/// is drawn.
pub fn shape_crop(image: &PropImage, options: &ShapeCropOptions) -> Result<PropImage> {
    let (source_width, source_height) = (image.width(), image.height());
    if source_width == 0 || source_height == 0 {
        return Err(PropError::ImplausibleDimensions {
            width: 0,
            height: 0,
        });
    }
    let side = source_width.min(source_height);
    check_dims(side, side)?;

    let zoom = sane_zoom(options.zoom);
    let rotation = sane_rotation(options.rotation);
    let radians = -rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let half = side as f32 / 2.0;
    let source_center_x = source_width as f32 / 2.0;
    let source_center_y = source_height as f32 / 2.0;

    let mut out = vec![0u8; (side as usize) * (side as usize) * 4];
    for y in 0..side {
        for x in 0..side {
            if !mask_keeps(options.shape, x, y, side) {
                continue;
            }
            // Inverse map the output pixel's centre back into the source.
            let qx = x as f32 + 0.5 - half;
            let qy = y as f32 + 0.5 - half;
            let rx = qx * cos - qy * sin;
            let ry = qx * sin + qy * cos;
            let u = source_center_x + rx / zoom;
            let v = source_center_y + ry / zoom;
            let pixel = sample_outside_transparent(image, u, v, options.resample);
            let at = (y as usize * side as usize + x as usize) * 4;
            out[at..at + 4].copy_from_slice(&pixel);
        }
    }
    PropImage::from_rgba(side, side, out)
}

// -- Frame helpers -----------------------------------------------------------

/// Apply an image operation to every layer of one frame, in place.
///
/// This is the bridge from the pure pixel operations above to the editor's
/// frame model: it maps each layer's pixels through `transform` and rebuilds
/// the frame, so the canvas takes the transformed base layer's size — which is
/// what a whole-frame flip, rotate, crop, resize or shape crop needs. A
/// layer's id, name, opacity, placement [`crate::editor::Transform`] and
/// visibility are preserved: this is a pixel operation, not a placement one.
///
/// If any layer's operation fails, the frame is left exactly as it was.
pub fn transform_frame(
    frame: &mut Frame,
    mut transform: impl FnMut(&PropImage) -> Result<PropImage>,
) -> Result<()> {
    let mut images = Vec::with_capacity(frame.layer_count());
    for layer in frame.layers() {
        images.push(transform(layer.image())?);
    }
    let mut images = images.into_iter();
    let Some(base_image) = images.next() else {
        return Ok(());
    };

    let mut rebuilt = Frame::from_base(base_image);
    if let (Some(original), Some(new_base)) = (frame.layer(0), rebuilt.layer_mut(0)) {
        new_base.id = original.id;
        new_base.name.clone_from(&original.name);
        new_base.opacity = original.opacity;
        new_base.transform = original.transform;
        new_base.visible = original.visible;
    }
    for (original, image) in frame.layers().iter().skip(1).zip(images) {
        let mut layer = Layer::new(original.id, original.name.clone(), image);
        layer.opacity = original.opacity;
        layer.transform = original.transform;
        layer.visible = original.visible;
        rebuilt.push_layer(layer);
    }
    *frame = rebuilt;
    Ok(())
}

/// Rotate every frame of a list by the same exact right angle — PalaceChat's
/// "Rotate all frames". Returns how many frames were rotated.
///
/// Frames are transformed in order; on a failure the frames before it are
/// already rotated, so callers should snapshot for undo first.
pub fn rotate_frames(frames: &mut [Frame], turn: QuarterTurn) -> Result<usize> {
    for frame in frames.iter_mut() {
        transform_frame(frame, |image| rotate(image, turn))?;
    }
    Ok(frames.len())
}

/// Shape-crop one frame in place.
pub fn shape_crop_frame(frame: &mut Frame, options: &ShapeCropOptions) -> Result<()> {
    transform_frame(frame, |image| shape_crop(image, options))
}

/// Shape-crop the selected frame, or every frame when the panel's
/// "apply to all frames" box is checked — PalaceChat's ShapeCropFrames.
///
/// `frames` is the document's frame list and `current` the selected index.
/// Returns how many frames changed; an out-of-range `current` with the box
/// unchecked changes nothing. Like [`rotate_frames`], a failure part-way
/// through leaves the earlier frames transformed, so snapshot for undo first.
pub fn shape_crop_frames(
    frames: &mut [Frame],
    current: usize,
    options: &ShapeCropOptions,
    apply_to_all_frames: bool,
) -> Result<usize> {
    if apply_to_all_frames {
        for frame in frames.iter_mut() {
            shape_crop_frame(frame, options)?;
        }
        return Ok(frames.len());
    }
    match frames.get_mut(current) {
        Some(frame) => {
            shape_crop_frame(frame, options)?;
            Ok(1)
        }
        None => Ok(0),
    }
}

// -- Sampling helpers --------------------------------------------------------

/// Nearest-neighbour scaling, mapping each destination pixel to the source
/// pixel containing its centre.
fn resample_nearest(image: &PropImage, width: u32, height: u32) -> Vec<u8> {
    let source_width = image.width() as usize;
    let scale_x = image.width() as f64 / f64::from(width);
    let scale_y = image.height() as f64 / f64::from(height);
    let src = image.as_rgba();
    let row = width as usize * 4;
    let mut out = vec![0u8; row * height as usize];
    for y in 0..height as usize {
        let source_y =
            (((y as f64 + 0.5) * scale_y).floor() as usize).min(image.height() as usize - 1);
        let dst_row = &mut out[y * row..(y + 1) * row];
        for (x, pixel) in dst_row.chunks_exact_mut(4).enumerate() {
            let source_x =
                (((x as f64 + 0.5) * scale_x).floor() as usize).min(image.width() as usize - 1);
            let at = (source_y * source_width + source_x) * 4;
            pixel.copy_from_slice(&src[at..at + 4]);
        }
    }
    out
}

/// Bilinear scaling with clamped edges: an enlarged edge extends the border
/// pixel instead of fading to transparent, so no dark fringe appears.
fn resample_bilinear(image: &PropImage, width: u32, height: u32) -> Vec<u8> {
    let source_width = image.width();
    let source_height = image.height();
    let scale_x = f64::from(source_width) / f64::from(width);
    let scale_y = f64::from(source_height) / f64::from(height);
    let row = width as usize * 4;
    let mut out = vec![0u8; row * height as usize];
    for y in 0..height {
        let fy = (f64::from(y) + 0.5) * scale_y - 0.5;
        let ty = (fy - fy.floor()) as f32;
        let y0 = fy.floor() as i64;
        let top = y0.clamp(0, i64::from(source_height) - 1) as u32;
        let bottom = (y0 + 1).clamp(0, i64::from(source_height) - 1) as u32;
        let dst_row = &mut out[y as usize * row..(y as usize + 1) * row];
        for (x, pixel) in dst_row.chunks_exact_mut(4).enumerate() {
            let fx = (x as f64 + 0.5) * scale_x - 0.5;
            let tx = (fx - fx.floor()) as f32;
            let x0 = fx.floor() as i64;
            let left = x0.clamp(0, i64::from(source_width) - 1) as u32;
            let right = (x0 + 1).clamp(0, i64::from(source_width) - 1) as u32;
            let p00 = image.pixel(left, top).unwrap_or([0, 0, 0, 0]);
            let p10 = image.pixel(right, top).unwrap_or([0, 0, 0, 0]);
            let p01 = image.pixel(left, bottom).unwrap_or([0, 0, 0, 0]);
            let p11 = image.pixel(right, bottom).unwrap_or([0, 0, 0, 0]);
            pixel.copy_from_slice(&blend(p00, p10, p01, p11, tx, ty));
        }
    }
    out
}

/// Sample the source at continuous coordinates `(u, v)`, where pixel `(i, j)`
/// covers `[i, i+1) x [j, j+1)`. Anything outside the source reads fully
/// transparent, which is what rotation and zoom need.
fn sample_outside_transparent(image: &PropImage, u: f32, v: f32, resample: Resample) -> [u8; 4] {
    match resample {
        Resample::Nearest => {
            let x = u.floor();
            let y = v.floor();
            if x < 0.0 || y < 0.0 || x >= image.width() as f32 || y >= image.height() as f32 {
                return [0, 0, 0, 0];
            }
            image.pixel(x as u32, y as u32).unwrap_or([0, 0, 0, 0])
        }
        Resample::Bilinear => {
            let fx = u - 0.5;
            let fy = v - 0.5;
            let x0 = fx.floor();
            let y0 = fy.floor();
            let tx = fx - x0;
            let ty = fy - y0;
            let x0 = x0 as i32;
            let y0 = y0 as i32;
            let p00 = pixel_or_transparent(image, x0, y0);
            let p10 = pixel_or_transparent(image, x0 + 1, y0);
            let p01 = pixel_or_transparent(image, x0, y0 + 1);
            let p11 = pixel_or_transparent(image, x0 + 1, y0 + 1);
            blend(p00, p10, p01, p11, tx, ty)
        }
    }
}

/// One source pixel by signed coordinates, transparent when outside.
fn pixel_or_transparent(image: &PropImage, x: i32, y: i32) -> [u8; 4] {
    if x < 0 || y < 0 {
        return [0, 0, 0, 0];
    }
    image.pixel(x as u32, y as u32).unwrap_or([0, 0, 0, 0])
}

/// Bilinear blend of four pixels in premultiplied alpha, unpremultiplied on
/// output. Alpha is interpolated linearly; colour only follows where alpha
/// survives, so transparent neighbours contribute no colour.
fn blend(p00: [u8; 4], p10: [u8; 4], p01: [u8; 4], p11: [u8; 4], tx: f32, ty: f32) -> [u8; 4] {
    let w00 = (1.0 - tx) * (1.0 - ty);
    let w10 = tx * (1.0 - ty);
    let w01 = (1.0 - tx) * ty;
    let w11 = tx * ty;
    let alpha = w00 * f32::from(p00[3])
        + w10 * f32::from(p10[3])
        + w01 * f32::from(p01[3])
        + w11 * f32::from(p11[3]);
    let mut out = [0u8; 4];
    out[3] = alpha.round().clamp(0.0, 255.0) as u8;
    if alpha > 0.0 {
        for channel in 0..3 {
            let premultiplied = w00 * f32::from(p00[channel]) * f32::from(p00[3])
                + w10 * f32::from(p10[channel]) * f32::from(p10[3])
                + w01 * f32::from(p01[channel]) * f32::from(p01[3])
                + w11 * f32::from(p11[channel]) * f32::from(p11[3]);
            out[channel] = (premultiplied / alpha).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Whether a shape mask keeps the pixel whose centre is `(x + 0.5, y + 0.5)`
/// on a `side` x `side` canvas. The mask is hard-edged (no antialiasing),
/// which keeps the result deterministic and matches a sprite editor's mask.
fn mask_keeps(shape: CropShape, x: u32, y: u32, side: u32) -> bool {
    let side_f = side as f32;
    let half = side_f / 2.0;
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    match shape {
        CropShape::Square => true,
        CropShape::Circle => {
            let dx = px - half;
            let dy = py - half;
            dx * dx + dy * dy <= half * half
        }
        CropShape::Rounded { radius } => {
            let r = (radius as f32).min(half);
            if r <= 0.0 {
                return true;
            }
            let beyond_x = if px < r {
                r - px
            } else if px > side_f - r {
                px - (side_f - r)
            } else {
                0.0
            };
            let beyond_y = if py < r {
                r - py
            } else if py > side_f - r {
                py - (side_f - r)
            } else {
                0.0
            };
            beyond_x * beyond_x + beyond_y * beyond_y <= r * r
        }
    }
}

/// Clamp a requested zoom into the range the sampler can work with.
fn sane_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// Normalise a requested rotation into `0.0..360.0`; non-finite means none.
fn sane_rotation(rotation: f32) -> f32 {
    if rotation.is_finite() {
        rotation.rem_euclid(360.0)
    } else {
        0.0
    }
}

/// Refuse a size the crate will not carry: zero, too wide/tall, or too many
/// pixels in total. The `i16` fields of [`PropError`] saturate instead of
/// wrapping, so an enormous request still names a plausible number.
fn check_dims(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION as u32 || height > MAX_DIMENSION as u32 {
        return Err(PropError::ImplausibleDimensions {
            width: display_dim(width),
            height: display_dim(height),
        });
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(PropError::ImageTooLarge {
            width: display_dim(width),
            height: display_dim(height),
        });
    }
    Ok(())
}

/// Fit a dimension into the `i16` error field without wrapping.
fn display_dim(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

//! Paint tools: brush, eraser, flood fill, colour picker, clear.
//!
//! Every operation here is a pure transform: it reads an existing [`PropImage`],
//! [`Layer`] or [`Frame`] and returns a new one. Nothing touches the filesystem,
//! the network or the webview, and no function mutates its input. The caller
//! decides when to push an undo snapshot and when to swap the result back into
//! the document (see [`crate::editor::EditorDocument::push_snapshot`]).
//!
//! Coordinates are pixels on the layer's own grid, `(0, 0)` at the top-left,
//! `y` growing downward, matching [`PropImage`]. A brush stroke may start or end
//! outside the canvas: the pixels that land inside are painted and the rest are
//! clipped, so a drag that leaves the canvas and comes back is safe.
//!
//! # The brush
//!
//! The stamp is the project's existing square stamp (the same convention as
//! `palace_render::draw`'s brush): `size` is the side length in pixels and is
//! clamped to `1..=[MAX_BRUSH_SIZE]`, so a size-1 brush paints exactly one pixel
//! and a size-48 brush a 48x48 block. A stroke walks a Bresenham line from `from`
//! to `to` and stamps at every step, so a drag is a connected line rather than a
//! dot.
//!
//! The brush is opaque by contract, matching the panel's "Brush tool (opaque
//! stroke)" tooltip: every painted pixel gets alpha 255, whatever alpha the
//! caller's colour carries. The eraser is its mirror image and writes fully
//! transparent pixels (`0, 0, 0, 0`), so it clears alpha instead of painting over
//! it.
//!
//! # Flood fill and tolerance
//!
//! [`flood_fill`] is a 4-connected fill from a seed pixel. `tolerance` is a
//! fraction of full scale: `0.0` accepts only pixels whose RGBA is exactly the
//! seed's, `1.0` accepts anything, and in between a channel matches when it lies
//! within `tolerance * 255` of the seed's channel. All four channels take part,
//! so an exact fill cannot leak through a one-pixel border.
//!
//! # Applying to every frame
//!
//! [`apply_to_all_frames`] and its named wrappers run an operation over each
//! frame of a document. Frames whose stack has no layer at the requested index
//! are returned unchanged; the individual `_frame` functions return
//! `Ok(None)` in that case for callers that need to know.
//!
//! # Example
//!
//! ```
//! use palace_prop::editor::tools::paint::{brush_stroke, clear_pixels};
//! use palace_prop::image::PropImage;
//!
//! let blank = PropImage::transparent(8, 8);
//! // A one-pixel drag from (1, 1) to (6, 1) paints a horizontal line.
//! let stroke = brush_stroke(&blank, (1, 1), (6, 1), [255, 0, 0, 255], 1).unwrap();
//! assert_eq!(stroke.pixel(3, 1), Some([255, 0, 0, 255]));
//! assert_eq!(stroke.pixel(3, 2), Some([0, 0, 0, 0]));
//!
//! // And clearing gives the blank canvas back.
//! assert_eq!(clear_pixels(&stroke).as_rgba(), blank.as_rgba());
//! ```

use crate::error::Result;
use crate::image::PropImage;

use super::super::{Frame, Layer};

/// The smallest brush the tools accept.
pub const MIN_BRUSH_SIZE: u32 = 1;

/// The largest brush the tools accept, matching the panel's size slider.
pub const MAX_BRUSH_SIZE: u32 = 48;

/// The reference editor's default flood-fill tolerance (`0.3` of full scale).
pub const DEFAULT_FILL_TOLERANCE: f32 = 0.3;

// -- Image primitives -------------------------------------------------------

/// Paint an opaque brush stroke between two points.
///
/// The whole Bresenham line from `from` to `to` is stamped, so one call serves
/// both a click (equal endpoints) and a drag (distinct endpoints). `color` is
/// RGBA; its alpha is overridden to 255 because the brush is opaque. `size` is
/// the stamp's side length and is clamped to [`MIN_BRUSH_SIZE`]..=
/// [`MAX_BRUSH_SIZE`].
pub fn brush_stroke(
    image: &PropImage,
    from: (i32, i32),
    to: (i32, i32),
    color: [u8; 4],
    size: u32,
) -> Result<PropImage> {
    let [red, green, blue, _] = color;
    let stamp = Stamp::new(size, [red, green, blue, 255]);
    stamp.over(image, from, to)
}

/// Erase along a stroke: every stamped pixel becomes fully transparent.
///
/// This clears alpha (`0, 0, 0, 0`) rather than painting a background colour
/// over the pixels, matching the "Eraser tool (clears alpha)" tooltip.
pub fn erase_stroke(
    image: &PropImage,
    from: (i32, i32),
    to: (i32, i32),
    size: u32,
) -> Result<PropImage> {
    let stamp = Stamp::new(size, [0, 0, 0, 0]);
    stamp.over(image, from, to)
}

/// Flood-fill the contiguous region around `(x, y)` with `color`.
///
/// The region is 4-connected: pixels are joined through shared edges, not
/// diagonals. A pixel joins the fill when its seed's RGBA channels all lie
/// within `tolerance * 255`; `tolerance` is clamped to `0.0..=1.0` and a NaN
/// tolerance is read as `0.0`. The fill writes `color` as given, alpha included.
///
/// A seed outside the image returns the image unchanged, as does a fill whose
/// colour equals the seed's. Stray NaN tolerances and out-of-range coordinates
/// therefore cannot panic.
pub fn flood_fill(
    image: &PropImage,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: f32,
) -> Result<PropImage> {
    let Some(seed) = image.pixel(x, y) else {
        return Ok(image.clone());
    };
    let limit = normalize_tolerance(tolerance) * 255.0;
    let width = image.width() as usize;
    let height = image.height() as usize;

    let mut bytes = image.as_rgba().to_vec();
    let mut visited = vec![false; width * height];
    let mut stack = vec![(x as usize, y as usize)];
    visited[y as usize * width + x as usize] = true;

    while let Some((px, py)) = stack.pop() {
        let at = (py * width + px) * 4;
        if let Some(pixel) = bytes.get_mut(at..at + 4) {
            pixel.copy_from_slice(&color);
        }

        // `wrapping_sub` turns a left/top edge into an index that the `>=`
        // bounds test rejects, so no branch is needed for zero.
        let neighbours = [
            (px.wrapping_sub(1), py),
            (px + 1, py),
            (px, py.wrapping_sub(1)),
            (px, py + 1),
        ];
        for (nx, ny) in neighbours {
            if nx >= width || ny >= height {
                continue;
            }
            let index = ny * width + nx;
            if visited[index] {
                continue;
            }
            let matches = image
                .pixel(nx as u32, ny as u32)
                .is_some_and(|pixel| within(pixel, seed, limit));
            if matches {
                visited[index] = true;
                stack.push((nx, ny));
            }
        }
    }

    PropImage::from_rgba(image.width(), image.height(), bytes)
}

/// Sample the colour at `(x, y)`, or `None` when the point is off the image.
///
/// The picker reads the layer's own pixels, not the composited frame; a caller
/// that wants the flattened colour can composite first.
#[must_use]
pub fn pick_color(image: &PropImage, x: u32, y: u32) -> Option<[u8; 4]> {
    image.pixel(x, y)
}

/// A fully transparent image of the same size: "clear current frame to
/// transparent" at the pixel level.
#[must_use]
pub fn clear_pixels(image: &PropImage) -> PropImage {
    PropImage::transparent(image.width(), image.height())
}

// -- Layer operations -------------------------------------------------------

/// Paint a brush stroke on a copy of `layer`, keeping its identity, name,
/// opacity, transform and visibility.
pub fn brush_stroke_layer(
    layer: &Layer,
    from: (i32, i32),
    to: (i32, i32),
    color: [u8; 4],
    size: u32,
) -> Result<Layer> {
    let painted = brush_stroke(layer.image(), from, to, color, size)?;
    Ok(layer_with_image(layer, painted))
}

/// Erase along a stroke on a copy of `layer`, keeping its metadata.
pub fn erase_stroke_layer(
    layer: &Layer,
    from: (i32, i32),
    to: (i32, i32),
    size: u32,
) -> Result<Layer> {
    let erased = erase_stroke(layer.image(), from, to, size)?;
    Ok(layer_with_image(layer, erased))
}

/// Flood-fill a copy of `layer`, keeping its metadata.
pub fn flood_fill_layer(
    layer: &Layer,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: f32,
) -> Result<Layer> {
    let filled = flood_fill(layer.image(), x, y, color, tolerance)?;
    Ok(layer_with_image(layer, filled))
}

/// Sample the colour at `(x, y)` on the layer's pixels.
#[must_use]
pub fn pick_color_layer(layer: &Layer, x: u32, y: u32) -> Option<[u8; 4]> {
    pick_color(layer.image(), x, y)
}

/// A copy of `layer` with transparent pixels, keeping its metadata.
#[must_use]
pub fn clear_layer(layer: &Layer) -> Layer {
    layer_with_image(layer, clear_pixels(layer.image()))
}

// -- Frame operations -------------------------------------------------------

/// Paint a brush stroke on layer `index` of a copy of `frame`.
///
/// Returns `Ok(None)` when the frame has no layer at `index`; the frame is
/// otherwise returned with every other layer untouched.
pub fn brush_stroke_frame(
    frame: &Frame,
    layer: usize,
    from: (i32, i32),
    to: (i32, i32),
    color: [u8; 4],
    size: u32,
) -> Result<Option<Frame>> {
    frame_with_painted_layer(frame, layer, |target| {
        brush_stroke_layer(target, from, to, color, size)
    })
}

/// Erase along a stroke on layer `index` of a copy of `frame`.
///
/// Returns `Ok(None)` when the frame has no layer at `index`.
pub fn erase_stroke_frame(
    frame: &Frame,
    layer: usize,
    from: (i32, i32),
    to: (i32, i32),
    size: u32,
) -> Result<Option<Frame>> {
    frame_with_painted_layer(frame, layer, |target| {
        erase_stroke_layer(target, from, to, size)
    })
}

/// Flood-fill layer `index` of a copy of `frame`.
///
/// Returns `Ok(None)` when the frame has no layer at `index`.
pub fn flood_fill_frame(
    frame: &Frame,
    layer: usize,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: f32,
) -> Result<Option<Frame>> {
    frame_with_painted_layer(frame, layer, |target| {
        flood_fill_layer(target, x, y, color, tolerance)
    })
}

/// Sample the colour at `(x, y)` on layer `index` of `frame`.
#[must_use]
pub fn pick_color_frame(frame: &Frame, layer: usize, x: u32, y: u32) -> Option<[u8; 4]> {
    pick_color_layer(frame.layer(layer)?, x, y)
}

/// A copy of `frame` whose layers are all transparent.
///
/// The layer stack, ids, names, opacity, transforms and visibility are kept:
/// clearing a frame empties it, it does not restructure it. "Clear current
/// frame to transparent" per the panel tooltip.
#[must_use]
pub fn clear_frame(frame: &Frame) -> Frame {
    let mut cleared = frame.clone();
    for layer in cleared.layers_mut() {
        layer.image = clear_pixels(layer.image());
    }
    cleared
}

// -- Apply to every frame ---------------------------------------------------

/// Run `operation` over every frame, collecting the results in order.
///
/// The operation is handed each frame and returns its replacement, which keeps
/// this composable with any pure frame edit and keeps the caller in charge of
/// which frames change.
pub fn apply_to_all_frames(
    frames: &[Frame],
    mut operation: impl FnMut(&Frame) -> Result<Frame>,
) -> Result<Vec<Frame>> {
    frames.iter().map(&mut operation).collect()
}

/// Paint the same brush stroke on layer `index` of every frame.
///
/// Frames without a layer at `index` (or whose stroke fails) are returned
/// unchanged; see [`brush_stroke_frame`] for the per-frame result.
pub fn brush_stroke_all_frames(
    frames: &[Frame],
    layer: usize,
    from: (i32, i32),
    to: (i32, i32),
    color: [u8; 4],
    size: u32,
) -> Result<Vec<Frame>> {
    apply_to_all_frames(frames, |frame| {
        Ok(brush_stroke_frame(frame, layer, from, to, color, size)?
            .unwrap_or_else(|| frame.clone()))
    })
}

/// Erase the same stroke on layer `index` of every frame.
pub fn erase_stroke_all_frames(
    frames: &[Frame],
    layer: usize,
    from: (i32, i32),
    to: (i32, i32),
    size: u32,
) -> Result<Vec<Frame>> {
    apply_to_all_frames(frames, |frame| {
        Ok(erase_stroke_frame(frame, layer, from, to, size)?.unwrap_or_else(|| frame.clone()))
    })
}

/// Flood-fill the same seed on layer `index` of every frame.
pub fn flood_fill_all_frames(
    frames: &[Frame],
    layer: usize,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: f32,
) -> Result<Vec<Frame>> {
    apply_to_all_frames(frames, |frame| {
        Ok(
            flood_fill_frame(frame, layer, x, y, color, tolerance)?
                .unwrap_or_else(|| frame.clone()),
        )
    })
}

/// Clear every frame, keeping each frame's layer structure.
#[must_use]
pub fn clear_all_frames(frames: &[Frame]) -> Vec<Frame> {
    frames.iter().map(clear_frame).collect()
}

// -- Helpers ----------------------------------------------------------------

/// A square brush stamp: the offsets it covers and what it writes.
#[derive(Clone, Copy)]
struct Stamp {
    /// First covered offset from the centre, inclusive.
    lo: i32,
    /// One past the last covered offset from the centre.
    hi: i32,
    /// The RGBA written to every covered pixel.
    rgba: [u8; 4],
}

impl Stamp {
    /// Clamp `size` into the supported range and build the stamp.
    fn new(size: u32, rgba: [u8; 4]) -> Self {
        let side = size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) as i32;
        let half = side / 2;
        Stamp {
            lo: -half,
            hi: half + side % 2,
            rgba,
        }
    }

    /// Clone the image's bytes, stamp the line, and rebuild the image.
    fn over(&self, image: &PropImage, from: (i32, i32), to: (i32, i32)) -> Result<PropImage> {
        let width = image.width();
        let height = image.height();
        let mut bytes = image.as_rgba().to_vec();
        self.stroke(&mut bytes, width, height, from, to);
        PropImage::from_rgba(width, height, bytes)
    }

    /// Stamp every step of the Bresenham line from `from` to `to`.
    fn stroke(&self, bytes: &mut [u8], width: u32, height: u32, from: (i32, i32), to: (i32, i32)) {
        bresenham(from, to, |x, y| {
            for dy in self.lo..self.hi {
                for dx in self.lo..self.hi {
                    paint_pixel(bytes, width, height, x + dx, y + dy, self.rgba);
                }
            }
        });
    }
}

/// Write one pixel when `(x, y)` is on the canvas; off-canvas points are
/// clipped silently.
fn paint_pixel(bytes: &mut [u8], width: u32, height: u32, x: i32, y: i32, rgba: [u8; 4]) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let at = (y as usize * width as usize + x as usize) * 4;
    if let Some(pixel) = bytes.get_mut(at..at + 4) {
        pixel.copy_from_slice(&rgba);
    }
}

/// Walk the integer line from `from` to `to`, calling `visit` once per pixel.
///
/// The classic Bresenham loop with widened deltas, so extreme `i32` endpoints
/// cannot overflow the error term. Every visited point lies between the two
/// endpoints, so the values always fit back into `i32`.
fn bresenham(from: (i32, i32), to: (i32, i32), mut visit: impl FnMut(i32, i32)) {
    let (x0, y0) = from;
    let (x1, y1) = to;
    let dx = (i64::from(x1) - i64::from(x0)).abs();
    let dy = (i64::from(y1) - i64::from(y0)).abs();
    let step_x: i64 = if x0 < x1 { 1 } else { -1 };
    let step_y: i64 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let mut x = i64::from(x0);
    let mut y = i64::from(y0);
    loop {
        visit(x as i32, y as i32);
        if x == i64::from(x1) && y == i64::from(y1) {
            break;
        }
        let doubled = 2 * err;
        if doubled > -dy {
            err -= dy;
            x += step_x;
        }
        if doubled < dx {
            err += dx;
            y += step_y;
        }
    }
}

/// Whether every channel of `pixel` lies within `limit` of `seed`'s.
fn within(pixel: [u8; 4], seed: [u8; 4], limit: f32) -> bool {
    (0..4).all(|channel| f32::from(pixel[channel].abs_diff(seed[channel])) <= limit)
}

/// Read a caller's tolerance as a real `0.0..=1.0` fraction.
fn normalize_tolerance(tolerance: f32) -> f32 {
    if tolerance.is_nan() {
        0.0
    } else {
        tolerance.clamp(0.0, 1.0)
    }
}

/// A copy of `layer` carrying `image` instead of its old pixels.
fn layer_with_image(layer: &Layer, image: PropImage) -> Layer {
    let mut next = layer.clone();
    next.image = image;
    next
}

/// Paint layer `index` of a clone of `frame`, or report a missing layer.
fn frame_with_painted_layer(
    frame: &Frame,
    index: usize,
    paint: impl FnOnce(&Layer) -> Result<Layer>,
) -> Result<Option<Frame>> {
    let Some(layer) = frame.layer(index) else {
        return Ok(None);
    };
    let painted = paint(layer)?;
    let mut next = frame.clone();
    match next.layer_mut(index) {
        Some(slot) => {
            *slot = painted;
            Ok(Some(next))
        }
        None => Ok(None),
    }
}

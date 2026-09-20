//! One editor layer: its pixels, opacity, transform and visibility.
//!
//! A layer is the unit a paint tool touches. It reuses [`PropImage`] for pixels
//! rather than inventing a second image type: `PropImage` is already the crate's
//! RGBA8 buffer, and a parallel type would fork the one guarantee that matters
//! (exactly `width * height * 4` bytes).
//!
//! `PropImage` is immutable once built, so editing a layer rebuilds its buffer
//! through [`Layer::edit_pixels`] / [`Layer::set_pixel`]. At the editor's 44x44
//! default canvas that is a 7,744-byte copy per edit, which is the documented
//! cost of not carrying a second mutable pixel buffer and its invariants. A
//! caller that draws many pixels should use one [`Layer::edit_pixels`] call
//! rather than a `set_pixel` loop.

use crate::error::Result;
use crate::image::PropImage;

/// Stable identity for a layer, unique within a document.
///
/// The document is the only allocator: a bare [`Layer`] may be built with any id,
/// but every frame the document adopts is reissued fresh ids. Snapshot undo/redo
/// preserves ids, and the document never reissues one, so a layer id stays unique
/// for the life of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(u64);

impl LayerId {
    /// Wrap a raw id.
    #[must_use]
    pub fn new(raw: u64) -> Self {
        LayerId(raw)
    }

    /// The raw id, for state that must be persisted or compared.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Where a layer's pixels sit on its frame's canvas.
///
/// The canvas is the frame's `width` x `height` grid. A layer is scaled and
/// rotated about its own centre, then translated so that its untransformed
/// top-left corner lands at `(x, y)`. `rotation` is in degrees and is clockwise
/// on screen, matching a paint canvas whose y axis grows downward; `scale` is a
/// multiple of the layer's natural size.
///
/// The defaults are the identity: no offset, no rotation, natural size. A scale
/// of zero hides the layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Horizontal translation of the layer's untransformed top-left.
    pub x: i32,
    /// Vertical translation of the layer's untransformed top-left.
    pub y: i32,
    /// Clockwise rotation in degrees about the layer centre.
    pub rotation: f32,
    /// Uniform scale; `1.0` is natural size.
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            x: 0,
            y: 0,
            rotation: 0.0,
            scale: 1.0,
        }
    }
}

impl Transform {
    /// Whether this transform leaves pixels where they are.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.x == 0 && self.y == 0 && self.rotation == 0.0 && self.scale == 1.0
    }
}

/// A single layer: RGBA pixels plus how they are drawn.
///
/// `opacity` is clamped to `0.0..=1.0` when the layer is composited, so a stored
/// value outside that range is harmless. `visible == false` skips the layer
/// entirely, which is distinct from `opacity == 0.0` only in intent.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Document-unique identity.
    pub id: LayerId,
    /// Human-readable name for the layer list.
    pub name: String,
    /// The layer's pixels.
    pub image: PropImage,
    /// Draw opacity, clamped to `0.0..=1.0` at composite time.
    pub opacity: f32,
    /// Placement on the frame canvas.
    pub transform: Transform,
    /// Whether the layer is drawn at all.
    pub visible: bool,
}

impl Layer {
    /// A fully opaque, visible, untransformed layer.
    #[must_use]
    pub fn new(id: LayerId, name: impl Into<String>, image: PropImage) -> Self {
        Layer {
            id,
            name: name.into(),
            image,
            opacity: 1.0,
            transform: Transform::default(),
            visible: true,
        }
    }

    /// Override the opacity, builder-style.
    #[must_use]
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Override the transform, builder-style.
    #[must_use]
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Override the visibility, builder-style.
    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// The layer's pixels.
    #[must_use]
    pub fn image(&self) -> &PropImage {
        &self.image
    }

    /// Rebuild the pixel buffer through a closure.
    ///
    /// `PropImage` does not hand out a mutable slice, so the buffer is cloned,
    /// handed to `edit`, and rebuilt. The rebuild cannot change the dimensions or
    /// the byte length, so it cannot fail on a size mismatch; the [`Result`] only
    /// carries the image-layer error type.
    pub fn edit_pixels(&mut self, edit: impl FnOnce(&mut [u8])) -> Result<()> {
        let (width, height) = (self.image.width(), self.image.height());
        let mut bytes = self.image.as_rgba().to_vec();
        edit(&mut bytes);
        self.image = PropImage::from_rgba(width, height, bytes)?;
        Ok(())
    }

    /// Set one pixel, or do nothing when `(x, y)` is off the layer.
    ///
    /// A convenience over [`Layer::edit_pixels`] for one pixel. Drawing many
    /// pixels is cheaper through a single `edit_pixels` call.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) -> Result<()> {
        let width = self.image.width() as usize;
        let at = (y as usize)
            .checked_mul(width)
            .and_then(|row| row.checked_add(x as usize))
            .and_then(|px| px.checked_mul(4));
        let Some(at) = at else {
            return Ok(());
        };
        self.edit_pixels(|bytes| {
            if let Some(pixel) = bytes.get_mut(at..at + 4) {
                pixel.copy_from_slice(&rgba);
            }
        })
    }

    /// Reassign this layer's identity. The document owns id allocation.
    pub(crate) fn set_id(&mut self, id: LayerId) {
        self.id = id;
    }

    /// The source pixel that lands on canvas pixel `(dx, dy)`, if any.
    ///
    /// This is the inverse of the layer transform: the canvas point is moved back
    /// through the translation, rotation and scale to a layer coordinate, and the
    /// nearest pixel is sampled. Nearest-neighbour sampling keeps compositing
    /// deterministic and adds no smoothing to the model. `None` means the canvas
    /// pixel is outside the transformed layer or the transform is degenerate
    /// (zero or non-finite scale).
    pub(crate) fn source_pixel(&self, dx: i32, dy: i32) -> Option<[u8; 4]> {
        let width = self.image.width() as i32;
        let height = self.image.height() as i32;
        if width <= 0 || height <= 0 {
            return None;
        }
        let transform = self.transform;

        let (sx, sy) = if transform.is_identity() {
            (dx, dy)
        } else if transform.rotation == 0.0 && transform.scale == 1.0 {
            (dx - transform.x, dy - transform.y)
        } else {
            if !transform.rotation.is_finite() || !transform.scale.is_finite() {
                return None;
            }
            let scale = transform.scale;
            if scale == 0.0 {
                return None;
            }
            // Undo the translation, then rotate/scale about the layer centre.
            let cx = width as f32 / 2.0;
            let cy = height as f32 / 2.0;
            let qx = (dx - transform.x) as f32 - cx;
            let qy = (dy - transform.y) as f32 - cy;
            let radians = -transform.rotation.to_radians();
            let (sin, cos) = radians.sin_cos();
            let rx = qx * cos - qy * sin;
            let ry = qx * sin + qy * cos;
            (
                (rx / scale + cx).round() as i32,
                (ry / scale + cy).round() as i32,
            )
        };

        if sx < 0 || sy < 0 || sx >= width || sy >= height {
            return None;
        }
        self.image.pixel(sx as u32, sy as u32)
    }
}

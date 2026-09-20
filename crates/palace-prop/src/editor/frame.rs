//! A frame: one canvas size and an ordered stack of layers.
//!
//! Layer index 0 is the base; every layer after it is an overlay drawn on top.
//! The frame is the unit the document orders, duplicates, copies and deletes. Its
//! layers keep their own sizes and transforms, and are flattened onto the frame's
//! `width` x `height` canvas by [`Frame::composite`].
//!
//! A frame built here carries placeholder layer ids; the document reissues real
//! ids when it adopts the frame. Building layers directly is therefore only
//! meaningful inside the document or in tests.

use crate::error::Result;
use crate::image::PropImage;

use super::layer::{Layer, LayerId};

/// The name given to a frame's bottom layer.
pub const BASE_LAYER_NAME: &str = "Base";

/// A canvas size plus its ordered layer stack (base first, overlays above).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    width: u32,
    height: u32,
    layers: Vec<Layer>,
}

impl Frame {
    /// A frame with one transparent base layer.
    #[must_use]
    pub fn blank(width: u32, height: u32) -> Self {
        let base = Layer::new(
            LayerId::new(0),
            BASE_LAYER_NAME,
            PropImage::transparent(width, height),
        );
        Frame {
            width,
            height,
            layers: vec![base],
        }
    }

    /// A frame whose base layer is `image`, sized to match it.
    #[must_use]
    pub fn from_base(image: PropImage) -> Self {
        let width = image.width();
        let height = image.height();
        let base = Layer::new(LayerId::new(0), BASE_LAYER_NAME, image);
        Frame {
            width,
            height,
            layers: vec![base],
        }
    }

    /// Canvas width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Canvas height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Every layer, bottom (base) first.
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Mutable access to the layers, base first.
    #[must_use]
    pub fn layers_mut(&mut self) -> &mut [Layer] {
        &mut self.layers
    }

    /// How many layers the frame carries, base included.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// How many overlays sit above the base.
    #[must_use]
    pub fn overlay_count(&self) -> usize {
        self.layers.len().saturating_sub(1)
    }

    /// The layer at `index`, or `None` when out of range.
    #[must_use]
    pub fn layer(&self, index: usize) -> Option<&Layer> {
        self.layers.get(index)
    }

    /// Mutable access to the layer at `index`.
    #[must_use]
    pub fn layer_mut(&mut self, index: usize) -> Option<&mut Layer> {
        self.layers.get_mut(index)
    }

    /// The base layer (index 0), always present.
    #[must_use]
    pub fn base(&self) -> Option<&Layer> {
        self.layers.first()
    }

    /// Add a layer on top of the stack, returning its index.
    pub fn push_layer(&mut self, layer: Layer) -> usize {
        self.layers.push(layer);
        self.layers.len() - 1
    }

    /// Remove the layer at `index`, unless it is the base or the last layer.
    ///
    /// The base stays at index 0 and a frame always keeps at least one layer, so
    /// the caller can rely on [`Frame::base`] never returning `None`.
    pub fn remove_layer(&mut self, index: usize) -> Option<Layer> {
        if index == 0 || self.layers.len() <= 1 || index >= self.layers.len() {
            return None;
        }
        Some(self.layers.remove(index))
    }

    /// Flatten the visible layers into one RGBA image of the canvas size.
    ///
    /// Layers are drawn bottom to top with source-over blending and their
    /// per-layer opacity. A hidden, zero-opacity or fully transparent layer
    /// contributes nothing. The result is deterministic: the same layers always
    /// produce the same bytes, so it can be compared directly or hashed as an
    /// undo/redo digest.
    pub fn composite(&self) -> Result<PropImage> {
        let width = self.width as usize;
        let height = self.height as usize;
        let mut out = vec![0u8; width * height * 4];

        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            let opacity = if layer.opacity.is_finite() {
                layer.opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            if opacity <= 0.0 {
                continue;
            }
            for y in 0..self.height {
                for x in 0..self.width {
                    let Some(src) = layer.source_pixel(x as i32, y as i32) else {
                        continue;
                    };
                    let sa = f32::from(src[3]) / 255.0 * opacity;
                    if sa <= 0.0 {
                        continue;
                    }
                    let at = (y as usize * width + x as usize) * 4;
                    let Some(dst) = out.get_mut(at..at + 4) else {
                        continue;
                    };
                    // Source-over, straight (non-premultiplied) alpha.
                    let da = f32::from(dst[3]) / 255.0;
                    let oa = sa + da * (1.0 - sa);
                    if oa <= 0.0 {
                        continue;
                    }
                    for channel in 0..3 {
                        let sc = f32::from(src[channel]) / 255.0;
                        let dc = f32::from(dst[channel]) / 255.0;
                        let blended = (sc * sa + dc * da * (1.0 - sa)) / oa;
                        dst[channel] = (blended * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    dst[3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }

        PropImage::from_rgba(self.width, self.height, out)
    }
}

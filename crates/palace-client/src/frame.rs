//! The current frame and the presentation geometry that goes with it.
//!
//! The compositor produces a device-pixel bitmap of the room; this module holds
//! it as PNG bytes and describes, using `palace-render`'s verified
//! [`ViewTransform`], how the presentation layer should place it. The frontend
//! never computes a scale of its own — it applies what is reported here.

use std::sync::Mutex;

use palace_render::{ScaleMode, SizeF, ViewTransform};
use serde::Serialize;

/// Placement of the room bitmap inside the viewport, in CSS pixels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewGeometry {
    pub room_w: f64,
    pub room_h: f64,
    pub viewport_w: f64,
    pub viewport_h: f64,
    pub dpr: f64,
    pub zoom: f64,
    pub native: bool,
    pub scale: f64,
    pub content_x: f64,
    pub content_y: f64,
    pub content_w: f64,
    pub content_h: f64,
    pub bitmap_w: u32,
    pub bitmap_h: u32,
}

impl ViewGeometry {
    /// Project a room size into a viewport using the shared transform.
    #[must_use]
    pub fn compute(room: SizeF, viewport: SizeF, zoom: f64, native: bool, dpr: f64) -> Self {
        let mode = if native {
            ScaleMode::Native
        } else {
            ScaleMode::Fit
        };
        let transform = ViewTransform::new(room, viewport, zoom, mode, dpr);
        let rect = transform.content_rect();
        let (bitmap_w, bitmap_h) = transform.buffer_size();
        ViewGeometry {
            room_w: room.width,
            room_h: room.height,
            viewport_w: viewport.width,
            viewport_h: viewport.height,
            dpr: transform.dpr(),
            zoom: transform.zoom(),
            native,
            scale: transform.scale(),
            content_x: rect.x,
            content_y: rect.y,
            content_w: rect.width,
            content_h: rect.height,
            bitmap_w,
            bitmap_h,
        }
    }
}

/// Everything the UI needs to draw the current room.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenState {
    pub version: u64,
    pub room_id: i32,
    pub room_name: String,
    pub avatars: usize,
    pub loose_props: usize,
    pub props_pending: usize,
    pub notes: Vec<String>,
    pub geometry: ViewGeometry,
}

#[derive(Debug, Default)]
struct Inner {
    png: Vec<u8>,
    version: u64,
}

/// The latest composited frame, shared with the URI-scheme handler.
#[derive(Debug, Default)]
pub struct FrameStore {
    inner: Mutex<Inner>,
}

impl FrameStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        FrameStore::default()
    }

    /// Replace the frame, returning its new version. Versions start at 1.
    pub fn put(&self, png: Vec<u8>) -> u64 {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.png = png;
        inner.version += 1;
        inner.version
    }

    /// A copy of the current PNG, if one has been composed.
    #[must_use]
    pub fn png(&self) -> Option<Vec<u8>> {
        let inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if inner.png.is_empty() {
            None
        } else {
            Some(inner.png.clone())
        }
    }

    /// The current frame version, 0 before the first frame.
    #[must_use]
    pub fn version(&self) -> u64 {
        match self.inner.lock() {
            Ok(guard) => guard.version,
            Err(poisoned) => poisoned.into_inner().version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(room: (f64, f64), viewport: (f64, f64), zoom: f64, native: bool, dpr: f64) -> ViewGeometry {
        ViewGeometry::compute(
            SizeF::new(room.0, room.1),
            SizeF::new(viewport.0, viewport.1),
            zoom,
            native,
            dpr,
        )
    }

    #[test]
    fn fit_letterboxes_the_constrained_axis() {
        let g = geometry((512.0, 384.0), (960.0, 540.0), 1.0, false, 1.0);
        assert!((g.scale - 1.40625).abs() < 1e-9);
        assert!((g.content_w - 720.0).abs() < 1e-9);
        assert!((g.content_h - 540.0).abs() < 1e-9);
        assert!((g.content_x - 120.0).abs() < 1e-9);
        assert!((g.content_y - 0.0).abs() < 1e-9);
        assert_eq!((g.bitmap_w, g.bitmap_h), (512, 384));
    }

    #[test]
    fn native_mode_is_one_room_pixel_per_css_pixel() {
        let g = geometry((512.0, 384.0), (960.0, 540.0), 2.5, true, 1.0);
        assert!((g.scale - 1.0).abs() < 1e-9, "zoom is ignored in 1:1");
        assert!((g.content_w - 512.0).abs() < 1e-9);
        assert!((g.content_x - 224.0).abs() < 1e-9);
        assert!((g.content_y - 78.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_multiplies_the_fit_scale_and_may_overflow() {
        let g = geometry((512.0, 384.0), (960.0, 540.0), 2.0, false, 1.0);
        assert!((g.scale - 2.8125).abs() < 1e-9);
        assert!((g.content_x + 240.0).abs() < 1e-9);
        assert!((g.content_y + 270.0).abs() < 1e-9);
    }

    #[test]
    fn a_non_four_three_room_fits_its_own_aspect() {
        let g = geometry((1280.0, 720.0), (800.0, 600.0), 1.0, false, 1.0);
        assert!((g.scale - 0.625).abs() < 1e-9);
        assert!((g.content_w - 800.0).abs() < 1e-9);
        assert!((g.content_h - 450.0).abs() < 1e-9);
        assert!((g.content_y - 75.0).abs() < 1e-9);
    }

    #[test]
    fn dpr_is_clamped_and_drives_the_bitmap() {
        let g = geometry((512.0, 384.0), (960.0, 540.0), 1.0, false, 3.0);
        assert!((g.dpr - 2.0).abs() < 1e-9);
        assert_eq!((g.bitmap_w, g.bitmap_h), (1024, 768));
    }

    #[test]
    fn zoom_is_clamped_to_the_supported_range() {
        let low = geometry((512.0, 384.0), (960.0, 540.0), 0.01, false, 1.0);
        assert!((low.zoom - 0.5).abs() < 1e-9);
        let high = geometry((512.0, 384.0), (960.0, 540.0), 99.0, false, 1.0);
        assert!((high.zoom - 3.0).abs() < 1e-9);
    }
}

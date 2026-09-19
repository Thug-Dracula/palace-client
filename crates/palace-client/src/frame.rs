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
    /// Rebuild the verified transform these numbers came from.
    ///
    /// The runtime needs it to turn a viewport click back into room
    /// coordinates; rebuilding from the reported geometry guarantees the
    /// inverse uses exactly the transform the frame was composited with.
    #[must_use]
    pub fn transform(&self) -> ViewTransform {
        let mode = if self.native {
            ScaleMode::Native
        } else {
            ScaleMode::Fit
        };
        ViewTransform::new(
            SizeF::new(self.room_w, self.room_h),
            SizeF::new(self.viewport_w, self.viewport_h),
            self.zoom,
            mode,
            self.dpr,
        )
    }

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
    /// Version of the middle board layer, or `None` when that layer is empty.
    /// The middle layer sits above the base bitmap and below the avatar cards.
    pub mid_version: Option<u64>,
    /// Version of the top board layer, or `None` when that layer is empty.
    /// The top layer sits above the avatar cards.
    pub top_version: Option<u64>,
    pub room_id: i32,
    pub room_name: String,
    pub avatars: usize,
    pub loose_props: usize,
    pub props_pending: usize,
    pub notes: Vec<String>,
    pub geometry: ViewGeometry,
}

#[derive(Debug, Default)]
struct Layer {
    png: Vec<u8>,
    version: u64,
}

#[derive(Debug, Default)]
struct Inner {
    base: Layer,
    mid: Option<Layer>,
    top: Option<Layer>,
}

/// The latest composited frame layers, shared with the URI-scheme handler.
///
/// `put`/`png`/`version` describe the base layer and are unchanged for existing
/// callers. The middle and top layers are optional: each is absent until a
/// non-empty bitmap is stored for it.
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Replace the base layer, returning its version. Versions start at 1.
    ///
    /// The version only advances when the bytes actually change. An unchanged
    /// layer keeps the version it already had, exactly like the middle and top
    /// layers: the display keys its base fetch on this number, so bumping it
    /// for a byte-identical compose would make it re-download the whole base
    /// bitmap (megabytes at DPR 2) even though nothing on the board moved.
    /// The comparison and the store happen under one lock, so two callers can
    /// never interleave between them.
    pub fn put(&self, png: Vec<u8>) -> u64 {
        let mut inner = self.lock();
        if inner.base.png == png {
            return inner.base.version;
        }
        inner.base.png = png;
        inner.base.version += 1;
        inner.base.version
    }

    /// A copy of the current base PNG, if one has been composed.
    #[must_use]
    pub fn png(&self) -> Option<Vec<u8>> {
        let inner = self.lock();
        if inner.base.png.is_empty() {
            None
        } else {
            Some(inner.base.png.clone())
        }
    }

    /// The current base layer version, 0 before the first frame.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.lock().base.version
    }

    /// Store the middle layer with the version reported by the renderer.
    ///
    /// An empty bitmap removes the layer, so `mid_png`/`mid_version` report
    /// `None` again.
    pub fn put_mid(&self, png: Vec<u8>, version: u64) {
        let layer = (!png.is_empty()).then_some(Layer { png, version });
        self.lock().mid = layer;
    }

    /// Store the top layer with the version reported by the renderer.
    ///
    /// An empty bitmap removes the layer, so `top_png`/`top_version` report
    /// `None` again.
    pub fn put_top(&self, png: Vec<u8>, version: u64) {
        let layer = (!png.is_empty()).then_some(Layer { png, version });
        self.lock().top = layer;
    }

    /// A copy of the current middle PNG, or `None` when the layer is absent.
    #[must_use]
    pub fn mid_png(&self) -> Option<Vec<u8>> {
        self.lock().mid.as_ref().map(|layer| layer.png.clone())
    }

    /// A copy of the current top PNG, or `None` when the layer is absent.
    #[must_use]
    pub fn top_png(&self) -> Option<Vec<u8>> {
        self.lock().top.as_ref().map(|layer| layer.png.clone())
    }

    /// The middle layer's version, or `None` when the layer is absent.
    #[must_use]
    pub fn mid_version(&self) -> Option<u64> {
        self.lock().mid.as_ref().map(|layer| layer.version)
    }

    /// The top layer's version, or `None` when the layer is absent.
    #[must_use]
    pub fn top_version(&self) -> Option<u64> {
        self.lock().top.as_ref().map(|layer| layer.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_render::PointF;

    fn geometry(
        room: (f64, f64),
        viewport: (f64, f64),
        zoom: f64,
        native: bool,
        dpr: f64,
    ) -> ViewGeometry {
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

    const EPS: f64 = 1e-9;

    /// The room/viewport shapes the round-trip sweep runs over. `wide` and `tall`
    /// differ in aspect from every room so both a pillarbox and a letterbox are in
    /// the sweep; `small` makes the letterbox offsets negative (zoom-in overflow).
    const VIEWPORTS: [(f64, f64); 5] = [
        (960.0, 540.0),
        (600.0, 400.0),
        (1000.0, 700.0),
        (500.0, 800.0),
        (200.0, 150.0),
    ];
    const ZOOMS: [f64; 5] = [0.5, 0.75, 1.0, 1.25, 3.0];
    const DPRS: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    /// Room coordinates picked w.r.t. the 512×384 reference room; the sweep scales
    /// them to each room in the list so the same fractions are hit everywhere.
    fn room_fractions() -> Vec<(f64, f64)> {
        vec![
            (0.0, 0.0),
            (0.5, 0.5),
            (0.25, 0.75),
            (0.999, 0.001),
            (-40.0, -25.0),
            (512.0 + 60.0, 384.0 + 45.0),
        ]
    }

    /// The point a viewport click resolves to. This is the client's
    /// `click_room_point` (runtime.rs): the transform inverse, then `round`.
    fn clicked_room_point(g: &ViewGeometry, screen: (f64, f64)) -> (f64, f64) {
        let p = g
            .transform()
            .viewport_to_room(PointF::new(screen.0, screen.1));
        (p.x.round(), p.y.round())
    }

    /// Half a logical pixel: the largest error a nearest-cell pick can introduce,
    /// and the tolerance every round trip through a click is held to. The two
    /// transforms are exact inverses (viewport.rs), so a larger error means the
    /// wrong cell was picked, not float noise.
    const PICK_TOLERANCE: f64 = 0.5;

    fn pick_error(g: &ViewGeometry, room: (f64, f64)) -> (f64, f64) {
        let screen = g.transform().room_to_viewport(PointF::new(room.0, room.1));
        let clicked = clicked_room_point(g, (screen.x, screen.y));
        ((clicked.0 - room.0).abs(), (clicked.1 - room.1).abs())
    }

    #[test]
    fn a_room_point_round_trips_through_the_viewport_at_every_zoom_dpr_and_viewport_shape() {
        for (room_w, room_h) in [(512.0, 384.0), (1000.0, 383.0)] {
            for (viewport_w, viewport_h) in VIEWPORTS {
                for zoom in ZOOMS {
                    for dpr in DPRS {
                        for native in [false, true] {
                            let g = geometry(
                                (room_w, room_h),
                                (viewport_w, viewport_h),
                                zoom,
                                native,
                                dpr,
                            );
                            let transform = g.transform();
                            for (fx, fy) in room_fractions() {
                                let room = (room_w * fx, room_h * fy);
                                let screen =
                                    transform.room_to_viewport(PointF::new(room.0, room.1));
                                let back =
                                    transform.viewport_to_room(PointF::new(screen.x, screen.y));
                                // viewport_to_room is room_to_viewport's algebraic
                                // inverse, so the only error is floating-point
                                // rounding (~1e-13 here), never a pixel.
                                assert!(
                                    (back.x - room.0).abs() <= EPS && (back.y - room.1).abs() <= EPS,
                                    "room {room:?} -> {screen:?} -> {back:?} \
                                     (viewport {viewport_w}x{viewport_h}, zoom {zoom}, dpr {dpr}, native {native})"
                                );
                                let (ex, ey) = pick_error(&g, room);
                                assert!(
                                    ex <= PICK_TOLERANCE && ey <= PICK_TOLERANCE,
                                    "the clicked cell must be the nearest cell to {room:?}, \
                                     off by ({ex}, {ey}) at {screen:?} \
                                     (viewport {viewport_w}x{viewport_h}, zoom {zoom}, dpr {dpr}, native {native})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// `dpr` sizes the frame buffer, so it must never move where a CSS click
    /// lands: if a device-pixel factor leaks into the inverse, the same click
    /// resolves a whole cell away at DPR 2.
    #[test]
    fn the_click_inverse_is_a_css_only_map_and_does_not_move_with_dpr() {
        let room = (512.0, 384.0);
        let viewport = (878.0, 412.0);
        for (sx, sy) in [
            (0.0, 0.0),
            (160.0, 90.0),
            (439.0, 206.0),
            (600.0, 300.0),
            viewport,
        ] {
            let reference =
                clicked_room_point(&geometry(room, viewport, 1.0, false, 1.0), (sx, sy));
            for dpr in [1.0, 1.5, 2.0] {
                let g = geometry(room, viewport, 1.0, false, dpr);
                assert_eq!(
                    clicked_room_point(&g, (sx, sy)),
                    reference,
                    "the CSS click ({sx},{sy}) must resolve to the same cell at dpr {dpr}"
                );
            }
        }
    }

    /// A device-pixel position fed to the CSS inverse must not pass for the CSS
    /// position it came from; the error is many cells, not float noise.
    #[test]
    fn a_device_pixel_position_is_not_a_css_viewport_position() {
        let room = (512.0, 384.0);
        let viewport = (878.0, 412.0);
        let dpr = 2.0;
        let g = geometry(room, viewport, 1.0, false, dpr);
        for (rx, ry) in [(260.0, 251.0), (255.0, 92.0), (250.0, 221.0)] {
            let css = g.transform().room_to_viewport(PointF::new(rx, ry));
            let device = (css.x * dpr, css.y * dpr);
            assert_eq!(
                clicked_room_point(&g, (css.x, css.y)),
                (rx.round(), ry.round()),
                "the CSS position of room ({rx},{ry}) resolves to that cell"
            );
            let via_device = clicked_room_point(&g, device);
            assert!(
                (via_device.0 - rx).abs() > 1.0 || (via_device.1 - ry).abs() > 1.0,
                "a device-pixel position ({device:?}) must not pass for the CSS one: \
                 room ({rx},{ry}) resolved to {via_device:?}"
            );
        }
    }

    #[test]
    fn a_missed_cell_click_returns_that_cell_not_a_neighbour() {
        for (viewport_w, viewport_h, dpr) in [
            (960.0, 540.0, 1.0),
            (1000.0, 700.0, 1.25),
            (200.0, 150.0, 2.0),
            (500.0, 800.0, 1.5),
        ] {
            let g = geometry((512.0, 384.0), (viewport_w, viewport_h), 1.0, false, dpr);
            let mut cell = -272;
            while cell <= 272 {
                for (rx, ry) in [
                    (cell, -272),
                    (cell, 272),
                    (-272, cell),
                    (272, cell),
                    (cell, cell),
                    (cell, -cell),
                ] {
                    let room = (f64::from(rx), f64::from(ry));
                    let (ex, ey) = pick_error(&g, room);
                    assert!(
                        ex <= PICK_TOLERANCE && ey <= PICK_TOLERANCE,
                        "cell {room:?} must survive the round trip, off by ({ex}, {ey})"
                    );
                }
                cell += 37;
            }
        }
    }

    #[test]
    fn a_click_in_the_letterbox_maps_outside_the_room_it_is_not_clamped_to_the_edge() {
        for (viewport_w, viewport_h) in [(600.0, 400.0), (1000.0, 700.0), (500.0, 800.0)] {
            for zoom in [1.0, 1.25] {
                let g = geometry((512.0, 384.0), (viewport_w, viewport_h), zoom, false, 1.0);
                let rect = g.transform().content_rect();
                let bars: [(f64, &str); 2] = [(rect.x, "left"), (rect.y, "top")];
                for (bar, edge) in bars {
                    if bar <= 0.0 {
                        continue;
                    }
                    let screen = if edge == "left" {
                        (bar / 2.0, rect.y + 5.0)
                    } else {
                        (rect.x + 5.0, bar / 2.0)
                    };
                    let (rx, ry) = clicked_room_point(&g, screen);
                    let outside = if edge == "left" { rx < 0.0 } else { ry < 0.0 };
                    assert!(
                        outside,
                        "a click {edge} of the room in the {edge} bar must not clamp to the edge"
                    );
                }

                // The visible-room corner is still in the room: the rejection
                // above must come from the letterbox, not from an off-by-one on
                // the content rect.
                let corner = (rect.x, rect.y);
                assert_eq!(
                    clicked_room_point(&g, corner),
                    (0.0, 0.0),
                    "the content-rect corner is room (0,0)"
                );
            }
        }
    }

    #[test]
    fn the_full_screen_room_buffer_screen_chain_composes_exactly() {
        for (room_w, room_h) in [(512.0, 384.0), (1000.0, 383.0)] {
            for (viewport_w, viewport_h) in VIEWPORTS {
                for zoom in ZOOMS {
                    for dpr in DPRS {
                        for native in [false, true] {
                            let g = geometry(
                                (room_w, room_h),
                                (viewport_w, viewport_h),
                                zoom,
                                native,
                                dpr,
                            );
                            let transform = g.transform();
                            let sizes = [
                                (transform.content_rect().x, transform.content_rect().y),
                                (viewport_w / 3.0, viewport_h / 3.0),
                                (viewport_w + 12.0, -9.0),
                            ];
                            for (sx, sy) in sizes {
                                let screen = PointF::new(sx, sy);
                                let direct = transform.viewport_to_buffer(screen);
                                let two_step =
                                    transform.room_to_buffer(transform.viewport_to_room(screen));
                                // Two routes to the same space, so they may only
                                // differ by float rounding, never by a DPR factor.
                                assert!(
                                    (direct.x - two_step.x).abs() <= EPS
                                        && (direct.y - two_step.y).abs() <= EPS,
                                    "viewport {screen:?} -> direct {direct:?} vs {two_step:?} \
                                     (viewport {viewport_w}x{viewport_h}, zoom {zoom}, dpr {dpr}, native {native})"
                                );

                                let back =
                                    transform.buffer_to_viewport(PointF::new(direct.x, direct.y));
                                assert!(
                                    (back.x - sx).abs() <= EPS && (back.y - sy).abs() <= EPS,
                                    "buffer -> viewport lost {screen:?}, got {back:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_letterboxed_room_rect_maps_to_the_frame_pixels_the_room_was_drawn_into() {
        // The frame buffer is the room at DPR with no letterboxing, so the visible
        // content rect's corners must land on the buffer corners: that is what
        // makes a click on a drawn pixel find the prop drawn there.
        for (viewport_w, viewport_h) in [(600.0, 400.0), (1000.0, 700.0), (500.0, 800.0)] {
            for dpr in [1.0, 1.25, 1.5, 2.0] {
                let g = geometry((512.0, 384.0), (viewport_w, viewport_h), 1.0, false, dpr);
                let rect = g.transform().content_rect();
                let top_left = g
                    .transform()
                    .viewport_to_buffer(PointF::new(rect.x, rect.y));
                let bottom_right = g
                    .transform()
                    .viewport_to_buffer(PointF::new(rect.x + rect.width, rect.y + rect.height));
                assert_eq!(
                    (top_left.x.round(), top_left.y.round()),
                    (0.0, 0.0),
                    "the content rect starts at the buffer's top-left"
                );
                assert_eq!(
                    (bottom_right.x.round(), bottom_right.y.round()),
                    (f64::from(g.bitmap_w), f64::from(g.bitmap_h)),
                    "the content rect ends at the buffer's bottom-right"
                );
            }
        }
    }

    #[test]
    fn a_fresh_store_has_no_layers_and_a_zero_base_version() {
        let store = FrameStore::new();
        assert_eq!(store.png(), None);
        assert_eq!(store.version(), 0);
        assert_eq!(store.mid_png(), None);
        assert_eq!(store.mid_version(), None);
        assert_eq!(store.top_png(), None);
        assert_eq!(store.top_version(), None);
    }

    #[test]
    fn mid_and_top_keep_independent_versions_and_bytes() {
        let store = FrameStore::new();
        store.put_mid(vec![1, 2, 3], 7);
        store.put_top(vec![4, 5, 6], 9);
        assert_eq!(store.mid_version(), Some(7));
        assert_eq!(store.mid_png(), Some(vec![1, 2, 3]));
        assert_eq!(store.top_version(), Some(9));
        assert_eq!(store.top_png(), Some(vec![4, 5, 6]));

        store.put_mid(vec![8, 9], 10);
        assert_eq!(store.mid_version(), Some(10));
        assert_eq!(store.mid_png(), Some(vec![8, 9]));
        assert_eq!(
            store.top_version(),
            Some(9),
            "top is untouched by a mid put"
        );
        assert_eq!(store.top_png(), Some(vec![4, 5, 6]));
    }

    #[test]
    fn an_empty_layer_bitmap_clears_that_layer_only() {
        let store = FrameStore::new();
        store.put_mid(vec![1], 1);
        store.put_top(vec![2], 2);
        store.put_mid(Vec::new(), 3);
        assert_eq!(store.mid_version(), None);
        assert_eq!(store.mid_png(), None);
        assert_eq!(store.top_version(), Some(2));
        assert_eq!(store.top_png(), Some(vec![2]));
    }

    #[test]
    fn putting_the_base_does_not_disturb_mid_or_top() {
        let store = FrameStore::new();
        store.put_mid(vec![1, 2, 3], 5);
        store.put_top(vec![4, 5, 6], 6);
        let base_version = store.put(vec![7, 8, 9]);
        assert_eq!(base_version, 1);
        assert_eq!(store.version(), 1);
        assert_eq!(store.png(), Some(vec![7, 8, 9]));
        assert_eq!(store.mid_version(), Some(5));
        assert_eq!(store.mid_png(), Some(vec![1, 2, 3]));
        assert_eq!(store.top_version(), Some(6));
        assert_eq!(store.top_png(), Some(vec![4, 5, 6]));

        store.put_mid(vec![10], 11);
        store.put_top(vec![12], 13);
        assert_eq!(
            store.version(),
            1,
            "mid/top puts leave the base version alone"
        );
        assert_eq!(store.png(), Some(vec![7, 8, 9]));
    }

    #[test]
    fn an_identical_base_keeps_its_version() {
        let store = FrameStore::new();
        let first = store.put(vec![1, 2, 3]);
        let second = store.put(vec![1, 2, 3]);
        assert_eq!(
            second, first,
            "byte-identical bytes must not advance the version, or the display re-fetches the base"
        );
        assert_eq!(store.version(), first);
        assert_eq!(store.png(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn changed_base_bytes_advance_the_version_and_store_the_latest() {
        let store = FrameStore::new();
        let first = store.put(vec![1, 2, 3]);
        let second = store.put(vec![4, 5]);
        assert_eq!(
            second,
            first + 1,
            "different bytes must advance the version"
        );
        assert_eq!(store.version(), second);
        assert_eq!(
            store.png(),
            Some(vec![4, 5]),
            "the store holds the latest bytes"
        );

        let third = store.put(vec![4, 5]);
        assert_eq!(
            third, second,
            "a repeat of the latest bytes keeps the version stable again"
        );
        assert_eq!(store.png(), Some(vec![4, 5]));
    }
}

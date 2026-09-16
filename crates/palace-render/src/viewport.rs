//! Room ↔ viewport coordinate mapping.
//!
//! This is the module that stops clicks landing on the wrong hotspot, so it is
//! deliberately small, pure and exhaustively tested.
//!
//! ## Three coordinate spaces
//!
//! ```text
//!   room      logical Palace pixels. Hotspots, overlays, loose props and
//!             avatars all live here. Absolute (signed) pixels in the
//!             background image's own coordinate space.
//!
//!   viewport  CSS / layout pixels. This is what mouse events arrive in.
//!
//!   buffer    device pixels of the compositor's offscreen frame,
//!             floor(room * dpr) with dpr = clamp(devicePixelRatio, 1, 2).
//! ```
//!
//! Two projections, both implemented here:
//!
//! * **room ↔ viewport** — scale-to-fit (or 1:1) with a letterbox/pillarbox
//!   offset and a zoom multiplier. This is the input path.
//! * **room ↔ buffer** — a plain multiply by the effective DPR. This is the
//!   render path (see [`crate::canvas`]).
//!
//! The composition `viewport → buffer` is also exposed, so a caller holding a
//! device-pixel position can get back to a room coordinate without knowing which
//! intermediate space it came from.
//!
//! ## The maths
//!
//! ```text
//!   fit    = min(viewport.w / room.w, viewport.h / room.h)
//!   scale  = fit * zoom                     (ScaleMode::Fit)
//!   scale  = 1                              (ScaleMode::Native)
//!   content = (room.w * scale, room.h * scale)
//!   offset  = ((viewport.w - content.w) / 2, (viewport.h - content.h) / 2)
//!
//!   room → viewport :  p * scale + offset
//!   viewport → room :  (p - offset) / scale
//!
//!   room → buffer   :  p * dpr
//!   buffer → room   :  p / dpr
//! ```
//!
//! `Native` ignores zoom by design: the 1:1 toggle means exactly one room pixel
//! per viewport pixel, whatever the zoom slider says.
//!
//! ## What is *not* here
//!
//! Snapping the presentation transform to whole device pixels happens in the
//! presentation layer, not here — this module stays in floating point so the
//! inverse is exact to the last bit and the round-trip tests can demand
//! identity `<= 1e-9` rather than "about a pixel".

/// The protocol-level floor for a room's logical size. The real size comes from
/// the background image; a smaller background is padded up to this.
pub const MIN_ROOM_WIDTH: f64 = 512.0;
/// See [`MIN_ROOM_WIDTH`].
pub const MIN_ROOM_HEIGHT: f64 = 384.0;

/// Lowest user zoom.
pub const MIN_ZOOM: f64 = 0.5;
/// Highest user zoom.
pub const MAX_ZOOM: f64 = 3.0;

/// Lowest effective device-pixel ratio (a standard 1× display).
pub const MIN_DPR: f64 = 1.0;
/// Highest effective device-pixel ratio. The modern client caps at 2×; it is
/// cheap and sufficient, and it keeps the frame buffer bounded.
pub const MAX_DPR: f64 = 2.0;

/// How the room is scaled into the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// Scale to fit the viewport, letterboxing the short axis, then multiply by
    /// [`ViewTransform::zoom`]. The default.
    #[default]
    Fit,
    /// Native 1:1 — one room pixel per viewport pixel. Ignores zoom.
    Native,
}

/// A width/height pair in floating-point pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SizeF {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl SizeF {
    /// Construct a size.
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        SizeF { width, height }
    }
}

/// A position in floating-point pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointF {
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
}

impl PointF {
    /// Construct a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        PointF { x, y }
    }
}

/// An axis-aligned rectangle in floating-point pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RectF {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl RectF {
    /// Construct a rectangle.
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        RectF {
            x,
            y,
            width,
            height,
        }
    }
}

/// Clamp `dpr` into the supported `[1, 2]` range.
#[must_use]
pub fn clamp_dpr(dpr: f64) -> f64 {
    if !dpr.is_finite() {
        return MIN_DPR;
    }
    dpr.clamp(MIN_DPR, MAX_DPR)
}

/// Clamp `zoom` into the supported `[0.5, 3]` range.
#[must_use]
pub fn clamp_zoom(zoom: f64) -> f64 {
    if !zoom.is_finite() {
        return 1.0;
    }
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The complete room→viewport mapping for one frame.
///
/// Construct with [`ViewTransform::new`]; every field is clamped there, so the
/// type is always in a valid state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    room: SizeF,
    viewport: SizeF,
    zoom: f64,
    mode: ScaleMode,
    /// The **raw** device-pixel ratio, kept for reporting.
    device_ratio: f64,
}

impl ViewTransform {
    /// Build a transform. `zoom` is clamped to `[0.5, 3]` and `device_ratio` to
    /// `[1, 2]`.
    ///
    /// A non-positive room or viewport extent is treated as `1.0` so the scale
    /// never becomes infinite or NaN; a caller with no layout yet still gets a
    /// usable transform.
    #[must_use]
    pub fn new(
        room: SizeF,
        viewport: SizeF,
        zoom: f64,
        mode: ScaleMode,
        device_ratio: f64,
    ) -> Self {
        let sanitize = |v: f64| if v.is_finite() && v > 0.0 { v } else { 1.0 };
        ViewTransform {
            room: SizeF::new(sanitize(room.width), sanitize(room.height)),
            viewport: SizeF::new(sanitize(viewport.width), sanitize(viewport.height)),
            zoom: clamp_zoom(zoom),
            mode,
            device_ratio: clamp_dpr(device_ratio),
        }
    }

    /// The logical room size in room pixels.
    #[must_use]
    pub fn room(&self) -> SizeF {
        self.room
    }

    /// The viewport size in CSS pixels.
    #[must_use]
    pub fn viewport(&self) -> SizeF {
        self.viewport
    }

    /// The clamped zoom factor.
    #[must_use]
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// The scale mode.
    #[must_use]
    pub fn mode(&self) -> ScaleMode {
        self.mode
    }

    /// The effective device-pixel ratio actually used for the frame buffer
    /// (`clamp(devicePixelRatio, 1, 2)`).
    #[must_use]
    pub fn dpr(&self) -> f64 {
        self.device_ratio
    }

    /// Viewport pixels per room pixel.
    ///
    /// `Native` is exactly `1.0`; `Fit` is the letterbox scale times the zoom.
    #[must_use]
    pub fn scale(&self) -> f64 {
        match self.mode {
            ScaleMode::Native => 1.0,
            ScaleMode::Fit => {
                let fit = (self.viewport.width / self.room.width)
                    .min(self.viewport.height / self.room.height);
                fit * self.zoom
            }
        }
    }

    /// The room's on-screen size in CSS pixels (`room * scale`).
    #[must_use]
    pub fn content_size(&self) -> SizeF {
        let scale = self.scale();
        SizeF::new(self.room.width * scale, self.room.height * scale)
    }

    /// The letterbox/pillarboxed content rectangle in CSS pixels.
    ///
    /// `x`/`y` are the offsets of the room's top-left corner inside the
    /// viewport; they are positive when the room is smaller than the viewport on
    /// that axis and negative when it overflows (zoom in).
    #[must_use]
    pub fn content_rect(&self) -> RectF {
        let content = self.content_size();
        RectF::new(
            (self.viewport.width - content.width) / 2.0,
            (self.viewport.height - content.height) / 2.0,
            content.width,
            content.height,
        )
    }

    /// Forward: room pixels → CSS viewport pixels.
    #[must_use]
    pub fn room_to_viewport(&self, p: PointF) -> PointF {
        let scale = self.scale();
        let rect = self.content_rect();
        PointF::new(rect.x + p.x * scale, rect.y + p.y * scale)
    }

    /// Inverse: CSS viewport pixels → room pixels.
    ///
    /// This is the mouse-input path and the one the round-trip tests hammer.
    #[must_use]
    pub fn viewport_to_room(&self, p: PointF) -> PointF {
        let scale = self.scale();
        let rect = self.content_rect();
        PointF::new((p.x - rect.x) / scale, (p.y - rect.y) / scale)
    }

    /// Forward: room pixels → frame-buffer device pixels.
    #[must_use]
    pub fn room_to_buffer(&self, p: PointF) -> PointF {
        PointF::new(p.x * self.dpr(), p.y * self.dpr())
    }

    /// Inverse: frame-buffer device pixels → room pixels.
    #[must_use]
    pub fn buffer_to_room(&self, p: PointF) -> PointF {
        PointF::new(p.x / self.dpr(), p.y / self.dpr())
    }

    /// CSS viewport pixels → frame-buffer device pixels.
    ///
    /// The buffer is the room at DPR *without* letterboxing, so this composes
    /// the inverse of the fit transform with the DPR multiply.
    #[must_use]
    pub fn viewport_to_buffer(&self, p: PointF) -> PointF {
        self.room_to_buffer(self.viewport_to_room(p))
    }

    /// Frame-buffer device pixels → CSS viewport pixels.
    #[must_use]
    pub fn buffer_to_viewport(&self, p: PointF) -> PointF {
        self.room_to_viewport(self.buffer_to_room(p))
    }

    /// Frame-buffer dimensions: `floor(room * dpr)` on each axis.
    ///
    /// The floor mirrors the scope's `floor(roomW × dpr) × floor(roomH × dpr)`;
    /// at least 1×1 so an empty canvas is never constructed.
    #[must_use]
    pub fn buffer_size(&self) -> (u32, u32) {
        let w = (self.room.width * self.dpr()).floor().max(1.0);
        let h = (self.room.height * self.dpr()).floor().max(1.0);
        (w as u32, h as u32)
    }
}

/// The logical room size for a background of `(width, height)`.
///
/// **The single most important rule in this crate:** a room is *not* always
/// 512×384. Its logical size is the background's intrinsic size, floored at
/// 512×384. A smaller background pads; a larger one is a larger room.
#[must_use]
pub fn room_size_from_background(width: u32, height: u32) -> SizeF {
    SizeF::new(
        (width as f64).max(MIN_ROOM_WIDTH),
        (height as f64).max(MIN_ROOM_HEIGHT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn assert_close(a: f64, b: f64, label: &str) {
        assert!(
            (a - b).abs() <= EPS,
            "{label}: {a} vs {b} (delta {})",
            (a - b).abs()
        );
    }

    /// Every viewport size, aspect ratio, zoom and DPR combination the round-trip
    /// tests sweep.
    fn viewports() -> Vec<SizeF> {
        vec![
            SizeF::new(512.0, 384.0),   // exact native
            SizeF::new(1024.0, 768.0),  // 2x, same aspect
            SizeF::new(800.0, 600.0),   // same aspect, different scale
            SizeF::new(1000.0, 700.0),  // wider -> pillarbox
            SizeF::new(640.0, 700.0),   // taller -> letterbox
            SizeF::new(1281.0, 721.0),  // awkward non-integer
            SizeF::new(1920.0, 1080.0), // 16:9
            SizeF::new(3840.0, 2160.0), // 4K
            SizeF::new(320.0, 240.0),   // tiny
            SizeF::new(1.0, 1.0),       // degenerate but legal
        ]
    }

    fn rooms() -> Vec<SizeF> {
        vec![
            SizeF::new(512.0, 384.0),
            SizeF::new(640.0, 480.0),
            SizeF::new(1024.0, 768.0),
            SizeF::new(1000.0, 383.0), // wider than tall
            SizeF::new(500.0, 500.0),  // square
        ]
    }

    fn zooms() -> [f64; 6] {
        [0.5, 0.75, 1.0, 1.25, 2.0, 3.0]
    }

    fn dprs() -> [f64; 5] {
        [1.0, 1.25, 1.5, 1.75, 2.0]
    }

    #[test]
    fn room_size_is_derived_from_the_background_with_a_floor() {
        assert_eq!(
            room_size_from_background(512, 384),
            SizeF::new(512.0, 384.0)
        );
        // Smaller background pads up to the floor.
        assert_eq!(
            room_size_from_background(320, 240),
            SizeF::new(512.0, 384.0)
        );
        assert_eq!(
            room_size_from_background(100, 100),
            SizeF::new(512.0, 384.0)
        );
        // Larger background wins, per axis independently.
        assert_eq!(
            room_size_from_background(1200, 400),
            SizeF::new(1200.0, 400.0)
        );
        assert_eq!(
            room_size_from_background(400, 900),
            SizeF::new(512.0, 900.0)
        );
    }

    #[test]
    fn native_mode_is_one_to_one_regardless_of_zoom() {
        for zoom in zooms() {
            let t = ViewTransform::new(
                SizeF::new(512.0, 384.0),
                SizeF::new(1920.0, 1080.0),
                zoom,
                ScaleMode::Native,
                1.0,
            );
            assert_close(t.scale(), 1.0, "native scale ignores zoom");
        }
    }

    #[test]
    fn fit_scale_is_the_letterbox_scale_times_zoom() {
        // Room 512x384 (4:3) in a 1280x720 viewport (16:9): height is the limit.
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(1280.0, 720.0),
            1.0,
            ScaleMode::Fit,
            1.0,
        );
        // 720/384 = 1.875 < 1280/512 = 2.5
        assert_close(t.scale(), 1.875, "fit scale");
        let rect = t.content_rect();
        assert_close(rect.height, 720.0, "content height fills viewport");
        assert_close(rect.width, 960.0, "content width");
        assert_close(rect.x, 160.0, "pillarbox left");
        assert_close(rect.y, 0.0, "no vertical offset");
    }

    #[test]
    fn round_trip_room_viewport_room_is_identity() {
        for room in rooms() {
            for viewport in viewports() {
                for zoom in zooms() {
                    for dpr in dprs() {
                        for mode in [ScaleMode::Fit, ScaleMode::Native] {
                            let t = ViewTransform::new(room, viewport, zoom, mode, dpr);
                            for (rx, ry) in [
                                (0.0, 0.0),
                                (0.5, 0.5),
                                (room.width - 1.0, room.height - 1.0),
                                (room.width / 2.0, room.height / 3.0),
                                (-12.0, -7.0),
                                (room.width + 40.0, room.height + 40.0),
                            ] {
                                let room_pt = PointF::new(rx, ry);
                                let vp = t.room_to_viewport(room_pt);
                                let back = t.viewport_to_room(vp);
                                assert_close(back.x, room_pt.x, "rt x");
                                assert_close(back.y, room_pt.y, "rt y");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn round_trip_viewport_room_viewport_is_identity_including_offsets() {
        for room in rooms() {
            for viewport in viewports() {
                for zoom in zooms() {
                    for mode in [ScaleMode::Fit, ScaleMode::Native] {
                        let t = ViewTransform::new(room, viewport, zoom, mode, 2.0);
                        for (vx, vy) in [
                            (0.0, 0.0),
                            (1.0, 1.0),
                            (viewport.width, viewport.height),
                            (viewport.width / 2.0, viewport.height / 2.0),
                            (viewport.width + 33.0, -17.0),
                        ] {
                            let vp = PointF::new(vx, vy);
                            let room_pt = t.viewport_to_room(vp);
                            let back = t.room_to_viewport(room_pt);
                            assert_close(back.x, vp.x, "rt x");
                            assert_close(back.y, vp.y, "rt y");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn round_trip_room_buffer_room_is_identity() {
        for room in rooms() {
            for dpr in dprs() {
                let t =
                    ViewTransform::new(room, SizeF::new(800.0, 600.0), 1.0, ScaleMode::Fit, dpr);
                for (rx, ry) in [
                    (0.0, 0.0),
                    (1.0, 2.0),
                    (room.width, room.height),
                    (13.5, 9.25),
                ] {
                    let room_pt = PointF::new(rx, ry);
                    let buf = t.room_to_buffer(room_pt);
                    let back = t.buffer_to_room(buf);
                    assert_close(back.x, room_pt.x, "rt x");
                    assert_close(back.y, room_pt.y, "rt y");
                }
            }
        }
    }

    #[test]
    fn round_trip_buffer_viewport_buffer_is_identity() {
        for room in rooms() {
            for viewport in viewports() {
                for dpr in dprs() {
                    for mode in [ScaleMode::Fit, ScaleMode::Native] {
                        let t = ViewTransform::new(room, viewport, 1.0, mode, dpr);
                        for (bx, by) in [(0.0, 0.0), (5.0, 5.0), (123.75, 88.5)] {
                            let buf = PointF::new(bx, by);
                            let vp = t.buffer_to_viewport(buf);
                            let back = t.viewport_to_buffer(vp);
                            assert_close(back.x, buf.x, "rt x");
                            assert_close(back.y, buf.y, "rt y");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn viewport_to_room_maps_the_content_rect_corners_to_the_room_corners() {
        // The classic off-by-letterbox bug: the top-left of the visible room is
        // NOT the top-left of the viewport when the aspect ratios differ.
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(1000.0, 700.0),
            1.0,
            ScaleMode::Fit,
            1.0,
        );
        let rect = t.content_rect();
        // 1000/512 = 1.953, 700/384 = 1.823 -> height-limited; the leftover width
        // is split evenly as pillarbox.
        assert_close(rect.x, (1000.0 - 512.0 * (700.0 / 384.0)) / 2.0, "offset x");
        let top_left = t.viewport_to_room(PointF::new(rect.x, rect.y));
        assert_close(top_left.x, 0.0, "room origin x");
        assert_close(top_left.y, 0.0, "room origin y");

        let bottom_right =
            t.viewport_to_room(PointF::new(rect.x + rect.width, rect.y + rect.height));
        assert_close(bottom_right.x, 512.0, "room width x");
        assert_close(bottom_right.y, 384.0, "room height y");

        // A click 10px inside the letterbox must be a *negative* room coordinate,
        // and a click at viewport (0,0) must be outside the room.
        let outside = t.viewport_to_room(PointF::new(0.0, 0.0));
        assert!(outside.x < 0.0, "left of the room is negative x");
    }

    #[test]
    fn buffer_size_is_floor_room_times_dpr() {
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(800.0, 600.0),
            1.0,
            ScaleMode::Fit,
            2.0,
        );
        assert_eq!(t.buffer_size(), (1024, 768));

        // Non-integer DPR floors, per the scope.
        let t = ViewTransform::new(
            SizeF::new(1000.0, 383.0),
            SizeF::new(800.0, 600.0),
            1.0,
            ScaleMode::Fit,
            1.5,
        );
        assert_eq!(t.buffer_size(), (1500, 574)); // 383 * 1.5 = 574.5 -> 574
    }

    #[test]
    fn dpr_and_zoom_are_clamped_to_the_supported_range() {
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(800.0, 600.0),
            9.0,
            ScaleMode::Fit,
            4.0,
        );
        assert_close(t.zoom(), MAX_ZOOM, "zoom high clamp");
        assert_close(t.dpr(), MAX_DPR, "dpr high clamp");

        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(800.0, 600.0),
            0.01,
            ScaleMode::Fit,
            0.25,
        );
        assert_close(t.zoom(), MIN_ZOOM, "zoom low clamp");
        assert_close(t.dpr(), MIN_DPR, "dpr low clamp");

        // NaN / infinity must not poison the transform.
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(800.0, 600.0),
            f64::NAN,
            ScaleMode::Fit,
            f64::INFINITY,
        );
        assert!(t.zoom().is_finite() && t.dpr().is_finite());
        assert!(t.scale().is_finite());
    }

    #[test]
    fn degenerate_extents_do_not_produce_nan() {
        let t = ViewTransform::new(
            SizeF::new(0.0, 0.0),
            SizeF::new(0.0, 0.0),
            1.0,
            ScaleMode::Fit,
            1.0,
        );
        assert!(t.scale().is_finite() && t.scale() > 0.0);
        let p = t.room_to_viewport(PointF::new(4.0, 4.0));
        assert!(p.x.is_finite() && p.y.is_finite());
        assert!(t.buffer_size().0 >= 1 && t.buffer_size().1 >= 1);
    }

    #[test]
    fn zoom_scales_about_the_viewport_centre() {
        // Zooming in with fit mode must keep the room centre pinned to the
        // viewport centre.
        let room = SizeF::new(512.0, 384.0);
        let viewport = SizeF::new(1000.0, 700.0);
        let centre_room = PointF::new(room.width / 2.0, room.height / 2.0);
        for zoom in zooms() {
            let t = ViewTransform::new(room, viewport, zoom, ScaleMode::Fit, 1.0);
            let p = t.room_to_viewport(centre_room);
            assert_close(p.x, viewport.width / 2.0, "centre x");
            assert_close(p.y, viewport.height / 2.0, "centre y");
        }
    }

    #[test]
    fn a_room_that_fits_exactly_has_zero_offset() {
        let t = ViewTransform::new(
            SizeF::new(512.0, 384.0),
            SizeF::new(512.0, 384.0),
            1.0,
            ScaleMode::Fit,
            1.0,
        );
        let rect = t.content_rect();
        assert_close(rect.x, 0.0, "x");
        assert_close(rect.y, 0.0, "y");
        assert_close(t.scale(), 1.0, "scale");
    }
}

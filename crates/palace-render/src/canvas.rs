//! The frame buffer and its blitter.
//!
//! The canvas is the one place the DPR transform is applied. Everything the
//! compositor draws is expressed in **logical room pixels**; the canvas stores
//! `floor(room × dpr) × floor(room × dpr)` device pixels and replicates each
//! logical pixel across its device footprint.
//!
//! That is the whole DPR story: the *logic* never sees device units, and no
//! resampling of sprite content happens here. Presentation-time filtering
//! (nearest for sprites, smooth for photographic backgrounds) is somebody else's
//! job — see the scope's *Rendering* section. The canvas only ever replicates,
//! so a 1× buffer is a byte-exact copy of the logical frame.

use std::path::Path;

use palace_prop::PropImage;

use crate::error::RenderError;
use crate::viewport::{clamp_dpr, RectF};

/// An RGBA8 frame buffer whose coordinate system is logical room pixels.
///
/// Construct with [`Canvas::for_room`]; all drawing methods take logical
/// coordinates and never panic, whatever they are handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u32,
    height: u32,
    dpr: Dpr,
    pixels: Vec<u8>,
}

/// `[1, 2]`-clamped device-pixel ratio, stored as a fixed-point fraction so the
/// type derives `Eq` and hashes deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Dpr(u32);

impl Dpr {
    fn from_f64(value: f64) -> Self {
        Dpr((clamp_dpr(value) * 1000.0).round() as u32)
    }

    fn as_f64(self) -> f64 {
        self.0 as f64 / 1000.0
    }
}

/// Deflate mode for the frame PNG.
///
/// The frame is re-encoded on every click and served to a webview over the
/// in-process `palace://` scheme, so encoding latency dominates the compose path
/// while the transfer is essentially free. Stored (uncompressed) deflate trades
/// a larger frame for a much shorter encode: in a debug build the fastest deflate
/// still costs ~100 ms per 1024x768 frame, where storing is a copy.
const FRAME_COMPRESSION: png::Compression = png::Compression::NoCompression;

/// Row filter for the frame PNG. Filtering only helps compression, which is
/// disabled, so it would be pure per-pixel work.
const FRAME_FILTER: png::Filter = png::Filter::NoFilter;

/// Apply the frame-PNG encoder settings, shared by [`Canvas::to_png_bytes`] and
/// [`Canvas::write_png`] so the in-memory and file paths can never drift apart.
fn configure_encoder<W: std::io::Write>(encoder: &mut png::Encoder<'_, W>) {
    encoder.set_compression(FRAME_COMPRESSION);
    encoder.set_filter(FRAME_FILTER);
}

impl Canvas {
    /// Allocate a frame buffer for a room of `room_width × room_height` logical
    /// pixels at the given device-pixel ratio.
    ///
    /// The buffer is `floor(room × clamp(dpr, 1, 2))` on each axis, at least
    /// 1×1. The initial contents are transparent black; call [`Canvas::fill`]
    /// first for an opaque frame.
    #[must_use]
    pub fn for_room(room_width: f64, room_height: f64, dpr: f64) -> Self {
        let dpr = Dpr::from_f64(dpr);
        let scale = dpr.as_f64();
        let width = (room_width * scale).floor().max(1.0) as u32;
        let height = (room_height * scale).floor().max(1.0) as u32;
        let len = (width as usize) * (height as usize) * 4;
        Canvas {
            width,
            height,
            dpr,
            pixels: vec![0; len],
        }
    }

    /// Buffer width in device pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Buffer height in device pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The device-pixel ratio this canvas was built with.
    #[must_use]
    pub fn dpr(&self) -> f64 {
        self.dpr.as_f64()
    }

    /// The buffer in logical room units.
    #[must_use]
    pub fn logical_size(&self) -> (f64, f64) {
        (
            self.width as f64 / self.dpr.as_f64(),
            self.height as f64 / self.dpr.as_f64(),
        )
    }

    /// The raw RGBA bytes, `width * height * 4`.
    #[must_use]
    pub fn as_rgba(&self) -> &[u8] {
        &self.pixels
    }

    /// Replace every pixel with `rgba`.
    pub fn fill(&mut self, rgba: [u8; 4]) {
        for chunk in self.pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&rgba);
        }
    }

    /// Paint an axis-aligned rectangle given in **logical** coordinates.
    ///
    /// Used for the flat backdrop (no background image) and for placeholder
    /// boxes. Edges are rounded to whole device pixels.
    pub fn fill_rect(&mut self, rect: RectF, rgba: [u8; 4]) {
        let dpr = self.dpr.as_f64();
        let x0 = (rect.x * dpr).floor().max(0.0) as i64;
        let y0 = (rect.y * dpr).floor().max(0.0) as i64;
        let x1 = ((rect.x + rect.width) * dpr).ceil().min(self.width as f64) as i64;
        let y1 = ((rect.y + rect.height) * dpr)
            .ceil()
            .min(self.height as f64) as i64;
        for y in y0..y1.max(y0) {
            for x in x0..x1.max(x0) {
                self.write_device(x, y, rgba, 1.0);
            }
        }
    }

    /// Paint one logical pixel with source-over compositing.
    ///
    /// The logical pixel expands to its `dpr × dpr` device footprint exactly as
    /// a one-pixel [`Canvas::blit`] would, so draw commands stay expressed in
    /// room pixels. A fully transparent `rgba` is a no-op.
    pub fn paint_pixel(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        if rgba[3] == 0 {
            return;
        }
        let dpr = self.dpr.as_f64();
        let dx0 = (f64::from(x) * dpr).floor() as i64;
        let dy0 = (f64::from(y) * dpr).floor() as i64;
        let dx1 = ((f64::from(x) + 1.0) * dpr).floor() as i64;
        let dy1 = ((f64::from(y) + 1.0) * dpr).floor() as i64;
        for dy in dy0..dy1.max(dy0) {
            for dx in dx0..dx1.max(dx0) {
                self.write_device(dx, dy, rgba, 1.0);
            }
        }
    }

    /// Draw an RGBA image with its top-left at logical `(x, y)`.
    ///
    /// The image keeps its intrinsic pixel size: one source pixel becomes one
    /// logical pixel, which the buffer expands to its `dpr × dpr` device
    /// footprint. `alpha` scales the source's own alpha (0..1); ghost props pass
    /// `0.5`.
    ///
    /// Source pixels outside the buffer are clipped, not wrapped, so a sprite
    /// half off the edge draws correctly.
    pub fn blit(&mut self, image: &PropImage, x: f64, y: f64, alpha: f64) {
        if alpha <= 0.0 || image.width() == 0 || image.height() == 0 {
            return;
        }
        let alpha = alpha.min(1.0);
        let dpr = self.dpr.as_f64();
        let src_w = image.width() as i64;
        let src_h = image.height() as i64;
        let src_rgba = image.as_rgba();
        let stride = (src_w as usize) * 4;

        for sy in 0..src_h {
            let ly = y + sy as f64;
            let dy0 = (ly * dpr).floor() as i64;
            let dy1 = ((ly + 1.0) * dpr).floor() as i64;
            if dy1 <= 0 || dy0 >= self.height as i64 {
                continue;
            }
            let dy0c = dy0.max(0);
            let dy1c = dy1.min(self.height as i64);
            for sx in 0..src_w {
                let at = (sy as usize) * stride + (sx as usize) * 4;
                let Some(pixel) = src_rgba.get(at..at + 4) else {
                    continue;
                };
                let src = [pixel[0], pixel[1], pixel[2], pixel[3]];
                if src[3] == 0 {
                    continue;
                }
                let lx = x + sx as f64;
                let dx0 = (lx * dpr).floor() as i64;
                let dx1 = ((lx + 1.0) * dpr).floor() as i64;
                if dx1 <= 0 || dx0 >= self.width as i64 {
                    continue;
                }
                let dx0c = dx0.max(0);
                let dx1c = dx1.min(self.width as i64);
                for dy in dy0c..dy1c {
                    for dx in dx0c..dx1c {
                        self.write_device(dx, dy, src, alpha);
                    }
                }
            }
        }
    }

    /// Write a device pixel with source-over compositing.
    fn write_device(&mut self, x: i64, y: i64, src: [u8; 4], alpha: f64) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let at = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let Some(dst) = self.pixels.get_mut(at..at + 4) else {
            return;
        };
        let sa = (f64::from(src[3]) / 255.0) * alpha;
        if sa >= 1.0 {
            dst.copy_from_slice(&src);
            return;
        }
        if sa <= 0.0 {
            return;
        }
        let da = f64::from(dst[3]) / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            dst.copy_from_slice(&[0, 0, 0, 0]);
            return;
        }
        for channel in 0..3 {
            let value =
                (f64::from(src[channel]) * sa + f64::from(dst[channel]) * da * (1.0 - sa)) / out_a;
            dst[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
        dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    /// Multiply every pixel's alpha by `factor` — the room-dimming pass.
    ///
    /// `dim_level` is the Palace `DIMROOM` value normalised to `0..=1`, where
    /// `1.0` is "not dimmed". The reference multiplies room colour by the level;
    /// this does it as an extra black layer with `1 - level` alpha, which is the
    /// same visible result for an opaque frame and needs no colour maths.
    pub fn apply_dim(&mut self, level: f64) {
        let level = level.clamp(0.0, 1.0);
        if level >= 1.0 {
            return;
        }
        for chunk in self.pixels.chunks_exact_mut(4) {
            chunk[0] = (f64::from(chunk[0]) * level).round() as u8;
            chunk[1] = (f64::from(chunk[1]) * level).round() as u8;
            chunk[2] = (f64::from(chunk[2]) * level).round() as u8;
            // Alpha is left untouched: dimming must not punch holes in the frame.
        }
    }

    /// Encode the buffer as an 8-bit RGBA PNG and return the bytes.
    ///
    /// In-memory twin of [`Canvas::write_png`], for clients that hand a frame to
    /// a browser instead of a file.
    pub fn to_png_bytes(&self) -> Result<Vec<u8>, RenderError> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        configure_encoder(&mut encoder);
        let mut writer = encoder
            .write_header()
            .map_err(|e| RenderError::Png(e.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| RenderError::Png(e.to_string()))?;
        drop(writer);
        Ok(out)
    }

    /// Encode the buffer as an 8-bit RGBA PNG.
    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<(), RenderError> {
        let file = std::fs::File::create(path).map_err(RenderError::Io)?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        configure_encoder(&mut encoder);
        let mut writer = encoder
            .write_header()
            .map_err(|e| RenderError::Png(e.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| RenderError::Png(e.to_string()))?;
        Ok(())
    }

    /// The pixel at a device coordinate, or `None` when out of bounds. Test-only
    /// convenience.
    #[cfg(test)]
    #[must_use]
    fn device_pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.pixels
            .get(at..at + 4)
            .map(|s| [s[0], s[1], s[2], s[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> PropImage {
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            bytes.extend_from_slice(&rgba);
        }
        PropImage::from_rgba(w, h, bytes).expect("valid image")
    }

    #[test]
    fn buffer_is_floor_room_times_dpr_and_starts_transparent() {
        let c = Canvas::for_room(512.0, 384.0, 2.0);
        assert_eq!((c.width(), c.height()), (1024, 768));
        assert_eq!(c.dpr(), 2.0);
        assert!(c.as_rgba().iter().all(|&b| b == 0));
        assert_eq!(c.logical_size(), (512.0, 384.0));
    }

    #[test]
    fn dpr_at_one_is_a_byte_exact_copy() {
        let img = solid(3, 2, [10, 20, 30, 255]);
        let mut c = Canvas::for_room(4.0, 4.0, 1.0);
        c.blit(&img, 1.0, 1.0, 1.0);
        assert_eq!(c.device_pixel(1, 1), Some([10, 20, 30, 255]));
        assert_eq!(c.device_pixel(3, 2), Some([10, 20, 30, 255]));
        assert_eq!(c.device_pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(c.device_pixel(4, 4), None); // out of bounds
    }

    #[test]
    fn dpr_two_replicates_each_logical_pixel_into_a_two_by_two_block() {
        let img = solid(1, 1, [200, 100, 50, 255]);
        let mut c = Canvas::for_room(4.0, 4.0, 2.0);
        c.blit(&img, 1.0, 1.0, 1.0);
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            assert_eq!(
                c.device_pixel(x, y),
                Some([200, 100, 50, 255]),
                "device {x},{y}"
            );
        }
        assert_eq!(c.device_pixel(1, 1), Some([0, 0, 0, 0]));
        assert_eq!(c.device_pixel(4, 4), Some([0, 0, 0, 0]));
    }

    #[test]
    fn paint_pixel_expands_to_the_dpr_footprint_and_clips_offscreen() {
        let mut c = Canvas::for_room(4.0, 4.0, 2.0);
        c.paint_pixel(1, 1, [200, 100, 50, 255]);
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            assert_eq!(
                c.device_pixel(x, y),
                Some([200, 100, 50, 255]),
                "device {x},{y}"
            );
        }
        assert_eq!(c.device_pixel(1, 1), Some([0, 0, 0, 0]));
        // A transparent source and an off-buffer coordinate are both no-ops.
        c.paint_pixel(0, 0, [1, 2, 3, 0]);
        assert_eq!(c.device_pixel(0, 0), Some([0, 0, 0, 0]));
        c.paint_pixel(-5, -5, [1, 2, 3, 255]);
        c.paint_pixel(99, 99, [1, 2, 3, 255]);
    }

    #[test]
    fn fractional_dpr_covers_every_logical_pixel_without_gaps() {
        // 1.5x on a 3-wide logical room must produce a 4-device-wide buffer and
        // every logical column 0..3 must be covered.
        let mut c = Canvas::for_room(3.0, 1.0, 1.5);
        assert_eq!(c.width(), 4);
        for lx in 0..3u32 {
            c.blit(
                &solid(1, 1, [lx as u8 * 40, 0, 0, 255]),
                lx as f64,
                0.0,
                1.0,
            );
        }
        let covered: Vec<u8> = (0..4).map(|x| c.device_pixel(x, 0).unwrap()[0]).collect();
        assert_eq!(covered, vec![0, 40, 40, 80]);
    }

    #[test]
    fn source_over_composites_alpha() {
        let mut c = Canvas::for_room(2.0, 1.0, 1.0);
        c.fill_rect(RectF::new(0.0, 0.0, 2.0, 1.0), [0, 0, 0, 255]);
        let half = solid(1, 1, [255, 255, 255, 128]);
        c.blit(&half, 0.0, 0.0, 1.0);
        let px = c.device_pixel(0, 0).expect("pixel");
        // 255 * (128/255) + 0 * (1 - 128/255) ~= 128
        assert!((i32::from(px[0]) - 128).abs() <= 1, "got {px:?}");
        assert_eq!(px[3], 255);
    }

    #[test]
    fn global_alpha_scales_the_source_alpha() {
        let mut c = Canvas::for_room(1.0, 1.0, 1.0);
        c.fill_rect(RectF::new(0.0, 0.0, 1.0, 1.0), [0, 0, 0, 255]);
        c.blit(&solid(1, 1, [255, 255, 255, 255]), 0.0, 0.0, 0.5);
        let px = c.device_pixel(0, 0).expect("pixel");
        assert!((i32::from(px[0]) - 128).abs() <= 1, "got {px:?}");
    }

    #[test]
    fn blits_clip_at_the_buffer_edge_without_panicking() {
        let img = solid(4, 4, [255, 0, 0, 255]);
        let mut c = Canvas::for_room(4.0, 4.0, 1.0);
        c.blit(&img, -2.0, -2.0, 1.0);
        assert_eq!(c.device_pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(c.device_pixel(1, 1), Some([255, 0, 0, 255]));
        assert_eq!(c.device_pixel(2, 2), Some([0, 0, 0, 0]));
        // Fully off-screen is a no-op.
        c.blit(&img, 100.0, 100.0, 1.0);
        c.blit(&img, -100.0, 0.0, 1.0);
    }

    #[test]
    fn fill_rect_rounds_outward_so_no_logical_pixel_is_lost() {
        let mut c = Canvas::for_room(4.0, 4.0, 1.5);
        c.fill_rect(RectF::new(0.0, 0.0, 2.0, 2.0), [1, 2, 3, 255]);
        // 2 logical px * 1.5 = 3 device px, so columns/rows 0..3 are painted.
        assert_eq!(c.device_pixel(2, 2), Some([1, 2, 3, 255]));
        assert_eq!(c.device_pixel(3, 3), Some([0, 0, 0, 0]));
    }

    #[test]
    fn dim_scales_rgb_toward_black_and_keeps_alpha() {
        let mut c = Canvas::for_room(1.0, 1.0, 1.0);
        c.fill_rect(RectF::new(0.0, 0.0, 1.0, 1.0), [200, 100, 50, 255]);
        c.apply_dim(0.5);
        let px = c.device_pixel(0, 0).expect("pixel");
        assert_eq!(px, [100, 50, 25, 255]);
        // level 1 is a no-op
        let mut c = Canvas::for_room(1.0, 1.0, 1.0);
        c.fill_rect(RectF::new(0.0, 0.0, 1.0, 1.0), [200, 100, 50, 255]);
        c.apply_dim(1.0);
        assert_eq!(c.device_pixel(0, 0), Some([200, 100, 50, 255]));
    }

    #[test]
    fn png_roundtrip_preserves_every_rgba_byte() {
        let mut c = Canvas::for_room(17.0, 9.0, 2.0);
        c.fill_rect(RectF::new(0.0, 0.0, 17.0, 9.0), [12, 200, 7, 255]);
        c.blit(&solid(5, 5, [255, 0, 0, 128]), 3.0, 2.0, 0.5);
        c.blit(&solid(2, 2, [0, 0, 255, 255]), 15.0, 7.0, 1.0);

        let png = c.to_png_bytes().expect("encode");
        let mut reader = png::Decoder::new(std::io::Cursor::new(&png))
            .read_info()
            .expect("decode header");
        let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
        let info = reader.next_frame(&mut buf).expect("decode frame");
        assert_eq!((info.width, info.height), (c.width(), c.height()));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(&buf[..info.buffer_size()], c.as_rgba());
    }
}

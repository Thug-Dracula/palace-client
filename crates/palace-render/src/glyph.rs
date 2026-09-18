//! Shared glyph rasterization for the two text layers.
//!
//! Name tags ([`crate::nametag`]) and chat text ([`crate::chattext`]) draw the
//! same way: a string is laid out with an embedded face, rasterized into an 8-bit
//! coverage mask, dilated into a dark halo, and composited under a flat fill
//! colour. Only the geometry and the fill differ between the two. This module
//! owns the identical half — the font, the per-glyph cache, the line layout, and
//! the mask/dilation/composite pipeline — so neither text layer reimplements it.
//!
//! ## Font
//!
//! The reference name tag asks for Arial (`NameTag.mxml`), which is proprietary;
//! the chat bubble sets no family at all (`ChatBubbleSkin.mxml`). Both are served
//! here by one embedded face, Liberation Sans Bold (SIL OFL 1.1), which is
//! metric-compatible with Arial so the advance widths that drive the layout
//! match. The face is parsed once per process and every glyph is rasterized once
//! into a process-wide cache, so a frame with many tags and many chat lines pays
//! that cost once.
//!
//! ## The glow is approximated, not reproduced
//!
//! Flex's filters are two-pass Gaussian blurs. This crate has no blur dependency
//! and does not add one: it draws a dark silhouette by dilating the glyph mask
//! over the integer offsets within a small radius (a Max filter) and composites
//! that under the fill. The result is a hard black halo rather than a soft
//! gradient — visually close over busy room art, but **an approximation** of the
//! reference filter, not the filter itself. See [`crate::nametag`] for the name
//! tag's own filter and [`crate::chattext`] for the chat bubble's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fontdue::{Font, FontSettings};

/// The embedded face. Liberation Sans Bold, SIL OFL 1.1; see the licence note
/// beside it in `assets/`.
const FONT_TTF: &[u8] = include_bytes!("../assets/LiberationSans-Bold.ttf");

/// The size every text layer rasterizes at.
///
/// The reference name tag asks for `fontSize="12"` (`NameTag.mxml`); the chat
/// bubble's `RichText` sets no size and takes the Flex theme default, which is
/// also 12 px. One size keeps one glyph cache and one line metric.
pub(crate) const FONT_PX: f32 = 12.0;

/// One glyph, rasterized at [`FONT_PX`] and kept in the process-wide cache, so
/// repeated characters never re-rasterize.
pub(crate) struct RasterGlyph {
    pub(crate) advance: f32,
    pub(crate) xmin: i32,
    pub(crate) ymin: i32,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) coverage: Arc<[u8]>,
}

impl RasterGlyph {
    fn blank() -> Self {
        RasterGlyph {
            advance: 0.0,
            xmin: 0,
            ymin: 0,
            width: 0,
            height: 0,
            coverage: Arc::from(Vec::<u8>::new()),
        }
    }
}

/// A glyph positioned at the text origin: `(x, y)` is the bitmap's top-left in
/// text-box coordinates.
pub(crate) struct PlacedGlyph {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) glyph: Arc<RasterGlyph>,
}

/// One line of text, laid out and measured at [`FONT_PX`].
pub(crate) struct LineLayout {
    /// The glyphs that have ink, positioned in text-box coordinates.
    pub(crate) placed: Vec<PlacedGlyph>,
    /// The measured advance width — the total the cursor travelled.
    pub(crate) width: f32,
    /// Ink bounds in text-box coordinates (a blank line leaves these at `0`).
    pub(crate) min_x: f32,
    pub(crate) max_x: f32,
    pub(crate) min_y: f32,
    pub(crate) max_y: f32,
}

/// The parsed face, parsed at most once for the life of the process.
pub(crate) fn font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| Font::from_bytes(FONT_TTF, FontSettings::default()).ok())
        .as_ref()
}

/// The cached raster for `character`, rasterizing it on first use.
pub(crate) fn glyph(character: char) -> Arc<RasterGlyph> {
    static CACHE: OnceLock<Mutex<HashMap<char, Arc<RasterGlyph>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let cached = cache
        .lock()
        .ok()
        .and_then(|map| map.get(&character).cloned());
    if let Some(existing) = cached {
        return existing;
    }

    let raster = match font() {
        Some(font) => {
            let (metrics, bitmap) = font.rasterize(character, FONT_PX);
            RasterGlyph {
                advance: metrics.advance_width,
                xmin: metrics.xmin,
                ymin: metrics.ymin,
                width: metrics.width,
                height: metrics.height,
                coverage: Arc::from(bitmap.into_boxed_slice()),
            }
        }
        None => RasterGlyph::blank(),
    };
    let raster = Arc::new(raster);
    if let Ok(mut map) = cache.lock() {
        map.insert(character, Arc::clone(&raster));
    }
    raster
}

/// Truncate `text` to `max_chars` characters without splitting a `char`.
pub(crate) fn bounded(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// The measured advance width of the first `max_chars` characters of `text`, in
/// logical pixels.
pub(crate) fn measure(text: &str, max_chars: usize) -> f32 {
    bounded(text, max_chars)
        .chars()
        .map(|c| glyph(c).advance)
        .sum()
}

/// Lay `text` out on one line at [`FONT_PX`].
///
/// `None` when the embedded face cannot be parsed or reports no horizontal line
/// metrics, which also makes every glyph blank.
pub(crate) fn layout_line(text: &str) -> Option<LineLayout> {
    let font = font()?;
    let line = font.horizontal_line_metrics(FONT_PX)?;
    let ascent = line.ascent.max(FONT_PX * 0.5);
    let descent = line.descent.min(0.0);

    let mut placed: Vec<PlacedGlyph> = Vec::new();
    let mut cursor = 0.0f32;
    let mut min_x = 0.0f32;
    let mut max_x = 0.0f32;
    let mut min_y = 0.0f32;
    let mut max_y = ascent - descent;
    for character in text.chars() {
        let raster = glyph(character);
        if raster.width > 0 && raster.height > 0 {
            let x = cursor + raster.xmin as f32;
            let top = ascent - (raster.ymin + raster.height as i32) as f32;
            let bottom = ascent - raster.ymin as f32;
            min_x = min_x.min(x);
            max_x = max_x.max(x + raster.width as f32);
            min_y = min_y.min(top);
            max_y = max_y.max(bottom);
            placed.push(PlacedGlyph {
                x,
                y: top,
                glyph: Arc::clone(&raster),
            });
        }
        cursor += raster.advance;
    }
    max_x = max_x.max(cursor);

    Some(LineLayout {
        placed,
        width: cursor,
        min_x,
        max_x,
        min_y,
        max_y,
    })
}

/// Paint every placed glyph's coverage into `mask`, maximising on overlap.
///
/// `origin_x`/`origin_y` are the text box's top-left inside the mask, so a glyph
/// that overhangs the mask is clipped rather than wrapped.
pub(crate) fn paint_mask(
    mask: &mut [u8],
    width: usize,
    height: usize,
    origin_x: i32,
    origin_y: i32,
    placed: &[PlacedGlyph],
) {
    for p in placed {
        let left = origin_x + p.x.floor() as i32;
        let top = origin_y + p.y.floor() as i32;
        for row in 0..p.glyph.height {
            let y = top + row as i32;
            if y < 0 || y as usize >= height {
                continue;
            }
            let row_start = row * p.glyph.width;
            for col in 0..p.glyph.width {
                let x = left + col as i32;
                if x < 0 || x as usize >= width {
                    continue;
                }
                let Some(&coverage) = p.glyph.coverage.get(row_start + col) else {
                    continue;
                };
                if coverage == 0 {
                    continue;
                }
                let at = y as usize * width + x as usize;
                mask[at] = mask[at].max(coverage);
            }
        }
    }
}

/// Dilate `mask` over the integer offsets within `radius` — a Max filter that
/// stands in for the reference's Gaussian blur. See the module docs.
pub(crate) fn dilate(mask: &[u8], width: usize, height: usize, radius: i32) -> Vec<u8> {
    let mut glow = vec![0u8; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let mut best = 0u8;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    let sx = x as i32 + dx;
                    let sy = y as i32 + dy;
                    if sx < 0 || sy < 0 || sx as usize >= width || sy as usize >= height {
                        continue;
                    }
                    best = best.max(mask[sy as usize * width + sx as usize]);
                }
            }
            glow[y * width + x] = best;
        }
    }
    glow
}

/// Composite a dark glow under a `fill`-coloured mask into premultiplied-out
/// RGBA. A pixel with no ink and no glow stays fully transparent.
pub(crate) fn composite(fill: [u8; 3], mask: &[u8], glow: &[u8]) -> Option<Vec<u8>> {
    let len = mask.len().checked_mul(4)?;
    let mut rgba = vec![0u8; len];
    for (i, (&glow_alpha, &fill_alpha)) in glow.iter().zip(mask).enumerate() {
        if glow_alpha == 0 && fill_alpha == 0 {
            continue;
        }
        let black = f32::from(glow_alpha) / 255.0;
        let ink = f32::from(fill_alpha) / 255.0;
        let out_alpha = ink + black * (1.0 - ink);
        if out_alpha <= 0.0 {
            continue;
        }
        let at = i * 4;
        for channel in 0..3 {
            rgba[at + channel] = (f32::from(fill[channel]) * ink / out_alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        rgba[at + 3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    Some(rgba)
}

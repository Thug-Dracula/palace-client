//! Rasterizing user name tags.
//!
//! Name tags are layer 9 of the reference z-order: above the avatars and the
//! `AboveAvatars` overlay band, below everything flagged *pictures above name
//! tags*. The geometry and style are ported verbatim from OpenPalace's
//! `NameTag.mxml`:
//!
//! ```text
//!   x = user.x - 1 - width/2      (width = the label's measured text width)
//!   y = user.y + 17               (top of the label box)
//!   fontFamily="Arial" fontSize="12" fontWeight="bold" color="0xFFFFFF"
//!   <GlowFilter color="0x000000" alpha="1" blurX="2" blurY="2"
//!               quality="2" strength="5" />
//! ```
//!
//! ## Font
//!
//! The reference asks for Arial, which is proprietary. This embeds Liberation
//! Sans Bold instead ([`assets/LiberationSans-Bold.ttf`], SIL OFL 1.1): it is
//! metric-compatible with Arial, so the advance widths and line metrics that
//! drive the layout match. The face is parsed once per process and every glyph
//! is rasterized once into a process-wide cache, so a frame with many avatars
//! pays that cost once, not per avatar.
//!
//! ## The glow is approximated, not reproduced
//!
//! Flex's `GlowFilter(blurX=2, blurY=2, quality=2, strength=5)` is a two-pass
//! Gaussian blur of the glyph alpha, accumulated at the requested strength. This
//! crate has no blur dependency and does not add one: it draws a dark silhouette
//! by dilating the glyph mask over the integer offsets within a 2 px radius
//! (a Max filter) and composites that under the white fill. The result is a hard
//! black halo rather than a soft gradient — visually close over busy room art,
//! but **an approximation** of the reference filter, not the filter itself.
//!
//! ## Bounds
//!
//! A name is a `PString` on the wire (one length byte, so at most 255 bytes), but
//! this module is also reachable from a caller that hands it arbitrary text. The
//! reference never truncates, so neither does the visible output for any real
//! name; a name longer than [`MAX_NAME_TAG_CHARS`] is truncated to that many
//! characters purely so a hostile 10 KB string cannot ask for a multi-megabyte
//! glyph bitmap. 255 characters at 12 px is already about twice the width of the
//! widest Palace room.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fontdue::{Font, FontSettings};
use palace_prop::PropImage;

use crate::canvas::Canvas;

/// The embedded face. Liberation Sans Bold, SIL OFL 1.1; see the licence note
/// beside it in `assets/`.
const FONT_TTF: &[u8] = include_bytes!("../assets/LiberationSans-Bold.ttf");

/// The reference font size (`fontSize="12"`).
pub const NAME_TAG_FONT_PX: f32 = 12.0;
/// The reference `y` term (`y="{user.y + 17}"`).
pub const NAME_TAG_Y_OFFSET: i32 = 17;
/// The reference `x` fudge (`x="{user.x - 1 - width/2}"`).
pub const NAME_TAG_X_FUDGE: i32 = -1;
/// The `blurX`/`blurY` radius of the reference glow, reused as our dilation radius.
pub const NAME_TAG_GLOW_RADIUS: i32 = 2;
/// Longest name that gets rasterized, in characters. See the module docs.
pub const MAX_NAME_TAG_CHARS: usize = 255;

const FILL: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// One glyph, rasterized at [`NAME_TAG_FONT_PX`] and kept in the process-wide
/// cache, so repeated names never re-rasterize.
struct RasterGlyph {
    advance: f32,
    xmin: i32,
    ymin: i32,
    width: usize,
    height: usize,
    coverage: Arc<[u8]>,
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
struct PlacedGlyph {
    x: f32,
    y: f32,
    glyph: Arc<RasterGlyph>,
}

/// The parsed face, parsed at most once for the life of the process.
fn font() -> Option<&'static Font> {
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    FONT.get_or_init(|| Font::from_bytes(FONT_TTF, FontSettings::default()).ok())
        .as_ref()
}

/// The cached raster for `character`, rasterizing it on first use.
fn glyph(character: char) -> Arc<RasterGlyph> {
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
            let (metrics, bitmap) = font.rasterize(character, NAME_TAG_FONT_PX);
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

/// Truncate to [`MAX_NAME_TAG_CHARS`] characters without splitting a `char`.
fn bounded(name: &str) -> &str {
    match name.char_indices().nth(MAX_NAME_TAG_CHARS) {
        Some((end, _)) => &name[..end],
        None => name,
    }
}

/// The measured advance width of `name` at [`NAME_TAG_FONT_PX`], in logical
/// pixels. This is the `width` in the reference's `x - 1 - width/2`.
#[must_use]
pub fn measure_text(name: &str) -> f32 {
    bounded(name).chars().map(|c| glyph(c).advance).sum()
}

/// The reference placement of the tag's text box for an avatar anchor.
///
/// `text_width` is [`measure_text`]'s value; the caller blits the tag image so
/// that this point is the text box's top-left.
#[must_use]
pub fn name_tag_position(avatar_x: i32, avatar_y: i32, text_width: f32) -> (f64, f64) {
    (
        f64::from(avatar_x) + f64::from(NAME_TAG_X_FUDGE) - f64::from(text_width) / 2.0,
        f64::from(avatar_y) + f64::from(NAME_TAG_Y_OFFSET),
    )
}

/// A rasterized name tag plus where its text box sits inside the image.
#[derive(Debug, Clone)]
pub struct NameTag {
    /// The composited black-glow + white-fill image, with a 2 px glow margin.
    pub image: PropImage,
    /// The measured advance width the centring used.
    pub text_width: f32,
    /// Image pixel that is the text box's top-left corner.
    pub origin_x: i32,
    /// See [`NameTag::origin_x`].
    pub origin_y: i32,
}

/// Rasterize `name` into a tag image, or `None` for an empty/blank name (or if
/// the embedded face cannot be parsed, which also makes every glyph blank).
#[must_use]
pub fn name_tag(name: &str) -> Option<NameTag> {
    let text = bounded(name);
    if text.is_empty() || text.chars().all(char::is_whitespace) {
        return None;
    }
    let font = font()?;
    let line = font.horizontal_line_metrics(NAME_TAG_FONT_PX)?;
    let ascent = line.ascent.max(NAME_TAG_FONT_PX * 0.5);
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

    let pad = NAME_TAG_GLOW_RADIUS;
    let content_x0 = min_x.floor() as i32;
    let content_y0 = min_y.floor() as i32;
    let content_x1 = max_x.ceil() as i32;
    let content_y1 = max_y.ceil() as i32;
    let width = ((content_x1 - content_x0).max(1) + 2 * pad) as usize;
    let height = ((content_y1 - content_y0).max(1) + 2 * pad) as usize;
    let pixels = width.checked_mul(height)?;
    let len = pixels.checked_mul(4)?;

    let origin_x = pad - content_x0;
    let origin_y = pad - content_y0;

    let mut mask = vec![0u8; pixels];
    for p in &placed {
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

    let mut glow = vec![0u8; pixels];
    for y in 0..height {
        for x in 0..width {
            let mut best = 0u8;
            for dy in -pad..=pad {
                for dx in -pad..=pad {
                    if dx * dx + dy * dy > pad * pad {
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

    let mut rgba = vec![0u8; len];
    for (i, (&glow_alpha, &fill_alpha)) in glow.iter().zip(&mask).enumerate() {
        if glow_alpha == 0 && fill_alpha == 0 {
            continue;
        }
        let black = f32::from(glow_alpha) / 255.0;
        let white = f32::from(fill_alpha) / 255.0;
        let out_alpha = white + black * (1.0 - white);
        if out_alpha <= 0.0 {
            continue;
        }
        let at = i * 4;
        for channel in 0..3 {
            rgba[at + channel] = (f32::from(FILL[channel]) * white / out_alpha)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        rgba[at + 3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let image = PropImage::from_rgba(width as u32, height as u32, rgba).ok()?;
    Some(NameTag {
        image,
        text_width: cursor,
        origin_x,
        origin_y,
    })
}

/// Draw `name` above the avatar anchored at `(avatar_x, avatar_y)`.
///
/// The image is placed so the text box's top-left lands exactly on
/// [`name_tag_position`]'s value.
pub fn draw_name_tag(canvas: &mut Canvas, avatar_x: i32, avatar_y: i32, name: &str) {
    let Some(tag) = name_tag(name) else {
        return;
    };
    let (text_x, text_y) = name_tag_position(avatar_x, avatar_y, tag.text_width);
    canvas.blit(
        &tag.image,
        text_x - f64::from(tag.origin_x),
        text_y - f64::from(tag.origin_y),
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_face_parses() {
        assert!(
            font().is_some(),
            "the embedded Liberation Sans Bold must load"
        );
    }

    #[test]
    fn measurement_is_zero_for_empty_and_grows_with_text() {
        assert_eq!(measure_text(""), 0.0);
        assert!(measure_text("A") > 0.0);
        assert!(measure_text("AAAA") > measure_text("AAA"));
        assert!(measure_text("WWW") > measure_text("iii"), "wide vs narrow");
    }

    /// Pins the measurement to the embedded face so a font swap cannot silently
    /// change every tag's position. 28.68164 is the advance width of "Alice" at
    /// 12 px bold in Liberation Sans Bold.
    #[test]
    fn measurement_pins_a_known_advance_width() {
        let width = measure_text("Alice");
        assert!(
            (width - 28.681_64).abs() < 0.001,
            "Alice should measure 28.68164 at 12px bold, got {width}"
        );
    }

    #[test]
    fn placement_matches_the_reference_formula() {
        // x = user.x - 1 - width/2, y = user.y + 17.
        assert_eq!(name_tag_position(100, 200, 40.0), (79.0, 217.0));
        // The reference keeps the half pixel for an odd width.
        assert_eq!(name_tag_position(100, 200, 41.0), (78.5, 217.0));
        assert_eq!(name_tag_position(0, 0, 0.0), (-1.0, 17.0));
    }

    #[test]
    fn a_tag_has_white_fill_and_a_dark_glow() {
        let tag = name_tag("M").expect("a name produces an image");
        assert!(tag.image.width() > 0 && tag.image.height() > 0);
        let mut white = 0usize;
        let mut dark = 0usize;
        for y in 0..tag.image.height() {
            for x in 0..tag.image.width() {
                let Some(p) = tag.image.pixel(x, y) else {
                    continue;
                };
                if p[3] == 0 {
                    continue;
                }
                if p[0] > 200 && p[1] > 200 && p[2] > 200 {
                    white += 1;
                }
                if p[0] < 60 && p[1] < 60 && p[2] < 60 {
                    dark += 1;
                }
            }
        }
        assert!(white > 0, "the glyph fill must be drawn");
        assert!(dark > 0, "the approximated glow must surround the fill");
    }

    #[test]
    fn blank_names_produce_no_tag() {
        assert!(name_tag("").is_none());
        assert!(name_tag("   ").is_none());
        assert!(name_tag("\t\n").is_none());
    }

    #[test]
    fn a_ten_kilobyte_name_is_clamped_to_the_wire_bound() {
        // A hostile name must not turn into a multi-megabyte bitmap.
        let hostile = "W".repeat(10_000);
        assert_eq!(
            measure_text(&hostile),
            measure_text(&"W".repeat(MAX_NAME_TAG_CHARS)),
            "only the first MAX_NAME_TAG_CHARS characters are measured"
        );
        let tag = name_tag(&hostile).expect("the clamped name still renders");
        // 255 characters at 12 px bold cannot exceed 32 px of advance each.
        assert!(
            tag.image.width() <= (MAX_NAME_TAG_CHARS as u32) * 32 + 4,
            "clamped image width {} is unbounded",
            tag.image.width()
        );
    }

    #[test]
    fn draw_name_tag_paints_pixels_above_the_avatar() {
        let mut with_tag = Canvas::for_room(128.0, 96.0, 1.0);
        with_tag.fill([0, 0, 0, 255]);
        draw_name_tag(&mut with_tag, 64, 40, "Bob");

        let mut plain = Canvas::for_room(128.0, 96.0, 1.0);
        plain.fill([0, 0, 0, 255]);

        let changed = with_tag
            .as_rgba()
            .chunks_exact(4)
            .zip(plain.as_rgba().chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 0, "the tag must change the frame");
    }
}

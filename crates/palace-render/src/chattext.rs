//! Rasterizing chat text.
//!
//! Chat text is layer 11 of the reference z-order: above the name tags and the
//! *pictures above name tags* overlay band, below *pictures above all*. The
//! reference renders it as one `ChatBubble` per message, anchored at the
//! speaking avatar rather than at a shared text field: `PalaceRoomView.mxml:481-488`
//! copies `user.x`/`user.y` onto the message (`(0, 10)` when there is no user),
//! and `PalaceRoomView.mxml:533-534` places the bubble at exactly that point. The
//! bubble's own skin then offsets itself relative to the anchor — the default
//! `bottomRight` state (`ChatBubble.as:26-31`) sits `x="20"`, `y` unset, i.e. 20 px
//! to the right of the avatar at its own height (`ChatBubbleSkin.mxml:2-8`).
//! `linpal` anchors the same way and flips to the left of the avatar when the
//! bubble would run off the right edge (`palbubble.cpp:78-91`).
//!
//! ## What is ported
//!
//! * The anchor is the speaker's avatar position, per message.
//! * The horizontal gap is the reference's 20 px ([`CHAT_X_GAP`]).
//! * A box that would overflow the right edge flips to the left of the anchor
//!   (`palbubble.cpp:86-91`), and both axes are clamped into the room
//!   (`palbubble.cpp:80-84`).
//! * One fill colour per message kind, so kinds are distinguishable at all.
//!
//! ## What is approximated or omitted
//!
//! * **The bubble chrome is not drawn.** The reference's rounded, tinted
//!   background, bevel, gradient and drop shadow (`ChatBubbleSkin.mxml:81-138`)
//!   are not reproduced; this crate has no blur dependency, and the established
//!   approach in `crate::glyph` is a glyph fill over a dilated dark halo, exactly
//!   as name tags do. The fill colours are therefore *text* colours, not the
//!   reference's background tints.
//! * **Per-speaker tint is not used.** The reference keys the bubble colour off
//!   the speaker's colour index (`PalaceRoomView.mxml:479`, `Avatar.mxml:47-64`).
//!   This layer has no colour channel on the wire model, so the fill is chosen by
//!   kind instead.
//! * **Whispers are not italic.** `ChatBubble.as:59-64` italicises the RichText;
//!   this embedded face has no italic cut.
//! * **No wrapping and no collision resolution.** The reference wraps at
//!   `maxWidth="125"` (`ChatBubbleSkin.mxml:141`) and tries four bubble positions
//!   to avoid overlaps (`PalaceRoomView.mxml:545-602`). A line here is one bounded
//!   run; overlapping bubbles simply stack.
//! * **The `@x,y` relocation prefix is not interpreted** (`ChatMessage.as:16-61`);
//!   a message is placed at its speaker's position only.
//!
//! ## Bounds
//!
//! Like [`crate::nametag`], a message longer than [`MAX_CHAT_TEXT_CHARS`] is
//! truncated so a hostile string cannot ask for a multi-megabyte glyph bitmap.
//! Real chat far below that limit renders in full.

use palace_prop::PropImage;

use crate::canvas::Canvas;
use crate::glyph;

/// The reference font size, shared with name tags (see `crate::glyph`).
pub const CHAT_FONT_PX: f32 = glyph::FONT_PX;
/// The reference gap between the avatar anchor and the bubble (`x="20"`).
pub const CHAT_X_GAP: i32 = 20;
/// The reference vertical offset of the default bubble state (unset, i.e. none).
pub const CHAT_Y_OFFSET: i32 = 0;
/// The dilation radius of the approximated dark halo.
pub const CHAT_GLOW_RADIUS: i32 = 2;
/// Longest message that gets rasterized, in characters. See the module docs.
pub const MAX_CHAT_TEXT_CHARS: usize = 255;

/// Fill for a spoken line — the reference's name-tag white.
pub const TALK_FILL: [u8; 3] = [0xFF, 0xFF, 0xFF];
/// Fill for a whisper — the reference `ChatBubble` default tint (`ChatBubble.as:33`).
pub const WHISPER_FILL: [u8; 3] = [0xFF, 0xFF, 0xAA];
/// Fill for a system notice — the reference's user-less tint (`PalaceRoomView.mxml:479`).
pub const SYSTEM_FILL: [u8; 3] = [0xF9, 0xF9, 0xF9];
/// Fill for an error — the reference's first colour index (`Avatar.mxml:48`).
pub const ERROR_FILL: [u8; 3] = [0xFF, 0x99, 0x99];

/// The kind of chat line, selecting the fill colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStyle {
    /// A spoken line.
    Talk,
    /// A whisper.
    Whisper,
    /// A client or server notice.
    System,
    /// A transcript error.
    Error,
}

impl ChatStyle {
    /// The fill colour this kind draws in.
    #[must_use]
    pub const fn fill(self) -> [u8; 3] {
        match self {
            ChatStyle::Talk => TALK_FILL,
            ChatStyle::Whisper => WHISPER_FILL,
            ChatStyle::System => SYSTEM_FILL,
            ChatStyle::Error => ERROR_FILL,
        }
    }
}

/// One chat line to rasterize, anchored at its speaker's avatar position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatText {
    /// The message text.
    pub text: String,
    /// Logical room x of the speaker's avatar anchor.
    pub x: i32,
    /// Logical room y of the speaker's avatar anchor.
    pub y: i32,
    /// The line's kind, selecting its fill.
    pub style: ChatStyle,
}

/// A rasterized chat line plus where its text box sits inside the image.
#[derive(Debug, Clone)]
pub struct ChatRender {
    /// The composited glow + fill image, with a [`CHAT_GLOW_RADIUS`] margin.
    pub image: PropImage,
    /// The measured advance width, used for the horizontal placement.
    pub text_width: f32,
    /// The measured ink height, used for the vertical clamp.
    pub text_height: f32,
    /// Image pixel that is the text box's top-left corner.
    pub origin_x: i32,
    /// See [`ChatRender::origin_x`].
    pub origin_y: i32,
}

/// Rasterize `item` into a bubble image, or `None` for empty/blank text (or if
/// the embedded face cannot be parsed, which also makes every glyph blank).
#[must_use]
pub fn chat_text(item: &ChatText) -> Option<ChatRender> {
    let text = glyph::bounded(&item.text, MAX_CHAT_TEXT_CHARS);
    if text.is_empty() || text.chars().all(char::is_whitespace) {
        return None;
    }
    let layout = glyph::layout_line(text)?;

    let pad = CHAT_GLOW_RADIUS;
    let content_x0 = layout.min_x.floor() as i32;
    let content_y0 = layout.min_y.floor() as i32;
    let content_x1 = layout.max_x.ceil() as i32;
    let content_y1 = layout.max_y.ceil() as i32;
    let width = ((content_x1 - content_x0).max(1) + 2 * pad) as usize;
    let height = ((content_y1 - content_y0).max(1) + 2 * pad) as usize;
    let pixels = width.checked_mul(height)?;

    let origin_x = pad - content_x0;
    let origin_y = pad - content_y0;

    let mut mask = vec![0u8; pixels];
    glyph::paint_mask(&mut mask, width, height, origin_x, origin_y, &layout.placed);
    let glow = glyph::dilate(&mask, width, height, pad);
    let rgba = glyph::composite(item.style.fill(), &mask, &glow)?;

    let image = PropImage::from_rgba(width as u32, height as u32, rgba).ok()?;
    let text_height = (content_y1 - content_y0).max(1) as f32;
    Some(ChatRender {
        image,
        text_width: layout.width,
        text_height,
        origin_x,
        origin_y,
    })
}

/// The reference placement of a bubble's text box for a speaker anchor.
///
/// The box sits [`CHAT_X_GAP`] to the right of the anchor at its own height; a
/// box that would overflow the room's right edge flips to the left of the
/// anchor, mirroring `linpal`'s `srcx - (width + 44)` (`palbubble.cpp:86-91`).
/// Both axes are then clamped into the room (`palbubble.cpp:80-84`). The caller
/// blits the image so that this point is the text box's top-left.
#[must_use]
pub fn chat_position(
    anchor_x: i32,
    anchor_y: i32,
    text_width: f32,
    text_height: f32,
    room_width: f64,
    room_height: f64,
) -> (f64, f64) {
    let mut x = f64::from(anchor_x + CHAT_X_GAP);
    if x + f64::from(text_width) > room_width {
        x = f64::from(anchor_x - CHAT_X_GAP) - f64::from(text_width);
    }
    x = x.max(0.0);

    let mut y = f64::from(anchor_y + CHAT_Y_OFFSET);
    y = y.min(room_height - f64::from(text_height)).max(0.0);

    (x, y)
}

/// Draw `item` at layer 11, anchored at its speaker's avatar position.
///
/// The image is placed so the text box's top-left lands exactly on
/// [`chat_position`]'s value, which is derived from the canvas's own logical
/// size.
pub fn draw_chat_text(canvas: &mut Canvas, item: &ChatText) {
    let Some(render) = chat_text(item) else {
        return;
    };
    let (room_width, room_height) = canvas.logical_size();
    let (text_x, text_y) = chat_position(
        item.x,
        item.y,
        render.text_width,
        render.text_height,
        room_width,
        room_height,
    );
    canvas.blit(
        &render.image,
        text_x - f64::from(render.origin_x),
        text_y - f64::from(render.origin_y),
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> ChatText {
        ChatText {
            text: text.to_string(),
            x: 0,
            y: 0,
            style: ChatStyle::Talk,
        }
    }

    #[test]
    fn measurement_is_zero_for_empty_and_grows_with_text() {
        assert!(chat_text(&line("")).is_none());
        let one = chat_text(&line("A")).expect("one character renders");
        let three = chat_text(&line("AAA")).expect("three render");
        let four = chat_text(&line("AAAA")).expect("four render");
        assert!(one.text_width > 0.0);
        assert!(four.text_width > three.text_width);
        let wide = chat_text(&line("WWW")).expect("wide renders");
        let narrow = chat_text(&line("iii")).expect("narrow renders");
        assert!(wide.text_width > narrow.text_width, "wide vs narrow");
    }

    /// Pins the measurement to the embedded face through the chat rasterizer, so
    /// a font swap cannot silently change the placement. 28.68164 is the advance
    /// width of "Alice" at 12 px bold in Liberation Sans Bold.
    #[test]
    fn measurement_pins_a_known_advance_width() {
        let render = chat_text(&line("Alice")).expect("the line renders");
        assert!(
            (render.text_width - 28.681_64).abs() < 0.001,
            "Alice should measure 28.68164 at 12px bold, got {}",
            render.text_width
        );
    }

    #[test]
    fn blank_text_produces_no_image() {
        assert!(chat_text(&line("")).is_none());
        assert!(chat_text(&line("   ")).is_none());
        assert!(chat_text(&line("\t\n")).is_none());
    }

    #[test]
    fn a_ten_kilobyte_message_is_clamped_to_the_wire_bound() {
        let hostile = line(&"W".repeat(10_000));
        let render = chat_text(&hostile).expect("the clamped message still renders");
        // 255 characters at 12 px bold cannot exceed 32 px of advance each.
        assert!(
            render.image.width() <= (MAX_CHAT_TEXT_CHARS as u32) * 32 + 4,
            "clamped image width {} is unbounded",
            render.image.width()
        );
        let clamped = line(&"W".repeat(MAX_CHAT_TEXT_CHARS));
        assert_eq!(
            chat_text(&hostile).map(|r| r.text_width),
            chat_text(&clamped).map(|r| r.text_width),
            "only the first MAX_CHAT_TEXT_CHARS characters are measured"
        );
    }

    #[test]
    fn a_message_has_fill_and_a_dark_glow() {
        let render = chat_text(&line("M")).expect("a message produces an image");
        assert!(render.image.width() > 0 && render.image.height() > 0);
        let mut fill = 0usize;
        let mut dark = 0usize;
        for y in 0..render.image.height() {
            for x in 0..render.image.width() {
                let Some(p) = render.image.pixel(x, y) else {
                    continue;
                };
                if p[3] == 0 {
                    continue;
                }
                if p[0] > 200 && p[1] > 200 && p[2] > 200 {
                    fill += 1;
                }
                if p[0] < 60 && p[1] < 60 && p[2] < 60 {
                    dark += 1;
                }
            }
        }
        assert!(fill > 0, "the glyph fill must be drawn");
        assert!(dark > 0, "the approximated glow must surround the fill");
    }

    #[test]
    fn each_style_draws_its_own_fill() {
        for style in [
            ChatStyle::Talk,
            ChatStyle::Whisper,
            ChatStyle::System,
            ChatStyle::Error,
        ] {
            let item = ChatText { style, ..line("M") };
            let render = chat_text(&item).expect("each style renders");
            let fill = style.fill();
            let expected = [fill[0], fill[1], fill[2], 255];
            let mut found = false;
            for y in 0..render.image.height() {
                for x in 0..render.image.width() {
                    if render.image.pixel(x, y) == Some(expected) {
                        found = true;
                    }
                }
            }
            assert!(found, "the {style:?} fill must appear in its image");
        }
    }

    #[test]
    fn placement_matches_the_reference_gap_and_flips_at_the_right_edge() {
        // 20 px to the right of the anchor, at the anchor's own height.
        assert_eq!(
            chat_position(100, 200, 40.0, 15.0, 512.0, 384.0),
            (120.0, 200.0)
        );
        // A box that would overflow the right edge flips to the left.
        assert_eq!(
            chat_position(500, 200, 40.0, 15.0, 512.0, 384.0),
            (440.0, 200.0)
        );
        // The bottom is clamped into the room; the top and left never go negative.
        assert_eq!(chat_position(100, 380, 40.0, 15.0, 512.0, 384.0).1, 369.0);
        assert_eq!(
            chat_position(-100, -50, 40.0, 15.0, 512.0, 384.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn draw_chat_text_paints_pixels() {
        let mut with_text = Canvas::for_room(128.0, 96.0, 1.0);
        with_text.fill([0, 0, 0, 255]);
        draw_chat_text(&mut with_text, &line("Bob"));

        let mut plain = Canvas::for_room(128.0, 96.0, 1.0);
        plain.fill([0, 0, 0, 255]);

        let changed = with_text
            .as_rgba()
            .chunks_exact(4)
            .zip(plain.as_rgba().chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 0, "the message must change the frame");
    }
}

//! The render input: a room flattened into drawable layers.
//!
//! The compositor is deliberately decoupled from `palace-room` and from the
//! filesystem. A [`Scene`] is the *resolved* form of a room: every prop, overlay
//! and background has already been turned into an RGBA image, and every position
//! is an absolute logical room pixel. That split is what makes the compositor
//! unit-testable without a corpus and keeps it deterministic.
//!
//! ## Layer buckets
//!
//! Palace does not have one flat z-order; a hotspot declares which of four
//! "pictures above…" bands it belongs to, and its state image is drawn in that
//! band. The bands come straight from `Hotspot.flags`:
//!
//! | Flag | Bit | Band |
//! |---|---|---|
//! | `FLAG_DRAGGABLE` | `0x01` | above everything |
//! | `FLAG_INVISIBLE` | `0x04` | above avatars |
//! | `FLAG_FILL` | `0x40` | above name tags |
//! | (none) | — | above nothing (directly over the background) |
//!
//! See [`crate::compositor`] for the full bottom-to-top order.

use palace_prop::PropImage;

use crate::error::AssetNote;

/// Which "pictures above…" band a hotspot-state image overlay belongs to.
///
/// Mirrors `PalaceHotspot.layerAboveAll` / `layerAboveAvatars` /
/// `layerAboveNameTags` / `layerNormal` in OpenPalace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    /// Drawn directly over the background, under every prop and avatar.
    AboveNothing,
    /// Drawn over avatars but under name tags.
    AboveAvatars,
    /// Drawn over name tags but under chat text.
    AboveNameTags,
    /// Drawn over everything.
    AboveEverything,
}

/// `FLAG_DRAGGABLE` — "pictures above all" in the 1999 spec.
pub const FLAG_PICTURES_ABOVE_ALL: i32 = 0x0000_0001;
/// `FLAG_DONT_MOVE_HERE`.
pub const FLAG_DONT_MOVE_HERE: i32 = 0x0000_0002;
/// `FLAG_INVISIBLE` — "pictures above props" in the 1999 spec.
pub const FLAG_PICTURES_ABOVE_PROPS: i32 = 0x0000_0004;
/// `FLAG_SHOW_NAME`.
pub const FLAG_SHOW_NAME: i32 = 0x0000_0008;
/// `FLAG_DRAW_FRAME`.
pub const FLAG_DRAW_FRAME: i32 = 0x0000_0010;
/// `FLAG_SHADOW`.
pub const FLAG_SHADOW: i32 = 0x0000_0020;
/// `FLAG_FILL` — "pictures above name tags" in the 1999 spec.
pub const FLAG_PICTURES_ABOVE_NAME_TAGS: i32 = 0x0000_0040;

/// Map a hotspot's raw flag word to its image-overlay band.
#[must_use]
pub fn layer_for_hotspot_flags(flags: i32) -> Layer {
    if flags & FLAG_PICTURES_ABOVE_ALL != 0 {
        Layer::AboveEverything
    } else if flags & FLAG_PICTURES_ABOVE_PROPS != 0 {
        Layer::AboveAvatars
    } else if flags & FLAG_PICTURES_ABOVE_NAME_TAGS != 0 {
        Layer::AboveNameTags
    } else {
        Layer::AboveNothing
    }
}

/// A single RGBA image placed at an absolute logical position.
#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    /// The decoded image.
    pub image: PropImage,
    /// Logical room x of the image's top-left.
    pub x: i32,
    /// Logical room y of the image's top-left.
    pub y: i32,
    /// Opacity multiplier, `0..=1` (ghost props pass `0.5`).
    pub alpha: f64,
    /// Within-layer ordering key. The compositor sorts each layer by this
    /// **stably**, so equal keys keep their insertion order.
    pub z: i64,
}

impl Sprite {
    /// A fully opaque sprite at `(x, y)` with sort key `z`.
    #[must_use]
    pub fn new(image: PropImage, x: i32, y: i32, z: i64) -> Self {
        Sprite {
            image,
            x,
            y,
            alpha: 1.0,
            z,
        }
    }

    /// Builder-style opacity setter.
    #[must_use]
    pub fn with_alpha(mut self, alpha: f64) -> Self {
        self.alpha = alpha;
        self
    }
}

/// One image that makes up an avatar, offset from the avatar's own anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct AvatarPart {
    /// The decoded prop image.
    pub image: PropImage,
    /// Offset from the avatar's logical position to this part's top-left.
    pub dx: i32,
    /// See [`AvatarPart::dx`].
    pub dy: i32,
    /// Per-prop opacity (ghost props pass `0.5`).
    pub alpha: f64,
}

/// A Type 0 avatar: up to nine 44×44 props stacked around one anchor.
///
/// The avatar's `(x, y)` is its room position (the point the server moves it
/// to). Each part carries its own offset, computed when the scene was built from
/// the prop headers, so the compositor only has to place and sort avatars.
#[derive(Debug, Clone, PartialEq)]
pub struct Avatar {
    /// Logical room x of the avatar's anchor.
    pub x: i32,
    /// Logical room y of the avatar's anchor.
    pub y: i32,
    /// The stacked prop images, in the order they should be drawn.
    pub parts: Vec<AvatarPart>,
}

/// Everything the compositor needs to draw one frame.
///
/// Build it with [`crate::build::SceneBuilder`], or by hand in tests.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    /// Logical room size in pixels. Never assume 512×384 — this is the
    /// background's size with that floor applied.
    pub size: (u32, u32),
    /// Flat colour used where no background image exists.
    pub backdrop: [u8; 4],
    /// The background image, drawn 1:1 at `(0, 0)`.
    pub background: Option<PropImage>,
    /// `DIMROOM` level, `1.0` = not dimmed. Applied over background+overlays.
    pub dim_level: f64,
    /// Hotspot-state overlays in the `AboveNothing` band.
    pub overlays_above_nothing: Vec<Sprite>,
    /// Hotspot-state overlays in the `AboveAvatars` band.
    pub overlays_above_avatars: Vec<Sprite>,
    /// Hotspot-state overlays in the `AboveNameTags` band.
    pub overlays_above_name_tags: Vec<Sprite>,
    /// Hotspot-state overlays in the `AboveEverything` band.
    pub overlays_above_everything: Vec<Sprite>,
    /// Loose props, drawn in scene order.
    pub loose_props: Vec<Sprite>,
    /// Avatars, y-sorted by the compositor.
    pub avatars: Vec<Avatar>,
    /// Non-fatal asset problems worth reporting.
    pub notes: Vec<AssetNote>,
}

impl Scene {
    /// A scene of the given logical size with a black backdrop.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Scene {
            size: (width.max(1), height.max(1)),
            backdrop: [0, 0, 0, 255],
            background: None,
            dim_level: 1.0,
            overlays_above_nothing: Vec::new(),
            overlays_above_avatars: Vec::new(),
            overlays_above_name_tags: Vec::new(),
            overlays_above_everything: Vec::new(),
            loose_props: Vec::new(),
            avatars: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// The logical size as `(width, height)` floats.
    #[must_use]
    pub fn logical_size(&self) -> (f64, f64) {
        (f64::from(self.size.0), f64::from(self.size.1))
    }

    /// Whether any asset problem was recorded.
    #[must_use]
    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_flags_select_the_reference_bands() {
        assert_eq!(layer_for_hotspot_flags(0), Layer::AboveNothing);
        assert_eq!(
            layer_for_hotspot_flags(FLAG_PICTURES_ABOVE_ALL),
            Layer::AboveEverything
        );
        assert_eq!(
            layer_for_hotspot_flags(FLAG_PICTURES_ABOVE_PROPS),
            Layer::AboveAvatars
        );
        assert_eq!(
            layer_for_hotspot_flags(FLAG_PICTURES_ABOVE_NAME_TAGS),
            Layer::AboveNameTags
        );
        // Priority order matches PalaceHotspot's getters: above-all wins.
        assert_eq!(
            layer_for_hotspot_flags(
                FLAG_PICTURES_ABOVE_ALL | FLAG_PICTURES_ABOVE_PROPS | FLAG_PICTURES_ABOVE_NAME_TAGS
            ),
            Layer::AboveEverything
        );
        assert_eq!(
            layer_for_hotspot_flags(FLAG_PICTURES_ABOVE_PROPS | FLAG_SHOW_NAME),
            Layer::AboveAvatars
        );
    }

    #[test]
    fn layer_order_is_bottom_to_top() {
        assert!(Layer::AboveNothing < Layer::AboveAvatars);
        assert!(Layer::AboveAvatars < Layer::AboveNameTags);
        assert!(Layer::AboveNameTags < Layer::AboveEverything);
    }
}

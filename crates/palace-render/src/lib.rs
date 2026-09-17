//! `palace-render` — the Palace room compositor.
//!
//! Turns a parsed `MSG_ROOMDESC` plus local assets into a rendered RGBA frame,
//! and owns the one piece of geometry the whole client depends on: the mapping
//! between room coordinates and viewport pixels.
//!
//! ## What it does
//!
//! ```text
//!   RoomDesc ─┐
//!   props    ─┼─► SceneBuilder ─► Scene ─► Compositor ─► Canvas ─► PNG
//!   media    ─┘
//! ```
//!
//! * [`assets`] resolves backgrounds, overlays and props from local files.
//! * [`build`] applies the room's geometry rules and produces a [`Scene`].
//! * [`compositor`] stacks the scene's layers in the reference z-order.
//! * [`viewport`] maps room ↔ viewport ↔ frame-buffer coordinates.
//! * [`canvas`] is the DPR-aware frame buffer.
//!
//! ## The two facts that matter most
//!
//! 1. **A room is not 512×384.** Its logical size is the background's intrinsic
//!    size, floored at 512×384 ([`viewport::room_size_from_background`]). Larger
//!    backgrounds make larger rooms; smaller ones are padded. Never hard-code the
//!    classic size.
//! 2. **The compositor works in logical room pixels.** The DPR transform lives
//!    entirely in [`canvas`], so every draw call is in room units and HI-DPI is a
//!    presentation concern.
//!
//! ## Determinism
//!
//! The render path never reads the wall clock. Animation is driven by an
//! injected [`compositor::AnimationClock`], so two renders of the same scene are
//! byte-identical and can be image-diffed in CI.
//!
//! ## It never panics on asset data
//!
//! A missing background renders a flat backdrop; a missing or undecodable prop
//! renders a placeholder and records an [`error::AssetNote`]. Nothing is silently
//! skipped.
//!
//! ## Not in this milestone
//!
//! No Tauri, no windowing, no network, no IPTSCRAE. Draw commands are parsed by
//! `palace-room` but not rasterized (0 of 799 corpus rooms contain one); name
//! tags and chat text are font work for the presentation layer.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod assets;
pub mod build;
pub mod canvas;
pub mod compositor;
pub mod corpus;
pub mod draw;
pub mod error;
pub mod face;
pub mod image_clut;
pub mod nametag;
pub mod scene;
pub mod viewport;

pub use assets::{decode_image_bytes, decode_prop_blob, DecodedProp, MediaStore, PropStore};
pub use build::{
    clamp_avatar_position, clamp_room_size, AvatarSpec, PlacedProp, SceneBuilder, AVATAR_HALF,
    AVATAR_SIZE, MAX_ROOM_DIMENSION,
};
pub use canvas::Canvas;
pub use compositor::{draw_into, render, sort_avatars, AnimationClock, RenderOptions};
pub use corpus::RoomSource;
pub use draw::{
    absolute_points, ellipse_geometry, is_ellipse, layer_of, rasterize_back, rasterize_front,
    rasterize_layer, DrawLayer, DrawList,
};
pub use error::{AssetNote, RenderError};
pub use face::{face_sheet_png, smiley_cell, COLOR_VARIANTS, FACE_CELL, FACE_VARIANTS};
pub use nametag::{
    draw_name_tag, measure_text, name_tag, name_tag_position, NameTag, MAX_NAME_TAG_CHARS,
    NAME_TAG_FONT_PX, NAME_TAG_GLOW_RADIUS, NAME_TAG_X_FUDGE, NAME_TAG_Y_OFFSET,
};
pub use scene::{
    layer_for_hotspot_flags, Avatar, AvatarPart, Layer, Scene, Sprite, FLAG_PICTURES_ABOVE_ALL,
    FLAG_PICTURES_ABOVE_NAME_TAGS, FLAG_PICTURES_ABOVE_PROPS,
};
pub use viewport::{
    clamp_dpr, clamp_zoom, room_size_from_background, PointF, RectF, ScaleMode, SizeF,
    ViewTransform, MAX_DPR, MAX_ZOOM, MIN_DPR, MIN_ROOM_HEIGHT, MIN_ROOM_WIDTH, MIN_ZOOM,
};

/// Re-export so downstream crates can name the room type without depending on
/// `palace-room` directly.
pub use palace_room::{Hotspot, LooseProp, PictureOverlay, RoomDesc};

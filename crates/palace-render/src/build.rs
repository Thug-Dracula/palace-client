//! Turning a [`RoomDesc`] plus local assets into a [`Scene`].
//!
//! This is where the geometry rules from the reference client live. Every rule
//! here is ported from a named source and marked as **verified** (read out of a
//! reference implementation) or **assumed** in the crate README.
//!
//! ## The rules
//!
//! * **Room size** = the background's intrinsic size, floored at 512×384
//!   ([`room_size_from_background`]). With no background, the size is that floor
//!   expanded to contain every piece of geometry, so nothing is clipped off the
//!   frame.
//! * **Image overlays** — a hotspot's *current state* selects a
//!   [`PictureOverlay`] by `pict_id`; the overlay is drawn **centred** on
//!   `hotspot.loc + state.pic_loc` (`HotSpotImage.mxml`:
//!   `x = hotspot.location.x + state.x - width/2`). Only the current state is
//!   visible.
//! * **Loose props** draw with their top-left at `loc`, in list order.
//! * **Avatars** are 44×44 anchors; each worn prop is placed at its prop-header
//!   `hOffset`/`vOffset` relative to the anchor box (`Avatar.mxml`). Positions
//!   are clamped to the room with a 22 px margin (`PalaceRoomView.updateSelfPosition`).

use std::collections::BTreeMap;

use palace_prop::PropImage;
use palace_room::RoomDesc;

use crate::assets::{MediaStore, PropStore};
use crate::error::AssetNote;
use crate::image_clut::image_clut as clut_lookup;
use crate::scene::{layer_for_hotspot_flags, Avatar, AvatarPart, Layer, Scene, Sprite};
use crate::viewport::{room_size_from_background, SizeF};

/// The drawn size of a Type 0 avatar.
pub const AVATAR_SIZE: i32 = 44;
/// Half a Type 0 avatar — the movement clamp margin and the draw anchor.
///
/// `PalaceRoomView.updateSelfPosition` clamps with `< 22` / `> width - 22`, so
/// the margin is a confirmed 22 px. `Avatar.mxml` positions the 44×44 container
/// at `x - 21`; that one-pixel disagreement is in the reference itself. We use
/// the symmetric 22 so the clamp and the anchor agree.
pub const AVATAR_HALF: i32 = 22;

/// One avatar to place in the scene.
///
/// A room payload carries no avatars — they arrive over `uLst`/`nPrs`/`mPrp`.
/// The caller supplies them; the CLI builds them from operator-provided prop
/// ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarSpec {
    /// Room x of the avatar anchor.
    pub x: i32,
    /// Room y of the avatar anchor.
    pub y: i32,
    /// Face index into the built-in sheet (clamped to `0..=12` when drawn).
    pub face: i16,
    /// Colour index into the built-in sheet (clamped to `0..=15` when drawn).
    pub color: i16,
    /// Optional label, for reporting only.
    pub name: Option<String>,
    /// Worn prop ids, drawn in order (up to nine in the real client).
    pub props: Vec<u32>,
}

impl AvatarSpec {
    /// An avatar wearing `props` at `(x, y)`.
    ///
    /// The built-in face defaults to cell `(0, 0)`; use [`AvatarSpec::with_face_color`]
    /// to select a different one.
    #[must_use]
    pub fn new(x: i32, y: i32, props: Vec<u32>) -> Self {
        AvatarSpec {
            x,
            y,
            face: 0,
            color: 0,
            name: None,
            props,
        }
    }

    /// Builder-style setter for the built-in face cell.
    #[must_use]
    pub fn with_face_color(mut self, face: i16, color: i16) -> Self {
        self.face = face;
        self.color = color;
        self
    }
}

/// The largest logical room edge the renderer will allocate for.
///
/// Palace rooms are well under this (the live corpus tops out around 1280 px),
/// so it is a safety cap rather than a real limit: it stops a malformed payload
/// with a wild coordinate, or a corrupted background, from asking for a
/// multi-gigabyte frame buffer.
pub const MAX_ROOM_DIMENSION: u32 = 8192;

/// Clamp a requested room size into `[512×384, MAX_ROOM_DIMENSION²]`.
#[must_use]
pub fn clamp_room_size(width: f64, height: f64) -> (u32, u32) {
    let w = width
        .max(crate::viewport::MIN_ROOM_WIDTH)
        .min(f64::from(MAX_ROOM_DIMENSION));
    let h = height
        .max(crate::viewport::MIN_ROOM_HEIGHT)
        .min(f64::from(MAX_ROOM_DIMENSION));
    (w as u32, h as u32)
}

/// A loose prop to place in a scene at an explicit position.
///
/// The room's own loose props come from the payload; this type lets a caller add
/// more (a render test, or an operator placing a prop by hand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedProp {
    /// Asset id.
    pub id: u32,
    /// Logical room x of the prop's top-left.
    pub x: i32,
    /// Logical room y of the prop's top-left.
    pub y: i32,
}

/// Clamp an avatar anchor so the whole 44×44 sprite stays inside the room.
///
/// Ported from `PalaceRoomView.updateSelfPosition`, which clamps with a 22 px
/// margin (`half of a 44 px avatar`). A room smaller than the avatar pins the
/// coordinate to the top-left.
#[must_use]
pub fn clamp_avatar_position(x: i32, y: i32, room_width: i32, room_height: i32) -> (i32, i32) {
    let max_x = (room_width - AVATAR_HALF).max(AVATAR_HALF);
    let max_y = (room_height - AVATAR_HALF).max(AVATAR_HALF);
    (x.clamp(AVATAR_HALF, max_x), y.clamp(AVATAR_HALF, max_y))
}

/// Build scenes from a room description and local asset stores.
#[derive(Debug)]
pub struct SceneBuilder {
    media: MediaStore,
    props: PropStore,
    pic_opacity: BTreeMap<(i16, i16), f64>,
    name_tags_visible: bool,
}

impl Default for SceneBuilder {
    fn default() -> Self {
        SceneBuilder {
            media: MediaStore::default(),
            props: PropStore::default(),
            pic_opacity: BTreeMap::new(),
            name_tags_visible: true,
        }
    }
}

impl SceneBuilder {
    /// A builder over the given media and prop stores.
    #[must_use]
    pub fn new(media: MediaStore, props: PropStore) -> Self {
        SceneBuilder {
            media,
            props,
            ..SceneBuilder::default()
        }
    }

    /// Show or hide name tags on the scenes this builder produces.
    ///
    /// Defaults to visible; a client flips this when the user toggles names in
    /// the UI and then rebuilds (or clears [`Scene::name_tags_visible`] on the
    /// scene it already holds).
    pub fn set_name_tags_visible(&mut self, visible: bool) {
        self.name_tags_visible = visible;
    }

    /// Whether name tags are currently shown.
    #[must_use]
    pub fn name_tags_visible(&self) -> bool {
        self.name_tags_visible
    }

    /// Draw a hotspot's picture at `alpha`, keyed by hotspot id and state index.
    /// `SETPICOPACITY` is a local-only effect, so this is a render input rather
    /// than a field of the room record.
    pub fn set_pic_opacity(&mut self, spot: i16, state: i16, alpha: f64) {
        self.pic_opacity
            .insert((spot, state), alpha.clamp(0.0, 1.0));
    }

    pub fn clear_pic_opacity(&mut self) {
        self.pic_opacity.clear();
    }

    /// The media store (backgrounds and overlays).
    #[must_use]
    pub fn media(&self) -> &MediaStore {
        &self.media
    }

    /// The prop store.
    #[must_use]
    pub fn props(&self) -> &PropStore {
        &self.props
    }

    /// The media store, for a live client that registers fetched files.
    pub fn media_mut(&mut self) -> &mut MediaStore {
        &mut self.media
    }

    /// The prop store, for a live client that inserts received blobs.
    pub fn props_mut(&mut self) -> &mut PropStore {
        &mut self.props
    }

    /// Resolve a room and `avatars` into a drawable [`Scene`].
    ///
    /// Never fails: a missing asset becomes a [`AssetNote`] plus a placeholder or
    /// flat backdrop, and the geometry is still rendered.
    #[must_use]
    pub fn build(&self, room: &RoomDesc, avatars: &[AvatarSpec]) -> Scene {
        self.build_with(room, avatars, &[])
    }

    /// As [`SceneBuilder::build`], plus `extra_props` placed on top of the room's
    /// own loose props (in the order given).
    #[must_use]
    pub fn build_with(
        &self,
        room: &RoomDesc,
        avatars: &[AvatarSpec],
        extra_props: &[PlacedProp],
    ) -> Scene {
        let mut notes: Vec<AssetNote> = room
            .warnings
            .iter()
            .map(|w| AssetNote::RoomWarning(w.to_string()))
            .collect();

        let background = self.load_background(room, &mut notes);
        let (width, height) = self.logical_size(room, background.as_ref());
        let mut scene = Scene::new(width, height);
        scene.dim_level = 1.0;

        let mut overlays: [Vec<Sprite>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for hotspot in &room.hotspots {
            let Some(sprite) = self.hotspot_overlay(room, hotspot, &mut notes) else {
                continue;
            };
            let band = layer_for_hotspot_flags(hotspot.flags);
            overlays[band_index(band)].push(sprite);
        }
        scene.overlays_above_nothing = std::mem::take(&mut overlays[0]);
        scene.overlays_above_avatars = std::mem::take(&mut overlays[1]);
        scene.overlays_above_name_tags = std::mem::take(&mut overlays[2]);
        scene.overlays_above_everything = std::mem::take(&mut overlays[3]);

        for (index, loose) in room.loose_props.iter().enumerate() {
            scene.loose_props.push(self.loose_prop(
                loose.spec.id,
                i32::from(loose.loc.h),
                i32::from(loose.loc.v),
                index as i64,
                &mut notes,
            ));
        }
        for (offset, extra) in extra_props.iter().enumerate() {
            let z = (room.loose_props.len() + offset) as i64;
            scene
                .loose_props
                .push(self.loose_prop(extra.id, extra.x, extra.y, z, &mut notes));
        }

        for spec in avatars {
            let (x, y) = clamp_avatar_position(spec.x, spec.y, width as i32, height as i32);
            scene
                .avatars
                .push(self.build_avatar(x, y, spec, &mut notes));
        }

        scene.draw = crate::draw::DrawList::from_commands(room.draw_cmds.iter().cloned());

        scene.background = background;
        scene.notes = notes;
        scene.name_tags_visible = self.name_tags_visible;
        scene
    }

    fn loose_prop(&self, id: u32, x: i32, y: i32, z: i64, notes: &mut Vec<AssetNote>) -> Sprite {
        let decoded = self.props.prop_or_placeholder(id, notes);
        Sprite::new(decoded.image, x, y, z).with_alpha(decoded.alpha)
    }

    fn load_background(&self, room: &RoomDesc, notes: &mut Vec<AssetNote>) -> Option<PropImage> {
        if room.picture.is_empty() {
            notes.push(AssetNote::MissingBackground {
                name: room.picture.clone(),
                detail: "room names no background".to_string(),
            });
            return None;
        }
        match self.media.load(&room.picture) {
            Ok(image) => Some(image),
            Err(err) => {
                notes.push(AssetNote::MissingBackground {
                    name: room.picture.clone(),
                    detail: err.to_string(),
                });
                None
            }
        }
    }

    /// The logical room size.
    ///
    /// With a background this is its intrinsic size floored at 512×384. Without
    /// one, the floor is expanded to contain every overlay, loose prop and avatar
    /// so a backgroundless render is still inspectable.
    fn logical_size(&self, room: &RoomDesc, background: Option<&PropImage>) -> (u32, u32) {
        if let Some(image) = background {
            let size = room_size_from_background(image.width(), image.height());
            return clamp_room_size(size.width, size.height);
        }
        let mut size = SizeF::new(
            crate::viewport::MIN_ROOM_WIDTH,
            crate::viewport::MIN_ROOM_HEIGHT,
        );
        let mut grow = |right: i32, bottom: i32| {
            size.width = size.width.max(f64::from(right));
            size.height = size.height.max(f64::from(bottom));
        };
        for hotspot in &room.hotspots {
            grow(i32::from(hotspot.loc.h), i32::from(hotspot.loc.v));
        }
        for loose in &room.loose_props {
            grow(
                i32::from(loose.loc.h) + AVATAR_SIZE,
                i32::from(loose.loc.v) + AVATAR_SIZE,
            );
        }
        clamp_room_size(size.width, size.height)
    }

    /// The image overlay for a hotspot's current state, if it has one.
    fn hotspot_overlay(
        &self,
        room: &RoomDesc,
        hotspot: &palace_room::Hotspot,
        notes: &mut Vec<AssetNote>,
    ) -> Option<Sprite> {
        let state_index = usize::try_from(hotspot.state).ok()?;
        let state = hotspot.states.get(state_index)?;
        let overlay = room
            .pictures
            .iter()
            .find(|picture| picture.pic_id == state.pict_id)?;
        let Some(name) = overlay.name.as_deref() else {
            notes.push(AssetNote::OverlayWithoutFile {
                pic_id: state.pict_id,
            });
            return None;
        };
        let mut image = match self.media.load(name) {
            Ok(image) => image,
            Err(err) => {
                notes.push(AssetNote::BadOverlay {
                    pic_id: state.pict_id,
                    file: name.to_string(),
                    detail: err.to_string(),
                });
                return None;
            }
        };
        apply_transparency_index(&mut image, overlay.trans_color);
        let alpha = self
            .pic_opacity
            .get(&(hotspot.id, hotspot.state))
            .copied()
            .unwrap_or(1.0);
        let width = image.width() as i32;
        let height = image.height() as i32;
        let x = i32::from(hotspot.loc.h) + i32::from(state.pic_loc.h) - width / 2;
        let y = i32::from(hotspot.loc.v) + i32::from(state.pic_loc.v) - height / 2;
        Some(Sprite::new(image, x, y, 0).with_alpha(alpha))
    }

    /// Stack an avatar's worn props around its anchor.
    ///
    /// The built-in face is added under the props unless a worn prop carries the
    /// `HEAD` flag (`Avatar.mxml`'s `checkFaceProps`), so a prop-less user — one
    /// the old runtime skipped entirely — is still visible.
    fn build_avatar(
        &self,
        x: i32,
        y: i32,
        spec: &AvatarSpec,
        notes: &mut Vec<AssetNote>,
    ) -> Avatar {
        let mut parts = Vec::with_capacity(spec.props.len() + 1);
        let mut has_head_prop = false;
        for id in &spec.props {
            let decoded = self.props.prop_or_placeholder(*id, notes);
            has_head_prop |= decoded.is_head;
            parts.push(AvatarPart {
                dx: -AVATAR_HALF + i32::from(decoded.h_offset),
                dy: -AVATAR_HALF + i32::from(decoded.v_offset),
                image: decoded.image,
                alpha: decoded.alpha,
            });
        }
        if !has_head_prop {
            parts.insert(
                0,
                AvatarPart {
                    image: crate::face::smiley_cell(spec.face, spec.color),
                    dx: 0,
                    dy: 0,
                    alpha: 1.0,
                },
            );
        }
        Avatar {
            x,
            y,
            parts,
            name: spec.name.clone(),
        }
    }
}

fn band_index(layer: Layer) -> usize {
    match layer {
        Layer::AboveNothing => 0,
        Layer::AboveAvatars => 1,
        Layer::AboveNameTags => 2,
        Layer::AboveEverything => 3,
    }
}

/// Apply the `PictureRec.transColor` rule to an overlay image.
///
/// Ported from `PalaceImageOverlay.processTransparency`:
///
/// * an image that already has any transparent pixel is left alone;
/// * `-1` means "no transparency";
/// * `0` means "the colour of pixel `(1, height - 1)` is the transparent one";
/// * any other value is an index into the hotspot-image palette
///   ([`crate::image_clut`]), whose colour becomes transparent.
///
/// The reference zeroes the matching pixels' alpha; so does this. One deliberate
/// divergence: the reference compares through a `0x00AAFF00` mask (which ignores
/// the blue channel) — an artefact of the Flash `threshold` call — while this
/// compares RGB exactly.
pub fn apply_transparency_index(image: &mut PropImage, trans_color: i16) {
    if trans_color < 0 || has_any_transparency(image) {
        return;
    }
    let Some(target) = transparency_colour(image, trans_color) else {
        return;
    };
    let (width, height) = (image.width(), image.height());
    let mut rgba = Vec::with_capacity(image.as_rgba().len());
    let mut changed = false;
    for y in 0..height {
        for x in 0..width {
            let Some(pixel) = image.pixel(x, y) else {
                continue;
            };
            if [pixel[0], pixel[1], pixel[2]] == target {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                changed = true;
            } else {
                rgba.extend_from_slice(&pixel);
            }
        }
    }
    if changed {
        if let Ok(updated) = PropImage::from_rgba(width, height, rgba) {
            *image = updated;
        }
    }
}

fn has_any_transparency(image: &PropImage) -> bool {
    image.as_rgba().chunks_exact(4).any(|p| p[3] < 0xFF)
}

fn transparency_colour(image: &PropImage, trans_color: i16) -> Option<[u8; 3]> {
    if trans_color > 0 {
        let argb = clut_lookup(trans_color as u8);
        return Some([
            ((argb >> 16) & 0xFF) as u8,
            ((argb >> 8) & 0xFF) as u8,
            (argb & 0xFF) as u8,
        ]);
    }
    let pixel = image.pixel(1, image.height().saturating_sub(1))?;
    Some([pixel[0], pixel[1], pixel[2]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::PropStore;
    use crate::scene::{
        FLAG_PICTURES_ABOVE_ALL, FLAG_PICTURES_ABOVE_NAME_TAGS, FLAG_PICTURES_ABOVE_PROPS,
    };

    fn builder() -> SceneBuilder {
        SceneBuilder::new(MediaStore::default(), PropStore::new())
    }

    #[test]
    fn avatar_positions_clamp_with_a_22px_margin() {
        // Confirmed against PalaceRoomView.updateSelfPosition.
        assert_eq!(clamp_avatar_position(-100, -100, 512, 384), (22, 22));
        assert_eq!(clamp_avatar_position(1000, 1000, 512, 384), (490, 362));
        assert_eq!(clamp_avatar_position(200, 100, 512, 384), (200, 100));
        // A room smaller than the avatar pins to the margin rather than inverting.
        let (x, y) = clamp_avatar_position(0, 0, 10, 10);
        assert_eq!((x, y), (22, 22));
    }

    #[test]
    fn an_empty_room_still_produces_a_scene_of_the_floor_size() {
        let payload = vec![0u8; 40];
        let room = palace_room::decode_payload(&payload, palace_wire::ByteOrder::Little)
            .expect("empty room decodes");
        let scene = builder().build(&room, &[]);
        assert_eq!(scene.size, (512, 384));
        assert!(scene.background.is_none());
        assert!(scene
            .notes
            .iter()
            .any(|n| matches!(n, AssetNote::MissingBackground { .. })));
    }

    #[test]
    fn transparency_index_zero_uses_the_bottom_left_pixel() {
        // 2x2 with a distinctive bottom-left (x=1, y=1) pixel; pixel (1,1) is the
        // one the reference reads.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[10, 20, 30, 255]); // (0,0)
        bytes.extend_from_slice(&[1, 2, 3, 255]); // (1,0)
        bytes.extend_from_slice(&[40, 50, 60, 255]); // (0,1)
        bytes.extend_from_slice(&[99, 98, 97, 255]); // (1,1) bottom-left
        let mut image = PropImage::from_rgba(2, 2, bytes).expect("image");
        apply_transparency_index(&mut image, 0);
        assert_eq!(image.pixel(1, 1).expect("px")[3], 0, "target cleared");
        assert_eq!(image.pixel(0, 0).expect("px")[3], 255, "other kept");
    }

    #[test]
    fn an_image_that_already_has_alpha_is_untouched() {
        let bytes = vec![10, 20, 30, 128, 1, 2, 3, 255, 40, 50, 60, 255, 7, 8, 9, 255];
        let mut image = PropImage::from_rgba(2, 2, bytes.clone()).expect("image");
        apply_transparency_index(&mut image, 0);
        assert_eq!(image.as_rgba(), bytes.as_slice());
    }

    #[test]
    fn transparency_index_minus_one_is_a_no_op() {
        let bytes = vec![10, 20, 30, 255, 1, 2, 3, 255, 40, 50, 60, 255, 7, 8, 9, 255];
        let mut image = PropImage::from_rgba(2, 2, bytes.clone()).expect("image");
        apply_transparency_index(&mut image, -1);
        assert_eq!(image.as_rgba(), bytes.as_slice());
    }

    #[test]
    fn a_positive_transparency_index_looks_up_the_hotspot_palette() {
        // Index 1 is 0xffffdfff -> rgb(255, 223, 255).
        let bytes = vec![
            255, 223, 255, 255, 1, 2, 3, 255, 40, 50, 60, 255, 7, 8, 9, 255,
        ];
        let mut image = PropImage::from_rgba(2, 2, bytes).expect("image");
        apply_transparency_index(&mut image, 1);
        assert_eq!(image.pixel(0, 0).expect("px")[3], 0);
        assert_eq!(image.pixel(1, 0).expect("px")[3], 255);
    }

    #[test]
    fn a_picture_opacity_override_reaches_the_overlay_sprite() {
        let dir =
            std::env::temp_dir().join(format!("palace-render-opacity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("over.png");
        {
            let file = std::fs::File::create(&path).expect("create png");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 4, 4);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let pixels = vec![255u8; 4 * 4 * 4];
            writer.write_image_data(&pixels).expect("png data");
        }

        let mut media = MediaStore::default();
        assert!(
            !media.insert_path("over.png", path),
            "a fresh store has not indexed this name yet"
        );

        let mut room = empty_room();
        room.pictures.push(palace_room::PictureOverlay {
            pic_id: 7,
            name: Some("over.png".to_string()),
            trans_color: -1,
            ..palace_room::PictureOverlay::default()
        });
        room.hotspots.push(palace_room::Hotspot {
            id: 3,
            state: 0,
            loc: palace_wire::messages::Point::new(100, 100),
            states: vec![palace_room::HotspotState {
                pict_id: 7,
                ..palace_room::HotspotState::default()
            }],
            ..palace_room::Hotspot::default()
        });

        let mut builder = SceneBuilder::new(media, PropStore::new());
        let before = builder.build(&room, &[]);
        assert_eq!(
            before.overlays_above_nothing[0].alpha, 1.0,
            "a picture is opaque until a script says otherwise"
        );

        builder.set_pic_opacity(3, 0, 0.25);
        let after = builder.build(&room, &[]);
        assert_eq!(after.overlays_above_nothing[0].alpha, 0.25);

        builder.clear_pic_opacity();
        let cleared = builder.build(&room, &[]);
        assert_eq!(cleared.overlays_above_nothing[0].alpha, 1.0);
    }

    #[test]
    fn avatar_parts_are_relative_to_the_anchor_not_absolute() {
        let dir = std::env::temp_dir().join(format!("palace-render-avatar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let image = PropImage::from_rgba(44, 44, vec![255; 44 * 44 * 4]).expect("image");
        let blob = palace_prop::encode_s20_blob(&image, 7, 7, 0).expect("encode");
        std::fs::write(dir.join("9001_unnamed.bin"), &blob).expect("write");

        let mut props = PropStore::new();
        props.add_directory(&dir);
        let scene_builder = SceneBuilder::new(MediaStore::default(), props);
        let empty =
            palace_room::decode_payload(&[0u8; 40], palace_wire::ByteOrder::Little).expect("room");
        let scene = scene_builder.build(&empty, &[AvatarSpec::new(300, 200, vec![9001])]);
        let avatar = &scene.avatars[0];
        assert_eq!((avatar.x, avatar.y), (300, 200));
        // The compositor places a part at (avatar.x + dx, avatar.y + dy), so dx
        // must be the -22 box half plus the prop offset, never the absolute x.
        let worn_prop_after_the_face = avatar.parts.get(1).expect("the worn prop is present");
        assert_eq!(worn_prop_after_the_face.dx, -AVATAR_HALF + 7);
        assert_eq!(worn_prop_after_the_face.dy, -AVATAR_HALF + 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_loose_props_append_after_the_rooms_own() {
        let scene_builder = builder();
        let empty =
            palace_room::decode_payload(&[0u8; 40], palace_wire::ByteOrder::Little).expect("room");
        let scene = scene_builder.build_with(
            &empty,
            &[],
            &[PlacedProp {
                id: 1,
                x: 10,
                y: 20,
            }],
        );
        assert_eq!(scene.loose_props.len(), 1);
        assert_eq!((scene.loose_props[0].x, scene.loose_props[0].y), (10, 20));
        assert!(scene
            .notes
            .iter()
            .any(|n| matches!(n, AssetNote::MissingProp { id: 1 })));
    }

    fn empty_room() -> RoomDesc {
        palace_room::decode_payload(&[0u8; 40], palace_wire::ByteOrder::Little).expect("empty room")
    }

    fn canvas_pixel(canvas: &crate::canvas::Canvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y as usize) * (canvas.width() as usize) + x as usize) * 4;
        let s = &canvas.as_rgba()[at..at + 4];
        [s[0], s[1], s[2], s[3]]
    }

    fn first_opaque(image: &PropImage) -> (u32, u32) {
        for y in 0..image.height() {
            for x in 0..image.width() {
                if matches!(image.pixel(x, y), Some(pixel) if pixel[3] == 255) {
                    return (x, y);
                }
            }
        }
        panic!("this face cell has no opaque pixels");
    }

    fn store_with_prop(tag: &str, id: u32, flags: u16) -> (std::path::PathBuf, PropStore) {
        let dir =
            std::env::temp_dir().join(format!("palace-render-face-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let image = PropImage::from_rgba(44, 44, vec![255; 44 * 44 * 4]).expect("image");
        let blob = palace_prop::encode_s20_blob(&image, 7, 7, flags).expect("encode");
        std::fs::write(dir.join(format!("{id}_unnamed.bin")), &blob).expect("write");
        let mut props = PropStore::new();
        props.add_directory(&dir);
        (dir, props)
    }

    /// A scene band as `(bucket, layer)` so a routing sweep can compare the two.
    fn bands_of(scene: &Scene) -> [(&[Sprite], Layer); 4] {
        [
            (scene.overlays_above_nothing.as_slice(), Layer::AboveNothing),
            (scene.overlays_above_avatars.as_slice(), Layer::AboveAvatars),
            (
                scene.overlays_above_name_tags.as_slice(),
                Layer::AboveNameTags,
            ),
            (
                scene.overlays_above_everything.as_slice(),
                Layer::AboveEverything,
            ),
        ]
    }

    /// Every hotspot-flag band must land in its own scene bucket with its own
    /// image. A hotspot routed to the wrong bucket is invisible in this crate's
    /// unit tests (each bucket looks fine alone) and only shows up as a wrong
    /// z-order on screen.
    #[test]
    fn each_hotspot_flag_routes_its_overlay_to_the_matching_scene_band() {
        let dir =
            std::env::temp_dir().join(format!("palace-render-band-routing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let flags = [
            (0, Layer::AboveNothing),
            (FLAG_PICTURES_ABOVE_PROPS, Layer::AboveAvatars),
            (FLAG_PICTURES_ABOVE_NAME_TAGS, Layer::AboveNameTags),
            (FLAG_PICTURES_ABOVE_ALL, Layer::AboveEverything),
        ];
        let mut media = MediaStore::default();
        let mut expected: BTreeMap<Layer, PropImage> = BTreeMap::new();
        for (index, (_flag, layer)) in flags.iter().enumerate() {
            let name = format!("band-{index}.png");
            let value = (index as u8 + 1) * 40;
            {
                let file = std::fs::File::create(dir.join(&name)).expect("create png");
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 1, 1);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("png header");
                writer
                    .write_image_data(&[value, value, value, 255])
                    .expect("png data");
            }
            assert!(
                !media.insert_path(&name, dir.join(&name)),
                "a fresh store has not indexed this name yet"
            );
            expected.insert(
                *layer,
                PropImage::from_rgba(1, 1, vec![value, value, value, 255]).expect("image"),
            );
        }

        let mut room = empty_room();
        for (index, (flag, _)) in flags.iter().enumerate() {
            let name = format!("band-{index}.png");
            room.pictures.push(palace_room::PictureOverlay {
                pic_id: index as i16 + 7,
                name: Some(name),
                trans_color: -1,
                ..palace_room::PictureOverlay::default()
            });
            room.hotspots.push(palace_room::Hotspot {
                id: index as i16 + 1,
                state: 0,
                flags: *flag,
                loc: palace_wire::messages::Point::new(index as i16 * 50, 0),
                states: vec![palace_room::HotspotState {
                    pict_id: index as i16 + 7,
                    ..palace_room::HotspotState::default()
                }],
                ..palace_room::Hotspot::default()
            });
        }

        let scene = SceneBuilder::new(media, PropStore::new()).build(&room, &[]);
        for (bucket, layer) in bands_of(&scene) {
            assert_eq!(
                bucket.len(),
                1,
                "exactly one hotspot routes to {layer:?}, got {}",
                bucket.len()
            );
            assert_eq!(
                bucket[0].image, expected[&layer],
                "{layer:?} must hold the image of the hotspot flagged for it"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_prop_less_avatar_renders_its_face_cell_at_the_anchor() {
        use crate::compositor::{render, RenderOptions};

        let scene = builder().build(
            &empty_room(),
            &[AvatarSpec::new(200, 150, vec![]).with_face_color(3, 5)],
        );
        let avatar = &scene.avatars[0];
        assert_eq!(avatar.parts.len(), 1, "a prop-less avatar is just its face");
        let face = &avatar.parts[0];
        assert_eq!(
            (face.dx, face.dy),
            (0, 0),
            "the trimmed cell draws at the anchor"
        );
        assert_eq!(
            face.image,
            crate::face::smiley_cell(3, 5),
            "the (3, 5) cell, not some other"
        );

        let canvas = render(&scene, RenderOptions::at_dpr(1.0));
        let (fx, fy) = first_opaque(&face.image);
        assert_eq!(
            canvas_pixel(&canvas, 200 + fx, 150 + fy),
            face.image.pixel(fx, fy).expect("in bounds"),
            "the face's pixels reach the frame at (avatar.x, avatar.y)"
        );
    }

    #[test]
    fn different_face_and_colour_cells_render_different_pixels() {
        use crate::compositor::{render, RenderOptions};

        let room = empty_room();
        let one = builder().build(
            &room,
            &[AvatarSpec::new(200, 150, vec![]).with_face_color(0, 0)],
        );
        let two = builder().build(
            &room,
            &[AvatarSpec::new(200, 150, vec![]).with_face_color(4, 6)],
        );
        let a = render(&one, RenderOptions::at_dpr(1.0));
        let b = render(&two, RenderOptions::at_dpr(1.0));
        assert_ne!(
            a.as_rgba(),
            b.as_rgba(),
            "two different (face, colour) values must not render identically"
        );
    }

    #[test]
    fn a_head_prop_hides_the_builtin_face() {
        let (dir, props) = store_with_prop("head", 7001, palace_prop::FLAG_HEAD);
        let scene_builder = SceneBuilder::new(MediaStore::default(), props);
        let scene = scene_builder.build(
            &empty_room(),
            &[AvatarSpec::new(300, 200, vec![7001]).with_face_color(2, 4)],
        );
        let avatar = &scene.avatars[0];
        assert_eq!(avatar.parts.len(), 1, "only the head prop is drawn");
        assert_ne!(
            avatar.parts[0].image,
            crate::face::smiley_cell(2, 4),
            "the head prop must suppress the built-in face, not sit beside it"
        );
        assert_eq!(
            avatar.parts[0].dx,
            -AVATAR_HALF + 7,
            "the prop keeps its header offset"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_head_prop_keeps_the_builtin_face() {
        let (dir, props) = store_with_prop("body", 7002, 0);
        let scene_builder = SceneBuilder::new(MediaStore::default(), props);
        let scene = scene_builder.build(
            &empty_room(),
            &[AvatarSpec::new(300, 200, vec![7002]).with_face_color(1, 2)],
        );
        let avatar = &scene.avatars[0];
        assert_eq!(avatar.parts.len(), 2, "face plus the worn prop");
        assert_eq!(avatar.parts[0].image, crate::face::smiley_cell(1, 2));
        assert_eq!((avatar.parts[0].dx, avatar.parts[0].dy), (0, 0));
        assert_eq!(avatar.parts[1].dx, -AVATAR_HALF + 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_face_and_colour_are_clamped_when_drawn() {
        use crate::compositor::{render, RenderOptions};

        let room = empty_room();
        let wild = builder().build(
            &room,
            &[AvatarSpec::new(200, 150, vec![]).with_face_color(99, -7)],
        );
        let edge = builder().build(
            &room,
            &[AvatarSpec::new(200, 150, vec![]).with_face_color(12, 0)],
        );
        assert_eq!(
            wild.avatars[0].parts[0].image, edge.avatars[0].parts[0].image,
            "out-of-range values clamp to the edge cell"
        );
        let a = render(&wild, RenderOptions::at_dpr(1.0));
        let b = render(&edge, RenderOptions::at_dpr(1.0));
        assert_eq!(a.as_rgba(), b.as_rgba(), "and the frames match");
    }

    #[test]
    fn name_tags_default_to_visible_and_the_builder_can_hide_them() {
        let room = empty_room();
        let mut scene_builder = builder();
        assert!(
            scene_builder.name_tags_visible(),
            "name tags default to visible"
        );

        let spec = AvatarSpec {
            name: Some("Rico".to_string()),
            ..AvatarSpec::new(200, 150, vec![])
        };
        let scene = scene_builder.build(&room, std::slice::from_ref(&spec));
        assert!(scene.name_tags_visible, "the default reaches the scene");
        assert_eq!(
            scene.avatars[0].name.as_deref(),
            Some("Rico"),
            "the spec's name reaches the avatar"
        );

        scene_builder.set_name_tags_visible(false);
        assert!(!scene_builder.name_tags_visible());
        let scene = scene_builder.build(&room, std::slice::from_ref(&spec));
        assert!(!scene.name_tags_visible, "the toggle reaches the scene");
        assert_eq!(
            scene.avatars[0].name.as_deref(),
            Some("Rico"),
            "hiding tags must not discard the name"
        );
    }
}

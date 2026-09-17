//! The compositor: layers a [`Scene`] into a [`Canvas`].
//!
//! ## Z-order (bottom → top)
//!
//! This is the order of the children of `PalaceRoomView.mxml`, which is the
//! authoritative description of what the reference client stacks where:
//!
//! | # | Layer | Source |
//! |---|---|---|
//! | 1 | flat backdrop, then the background image at `(0, 0)`, 1:1 | `backgroundImage` |
//! | 2 | hotspot-state overlays flagged *pictures above nothing* | `hotSpotCanvas` → `imageGroup` |
//! | 3 | room dim (`DIMROOM`) — dims layers 1–2 only | the dim `s:Rect` |
//! | 4 | draw commands flagged *back* | `PaintLayer(drawBackCommands)` |
//! | 5 | loose props | `loosePropsGroup` |
//! | 6 | avatars, y-sorted | `avatarsGroup` |
//! | 7 | hotspot-state overlays flagged *pictures above props* | `hotSpotAboveAvatarsCanvas` |
//! | 8 | draw commands flagged *front* | `PaintLayer(drawFrontCommands)` |
//! | 9 | name tags | `nameTagGroup` |
//! | 10 | hotspot-state overlays flagged *pictures above name tags* | `hotSpotAboveNametagsCanvas` |
//! | 11 | chat text | `textGroup` |
//! | 12 | hotspot-state overlays flagged *pictures above all* | `hotSpotAboveEverythingCanvas` |
//!
//! Layers 4, 8, 9 and 11 are **not rasterized in this milestone**: no room in the
//! 799-payload corpus carries a draw command, and name tags and chat text are
//! font-dependent and belong with the presentation layer. They are listed here
//! so the ordering is on record rather than rediscovered.
//!
//! ## Avatar y-sort
//!
//! Avatars inside layer 6 are sorted by `(y, x)`, stably, so an avatar lower on
//! screen (larger `y`) draws in front of one behind it. See [`sort_avatars`].
//!
//! OpenPalace itself draws avatars in arrival order, and `linpal` draws them in
//! its client-list order; neither y-orders. The community web client *sparky*
//! does: it gives every avatar `zIndex = round(100 + y / roomHeight * 4999)` and
//! every loose prop `zIndex = 50 + index`, so avatars are y-ordered and always in
//! front of loose props. We reproduce that ordering explicitly (see the crate
//! README, "verified vs assumed").

use crate::canvas::Canvas;
use crate::scene::{Avatar, Scene, Sprite};

/// A monotonic animation clock.
///
/// The render path never reads the wall clock: callers pass this in, so two runs
/// with the same scene and clock produce byte-identical frames and CI can
/// image-diff them. This milestone renders one static frame, but the clock is in
/// the API so adding multi-frame prop animation later cannot accidentally
/// reintroduce wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnimationClock {
    millis: u64,
}

impl AnimationClock {
    /// A clock reading `millis` milliseconds since the client started.
    #[must_use]
    pub const fn at(millis: u64) -> Self {
        AnimationClock { millis }
    }

    /// The frozen zero clock used for deterministic renders.
    #[must_use]
    pub const fn zero() -> Self {
        AnimationClock { millis: 0 }
    }

    /// Milliseconds since the client started.
    #[must_use]
    pub const fn millis(&self) -> u64 {
        self.millis
    }

    /// Which frame of an animation with the given period this clock selects.
    ///
    /// A zero period always selects frame 0, so an unspecified animation cannot
    /// divide by zero.
    #[must_use]
    pub const fn frame_index(&self, period_ms: u64, frame_count: u32) -> u32 {
        if period_ms == 0 || frame_count == 0 {
            return 0;
        }
        ((self.millis / period_ms) % (frame_count as u64)) as u32
    }
}

/// How a frame should be rasterized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Device-pixel ratio, clamped to `[1, 2]` by the canvas.
    pub dpr: f64,
    /// The injected animation clock.
    pub clock: AnimationClock,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            dpr: 1.0,
            clock: AnimationClock::zero(),
        }
    }
}

impl RenderOptions {
    /// Options for a frame at `dpr` with a frozen clock.
    #[must_use]
    pub fn at_dpr(dpr: f64) -> Self {
        RenderOptions {
            dpr,
            clock: AnimationClock::zero(),
        }
    }
}

/// Sort avatars back-to-front by their room position.
///
/// The rule is `(y, x)` ascending: an avatar with a larger `y` is lower on
/// screen (further "in front") and is drawn later. The sort is stable, so two
/// avatars at the same position keep their insertion order — which makes the
/// output deterministic.
pub fn sort_avatars(avatars: &mut [Avatar]) {
    avatars.sort_by_key(|a| (a.y, a.x));
}

/// Draw a whole scene into a fresh canvas.
///
/// The canvas size comes from `scene.size` (the room's logical size) and the
/// requested DPR; nothing here assumes 512×384.
#[must_use]
pub fn render(scene: &Scene, options: RenderOptions) -> Canvas {
    let (width, height) = scene.logical_size();
    let mut canvas = Canvas::for_room(width, height, options.dpr);
    draw_into(&mut canvas, scene, options.clock);
    canvas
}

/// Draw a scene onto an existing canvas.
///
/// Exposed separately from [`render`] so a caller can reuse a buffer across
/// frames.
pub fn draw_into(canvas: &mut Canvas, scene: &Scene, _clock: AnimationClock) {
    canvas.fill(scene.backdrop);
    if let Some(background) = &scene.background {
        canvas.blit(background, 0.0, 0.0, 1.0);
    }

    blit_layer(canvas, &scene.overlays_above_nothing);
    canvas.apply_dim(scene.dim_level);
    blit_layer(canvas, &scene.loose_props);

    let mut avatars = scene.avatars.clone();
    sort_avatars(&mut avatars);
    for avatar in &avatars {
        draw_avatar(canvas, avatar);
    }

    blit_layer(canvas, &scene.overlays_above_avatars);
    if scene.name_tags_visible {
        for avatar in &avatars {
            if let Some(name) = avatar.name.as_deref() {
                crate::nametag::draw_name_tag(canvas, avatar.x, avatar.y, name);
            }
        }
    }
    blit_layer(canvas, &scene.overlays_above_name_tags);
    blit_layer(canvas, &scene.overlays_above_everything);
}

/// Blit one layer, stably ordered by each sprite's `z` key.
fn blit_layer(canvas: &mut Canvas, sprites: &[Sprite]) {
    let mut order: Vec<&Sprite> = sprites.iter().collect();
    order.sort_by_key(|s| s.z);
    for sprite in order {
        canvas.blit(
            &sprite.image,
            f64::from(sprite.x),
            f64::from(sprite.y),
            sprite.alpha,
        );
    }
}

/// Place every part of an avatar relative to its anchor.
fn draw_avatar(canvas: &mut Canvas, avatar: &Avatar) {
    for part in &avatar.parts {
        canvas.blit(
            &part.image,
            f64::from(avatar.x + part.dx),
            f64::from(avatar.y + part.dy),
            part.alpha,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::AvatarPart;
    use palace_prop::PropImage;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> PropImage {
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            bytes.extend_from_slice(&rgba);
        }
        PropImage::from_rgba(w, h, bytes).expect("valid image")
    }

    fn sprite(rgba: [u8; 4], x: i32, y: i32, z: i64) -> Sprite {
        Sprite::new(solid(1, 1, rgba), x, y, z)
    }

    fn avatar(rgba: [u8; 4], x: i32, y: i32) -> Avatar {
        Avatar {
            x,
            y,
            parts: vec![AvatarPart {
                image: solid(1, 1, rgba),
                dx: 0,
                dy: 0,
                alpha: 1.0,
            }],
            name: None,
        }
    }

    fn named_avatar(name: &str, x: i32, y: i32) -> Avatar {
        Avatar {
            name: Some(name.to_string()),
            ..avatar(GREEN, x, y)
        }
    }

    /// A square part centred on the anchor, the shape a real 44×44 prop has:
    /// its top-left sits at `(x - size/2, y - size/2)`.
    fn centred_avatar(rgba: [u8; 4], size: u32, x: i32, y: i32) -> Avatar {
        Avatar {
            x,
            y,
            parts: vec![AvatarPart {
                image: solid(size, size, rgba),
                dx: -(size as i32) / 2,
                dy: -(size as i32) / 2,
                alpha: 1.0,
            }],
            name: None,
        }
    }

    fn px(c: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y as usize) * (c.width() as usize) + x as usize) * 4;
        let s = &c.as_rgba()[at..at + 4];
        [s[0], s[1], s[2], s[3]]
    }

    #[test]
    fn layers_draw_bottom_to_top_in_the_reference_order() {
        let mut scene = Scene::new(4, 4);
        scene.backdrop = RED;
        scene.overlays_above_nothing = vec![sprite(GREEN, 0, 0, 0)];
        scene.loose_props = vec![sprite(BLUE, 0, 0, 0)];
        scene.avatars = vec![avatar(GREEN, 0, 0)];
        scene.overlays_above_everything = vec![sprite(BLUE, 0, 0, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        // Last layer wins at this pixel.
        assert_eq!(px(&c, 0, 0), BLUE);
        // A pixel only the loose-prop layer covers stays blue only if nothing
        // drawn later touched it; place the avatar elsewhere.
        let mut scene = Scene::new(4, 4);
        scene.loose_props = vec![sprite(BLUE, 0, 0, 0)];
        scene.avatars = vec![avatar(GREEN, 1, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 0, 0), BLUE, "loose prop");
        assert_eq!(px(&c, 1, 0), GREEN, "avatar over prop");
    }

    #[test]
    fn dim_only_affects_background_and_the_above_nothing_band() {
        let mut scene = Scene::new(2, 1);
        scene.backdrop = [200, 200, 200, 255];
        scene.overlays_above_nothing = vec![sprite([200, 200, 200, 255], 1, 0, 0)];
        scene.loose_props = vec![sprite([200, 200, 200, 255], 0, 0, 0)];
        scene.dim_level = 0.5;
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 0, 0), [200, 200, 200, 255], "props are not dimmed");
        // The backdrop pixel at x=1 is covered by the overlay, also dimmed.
        assert_eq!(px(&c, 1, 0), [100, 100, 100, 255], "overlay is dimmed");
    }

    #[test]
    fn layer_sprites_are_stable_sorted_by_z() {
        let mut scene = Scene::new(2, 1);
        scene.loose_props = vec![
            sprite(RED, 0, 0, 10),
            sprite(GREEN, 0, 0, 5),
            sprite(BLUE, 0, 0, 5),
        ];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        // z=5 group keeps insertion order, so GREEN (inserted first) is under
        // BLUE; z=10 RED is on top.
        assert_eq!(px(&c, 0, 0), RED);
    }

    #[test]
    fn avatars_are_sorted_so_larger_y_draws_in_front() {
        let mut avatars = vec![avatar(RED, 0, 10), avatar(GREEN, 0, 2), avatar(BLUE, 0, 6)];
        sort_avatars(&mut avatars);
        let ys: Vec<i32> = avatars.iter().map(|a| a.y).collect();
        assert_eq!(ys, vec![2, 6, 10]);
    }

    #[test]
    fn overlapping_avatars_draw_in_front_to_back_order() {
        let mut scene = Scene::new(2, 1);
        // The taller avatar is listed first but must end up behind.
        scene.avatars = vec![avatar(RED, 0, 0), avatar(GREEN, 0, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 0, 0), GREEN, "later (equal-y) avatar on top");
    }

    fn push_band(scene: &mut Scene, band: usize, s: Sprite) {
        match band {
            0 => scene.overlays_above_nothing.push(s),
            1 => scene.overlays_above_avatars.push(s),
            2 => scene.overlays_above_name_tags.push(s),
            _ => scene.overlays_above_everything.push(s),
        }
    }

    #[test]
    fn every_band_can_draw_on_its_own() {
        let colors = [RED, GREEN, BLUE, [255, 255, 0, 255]];
        for (band, color) in colors.iter().enumerate() {
            let mut scene = Scene::new(6, 4);
            // The band alone, away from the origin so the backdrop shows around it.
            push_band(&mut scene, band, sprite(*color, 1 + band as i32, 1, 0));
            let c = render(&scene, RenderOptions::at_dpr(1.0));
            assert_eq!(
                px(&c, 1 + band as u32, 1),
                *color,
                "band {band} must be drawn at all"
            );
        }
    }

    /// Every cross-band pair, not one hand-picked pair: an inverted comparison
    /// only shows up on the pairs that do not happen to be adjacent.
    #[test]
    fn every_cross_band_overlap_resolves_to_the_higher_band() {
        let colors = [RED, GREEN, BLUE, [255, 255, 0, 255]];
        for low in 0..4usize {
            for high in low + 1..4usize {
                let mut scene = Scene::new(4, 4);
                push_band(&mut scene, low, sprite(colors[low], 1, 1, 0));
                push_band(&mut scene, high, sprite(colors[high], 1, 1, 0));
                let c = render(&scene, RenderOptions::at_dpr(1.0));
                assert_eq!(
                    px(&c, 1, 1),
                    colors[high],
                    "band {high} must beat band {low} where they overlap"
                );
            }
        }
    }

    #[test]
    fn a_lower_band_does_not_win_a_pixel_its_higher_band_also_covers() {
        // The specifically reversed case: the lower band is drawn *after* the
        // higher band in list order. If the two band comparisons were swapped,
        // this is the frame where RED would win.
        let mut scene = Scene::new(2, 1);
        scene.overlays_above_everything = vec![sprite(RED, 0, 0, 0)];
        scene.overlays_above_nothing = vec![sprite(GREEN, 0, 0, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(
            px(&c, 0, 0),
            RED,
            "the above-everything band wins even when the lower band is listed second"
        );
    }

    #[test]
    fn a_still_lower_layer_does_not_win_either() {
        // Loose props sit below every overlay band; the sprite list order must not
        // change that.
        let mut scene = Scene::new(2, 1);
        scene.loose_props = vec![sprite(GREEN, 1, 0, 0)];
        scene.overlays_above_nothing = vec![sprite(RED, 0, 0, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 0, 0), RED, "overlay over the prop");
        assert_eq!(px(&c, 1, 0), GREEN, "prop where no overlay covers");
    }

    /// The two 44×44 bodies (78..=121 and 118..=161) overlap in x/y 118..=121. A
    /// wrong sort order shows up as the wrong body's colour in that band, which is
    /// exactly how the bug looks on screen.
    #[test]
    fn two_overlapping_avatars_draw_the_greater_y_on_top_whichever_way_they_arrive() {
        let behind = centred_avatar(RED, 44, 100, 100);
        let in_front = centred_avatar(GREEN, 44, 140, 140);
        let mut scene = Scene::new(512, 384);
        scene.avatars = vec![behind.clone(), in_front.clone()];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        for (x, y) in [(118, 118), (120, 120), (121, 121)] {
            assert_eq!(
                px(&c, x, y),
                GREEN,
                "greater y is nearer and must own the overlap at ({x},{y})"
            );
        }
        assert_eq!(
            px(&c, 100, 100),
            RED,
            "the farther avatar is only covered where they actually overlap"
        );

        // Same two avatars, listed the other way round: the sort, not the arrival
        // order, must decide.
        let mut scene = Scene::new(512, 384);
        scene.avatars = vec![in_front, behind];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 120, 120), GREEN, "greater y still on top");
    }

    #[test]
    fn avatars_at_the_same_y_draw_in_a_deterministic_order_with_the_second_on_top() {
        // Equal y sorts by x, so the same inputs must yield the same frame no
        // matter how many times it is rendered. If a HashMap ever leaked into the
        // avatar ordering this is the case that flips between runs.
        let left = avatar(RED, 100, 150);
        let right = avatar(GREEN, 200, 150);
        let mut scene = Scene::new(512, 384);
        scene.avatars = vec![left.clone(), right.clone()];
        let first = render(&scene, RenderOptions::at_dpr(1.0));
        let second = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(
            first.as_rgba(),
            second.as_rgba(),
            "re-rendering the same scene is byte-identical"
        );
        // The two parts do not overlap, so the tie-break is asserted directly.
        let mut for_sort = vec![left, right];
        sort_avatars(&mut for_sort);
        assert_eq!(for_sort[0].x, 100, "smaller x first");
        assert_eq!(for_sort[1].x, 200, "larger x later");
    }

    #[test]
    fn permuting_the_avatar_input_order_produces_identical_frames() {
        // Bodies centred at (120,160), (160,200) and (200,240) overlap pairwise,
        // so the frame is only input-order-independent if the key sorts them.
        let avatars = [
            centred_avatar(RED, 44, 120, 160),
            centred_avatar(GREEN, 44, 160, 200),
            centred_avatar(BLUE, 44, 200, 240),
        ];
        let mut scene = Scene::new(512, 384);
        scene.avatars = vec![avatars[0].clone(), avatars[1].clone(), avatars[2].clone()];
        let forward = render(&scene, RenderOptions::at_dpr(1.0));

        let mut scene = Scene::new(512, 384);
        scene.avatars = vec![avatars[2].clone(), avatars[0].clone(), avatars[1].clone()];
        let permuted = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(
            px(&forward, 160, 200),
            px(&permuted, 160, 200),
            "a pixel where two avatars overlap must not follow the input order"
        );
        assert_eq!(
            forward.as_rgba(),
            permuted.as_rgba(),
            "the y-sort must make the frame independent of the input order"
        );
    }

    #[test]
    fn a_background_is_drawn_at_the_origin_and_padded_by_the_backdrop() {
        let mut scene = Scene::new(4, 4);
        scene.backdrop = BLUE;
        scene.background = Some(solid(2, 2, RED));
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(px(&c, 0, 0), RED);
        assert_eq!(px(&c, 1, 1), RED);
        assert_eq!(px(&c, 3, 3), BLUE, "padding uses the backdrop");
    }

    #[test]
    fn render_size_is_the_scene_size_not_512x384() {
        let scene = Scene::new(1000, 300);
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!((c.width(), c.height()), (1000, 300));
        let c = render(&scene, RenderOptions::at_dpr(2.0));
        assert_eq!((c.width(), c.height()), (2000, 600));
    }

    #[test]
    fn the_clock_is_injected_and_deterministic() {
        let scene = Scene::new(1, 1);
        let a = render(
            &scene,
            RenderOptions {
                dpr: 1.0,
                clock: AnimationClock::at(1234),
            },
        );
        let b = render(
            &scene,
            RenderOptions {
                dpr: 1.0,
                clock: AnimationClock::at(1234),
            },
        );
        assert_eq!(a, b);
        assert_eq!(AnimationClock::at(99).frame_index(10, 4), 1);
        assert_eq!(AnimationClock::at(99).frame_index(0, 4), 0);
        assert_eq!(AnimationClock::zero().millis(), 0);
    }

    /// A frame coordinate that the name tag of `name` on an avatar at
    /// `(ax, ay)` paints, derived from the tag image and its placement.
    fn tag_pixel_frame_position(name: &str, ax: i32, ay: i32) -> (i32, i32) {
        let tag = crate::nametag::name_tag(name).expect("the tag renders");
        for y in 0..tag.image.height() {
            for x in 0..tag.image.width() {
                let opaque = tag.image.pixel(x, y).is_some_and(|p| p[3] == 255);
                if !opaque {
                    continue;
                }
                let (text_x, text_y) = crate::nametag::name_tag_position(ax, ay, tag.text_width);
                let fx = (text_x - f64::from(tag.origin_x) + f64::from(x)).floor() as i32;
                let fy = (text_y - f64::from(tag.origin_y) + f64::from(y)).floor() as i32;
                return (fx, fy);
            }
        }
        panic!("a rendered tag always has an opaque pixel");
    }

    #[test]
    fn a_named_avatar_renders_a_tag_an_unnamed_one_does_not() {
        let mut named = Scene::new(128, 96);
        named.avatars = vec![named_avatar("Bob", 64, 40)];
        let mut unnamed = Scene::new(128, 96);
        unnamed.avatars = vec![avatar(GREEN, 64, 40)];
        let a = render(&named, RenderOptions::at_dpr(1.0));
        let b = render(&unnamed, RenderOptions::at_dpr(1.0));
        assert_ne!(
            a.as_rgba(),
            b.as_rgba(),
            "a name must paint pixels the unnamed avatar does not"
        );
    }

    #[test]
    fn clearing_name_tags_visible_removes_the_tag_without_touching_the_avatar() {
        let mut shown = Scene::new(128, 96);
        shown.avatars = vec![named_avatar("Bob", 64, 40)];
        let mut hidden = Scene::new(128, 96);
        hidden.avatars = vec![named_avatar("Bob", 64, 40)];
        hidden.name_tags_visible = false;

        let with_tag = render(&shown, RenderOptions::at_dpr(1.0));
        let without_tag = render(&hidden, RenderOptions::at_dpr(1.0));
        assert_ne!(with_tag.as_rgba(), without_tag.as_rgba());

        let mut plain = Scene::new(128, 96);
        plain.avatars = vec![avatar(GREEN, 64, 40)];
        let nameless = render(&plain, RenderOptions::at_dpr(1.0));
        assert_eq!(
            without_tag.as_rgba(),
            nameless.as_rgba(),
            "hiding the tag must leave exactly the nameless frame"
        );
        assert_eq!(
            hidden.avatars[0].name.as_deref(),
            Some("Bob"),
            "hiding is a draw switch, not data loss"
        );
    }

    /// Layer 9 is above the `AboveAvatars` band and below the
    /// `AboveNameTags` band. Both halves are asserted at one shared pixel.
    #[test]
    fn name_tags_draw_above_avatars_and_below_the_above_name_tags_band() {
        let name = "M";
        let (ax, ay) = (64, 40);
        let (fx, fy) = tag_pixel_frame_position(name, ax, ay);
        let (ux, uy) = (fx as u32, fy as u32);

        let mut scene = Scene::new(128, 96);
        scene.avatars = vec![named_avatar(name, ax, ay)];
        scene.overlays_above_avatars = vec![sprite(RED, fx, fy, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_ne!(
            px(&c, ux, uy),
            RED,
            "the name tag must cover an AboveAvatars sprite at the same pixel"
        );

        let mut scene = Scene::new(128, 96);
        scene.avatars = vec![named_avatar(name, ax, ay)];
        scene.overlays_above_name_tags = vec![sprite(GREEN, fx, fy, 0)];
        let c = render(&scene, RenderOptions::at_dpr(1.0));
        assert_eq!(
            px(&c, ux, uy),
            GREEN,
            "an AboveNameTags sprite must cover the name tag at the same pixel"
        );
    }
}

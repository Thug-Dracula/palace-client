//! The avatar roster for the current room.
//!
//! Instead of painting every avatar into the room bitmap, the runtime describes
//! them as data and the webview draws them as sprites on top of the room image.
//! This module defines that contract: who is in the room, where each avatar
//! stands, and which artwork layers make it up.
//!
//! The roster is produced by the runtime and consumed by the webview sprite
//! layer; nothing in this module draws anything on its own.

use serde::Serialize;

use crate::frame::ViewGeometry;

/// Every avatar in the current room, plus the geometry needed to place them.
///
/// This is the whole payload the sprite layer receives. `version` increases
/// whenever the roster changes, so a consumer can ignore stale snapshots, and
/// `geometry` is the same view transform reported alongside the room frame so
/// sprite positions line up with the composited background.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AvatarRoster {
    /// Monotonic roster revision; a higher number supersedes lower ones.
    pub version: u64,
    /// The room these avatars belong to.
    pub room_id: i32,
    /// How the room bitmap is placed in the viewport, so sprites can share it.
    pub geometry: ViewGeometry,
    /// Whether player name tags should be drawn above avatars.
    pub name_tags_visible: bool,
    /// The avatars present in the room, in draw order.
    pub avatars: Vec<AvatarState>,
}

/// One avatar's identity, position and sprite layers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AvatarState {
    /// Server-assigned avatar id, unique within the session.
    pub id: i32,
    /// Display name shown next to (or over) the avatar.
    pub name: String,
    /// Room-space x coordinate, in room pixels.
    pub x: i32,
    /// Room-space y coordinate, in room pixels.
    pub y: i32,
    /// Facing direction code (which way the avatar looks).
    pub face: i16,
    /// Colour code applied to the avatar's face and body.
    pub color: i16,
    /// True when this avatar is the user's own.
    pub is_self: bool,
    /// True when the avatar is marked away (idle / not interacting).
    pub away: bool,
    /// The user's `avatarType`: [`AT_PROP`](palace_wire::messages::AT_PROP) for a
    /// classic prop avatar, [`AT_AVATAR`](palace_wire::messages::AT_AVATAR) for a
    /// Type 1 single-image avatar.
    pub avatar_type: i16,
    /// The artwork layers that make up the drawn avatar, back to front.
    pub parts: Vec<AvatarPartState>,
}

/// One artwork layer of an avatar, positioned relative to the avatar's anchor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AvatarPartState {
    /// Which piece of art this layer is (a face or a prop).
    pub art: AvatarArt,
    /// Horizontal offset from the avatar anchor, in room pixels.
    pub dx: i32,
    /// Vertical offset from the avatar anchor, in room pixels.
    pub dy: i32,
    /// Opacity, from 0.0 (invisible) to 1.0 (fully opaque).
    pub alpha: f64,
    /// Layer width in pixels.
    pub w: u32,
    /// Layer height in pixels.
    pub h: u32,
}

/// A tagged reference to a piece of avatar artwork.
///
/// Serializes with a `kind` tag so consumers can tell a face layer from a prop
/// layer without guessing: a face carries its own face and colour codes, while a
/// prop carries only its art id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AvatarArt {
    /// A face layer, parameterised by face shape and colour.
    Face {
        /// Face shape code.
        face: i16,
        /// Colour code for the face.
        color: i16,
    },
    /// A prop layer, identified by its art id.
    Prop {
        /// Identifier of the prop artwork to draw.
        id: u32,
    },
    /// A Type 1 avatar: one server-hosted image, identified by its hash.
    ///
    /// Distinct from [`AvatarArt::Prop`] on purpose: a Type 1 avatar is a single
    /// image the server stores and serves, not a 44x44 tile from the prop
    /// library. `hash` is the 20-byte content hash as lowercase hex, which is
    /// the key the `palace://type1-avatar/` route serves.
    Type1 {
        /// The content hash, lowercase hex.
        hash: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::Value;

    /// A minimal geometry with every field distinct enough to catch a swap.
    fn geometry() -> ViewGeometry {
        ViewGeometry {
            room_w: 512.0,
            room_h: 384.0,
            viewport_w: 960.0,
            viewport_h: 540.0,
            dpr: 1.0,
            zoom: 1.0,
            native: false,
            scale: 1.40625,
            content_x: 120.0,
            content_y: 0.0,
            content_w: 720.0,
            content_h: 540.0,
            bitmap_w: 512,
            bitmap_h: 384,
        }
    }

    /// Build the two-avatar fixture the shape test asserts on: one avatar with
    /// only a face layer, and one wearing a prop on top of its face.
    fn roster() -> AvatarRoster {
        AvatarRoster {
            version: 7,
            room_id: 42,
            geometry: geometry(),
            name_tags_visible: true,
            avatars: vec![
                AvatarState {
                    id: 1,
                    name: "Ada".to_string(),
                    x: 100,
                    y: 200,
                    face: 3,
                    color: 5,
                    is_self: true,
                    away: false,
                    avatar_type: palace_wire::messages::AT_PROP,
                    parts: vec![AvatarPartState {
                        art: AvatarArt::Face { face: 3, color: 5 },
                        dx: 0,
                        dy: -4,
                        alpha: 1.0,
                        w: 32,
                        h: 48,
                    }],
                },
                AvatarState {
                    id: 2,
                    name: "Bo".to_string(),
                    x: 300,
                    y: 250,
                    face: 1,
                    color: 2,
                    is_self: false,
                    away: true,
                    avatar_type: palace_wire::messages::AT_PROP,
                    parts: vec![
                        AvatarPartState {
                            art: AvatarArt::Face { face: 1, color: 2 },
                            dx: 0,
                            dy: -4,
                            alpha: 1.0,
                            w: 32,
                            h: 48,
                        },
                        AvatarPartState {
                            art: AvatarArt::Prop { id: 900 },
                            dx: 10,
                            dy: -20,
                            alpha: 0.5,
                            w: 24,
                            h: 24,
                        },
                    ],
                },
            ],
        }
    }

    /// Assert one `parts` entry has the required placement fields.
    fn assert_placement(part: &Value) {
        for key in ["dx", "dy", "alpha", "w", "h"] {
            assert!(
                part.get(key).is_some(),
                "part is missing the `{key}` field: {part}"
            );
        }
    }

    #[test]
    fn avatars_serialize_to_the_expected_json_shape() {
        let value = serde_json::to_value(roster()).expect("roster serializes");

        // Top level keys.
        for key in ["version", "room_id", "name_tags_visible", "avatars"] {
            assert!(
                value.get(key).is_some(),
                "roster is missing the `{key}` field: {value}"
            );
        }
        assert_eq!(value["version"], json!(7));
        assert_eq!(value["room_id"], json!(42));
        assert_eq!(value["name_tags_visible"], json!(true));

        let avatars = value["avatars"].as_array().expect("avatars is an array");
        assert_eq!(avatars.len(), 2, "one face-only and one prop avatar");

        // (a) Face-only avatar: exactly one part, tagged `face`.
        let ada = &avatars[0];
        assert_eq!(ada["name"], json!("Ada"));
        assert_eq!(ada["is_self"], json!(true));
        let ada_parts = ada["parts"].as_array().expect("parts is an array");
        assert_eq!(ada_parts.len(), 1);
        assert_eq!(ada_parts[0]["art"]["kind"], json!("face"));
        assert_eq!(ada_parts[0]["art"]["face"], json!(3));
        assert_eq!(ada_parts[0]["art"]["color"], json!(5));
        assert_placement(&ada_parts[0]);

        // (b) Prop avatar: face layer then prop layer tagged `prop` with an id.
        let bo = &avatars[1];
        assert_eq!(bo["name"], json!("Bo"));
        assert_eq!(bo["away"], json!(true));
        let bo_parts = bo["parts"].as_array().expect("parts is an array");
        assert_eq!(bo_parts.len(), 2);
        assert_eq!(bo_parts[0]["art"]["kind"], json!("face"));
        assert_eq!(bo_parts[1]["art"]["kind"], json!("prop"));
        assert_eq!(bo_parts[1]["art"]["id"], json!(900));
        assert_placement(&bo_parts[0]);
        assert_placement(&bo_parts[1]);
    }

    #[test]
    fn a_type1_avatar_is_one_tagged_image_layer_not_prop_tiles() {
        let hash = "a9993e364706816aba3e25717850c26c9cd0d89d";
        let mut roster = roster();
        roster.avatars[0].avatar_type = palace_wire::messages::AT_AVATAR;
        roster.avatars[0].parts = vec![AvatarPartState {
            art: AvatarArt::Type1 {
                hash: hash.to_string(),
            },
            dx: 0,
            dy: 0,
            alpha: 1.0,
            w: 132,
            h: 132,
        }];

        let value = serde_json::to_value(&roster).expect("roster serializes");
        let ada = &value["avatars"][0];
        assert_eq!(ada["avatar_type"], json!(palace_wire::messages::AT_AVATAR));
        let parts = ada["parts"].as_array().expect("parts is an array");
        assert_eq!(parts.len(), 1, "a Type 1 avatar is a single layer");
        assert_eq!(parts[0]["art"]["kind"], json!("type1"));
        assert_eq!(parts[0]["art"]["hash"], json!(hash));
        assert_eq!(parts[0]["w"], json!(132));
        assert_placement(&parts[0]);
    }
}

//! The client state a script can observe.
//!
//! [`HostView`] is a plain snapshot: the runtime rebuilds it from live session
//! state before each dispatch, so a script never borrows the network thread's
//! structures and dispatch stays a pure function of `(view, scripts, event)`.

use std::collections::BTreeMap;
use std::sync::Arc;

use palace_room::{Hotspot, RoomDesc};

/// The dimensions and origin offsets a prop header carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropFacts {
    /// Prop width in pixels.
    pub width: i32,
    /// Prop height in pixels.
    pub height: i32,
    /// Horizontal origin offset.
    pub h_offset: i32,
    /// Vertical origin offset.
    pub v_offset: i32,
}

/// Asset facts a script can answer geometry questions from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetFacts {
    /// Image size per picture id.
    pub pic_dims: BTreeMap<i32, (i32, i32)>,
    /// Header facts per prop asset id.
    pub prop_facts: BTreeMap<i64, PropFacts>,
}

/// A user currently in the room.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserView {
    /// User id.
    pub id: i32,
    /// Screen name.
    pub name: String,
    /// Horizontal position.
    pub x: i32,
    /// Vertical position.
    pub y: i32,
    /// Worn prop ids, in order.
    pub props: Vec<i64>,
}

/// A hotspot or door, as a script sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpotView {
    /// Hotspot id (`SELECT`, `SPOTNAME`, `GETSPOTSTATE` all key on this).
    pub id: i32,
    /// `HS_*` type: 0 spot, 1 door, 2 shuttable, 3 lockable, 4 bolt, 5 nav.
    pub kind: i32,
    /// Destination room (doors) or bolt id.
    pub dest: i32,
    /// Hotspot name, empty when unnamed.
    pub name: String,
    /// Current selected state index.
    pub state: i32,
    /// How many states the hotspot declares.
    pub nbr_states: i32,
    /// `HS_*` flags.
    pub flags: i32,
    /// Group id.
    pub group_id: i32,
    /// Nominal location `(x, y)`.
    pub loc: (i32, i32),
    /// Outline points `(x, y)`, empty for rectangular hotspots.
    pub points: Vec<(i32, i32)>,
    /// Per-state picture id and offset `(pict_id, dx, dy)`.
    pub state_pics: Vec<(i32, i32, i32)>,
}

impl SpotView {
    /// Build the script-visible form of a decoded hotspot.
    #[must_use]
    pub fn from_hotspot(spot: &Hotspot) -> Self {
        SpotView {
            id: i32::from(spot.id),
            kind: i32::from(spot.hotspot_type),
            dest: i32::from(spot.dest),
            name: spot.name.clone().unwrap_or_default(),
            state: i32::from(spot.state),
            nbr_states: i32::from(spot.nbr_states),
            flags: spot.flags,
            group_id: i32::from(spot.group_id),
            loc: (i32::from(spot.loc.h), i32::from(spot.loc.v)),
            points: spot
                .points
                .iter()
                .map(|p| (i32::from(p.h), i32::from(p.v)))
                .collect(),
            state_pics: spot
                .states
                .iter()
                .map(|s| {
                    (
                        i32::from(s.pict_id),
                        i32::from(s.pic_loc.h),
                        i32::from(s.pic_loc.v),
                    )
                })
                .collect(),
        }
    }

    /// Whether the room point `(x, y)` lies inside this hotspot.
    ///
    /// A hotspot with an outline is tested against its polygon, in absolute
    /// room coordinates first and then relative to [`SpotView::loc`] (the corpus
    /// contains both conventions). A 44×44 box centred on `loc` is kept as a
    /// floor for both cases — the avatar-sized rectangle the classic clients use
    /// when they have no picture to measure.
    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if self.points.len() >= 3 {
            if point_in_polygon(x, y, &self.points) {
                return true;
            }
            let shifted: Vec<(i32, i32)> = self
                .points
                .iter()
                .map(|(px, py)| (px + self.loc.0, py + self.loc.1))
                .collect();
            if point_in_polygon(x, y, &shifted) {
                return true;
            }
        }
        (x - self.loc.0).abs() <= 22 && (y - self.loc.1).abs() <= 22
    }
}

/// A loose prop on the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoosePropView {
    /// Asset id.
    pub id: i64,
    /// Horizontal position.
    pub x: i32,
    /// Vertical position.
    pub y: i32,
}

/// Everything a script can ask about.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostView {
    /// Your user id.
    pub self_id: i32,
    /// Your screen name.
    pub self_name: String,
    /// Your horizontal position.
    pub self_x: i32,
    /// Your vertical position.
    pub self_y: i32,
    /// Worn prop ids, in order.
    pub self_props: Vec<i64>,
    /// Current room id.
    pub room_id: i32,
    /// Current room name.
    pub room_name: String,
    /// Server name.
    pub server_name: String,
    /// Viewing-area width.
    pub room_width: i32,
    /// Viewing-area height.
    pub room_height: i32,
    /// You are a guest.
    pub is_guest: bool,
    /// You are an operator.
    pub is_wizard: bool,
    /// You own the server.
    pub is_god: bool,
    /// Mouse position `(x, y)` in room coordinates.
    pub mouse: (i32, i32),
    /// The last click was the right button.
    pub right_click: bool,
    /// Incoming chat text (`CHATSTR` for `ON INCHAT`).
    pub chat_string: String,
    /// The user a chat event concerns.
    pub who_chat: i32,
    /// The whisper target.
    pub who_target: i32,
    /// Face index (`SETFACE`).
    pub face: i32,
    /// Colour index (`SETCOLOR`).
    pub color: i32,
    /// Users in the room.
    pub users: Vec<UserView>,
    /// Hotspots and doors.
    pub spots: Vec<SpotView>,
    /// Loose props on the floor.
    pub loose_props: Vec<LoosePropView>,
    /// Geometry facts resolved from media files and prop blobs.
    pub assets: Arc<AssetFacts>,
}

impl HostView {
    /// Build the user and spot lists from a decoded room description.
    pub fn apply_room(&mut self, room: &RoomDesc) {
        self.room_id = i32::from(room.header.room_id);
        self.room_name = room.name.clone();
        self.spots = room.hotspots.iter().map(SpotView::from_hotspot).collect();
        self.loose_props = room
            .loose_props
            .iter()
            .map(|p| LoosePropView {
                id: i64::from(p.spec.id),
                x: i32::from(p.loc.h),
                y: i32::from(p.loc.v),
            })
            .collect();
    }

    /// Find a spot by id.
    #[must_use]
    pub fn spot(&self, id: i32) -> Option<&SpotView> {
        self.spots.iter().find(|s| s.id == id)
    }

    /// The highest-index spot containing `(x, y)`.
    ///
    /// Later hotspots are drawn on top, so a point covered by several resolves
    /// to the last one, matching the paint order the compositor uses.
    #[must_use]
    pub fn spot_at(&self, x: i32, y: i32) -> Option<&SpotView> {
        self.spots.iter().rev().find(|s| s.contains(x, y))
    }

    /// Index of a spot in [`HostView::spots`], or `-1`.
    #[must_use]
    pub fn spot_index(&self, id: i32) -> i64 {
        self.spots
            .iter()
            .position(|s| s.id == id)
            .map_or(-1, |i| i as i64)
    }

    /// A user by id.
    #[must_use]
    pub fn user(&self, id: i32) -> Option<&UserView> {
        self.users.iter().find(|u| u.id == id)
    }

    /// A user id by exact name (case-insensitive).
    #[must_use]
    pub fn user_by_name(&self, name: &str) -> i32 {
        self.users
            .iter()
            .find(|u| u.name.eq_ignore_ascii_case(name))
            .map_or(0, |u| u.id)
    }

    /// Index of a loose prop by asset id, or `-1`.
    #[must_use]
    pub fn loose_prop_index(&self, id: i64) -> i64 {
        self.loose_props
            .iter()
            .position(|p| p.id == id)
            .map_or(-1, |i| i as i64)
    }

    /// Whether you are standing inside the spot with id `id`.
    #[must_use]
    pub fn self_in_spot(&self, id: i32) -> bool {
        self.spot(id)
            .is_some_and(|s| s.contains(self.self_x, self.self_y))
    }

    /// The picture id and origin offset a spot/state pair selects.
    ///
    /// A negative state selects the spot's current state; a missing spot or an
    /// out-of-range state selects nothing.
    fn state_image(&self, spot: i32, state: i32) -> Option<(i32, i32, i32)> {
        let spot = self.spot(spot)?;
        let state = if state < 0 { spot.state } else { state };
        let index = usize::try_from(state).ok()?;
        spot.state_pics.get(index).copied()
    }

    /// The image size of a hotspot state (`GETPICDIMENSIONS`).
    #[must_use]
    pub fn pic_dimensions(&self, spot: i32, state: i32) -> (i32, i32) {
        self.state_image(spot, state)
            .and_then(|(pict_id, _, _)| self.assets.pic_dims.get(&pict_id).copied())
            .unwrap_or((0, 0))
    }

    /// The image origin of a hotspot state (`GETPICLOC`).
    #[must_use]
    pub fn pic_offset(&self, spot: i32, state: i32) -> (i32, i32) {
        self.state_image(spot, state)
            .map_or((0, 0), |(_, dx, dy)| (dx, dy))
    }

    /// A prop's size (`PROPDIMENSIONS`).
    #[must_use]
    pub fn prop_dimensions(&self, prop: i64) -> (i32, i32) {
        self.assets
            .prop_facts
            .get(&prop)
            .map_or((0, 0), |facts| (facts.width, facts.height))
    }

    /// A prop's origin offsets, less the avatar half-size (`PROPOFFSETS`).
    #[must_use]
    pub fn prop_offsets(&self, prop: i64) -> (i32, i32) {
        self.assets
            .prop_facts
            .get(&prop)
            .map_or((0, 0), |facts| (facts.h_offset - 22, facts.v_offset - 22))
    }

    /// The id of the single door-like spot, if the room has exactly one.
    #[must_use]
    pub fn sole_door(&self) -> Option<i32> {
        let mut doors = self.spots.iter().filter(|s| s.kind == 1);
        let first = doors.next()?;
        doors.next().is_none().then_some(first.id)
    }
}

/// Ray-casting point-in-polygon over `(x, y)` vertices.
#[must_use]
pub fn point_in_polygon(x: i32, y: i32, poly: &[(i32, i32)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = (f64::from(poly[i].0), f64::from(poly[i].1));
        let (xj, yj) = (f64::from(poly[j].0), f64::from(poly[j].1));
        let (px, py) = (f64::from(x), f64::from(y));
        let crosses = (yi > py) != (yj > py);
        if crosses {
            let x_at_y = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_at_y {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_containment_works_for_a_square() {
        let square = [(0, 0), (10, 0), (10, 10), (0, 10)];
        assert!(point_in_polygon(5, 5, &square));
        assert!(!point_in_polygon(15, 5, &square));
        assert!(!point_in_polygon(-1, 5, &square));
    }

    #[test]
    fn a_pointed_hotspot_uses_its_outline() {
        let spot = SpotView {
            id: 1,
            points: vec![(0, 0), (20, 0), (20, 20), (0, 20)],
            loc: (500, 500),
            ..SpotView::default()
        };
        assert!(spot.contains(10, 10), "absolute outline");
        assert!(!spot.contains(300, 300), "outside both interpretations");
        assert!(spot.contains(510, 510), "outline relative to loc");
    }

    #[test]
    fn a_rectangular_hotspot_uses_the_loc_box() {
        let spot = SpotView {
            id: 2,
            loc: (100, 50),
            ..SpotView::default()
        };
        assert!(spot.contains(100, 50));
        assert!(spot.contains(120, 70));
        assert!(!spot.contains(200, 200));
    }

    #[test]
    fn spot_at_prefers_the_last_overlapping_spot() {
        let view = HostView {
            spots: vec![
                SpotView {
                    id: 1,
                    loc: (0, 0),
                    ..SpotView::default()
                },
                SpotView {
                    id: 2,
                    loc: (0, 0),
                    ..SpotView::default()
                },
            ],
            ..HostView::default()
        };
        assert_eq!(view.spot_at(0, 0).map(|s| s.id), Some(2));
        assert_eq!(view.spot_index(1), 0);
        assert_eq!(view.spot_index(99), -1);
    }

    #[test]
    fn relative_outlines_are_also_tried() {
        let spot = SpotView {
            id: 3,
            points: vec![(0, 0), (20, 0), (20, 20), (0, 20)],
            loc: (100, 100),
            ..SpotView::default()
        };
        assert!(spot.contains(110, 110), "loc + point");
    }
}

//! Guides and snapping: grid, centre, safe area, rule of thirds, onion skin.
//!
//! This module is the editor's guide geometry only. It computes where guide
//! lines and snap targets *are*, as plain coordinates, so the UI can draw them.
//! It never touches pixels, never changes a frame, and never writes a document:
//! every function is pure, the same arguments always produce the same
//! coordinates, and nothing on the canvas moves because a guide is visible.
//!
//! # The toggles this serves
//!
//! The reference editor's guide switches are `propeditor_onion`,
//! `propeditor_showgrid`, `propeditor_snapgrid`, `propeditor_snapcenter`,
//! `propeditor_safearea` and `propeditor_thirds`. The arithmetic behind them
//! lives here so that "the nearest grid line" or "a third of 44 pixels" is one
//! definition shared by the model, the UI and the tests:
//!
//! * [`snap_to_grid`] / [`snap_point_to_grid`] move a coordinate to the nearest
//!   grid line; [`snap_to_centre`] / [`snap_point_to_centre`] move it to the
//!   canvas centre cross. [`GuideToggles::snap`] applies whichever snaps are on.
//! * [`grid_lines`], [`centre_cross`], [`safe_area`] and [`thirds_lines`] return
//!   the geometry for one canvas; [`GuideGeometry`] bundles all four.
//! * [`onion_neighbours`] returns which frames to ghost around the current one,
//!   and how far away each is, without reading or copying their pixels.
//!
//! # Coordinates
//!
//! Canvas sizes are `u32` pixels and positions are `i32`: a point being dragged
//! may sit outside the canvas before it settles, and a snap must return a value
//! the caller can compare against `0..width` without a cast. Lines come back as
//! [`GuideLine`] endpoints in that same space so the UI can draw them directly,
//! and the safe area as a [`Rect`]. A zero extent has no centre, and a zero
//! spacing has no grid, so the snap helpers return their input unchanged in
//! those degenerate cases rather than dividing by zero.
//!
//! # Defaults
//!
//! The classic Palace avatar is 44x44, so the defaults are tuned for it without
//! being hardcoded to it:
//!
//! * [`DEFAULT_GRID_SPACING`] = 4 px: eleven cells across a 44 px canvas.
//! * [`DEFAULT_SAFE_AREA_INSET`] = 4 px per side: the central 36x36 of a 44x44
//!   canvas, close to the 10% broadcast safe margin.
//! * [`DEFAULT_ONION_RADIUS`] = 1: one ghost frame before and one after.
//!
//! Every geometry function takes the size it works on, so the same rules apply
//! to a 44x44 avatar and to a 4096x4096 prop.
//!
//! # Non-destructive
//!
//! Nothing here returns pixels, and nothing here is ever composited into a
//! frame. Switching every guide on and computing every geometry value leaves a
//! document's composited bytes identical; the acceptance test pins that.
//!
//! # Example
//!
//! ```
//! use palace_prop::editor::tools::guides::{
//!     safe_area, snap_to_grid, thirds_lines, DEFAULT_SAFE_AREA_INSET,
//! };
//!
//! // Snap a free x coordinate to a 4 px grid line.
//! assert_eq!(snap_to_grid(6, 4), 8);
//!
//! // The safe area of a 44x44 canvas, inset 4 px on each side.
//! let safe = safe_area(44, 44, DEFAULT_SAFE_AREA_INSET);
//! assert_eq!((safe.x, safe.y, safe.width, safe.height), (4, 4, 36, 36));
//!
//! // The rule-of-thirds lines across a 44 px axis.
//! let thirds = thirds_lines(44, 44);
//! assert_eq!(thirds.vertical[0].from.x, 15);
//! assert_eq!(thirds.vertical[1].from.x, 29);
//! ```

use crate::editor::Frame;

/// Grid spacing in pixels used when a canvas does not choose its own.
///
/// Four pixels gives eleven cells across the classic 44x44 Palace avatar and
/// stays readable on larger props.
pub const DEFAULT_GRID_SPACING: u32 = 4;

/// Safe-area inset in pixels, applied to each side, when a canvas does not
/// choose its own.
///
/// Four pixels a side leaves the central 36x36 of the classic 44x44 avatar.
pub const DEFAULT_SAFE_AREA_INSET: u32 = 4;

/// How many frames on each side of the current one onion skinning ghosts when
/// the caller does not choose a radius.
///
/// One frame each way is the animation convention: you see where the motion
/// came from and where it goes without the neighbours crowding the current
/// drawing.
pub const DEFAULT_ONION_RADIUS: usize = 1;

/// A point in canvas pixel coordinates.
///
/// `i32` rather than `u32` because a dragged point can be off-canvas; snapping
/// returns something in that same space so the caller needs no cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    /// Horizontal coordinate; `0` is the left edge.
    pub x: i32,
    /// Vertical coordinate; `0` is the top edge.
    pub y: i32,
}

impl Point {
    /// A point at `(x, y)`.
    #[must_use]
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }
}

/// A straight guide line, as the two endpoints the UI connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideLine {
    /// The first endpoint.
    pub from: Point,
    /// The second endpoint.
    pub to: Point,
}

impl GuideLine {
    /// A vertical line at `x`, spanning the canvas height `0..height`.
    #[must_use]
    pub fn vertical(x: i32, height: i32) -> Self {
        GuideLine {
            from: Point::new(x, 0),
            to: Point::new(x, height),
        }
    }

    /// A horizontal line at `y`, spanning the canvas width `0..width`.
    #[must_use]
    pub fn horizontal(y: i32, width: i32) -> Self {
        GuideLine {
            from: Point::new(0, y),
            to: Point::new(width, y),
        }
    }
}

/// A rectangle in canvas pixel coordinates: top-left corner plus size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Grid lines for one canvas: full-height verticals and full-width horizontals.
///
/// Both lists are ascending and anchored at the top-left corner: a line sits at
/// every multiple of the spacing from 0 up to, but not including, the opposite
/// edge. The edge lines are not duplicated; the canvas border already shows
/// them. A trailing strip narrower than the spacing stays outside the last
/// line, so the grid never stretches its cells to fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridLines {
    /// Vertical lines, left to right.
    pub vertical: Vec<GuideLine>,
    /// Horizontal lines, top to bottom.
    pub horizontal: Vec<GuideLine>,
}

/// The centre cross: one vertical and one horizontal line through the middle.
///
/// For an even extent the centre falls between two pixels; the guide uses the
/// upper index (`extent / 2`), the first pixel of the lower-right half. For an
/// odd extent that index is the true middle pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CentreCross {
    /// The vertical centre line.
    pub vertical: GuideLine,
    /// The horizontal centre line.
    pub horizontal: GuideLine,
}

/// The two rule-of-thirds lines per axis.
///
/// `vertical[0]` and `vertical[1]` sit at one third and two thirds of the
/// width; `horizontal[0]` and `horizontal[1]` at one third and two thirds of
/// the height. Positions round to the nearest whole pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThirdsLines {
    /// The one-third and two-thirds vertical lines, left to right.
    pub vertical: [GuideLine; 2],
    /// The one-third and two-thirds horizontal lines, top to bottom.
    pub horizontal: [GuideLine; 2],
}

/// Every guide for one canvas: grid, centre, safe area and thirds.
///
/// This is the value the UI draws from. It is pure data with no reference back
/// to a document, so computing it cannot change a pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideGeometry {
    /// Canvas width the geometry was built for.
    pub width: u32,
    /// Canvas height the geometry was built for.
    pub height: u32,
    /// The grid lines.
    pub grid: GridLines,
    /// The centre cross.
    pub centre: CentreCross,
    /// The safe-area rectangle.
    pub safe_area: Rect,
    /// The rule-of-thirds lines.
    pub thirds: ThirdsLines,
}

impl GuideGeometry {
    /// Build every guide for a `width` x `height` canvas.
    ///
    /// `spacing` is the grid pitch in pixels and `inset` the safe-area margin
    /// per side. A zero spacing yields an empty grid; a zero inset yields the
    /// whole canvas as the safe area.
    #[must_use]
    pub fn for_canvas(width: u32, height: u32, spacing: u32, inset: u32) -> Self {
        GuideGeometry {
            width,
            height,
            grid: grid_lines(width, height, spacing),
            centre: centre_cross(width, height),
            safe_area: safe_area(width, height, inset),
            thirds: thirds_lines(width, height),
        }
    }

    /// Build every guide using [`DEFAULT_GRID_SPACING`] and
    /// [`DEFAULT_SAFE_AREA_INSET`].
    #[must_use]
    pub fn with_defaults(width: u32, height: u32) -> Self {
        GuideGeometry::for_canvas(width, height, DEFAULT_GRID_SPACING, DEFAULT_SAFE_AREA_INSET)
    }
}

/// The editor's six guide switches, exactly as the reference client names them.
///
/// The flags are presentation state: they choose what is drawn and which snaps
/// apply, never what the document holds. [`GuideToggles::snap`] is the only
/// behaviour here, and it is still pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuideToggles {
    /// `propeditor_onion`: ghost neighbouring frames.
    pub onion: bool,
    /// `propeditor_showgrid`: draw the grid.
    pub grid: bool,
    /// `propeditor_snapgrid`: snap free points to grid lines.
    pub snap_grid: bool,
    /// `propeditor_snapcenter`: snap free points to the centre cross.
    pub snap_centre: bool,
    /// `propeditor_safearea`: draw the safe-area rectangle.
    pub safe_area: bool,
    /// `propeditor_thirds`: draw the rule-of-thirds lines.
    pub thirds: bool,
}

impl GuideToggles {
    /// Every guide off, the editor's startup state.
    pub const ALL_OFF: Self = GuideToggles {
        onion: false,
        grid: false,
        snap_grid: false,
        snap_centre: false,
        safe_area: false,
        thirds: false,
    };

    /// Every guide on, useful for tests and for a "show everything" state.
    pub const ALL_ON: Self = GuideToggles {
        onion: true,
        grid: true,
        snap_grid: true,
        snap_centre: true,
        safe_area: true,
        thirds: true,
    };

    /// Apply the enabled snaps to a free point.
    ///
    /// With both snaps off the point is returned unchanged. With both on the
    /// centre wins: it is the stronger constraint, and a centre-snapped point
    /// is also a valid grid landmark on any spacing that divides the extent.
    /// `spacing` is the grid pitch; a zero spacing leaves the grid snap inert.
    #[must_use]
    pub fn snap(&self, point: Point, width: u32, height: u32, spacing: u32) -> Point {
        if self.snap_centre {
            snap_point_to_centre(point, width, height)
        } else if self.snap_grid {
            snap_point_to_grid(point, spacing)
        } else {
            point
        }
    }
}

/// A frame to ghost while onion skinning: which one, and how far off it is.
///
/// This is an index and a signed distance, not pixels, so the caller can look
/// the frame up, tint it, and decide how strongly to ghost it, all without this
/// module touching or copying any frame data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnionNeighbour {
    /// Index into the frame list the call was made with.
    pub index: usize,
    /// Signed distance from the current frame: `-1` is one frame earlier, `+1`
    /// one later. The sign is the direction, the magnitude the distance.
    pub offset: i32,
}

/// Snap one coordinate to the nearest grid line.
///
/// The grid is the multiples of `spacing` anchored at 0. A coordinate exactly
/// halfway between two lines rounds away from zero, so `2` snaps to `4` and
/// `-2` to `-4` with a spacing of 4. A spacing of 0 means no grid and the
/// coordinate is returned unchanged.
#[must_use]
pub fn snap_to_grid(value: i32, spacing: u32) -> i32 {
    if spacing == 0 {
        return value;
    }
    let step = i64::from(spacing);
    let raw = i64::from(value);
    let half = step / 2;
    let snapped = if raw >= 0 {
        (raw + half) / step * step
    } else {
        -((-raw + half) / step * step)
    };
    clamp_coordinate(snapped)
}

/// Snap a point to the nearest grid intersection.
///
/// Each axis snaps independently through [`snap_to_grid`], so a point on a line
/// in one axis stays there while the other axis moves.
#[must_use]
pub fn snap_point_to_grid(point: Point, spacing: u32) -> Point {
    Point::new(
        snap_to_grid(point.x, spacing),
        snap_to_grid(point.y, spacing),
    )
}

/// Snap one coordinate to a canvas centre line.
///
/// The centre cross is a single line per axis, so every coordinate on that axis
/// maps to the same result. A zero `extent` has no centre, so the coordinate is
/// returned unchanged.
#[must_use]
pub fn snap_to_centre(value: i32, extent: u32) -> i32 {
    if extent == 0 {
        return value;
    }
    centre_coordinate(extent)
}

/// Snap a point to the canvas centre: both axes move to the centre cross.
#[must_use]
pub fn snap_point_to_centre(point: Point, width: u32, height: u32) -> Point {
    Point::new(
        snap_to_centre(point.x, width),
        snap_to_centre(point.y, height),
    )
}

/// The centre coordinate of one axis, as the centre cross draws it.
///
/// For an even extent this is `extent / 2`, the first pixel of the lower-right
/// half; for an odd extent it is the true middle pixel. A zero extent returns 0.
#[must_use]
pub fn centre_coordinate(extent: u32) -> i32 {
    coordinate(extent / 2)
}

/// Grid lines for a `width` x `height` canvas at `spacing` pixel pitch.
///
/// Vertical lines sit at `0, spacing, 2 * spacing, ...` while the position is
/// below the width; horizontal lines do the same down the height. A zero
/// spacing returns an empty grid.
#[must_use]
pub fn grid_lines(width: u32, height: u32, spacing: u32) -> GridLines {
    let mut grid = GridLines {
        vertical: Vec::new(),
        horizontal: Vec::new(),
    };
    if spacing == 0 {
        return grid;
    }

    let span_y = coordinate(height);
    let mut x = 0_u32;
    while x < width {
        grid.vertical
            .push(GuideLine::vertical(coordinate(x), span_y));
        x = match x.checked_add(spacing) {
            Some(next) => next,
            None => break,
        };
    }

    let span_x = coordinate(width);
    let mut y = 0_u32;
    while y < height {
        grid.horizontal
            .push(GuideLine::horizontal(coordinate(y), span_x));
        y = match y.checked_add(spacing) {
            Some(next) => next,
            None => break,
        };
    }

    grid
}

/// The centre cross for a `width` x `height` canvas.
///
/// Both lines span the full canvas through the centre coordinate of their axis;
/// see [`centre_coordinate`] for the even/odd rule.
#[must_use]
pub fn centre_cross(width: u32, height: u32) -> CentreCross {
    CentreCross {
        vertical: GuideLine::vertical(centre_coordinate(width), coordinate(height)),
        horizontal: GuideLine::horizontal(centre_coordinate(height), coordinate(width)),
    }
}

/// The safe-area rectangle for a `width` x `height` canvas.
///
/// The rectangle is inset by `inset` pixels on each side, so it is centred on
/// the canvas and keeps the full width minus twice the inset. An inset that
/// reaches or passes the middle collapses the rectangle to zero width or height
/// at the middle rather than inverting it.
#[must_use]
pub fn safe_area(width: u32, height: u32, inset: u32) -> Rect {
    Rect {
        x: coordinate(inset.min(width / 2)),
        y: coordinate(inset.min(height / 2)),
        width: width.saturating_sub(inset.saturating_mul(2)),
        height: height.saturating_sub(inset.saturating_mul(2)),
    }
}

/// The rule-of-thirds lines for a `width` x `height` canvas.
///
/// Each axis gets a line at one third and at two thirds, rounded to the nearest
/// pixel, so the three bands differ by at most one pixel on sizes that do not
/// divide by three.
#[must_use]
pub fn thirds_lines(width: u32, height: u32) -> ThirdsLines {
    ThirdsLines {
        vertical: [
            GuideLine::vertical(coordinate(third(width, 1)), coordinate(height)),
            GuideLine::vertical(coordinate(third(width, 2)), coordinate(height)),
        ],
        horizontal: [
            GuideLine::horizontal(coordinate(third(height, 1)), coordinate(width)),
            GuideLine::horizontal(coordinate(third(height, 2)), coordinate(width)),
        ],
    }
}

/// Which frames to ghost around `current`, nearest first.
///
/// `radius` is how many frames on each side may be ghosted (see
/// [`DEFAULT_ONION_RADIUS`]); it is clamped to the list length. Neighbours are
/// ordered by distance, and within one distance the earlier frame comes before
/// the later one: `-1, +1, -2, +2, ...`. That order is also a sensible drawing
/// order for ghosts, though the UI may reverse it.
///
/// Nothing is read from `frames` beyond its length: the result is index data
/// only, so onion skinning cannot alter any frame's pixels. An out-of-range
/// `current` or a radius of 0 yields an empty list.
#[must_use]
pub fn onion_neighbours(frames: &[Frame], current: usize, radius: usize) -> Vec<OnionNeighbour> {
    if current >= frames.len() {
        return Vec::new();
    }
    let mut neighbours = Vec::new();
    let radius = radius.min(frames.len());
    for distance in 1..=radius {
        if current >= distance {
            neighbours.push(OnionNeighbour {
                index: current - distance,
                offset: -offset_of(distance),
            });
        }
        match current.checked_add(distance) {
            Some(index) if index < frames.len() => neighbours.push(OnionNeighbour {
                index,
                offset: offset_of(distance),
            }),
            _ => {}
        }
    }
    neighbours
}

/// One third or two thirds of an extent, rounded to the nearest pixel.
fn third(extent: u32, numerator: u32) -> u32 {
    let scaled = u64::from(extent) * 2 * u64::from(numerator) + 3;
    (scaled / 6).min(u64::from(u32::MAX)) as u32
}

/// A distance as a signed offset. Distances are clamped to the list length, so
/// this can only saturate for a frame list larger than `i32::MAX` entries.
fn offset_of(distance: usize) -> i32 {
    i32::try_from(distance).unwrap_or(i32::MAX)
}

/// Canvas lengths are `u32`; guide coordinates are `i32`. Saturate, never wrap.
fn coordinate(value: u32) -> i32 {
    value.min(i32::MAX as u32) as i32
}

/// Clamp arithmetic done in `i64` back into the `i32` coordinate range.
fn clamp_coordinate(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

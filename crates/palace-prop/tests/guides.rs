//! Acceptance tests for the editor's guide geometry and snapping (plan task 29).
//!
//! These exercise the module through its public surface only: no filesystem, no
//! network, no webview, no pixels. The required behaviours are snap-to-grid,
//! snap-to-centre, non-destructive toggling (a rendered digest cannot change),
//! correct rule-of-thirds and safe-area geometry for a known size, and correct
//! onion-skin neighbours at the first, middle and last frame.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use palace_prop::editor::tools::guides::{
    centre_coordinate, centre_cross, grid_lines, onion_neighbours, safe_area, snap_point_to_centre,
    snap_point_to_grid, snap_to_centre, snap_to_grid, thirds_lines, GuideGeometry, GuideToggles,
    OnionNeighbour, Point, DEFAULT_GRID_SPACING, DEFAULT_ONION_RADIUS, DEFAULT_SAFE_AREA_INSET,
};
use palace_prop::editor::{EditorDocument, Frame};
use palace_prop::image::PropImage;

/// A stable-for-this-run digest of an image's dimensions and pixels.
fn digest(image: &PropImage) -> u64 {
    let mut hasher = DefaultHasher::new();
    image.width().hash(&mut hasher);
    image.height().hash(&mut hasher);
    image.as_rgba().hash(&mut hasher);
    hasher.finish()
}

/// The documented defaults are the values the UI and the tests rely on.
#[test]
fn documented_defaults_are_pinned() {
    assert_eq!(DEFAULT_GRID_SPACING, 4);
    assert_eq!(DEFAULT_SAFE_AREA_INSET, 4);
    assert_eq!(DEFAULT_ONION_RADIUS, 1);
}

// (a) snap-to-grid moves a coordinate to the nearest line, in both axes.
#[test]
fn snap_to_grid_maps_a_point_to_the_nearest_line() {
    // A 4 px grid on the classic 44x44 canvas: lines at 0, 4, ..., 40.
    assert_eq!(snap_to_grid(0, DEFAULT_GRID_SPACING), 0);
    assert_eq!(snap_to_grid(1, DEFAULT_GRID_SPACING), 0);
    assert_eq!(snap_to_grid(3, DEFAULT_GRID_SPACING), 4);
    assert_eq!(snap_to_grid(5, DEFAULT_GRID_SPACING), 4);
    assert_eq!(snap_to_grid(6, DEFAULT_GRID_SPACING), 8);
    assert_eq!(snap_to_grid(43, DEFAULT_GRID_SPACING), 44);

    // A coordinate exactly halfway rounds away from zero.
    assert_eq!(snap_to_grid(2, DEFAULT_GRID_SPACING), 4);
    assert_eq!(snap_to_grid(-2, DEFAULT_GRID_SPACING), -4);

    // Negative coordinates snap to the same lattice.
    assert_eq!(snap_to_grid(-1, DEFAULT_GRID_SPACING), 0);
    assert_eq!(snap_to_grid(-5, DEFAULT_GRID_SPACING), -4);
    assert_eq!(snap_to_grid(-6, DEFAULT_GRID_SPACING), -8);

    // A spacing of zero is no grid: the coordinate is returned unchanged.
    assert_eq!(snap_to_grid(7, 0), 7);
    assert_eq!(snap_to_grid(i32::MIN, 0), i32::MIN);

    // The point form snaps both axes independently.
    assert_eq!(
        snap_point_to_grid(Point::new(6, 13), DEFAULT_GRID_SPACING),
        Point::new(8, 12)
    );
    assert_eq!(
        snap_point_to_grid(Point::new(-1, 2), DEFAULT_GRID_SPACING),
        Point::new(0, 4)
    );
}

// (b) snap-to-centre moves a coordinate to the middle of its axis.
#[test]
fn snap_to_centre_maps_to_the_centre() {
    // Even extent: the upper of the two central pixels.
    assert_eq!(snap_to_centre(0, 44), 22);
    assert_eq!(snap_to_centre(1, 44), 22);
    assert_eq!(snap_to_centre(43, 44), 22);
    assert_eq!(centre_coordinate(44), 22);

    // Odd extent: the true middle pixel.
    assert_eq!(centre_coordinate(45), 22);
    assert_eq!(snap_to_centre(0, 45), 22);

    // A zero extent has no centre: the coordinate is returned unchanged.
    assert_eq!(snap_to_centre(7, 0), 7);
    assert_eq!(centre_coordinate(0), 0);

    // The point form moves both axes to the centre cross.
    assert_eq!(
        snap_point_to_centre(Point::new(3, 40), 44, 44),
        Point::new(22, 22)
    );
    assert_eq!(
        snap_point_to_centre(Point::new(0, 0), 45, 45),
        Point::new(22, 22)
    );
}

// (c) the toggle struct mirrors the six editor switches and gates the snaps.
#[test]
fn guide_toggles_gate_which_snaps_apply() {
    let point = Point::new(7, 7);

    let off = GuideToggles::default();
    assert_eq!(off, GuideToggles::ALL_OFF, "default is every guide off");
    assert_eq!(off.snap(point, 44, 44, DEFAULT_GRID_SPACING), point);

    let grid = GuideToggles {
        snap_grid: true,
        ..GuideToggles::ALL_OFF
    };
    assert_eq!(
        grid.snap(point, 44, 44, DEFAULT_GRID_SPACING),
        Point::new(8, 8)
    );
    // A zero spacing leaves the grid snap inert even when it is on.
    assert_eq!(grid.snap(point, 44, 44, 0), point);

    let centre = GuideToggles {
        snap_centre: true,
        ..GuideToggles::ALL_OFF
    };
    assert_eq!(
        centre.snap(point, 44, 44, DEFAULT_GRID_SPACING),
        Point::new(22, 22)
    );

    // Centre wins when both snaps are on: it is the stronger constraint.
    let both = GuideToggles {
        snap_grid: true,
        snap_centre: true,
        ..GuideToggles::ALL_OFF
    };
    assert_eq!(
        both.snap(point, 44, 44, DEFAULT_GRID_SPACING),
        Point::new(22, 22)
    );

    let every = GuideToggles::ALL_ON;
    assert!(every.onion && every.grid && every.snap_grid);
    assert!(every.snap_centre && every.safe_area && every.thirds);
}

// (d) grid geometry is a lattice anchored at the top-left, spanning the canvas.
#[test]
fn grid_lines_are_spaced_from_the_origin() {
    let grid = grid_lines(44, 44, DEFAULT_GRID_SPACING);

    let xs: Vec<i32> = grid.vertical.iter().map(|line| line.from.x).collect();
    assert_eq!(xs, vec![0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40]);
    assert_eq!(grid.vertical.len(), 11, "eleven cells across 44 px");
    assert!(grid
        .vertical
        .iter()
        .all(|line| line.from.y == 0 && line.to.y == 44));

    let ys: Vec<i32> = grid.horizontal.iter().map(|line| line.from.y).collect();
    assert_eq!(ys, xs, "a square canvas has matching horizontal lines");
    assert!(grid
        .horizontal
        .iter()
        .all(|line| line.from.x == 0 && line.to.x == 44));

    // A trailing strip narrower than the spacing does not add a line.
    let uneven = grid_lines(45, 45, DEFAULT_GRID_SPACING);
    assert_eq!(uneven.vertical.len(), 12, "0, 4, ..., 44 below 45");
    assert_eq!(uneven.vertical.last().map(|line| line.from.x), Some(44));

    // Zero spacing draws nothing, and must not loop forever.
    let empty = grid_lines(44, 44, 0);
    assert!(empty.vertical.is_empty());
    assert!(empty.horizontal.is_empty());
}

// (e) the centre cross marks the middle of each axis.
#[test]
fn centre_cross_marks_the_middle_of_each_axis() {
    let cross = centre_cross(44, 44);
    assert_eq!(cross.vertical.from, Point::new(22, 0));
    assert_eq!(cross.vertical.to, Point::new(22, 44));
    assert_eq!(cross.horizontal.from, Point::new(0, 22));
    assert_eq!(cross.horizontal.to, Point::new(44, 22));

    let odd = centre_cross(45, 45);
    assert_eq!(odd.vertical.from.x, 22);
    assert_eq!(odd.horizontal.from.y, 22);
}

// (f) thirds and safe area are correct for a known size, and the bundle agrees.
#[test]
fn thirds_and_safe_area_geometry_is_correct_for_a_known_size() {
    // 44 is not divisible by three, so both lines round to the nearest pixel.
    let thirds = thirds_lines(44, 44);
    assert_eq!(thirds.vertical[0].from, Point::new(15, 0));
    assert_eq!(thirds.vertical[0].to, Point::new(15, 44));
    assert_eq!(thirds.vertical[1].from, Point::new(29, 0));
    assert_eq!(thirds.horizontal[0].from, Point::new(0, 15));
    assert_eq!(thirds.horizontal[1].from, Point::new(0, 29));

    // Exact thirds when the size divides by three.
    let exact = thirds_lines(45, 60);
    assert_eq!(exact.vertical[0].from.x, 15);
    assert_eq!(exact.vertical[1].from.x, 30);
    assert_eq!(exact.horizontal[0].from.y, 20);
    assert_eq!(exact.horizontal[1].from.y, 40);

    // The safe area insets each side, keeping the rectangle centred.
    let rect = safe_area(44, 44, DEFAULT_SAFE_AREA_INSET);
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (4, 4, 36, 36));
    assert_eq!(rect.x + rect.width as i32 + rect.x, 44);
    assert_eq!(rect.y + rect.height as i32 + rect.y, 44);

    // A zero inset is the whole canvas.
    let full = safe_area(44, 44, 0);
    assert_eq!((full.x, full.y, full.width, full.height), (0, 0, 44, 44));

    // An oversized inset collapses to an empty rectangle at the middle.
    let collapsed = safe_area(44, 44, 100);
    assert_eq!((collapsed.x, collapsed.y), (22, 22));
    assert_eq!((collapsed.width, collapsed.height), (0, 0));

    // The bundle is exactly the four individual functions.
    let bundle = GuideGeometry::with_defaults(44, 44);
    assert_eq!(bundle.width, 44);
    assert_eq!(bundle.height, 44);
    assert_eq!(bundle.grid, grid_lines(44, 44, DEFAULT_GRID_SPACING));
    assert_eq!(bundle.centre, centre_cross(44, 44));
    assert_eq!(bundle.safe_area, safe_area(44, 44, DEFAULT_SAFE_AREA_INSET));
    assert_eq!(bundle.thirds, thirds_lines(44, 44));
}

// (g) onion neighbours are right at the first, middle and last frame.
#[test]
fn onion_neighbours_are_correct_at_first_middle_and_last_frame() {
    let frames: Vec<Frame> = (0..5).map(|_| Frame::blank(4, 4)).collect();

    // First frame: nothing before it, one ghost after at radius 1.
    assert_eq!(
        onion_neighbours(&frames, 0, 1),
        vec![OnionNeighbour {
            index: 1,
            offset: 1
        }]
    );

    // Middle frame: one ghost on each side.
    assert_eq!(
        onion_neighbours(&frames, 2, 1),
        vec![
            OnionNeighbour {
                index: 1,
                offset: -1
            },
            OnionNeighbour {
                index: 3,
                offset: 1
            },
        ]
    );

    // Last frame: only the one before.
    assert_eq!(
        onion_neighbours(&frames, 4, 1),
        vec![OnionNeighbour {
            index: 3,
            offset: -1
        }]
    );

    // Radius 2 reaches two each way, nearest first and earlier before later.
    assert_eq!(
        onion_neighbours(&frames, 2, 2),
        vec![
            OnionNeighbour {
                index: 1,
                offset: -1
            },
            OnionNeighbour {
                index: 3,
                offset: 1
            },
            OnionNeighbour {
                index: 0,
                offset: -2
            },
            OnionNeighbour {
                index: 4,
                offset: 2
            },
        ]
    );

    // At the first frame the reach forward still works and clamps at the end.
    assert_eq!(
        onion_neighbours(&frames, 0, 2),
        vec![
            OnionNeighbour {
                index: 1,
                offset: 1
            },
            OnionNeighbour {
                index: 2,
                offset: 2
            },
        ]
    );

    // Degenerate inputs ghost nothing rather than panicking or looping.
    assert!(onion_neighbours(&frames, 2, 0).is_empty(), "radius zero");
    assert!(onion_neighbours(&frames, 9, 1).is_empty(), "bad index");
    assert!(onion_neighbours(&[], 0, 1).is_empty(), "empty list");
    assert!(
        onion_neighbours(&[Frame::blank(2, 2)], 0, 4).is_empty(),
        "one frame has no neighbours"
    );

    // An enormous radius is clamped to the list, not looped over.
    assert_eq!(onion_neighbours(&frames, 2, usize::MAX).len(), 4);

    // The documented default radius is one frame each side.
    assert_eq!(onion_neighbours(&frames, 2, DEFAULT_ONION_RADIUS).len(), 2);
}

// (h) toggling guides never changes a rendered prop digest.
#[test]
fn toggling_guides_never_changes_a_rendered_prop_digest() {
    // A two-frame document with painted, non-uniform pixels.
    let mut document = EditorDocument::blank(44, 44).expect("a valid canvas");
    document
        .current_frame_mut()
        .expect("a current frame")
        .layer_mut(0)
        .expect("a base layer")
        .edit_pixels(|bytes| {
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = ((i * 37 + 11) % 256) as u8;
            }
        })
        .expect("pixels rebuild");
    assert_eq!(document.add_blank_frame(), Some(1));
    document
        .current_frame_mut()
        .expect("a current frame")
        .layer_mut(0)
        .expect("a base layer")
        .set_pixel(10, 10, [255, 0, 0, 255])
        .expect("set_pixel");

    let frames_before = document.frames().to_vec();
    let digest_before = digest(&document.composite_current().expect("a composite"));

    // Switch every guide on and compute every kind of geometry.
    let toggles = GuideToggles::ALL_ON;
    let bundle = GuideGeometry::with_defaults(44, 44);
    assert_eq!(bundle.grid.vertical.len(), 11);
    let _ = centre_cross(44, 44);
    let _ = safe_area(44, 44, DEFAULT_SAFE_AREA_INSET);
    let _ = thirds_lines(44, 44);
    let _ = grid_lines(44, 44, DEFAULT_GRID_SPACING);

    // Snapping and onion skinning are data too, and they mutate nothing.
    let snapped = toggles.snap(Point::new(7, 7), 44, 44, DEFAULT_GRID_SPACING);
    assert_eq!(snapped, Point::new(22, 22));
    let ghosts = onion_neighbours(
        document.frames(),
        document.current_index(),
        DEFAULT_ONION_RADIUS,
    );
    assert_eq!(
        ghosts,
        vec![OnionNeighbour {
            index: 0,
            offset: -1
        }]
    );

    // Switching them all back off changes nothing either.
    let off = GuideToggles::ALL_OFF;
    assert_eq!(off.snap(Point::new(7, 7), 44, 44, 4), Point::new(7, 7));

    // The rendered prop and the frames are byte-for-byte what they were.
    assert_eq!(
        digest(&document.composite_current().expect("a composite")),
        digest_before,
        "guides must never alter a rendered prop"
    );
    assert_eq!(
        document.frames(),
        frames_before.as_slice(),
        "guides must never alter a frame"
    );
}

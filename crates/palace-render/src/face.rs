//! The built-in Palace face sheet.
//!
//! Every user has a face (`UserInfo::face` / `.color` in the client runtime),
//! drawn from the standard smiley sheet unless the user wears a prop carrying the
//! `HEAD` flag. The sheet is embedded in the binary and decoded once, so a render
//! never touches the filesystem and a missing asset cannot take the face away.
//!
//! ## Layout
//!
//! A 13 (face) by 16 (colour) grid of 44×44 cells, row-major, with no padding:
//! cell `(face, color)` starts at `(face * 44, color * 44)`. 44 px is the size
//! Palace draws a face at, so a cell is blitted at offset `(0, 0)` with no
//! scaling and no fudge.
//!
//! The reference hides the face when a worn prop's header has the `HEAD` flag
//! (`Avatar.mxml`, `checkFaceProps`); [`crate::build`] applies that rule.

use std::sync::OnceLock;

use palace_prop::PropImage;

use crate::assets::decode_image_bytes;

/// The embedded smiley sheet: 13 columns (faces) × 16 rows (colours).
const SMILEYS_PNG: &[u8] = include_bytes!("../assets/smileys.png");

/// Width and height of one face cell, in pixels.
pub const FACE_CELL: u32 = 44;

/// The embedded smiley sheet as PNG bytes.
///
/// The presentation layer fetches this over its URI scheme to draw a face
/// picker. It is the same asset [`smiley_cell`] crops from, so the picker and
/// the composited frame can never disagree about the sheet.
#[must_use]
pub fn face_sheet_png() -> &'static [u8] {
    SMILEYS_PNG
}
/// Number of face variants. Valid `face` values are `0..FACE_VARIANTS`.
pub const FACE_VARIANTS: i16 = 13;
/// Number of colour variants. Valid `color` values are `0..COLOR_VARIANTS`.
pub const COLOR_VARIANTS: i16 = 16;

/// The decoded sheet, decoded exactly once for the life of the process.
fn sheet() -> &'static PropImage {
    static SHEET: OnceLock<PropImage> = OnceLock::new();
    SHEET.get_or_init(|| {
        decode_image_bytes(SMILEYS_PNG).unwrap_or_else(|_| {
            PropImage::transparent(
                FACE_CELL * FACE_VARIANTS as u32,
                FACE_CELL * COLOR_VARIANTS as u32,
            )
        })
    })
}

/// One `(face, color)` cell of the built-in sheet, as a 44×44 RGBA image.
///
/// `face` is clamped to `0..=12` and `color` to `0..=15`, so a hostile or
/// corrupted server value can never index outside the sheet.
#[must_use]
pub fn smiley_cell(face: i16, color: i16) -> PropImage {
    let x = (face.clamp(0, FACE_VARIANTS - 1) as u32) * FACE_CELL;
    let y = (color.clamp(0, COLOR_VARIANTS - 1) as u32) * FACE_CELL;
    crop(sheet(), x, y).unwrap_or_else(|| PropImage::transparent(FACE_CELL, FACE_CELL))
}

/// Copy the 44×44 cell whose top-left is `(x, y)` out of `sheet`.
///
/// `None` means the cell would fall outside the sheet — the only way a
/// malformed embedded image avoids an out-of-bounds panic.
fn crop(sheet: &PropImage, x: u32, y: u32) -> Option<PropImage> {
    if x.checked_add(FACE_CELL)? > sheet.width() || y.checked_add(FACE_CELL)? > sheet.height() {
        return None;
    }
    let stride = (sheet.width() as usize) * 4;
    let start_x = (x as usize) * 4;
    let mut rgba = Vec::with_capacity((FACE_CELL * FACE_CELL * 4) as usize);
    let source = sheet.as_rgba();
    for row in 0..FACE_CELL as usize {
        let at = (y as usize + row) * stride + start_x;
        let end = at + (FACE_CELL as usize) * 4;
        rgba.extend_from_slice(source.get(at..end)?);
    }
    PropImage::from_rgba(FACE_CELL, FACE_CELL, rgba).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_is_the_expected_grid() {
        let sheet = sheet();
        assert_eq!(
            (sheet.width(), sheet.height()),
            (
                FACE_CELL * FACE_VARIANTS as u32,
                FACE_CELL * COLOR_VARIANTS as u32
            )
        );
    }

    #[test]
    fn the_face_sheet_is_served_as_png_bytes() {
        let bytes = face_sheet_png();
        assert_eq!(
            &bytes[..8],
            b"\x89PNG\r\n\x1a\n",
            "the URI route must serve a real PNG"
        );
        assert_eq!(bytes, SMILEYS_PNG, "the accessor serves the embedded sheet");
    }

    #[test]
    fn a_cell_is_44_by_44_and_has_visible_pixels() {
        let cell = smiley_cell(0, 0);
        assert_eq!((cell.width(), cell.height()), (FACE_CELL, FACE_CELL));
        assert!(
            cell.as_rgba().chunks_exact(4).any(|p| p[3] > 0),
            "a face cell must not be fully transparent"
        );
    }

    #[test]
    fn out_of_range_indices_clamp_to_the_edge_cells() {
        assert_eq!(smiley_cell(99, -3), smiley_cell(FACE_VARIANTS - 1, 0));
        assert_eq!(smiley_cell(-7, 99), smiley_cell(0, COLOR_VARIANTS - 1));
        // No value panics or reads outside the sheet.
        for face in [-5i16, -1, 0, 12, 13, 500] {
            for color in [-5i16, -1, 0, 15, 16, 500] {
                let cell = smiley_cell(face, color);
                assert_eq!((cell.width(), cell.height()), (FACE_CELL, FACE_CELL));
            }
        }
    }

    #[test]
    fn different_cells_are_different_pixels() {
        assert_ne!(smiley_cell(0, 0), smiley_cell(1, 0), "faces differ");
        assert_ne!(smiley_cell(0, 0), smiley_cell(0, 1), "colours differ");
        assert_ne!(
            smiley_cell(3, 5),
            smiley_cell(4, 6),
            "face and colour differ"
        );
    }
}

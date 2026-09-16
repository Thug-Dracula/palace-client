//! The Palace "M&M" colour lookup table.
//!
//! 8-bit props store one palette index per pixel; this is the table those
//! indices select from. The values are ARGB (`0xAARRGGBB`) exactly as they
//! appear in OpenPalace's `PalacePalette.as` (`clutARGB`), which in turn matches
//! the hard-coded table in Taj's `PalaceProp.cs` entry for entry.
//!
//! `clutARGB` is used for props; `imageClutARGB` (a different ordering, also in
//! `PalacePalette.as`) is only used for hotspot images, so it is deliberately not
//! included here.
//!
//! The table is a compile-time constant, not parsed input, which is why it is
//! the one thing in this crate that is allowed to index without a bounds check.
//!
//! ```text
//! index  0 = 0xffffffff (opaque white)
//! index  6 = 0xffffdfff
//! index 54 = 0xffeeeeee
//! index 255 = 0xff000000 (opaque black)
//! ```
//!
//! Source: `$CORPUS/reference/PalacePalette.as` (a copy of
//! `OpenPalace/PalaceClient/src/net/codecomposer/palace/model/PalacePalette.as`).
//! Regenerate with `tools/gen_palette.py` rather than editing by hand.

/// Number of palette entries. 8-bit RLE pixels are bytes, so this is exactly one
/// byte's worth.
pub const PALETTE_LEN: usize = 256;

/// The prop palette as packed ARGB (`0xAARRGGBB`).
///
/// Look up with [`lookup`] to keep the "index is a byte" invariant in one place.
pub static PALETTE: [u32; PALETTE_LEN] = [
    0xffffffff, 0xffccffff, 0xff99ffff, 0xff66ffff, 0xff33ffff, 0xff00ffff, 0xffffdfff, 0xffccdfff,
    0xff99dfff, 0xff66dfff, 0xff33dfff, 0xff00dfff, 0xffffbfff, 0xffccbfff, 0xff99bfff, 0xff66bfff,
    0xff33bfff, 0xff00bfff, 0xffff9fff, 0xffcc9fff, 0xff999fff, 0xff669fff, 0xff339fff, 0xff009fff,
    0xffff7fff, 0xffcc7fff, 0xff997fff, 0xff667fff, 0xff337fff, 0xff007fff, 0xffff5fff, 0xffcc5fff,
    0xff995fff, 0xff665fff, 0xff335fff, 0xff005fff, 0xffff3fff, 0xffcc3fff, 0xff993fff, 0xff663fff,
    0xff333fff, 0xff003fff, 0xffff1fff, 0xffcc1fff, 0xff991fff, 0xff661fff, 0xff331fff, 0xff001fff,
    0xffff00ff, 0xffcc00ff, 0xff9900ff, 0xff6600ff, 0xff3300ff, 0xff0000ff, 0xffeeeeee, 0xffdddddd,
    0xffcccccc, 0xffbbbbbb, 0xffffffaa, 0xffccffaa, 0xff99ffaa, 0xff66ffaa, 0xff33ffaa, 0xff00ffaa,
    0xffffdfaa, 0xffccdfaa, 0xff99dfaa, 0xff66dfaa, 0xff33dfaa, 0xff00dfaa, 0xffffbfaa, 0xffccbfaa,
    0xff99bfaa, 0xff66bfaa, 0xff33bfaa, 0xff00bfaa, 0xffaaaaaa, 0xffff9faa, 0xffcc9faa, 0xff999faa,
    0xff669faa, 0xff339faa, 0xff009faa, 0xffff7faa, 0xffcc7faa, 0xff997faa, 0xff667faa, 0xff337faa,
    0xff007faa, 0xffff5faa, 0xffcc5faa, 0xff995faa, 0xff665faa, 0xff335faa, 0xff005faa, 0xffff3faa,
    0xffcc3faa, 0xff993faa, 0xff663faa, 0xff333faa, 0xff003faa, 0xffff1faa, 0xffcc1faa, 0xff991faa,
    0xff661faa, 0xff331faa, 0xff001faa, 0xffff00aa, 0xffcc00aa, 0xff9900aa, 0xff6600aa, 0xff3300aa,
    0xff0000aa, 0xff999999, 0xff888888, 0xff777777, 0xff666666, 0xffffff55, 0xffccff55, 0xff99ff55,
    0xff66ff55, 0xff33ff55, 0xff00ff55, 0xffffdf55, 0xffccdf55, 0xff99df55, 0xff66df55, 0xff33df55,
    0xff00df55, 0xffffbf55, 0xffccbf55, 0xff99bf55, 0xff66bf55, 0xff33bf55, 0xff00bf55, 0xffff9f55,
    0xffcc9f55, 0xff999f55, 0xff669f55, 0xff339f55, 0xff009f55, 0xffff7f55, 0xffcc7f55, 0xff997f55,
    0xff667f55, 0xff337f55, 0xff007f55, 0xffff5f55, 0xffcc5f55, 0xff995f55, 0xff665f55, 0xff335f55,
    0xff005f55, 0xff555555, 0xffff3f55, 0xffcc3f55, 0xff993f55, 0xff663f55, 0xff333f55, 0xff003f55,
    0xffff1f55, 0xffcc1f55, 0xff991f55, 0xff661f55, 0xff331f55, 0xff001f55, 0xffff0055, 0xffcc0055,
    0xff990055, 0xff660055, 0xff330055, 0xff000055, 0xff444444, 0xff333333, 0xff222222, 0xff111111,
    0xffffff00, 0xffccff00, 0xff99ff00, 0xff66ff00, 0xff33ff00, 0xff00ff00, 0xffffdf00, 0xffccdf00,
    0xff99df00, 0xff66df00, 0xff33df00, 0xff00df00, 0xffffbf00, 0xffccbf00, 0xff99bf00, 0xff66bf00,
    0xff33bf00, 0xff00bf00, 0xffff9f00, 0xffcc9f00, 0xff999f00, 0xff669f00, 0xff339f00, 0xff009f00,
    0xffff7f00, 0xffcc7f00, 0xff997f00, 0xff667f00, 0xff337f00, 0xff007f00, 0xffff5f00, 0xffcc5f00,
    0xff995f00, 0xff665f00, 0xff335f00, 0xff005f00, 0xffff3f00, 0xffcc3f00, 0xff993f00, 0xff663f00,
    0xff333f00, 0xff003f00, 0xffff1f00, 0xffcc1f00, 0xff991f00, 0xff661f00, 0xff331f00, 0xff001f00,
    0xffff0000, 0xffcc0000, 0xff990000, 0xff660000, 0xff330000, 0xff000000, 0xff000000, 0xff000000,
    0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000,
    0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000,
    0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000, 0xff000000,
];

/// Look up an 8-bit RLE palette index.
///
/// The index is a `u8`, so it is in range by construction — this cannot fail and
/// is not a panic path.
#[must_use]
pub fn lookup(index: u8) -> u32 {
    PALETTE[usize::from(index)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_the_reference_table() {
        // First, middle and last entries pinned against PalacePalette.as.
        assert_eq!(lookup(0), 0xffff_ffff);
        assert_eq!(lookup(6), 0xffff_dfff);
        assert_eq!(lookup(54), 0xffee_eeee);
        assert_eq!(lookup(255), 0xff00_0000);
    }

    #[test]
    fn every_entry_is_opaque() {
        // The M&M table has alpha 0xff throughout; 8-bit props have no alpha
        // channel of their own. A non-opaque entry would mean the table was
        // transcribed from the wrong array.
        assert!(
            PALETTE.iter().all(|c| c >> 24 == 0xff),
            "palette contains a non-opaque entry"
        );
    }

    #[test]
    fn trailing_entries_are_black() {
        // 229..=255 are the black padding run at the end of the reference table
        // (the 6-6-6 cube runs out of room long before 256 entries).
        assert!(PALETTE[229..].iter().all(|c| *c == 0xff00_0000));
    }
}

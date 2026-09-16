//! 8-bit: nibble run-length, palette-indexed, **not** compressed.
//!
//! Each control byte is two nibbles, `(skip << 4) | count`:
//!
//! ```text
//! bit  7 6 5 4 | 3 2 1 0
//!      skip      count
//! ```
//!
//! `count` palette bytes follow in the stream; `skip` columns are left at their
//! initial value (transparent, i.e. palette index 0 — which is *opaque white* in
//! the palette, so in practice a skipped column is simply not written and the
//! client shows whatever is behind the prop). A row is finished when exactly
//! `width` columns have been accounted for.
//!
//! ## Scanline order
//!
//! The reference loops `y` from `height - 1` down to 0 while writing to a buffer
//! whose write cursor starts at offset `width`. Those two facts cancel out: the
//! **first row in the stream is the first (top) row of the image**, and column 0
//! is the leftmost column. Both reference implementations agree: OpenPalace copies
//! from `pixData[44]` upward, and `prop_decoder.py` writes to row
//! `height - 1 - y`. The extra leading `width` of buffer is why the reference copy
//! loop begins at 44, and why its decoder is strictly speaking correct only for a
//! 44-pixel-wide prop.
//!
//! ## The runaway guard
//!
//! A `0x00` control byte advances neither nibble and would spin forever, so the
//! reference carries a 6000-control-byte budget. We keep the same number for
//! fidelity but also reject a zero-progress control byte immediately, which is
//! the same outcome without the spin.

use crate::error::{PropError, Result};
use crate::image::PropImage;
use crate::palette;

/// The reference decoder's runaway guard, in control bytes per prop.
pub const MAX_CONTROL_BYTES: usize = 6000;

/// Decode an 8-bit payload into a `width` x `height` image.
pub fn decode(payload: &[u8], width: u32, height: u32) -> Result<PropImage> {
    let w = width as usize;
    let h = height as usize;
    let mut buffer = vec![0u32; w * (h + 1)];
    let mut cursor = 0usize;
    let mut control_bytes = 0usize;

    for row in 0..h {
        let mut remaining = width as i32;
        let mut column = 0usize;
        while remaining > 0 {
            // The reference reads a ByteArray, where a read past the end yields 0
            // and still advances the cursor. That matters: a `0x00` control byte
            // makes no progress but consumes a byte, so a truncated stream walks
            // into its own tail until the runaway guard below stops it. Mirroring
            // this is what makes our diagnosis of a bad prop match the client's.
            let control = payload.get(cursor).copied().unwrap_or(0);
            cursor += 1;
            let skip = usize::from(control >> 4);
            let count = usize::from(control & 0x0F);
            remaining -= (skip + count) as i32;
            if remaining < 0 {
                return Err(PropError::RleRowOverflow { row });
            }
            control_bytes += 1;
            if control_bytes > MAX_CONTROL_BYTES {
                return Err(PropError::RleRunaway { row });
            }
            column += skip;
            for _ in 0..count {
                // Reference tolerance: its inner guard is `if (data.length > n)`,
                // so a payload that runs out mid-run leaves the rest of the run
                // untouched and still finishes the row. Only reachable in the last
                // row, because anywhere else the next control byte is past the end
                // and the guard above fires; 5 props in `pserver.prp` depend on it
                // and this is what the reference client renders.
                if let Some(index) = payload.get(cursor) {
                    let at = w + row * w + column;
                    let Some(slot) = buffer.get_mut(at) else {
                        return Err(PropError::RleRowOverflow { row });
                    };
                    *slot = palette::lookup(*index);
                    column += 1;
                    cursor += 1;
                }
            }
        }
    }

    Ok(PropImage::from_argb(width, height, &buffer[w..w + w * h]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_opaque_row_becomes_the_first_image_row() {
        // width 4, height 2. Row 0: 4 pixels of palette index 1 in one run.
        // Row 1: skip 2, write 2 pixels of index 2.
        let payload = [0x04, 0x01, 0x01, 0x01, 0x01, 0x22, 0x02, 0x02];
        let image = decode(&payload, 4, 2).unwrap();
        assert_eq!(image.pixel(0, 0), Some(argb_of(1)));
        assert_eq!(image.pixel(3, 0), Some(argb_of(1)));
        assert_eq!(image.pixel(0, 1), Some([0, 0, 0, 0]));
        assert_eq!(image.pixel(2, 1), Some(argb_of(2)));
    }

    fn argb_of(index: u8) -> [u8; 4] {
        let word = palette::lookup(index);
        [
            (word >> 16) as u8,
            (word >> 8) as u8,
            word as u8,
            (word >> 24) as u8,
        ]
    }

    #[test]
    fn a_zero_control_byte_is_a_runaway_error_not_a_hang() {
        assert_eq!(decode(&[0x00], 4, 1), Err(PropError::RleRunaway { row: 0 }));
    }

    #[test]
    fn a_row_that_overruns_its_width_is_an_error() {
        // skip 1 + count 4 = 5 > width 4.
        assert_eq!(
            decode(&[0x14, 1, 2, 3, 4], 4, 1),
            Err(PropError::RleRowOverflow { row: 0 })
        );
    }

    #[test]
    fn a_row_that_underruns_trips_the_runaway_guard() {
        // One byte accounting for only 2 of 4 columns, then the stream ends. The
        // reference reads 0 past the end, makes no progress and hits its guard; we
        // mirror that rather than inventing a distinct "truncated" diagnosis, so a
        // reporter sees the same reason a real client would log.
        assert_eq!(
            decode(&[0x02, 1, 2], 4, 1),
            Err(PropError::RleRunaway { row: 0 })
        );
    }

    #[test]
    fn trailing_bytes_after_the_last_row_are_ignored() {
        let payload = [0x04, 1, 1, 1, 1, 0xff, 0xff, 0xff];
        assert!(decode(&payload, 4, 1).is_ok());
    }

    #[test]
    fn a_payload_ending_inside_the_final_pixel_run_is_tolerated() {
        // The reference's `if (data.length > n)` guard leaves the remaining
        // pixels of the run untouched and the row still finishes. Five props in
        // `pserver.prp` end exactly like this.
        let image = decode(&[0x04, 1, 2], 4, 1).unwrap();
        assert_eq!(image.pixel(0, 0), Some(argb_of(1)));
        assert_eq!(image.pixel(1, 0), Some(argb_of(2)));
        assert_eq!(image.pixel(2, 0), Some([0, 0, 0, 0]));
        assert_eq!(image.pixel(3, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_missing_control_byte_is_still_an_error() {
        // A row that has not finished and no byte left to direct it: the guard,
        // not a panic and not a silent short image.
        assert!(matches!(
            decode(&[0x02, 1, 2], 4, 1),
            Err(PropError::RleRunaway { row: 0 })
        ));
    }

    #[test]
    fn a_zero_control_byte_is_skipped_not_fatal_when_the_row_can_finish() {
        // The reference consumes a `0x00` byte (no progress, cursor advances) and
        // carries on, so a prop with a stray zero byte still renders. An earlier
        // version of this decoder rejected it outright, which would have been
        // stricter than the client.
        let image = decode(&[0x00, 0x04, 1, 2, 3, 4], 4, 1).unwrap();
        assert_eq!(image.pixel(3, 0), Some(argb_of(4)));
    }

    #[test]
    fn the_width_of_the_payload_does_not_leak_into_the_next_row() {
        // Row 0 consumes 4 columns with skip 2 + count 2; row 1 must start at
        // column 0 of its own row, not continue row 0's cursor.
        let payload = [0x22, 0x05, 0x05, 0x04, 0x07, 0x07, 0x07, 0x07];
        let image = decode(&payload, 4, 2).unwrap();
        assert_eq!(image.pixel(2, 0), Some(argb_of(5)));
        assert_eq!(image.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(image.pixel(0, 1), Some(argb_of(7)));
        assert_eq!(image.pixel(3, 1), Some(argb_of(7)));
    }
}

//! Error type shared by every decode path in `palace-prop`.
//!
//! A prop is attacker-controlled data: it comes off the wire, out of an asset
//! file, or from a 1997 room description. **Nothing in this crate panics or
//! unwraps on protocol input.** Every failure — a truncated RLE row, a corrupt
//! zlib stream, a header claiming 30000×30000 — is an [`PropError`].
//!
//! The one deliberate exception is the palette: [`crate::palette::PALETTE`] is a
//! compile-time constant, not input.

use std::fmt;

/// Largest dimension a prop may declare.
///
/// Palace props are 44×44 in every corpus we have (see the crate README), and
/// even the store's own assembler caps a type-0 avatar at that. The bound exists
/// so a corrupt header (`0x7fff`) cannot make us allocate gigabytes before we
/// have looked at a single pixel.
pub const MAX_DIMENSION: i16 = 4096;

/// Anything that can go wrong while decoding or encoding a prop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropError {
    /// Fewer than the 12 header bytes were available.
    HeaderTooShort { available: usize },
    /// A dimension in the header was zero, negative or above [`MAX_DIMENSION`].
    ImplausibleDimensions { width: i16, height: i16 },
    /// The width/height product overflowed or exceeded [`MAX_PIXELS`].
    ImageTooLarge { width: i16, height: i16 },
    /// A pixel payload was shorter than the format requires.
    PayloadTooShort {
        format: &'static str,
        needed: usize,
        available: usize,
    },
    /// An 8-bit RLE control byte ran a row past its end (`x < 0` in the
    /// reference decoder).
    RleRowOverflow { row: usize },
    /// An 8-bit RLE stream blew the reference decoder's 6000-control-byte runaway
    /// guard. That is the reference's own diagnosis, and it covers both a stream
    /// that stalls (zero-progress control bytes, including the zeroes read past the
    /// end of a truncated payload) and one that circles without finishing a row.
    RleRunaway { row: usize },
    /// The zlib payload did not decompress.
    Zlib { detail: String },
    /// The zlib payload decompressed, but to fewer bytes than the format needs.
    DecompressedTooShort {
        format: &'static str,
        expected: usize,
        actual: usize,
    },
    /// The zlib payload produced more bytes than the format's cap allows, so we
    /// stopped inflating instead of growing the buffer unboundedly.
    DecompressedTooLarge { format: &'static str, limit: usize },
    /// An encoder was handed an image whose dimensions cannot be encoded.
    UnencodableImage { width: u32, height: u32 },
    /// PNG writing failed (visual inspection only; never part of decoding).
    Png { detail: String },
    /// A prop-bag (`*.pids` / `*.props`) file could not be read.
    ///
    /// The detail already names the offending path, so callers can print it
    /// directly.
    BagIo { detail: String },
    /// A directory contained no matching `*.pids` / `*.props` pair.
    BagNotABundle { path: String },
    /// A prop-bag blob is shorter than the 32-byte per-blob prefix, so it has no
    /// prop to decode.
    BagBlobTooShort { available: usize },
}

/// Largest number of pixels we will decode into memory (16 MP RGBA = 64 MiB).
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;

impl fmt::Display for PropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropError::HeaderTooShort { available } => write!(
                f,
                "prop is {available} byte(s) long; at least 12 are required for the header"
            ),
            PropError::ImplausibleDimensions { width, height } => {
                write!(f, "implausible prop dimensions {width}x{height}")
            }
            PropError::ImageTooLarge { width, height } => write!(
                f,
                "prop dimensions {width}x{height} exceed the {MAX_PIXELS}-pixel decode budget"
            ),
            PropError::PayloadTooShort {
                format,
                needed,
                available,
            } => write!(
                f,
                "{format} payload needs {needed} byte(s) but only {available} are present"
            ),
            PropError::RleRowOverflow { row } => {
                write!(f, "8-bit RLE row {row} ran past the end of the scanline")
            }
            PropError::RleRunaway { row } => write!(
                f,
                "8-bit RLE tripped the reference runaway guard at row {row}"
            ),
            PropError::Zlib { detail } => write!(f, "zlib payload did not decompress: {detail}"),
            PropError::DecompressedTooShort {
                format,
                expected,
                actual,
            } => write!(
                f,
                "{format} payload decompressed to {actual} byte(s), expected at least {expected}"
            ),
            PropError::DecompressedTooLarge { format, limit } => write!(
                f,
                "{format} payload decompressed past the {limit}-byte cap; refusing to inflate further"
            ),
            PropError::UnencodableImage { width, height } => write!(
                f,
                "cannot encode a {width}x{height} image (S20 props are 44x44)"
            ),
            PropError::Png { detail } => write!(f, "could not write PNG: {detail}"),
            PropError::BagIo { detail } => write!(f, "prop bag: {detail}"),
            PropError::BagNotABundle { path } => write!(
                f,
                "{path}: no matching .pids/.props pair found in this directory"
            ),
            PropError::BagBlobTooShort { available } => write!(
                f,
                "prop bag blob is {available} byte(s) long; at least 32 are required for the bag prefix"
            ),
        }
    }
}

impl std::error::Error for PropError {}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, PropError>;

//! Error type shared by every decode path in `palace-wire`.
//!
//! The library never panics on protocol-derived data: every fallible read
//! returns [`WireError`]. Callers decide whether an error is fatal (a framing
//! error is) or recoverable (an unknown opcode is not an error at all — see
//! [`crate::frame::Frame::decode`]).

use std::fmt;

/// Maximum payload length we will accept, in bytes.
///
/// The reference Python walker rejects messages larger than this; it guards
/// against a desynchronised stream being interpreted as an absurd length and
/// causing a huge allocation. `MSG_ROOMDESC` for large rooms is comfortably
/// below it (the live Balamb Garden initial room is ~1.2 KiB).
pub const MAX_PAYLOAD_LEN: u32 = 50_000_000;

/// Anything that can go wrong while reading or writing Palace traffic.
#[derive(Debug)]
pub enum WireError {
    /// A read ran past the end of the available bytes.
    UnexpectedEof { needed: usize, available: usize },
    /// A frame header claimed a payload length above [`MAX_PAYLOAD_LEN`].
    ImplausibleLength { length: u32, max: u32 },
    /// A payload could not fit in a `u32` length field.
    PayloadTooLarge { length: usize },
    /// The server opened in HTTP-tunnel mode (`pser`), which we do not support.
    UnsupportedTransport { banner: [u8; 4] },
    /// The first four bytes were not a recognised Palace banner.
    UnknownBanner { banner: [u8; 4] },
    /// A fixed-size field was consumed but bytes were left over.
    TrailingBytes { remaining: usize },
    /// An underlying I/O failure.
    Io(std::io::Error),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnexpectedEof { needed, available } => write!(
                f,
                "unexpected end of data: needed {needed} byte(s), {available} available"
            ),
            WireError::ImplausibleLength { length, max } => write!(
                f,
                "implausible frame payload length {length} (maximum accepted is {max})"
            ),
            WireError::PayloadTooLarge { length } => {
                write!(
                    f,
                    "payload of {length} bytes does not fit the u32 length field"
                )
            }
            WireError::UnsupportedTransport { banner } => write!(
                f,
                "server offered HTTP-tunnel mode (banner {:?}); this is not supported",
                String::from_utf8_lossy(banner)
            ),
            WireError::UnknownBanner { banner } => write!(
                f,
                "unexpected server banner {:?} (expected 'ryit', 'tiyr' or 'pser')",
                String::from_utf8_lossy(banner)
            ),
            WireError::TrailingBytes { remaining } => {
                write!(f, "{remaining} unparsed byte(s) remained after decode")
            }
            WireError::Io(e) => write!(f, "i/o error: {e}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WireError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WireError {
    fn from(e: std::io::Error) -> Self {
        WireError::Io(e)
    }
}

impl From<serde_json::Error> for WireError {
    fn from(e: serde_json::Error) -> Self {
        WireError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, WireError>;

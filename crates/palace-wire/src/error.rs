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
    /// The server's first packet was `SERVERDOWN` (`down`): it ended the
    /// connection before the logon handshake completed, and for a reason.
    ServerDown {
        /// The decoded reason and, for `K_Verbose`, the server's own message.
        reason: crate::messages::ServerDown,
    },
    /// A fixed-size field was consumed but bytes were left over.
    TrailingBytes { remaining: usize },
    /// A prop cannot be registered with `ASSET_REGI` because the reference
    /// client refuses to upload anything that is not a standard wearable prop.
    ///
    /// `PalaceProp.as::assetData` is only ever reached through
    /// `PalaceClient.as::sendPropToServer` (lines 800-818), which drops the
    /// request — "web service big prop... ignore request" — unless the prop is
    /// exactly 44x44 with both offsets in `-44..=88`. We report the rejection
    /// instead of silently writing nothing.
    UnwearableProp {
        /// The rejected prop's width.
        width: i16,
        /// The rejected prop's height.
        height: i16,
        /// The rejected prop's horizontal offset.
        h_offset: i16,
        /// The rejected prop's vertical offset.
        v_offset: i16,
    },
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
            WireError::ServerDown { reason } => {
                write!(f, "server ended the session: {}", reason.reason_text())
            }
            WireError::TrailingBytes { remaining } => {
                write!(f, "{remaining} unparsed byte(s) remained after decode")
            }
            WireError::UnwearableProp {
                width,
                height,
                h_offset,
                v_offset,
            } => write!(
                f,
                "prop {width}x{height} with offsets h={h_offset}, v={v_offset} cannot be \
                 registered: a wearable prop must be 44x44 with offsets in -44..=88"
            ),
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

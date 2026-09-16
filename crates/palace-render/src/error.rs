//! Errors and the "never panic on asset data" policy.
//!
//! The compositor is fed three untrusted things: a parsed room description,
//! decoded prop blobs and background files. None of them may take the process
//! down. This module defines the single error type every fallible operation
//! returns, plus the [`AssetNote`] record used to *report* an asset that could
//! not be resolved instead of silently skipping it.

use std::fmt;

/// Anything that can go wrong while assembling or writing a frame.
#[derive(Debug)]
pub enum RenderError {
    /// An I/O failure that prevents producing output at all (e.g. the output
    /// directory cannot be written).
    Io(std::io::Error),
    /// A background image could not be decoded. The room is still rendered on a
    /// flat backdrop; this is reported, not fatal.
    BackgroundDecode { file: String, detail: String },
    /// A prop blob could not be decoded.
    PropDecode { id: u32, detail: String },
    /// Encoding the finished frame to PNG failed.
    Png(String),
    /// The requested room was not found in the corpus.
    RoomNotFound { id: i32 },
    /// No room payload could be read from the given source.
    NoPayload(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Io(e) => write!(f, "i/o error: {e}"),
            RenderError::BackgroundDecode { file, detail } => {
                write!(f, "background {file:?} failed to decode: {detail}")
            }
            RenderError::PropDecode { id, detail } => {
                write!(f, "prop {id} failed to decode: {detail}")
            }
            RenderError::Png(detail) => write!(f, "png encode failed: {detail}"),
            RenderError::RoomNotFound { id } => write!(f, "room {id} not found"),
            RenderError::NoPayload(detail) => write!(f, "no room payload: {detail}"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RenderError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RenderError {
    fn from(value: std::io::Error) -> Self {
        RenderError::Io(value)
    }
}

/// A non-fatal problem encountered while building a frame.
///
/// The renderer never drops an asset on the floor without saying so: a missing
/// background, an undecodable prop and a hotspot picture with no file all land
/// here, and the caller decides whether to print them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetNote {
    /// The room's background picture could not be resolved or decoded; the flat
    /// backdrop was used instead.
    MissingBackground { name: String, detail: String },
    /// A prop id had no blob in the store; a placeholder was drawn.
    MissingProp { id: u32 },
    /// A prop blob was present but did not decode; a placeholder was drawn.
    BadProp { id: u32, detail: String },
    /// A hotspot-state image overlay referenced a picture id with no file name.
    OverlayWithoutFile { pic_id: i16 },
    /// A hotspot-state image overlay named a file that could not be decoded.
    BadOverlay {
        pic_id: i16,
        file: String,
        detail: String,
    },
    /// The room description itself carried recoverable parse warnings.
    RoomWarning(String),
}

impl fmt::Display for AssetNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetNote::MissingBackground { name, detail } => {
                write!(
                    f,
                    "background {name:?} not found ({detail}); used flat backdrop"
                )
            }
            AssetNote::MissingProp { id } => write!(f, "prop {id} missing; drew placeholder"),
            AssetNote::BadProp { id, detail } => {
                write!(f, "prop {id} undecodable ({detail}); drew placeholder")
            }
            AssetNote::OverlayWithoutFile { pic_id } => {
                write!(f, "overlay picture {pic_id} has no file name")
            }
            AssetNote::BadOverlay {
                pic_id,
                file,
                detail,
            } => write!(f, "overlay picture {pic_id} ({file:?}) failed: {detail}"),
            AssetNote::RoomWarning(w) => write!(f, "room warning: {w}"),
        }
    }
}

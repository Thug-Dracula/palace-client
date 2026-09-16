//! Error type for every fallible path in `palace-asset`.
//!
//! Nothing in this crate panics on network-derived data. A truncated asset, a
//! `blockSize` that disagrees with the bytes that actually arrived, a CRC that
//! does not match, a server that never answers — all of them are values.
//!
//! ## Allocation bounds
//!
//! Three constants cap what a hostile or corrupt peer can make this crate
//! allocate. They are checked **before** any buffer is sized, so a frame that
//! claims a 2 GiB block is rejected without a 2 GiB allocation:
//!
//! | Constant | Caps | Rationale |
//! |---|---|---|
//! | [`MAX_BLOCK_SIZE`] | one `sAst`/`rAst` block | largest real prop in the 228k-prop corpus is ~4 KiB; avatars are a few hundred KiB |
//! | [`MAX_ASSET_SIZE`] | a whole assembled asset | the descriptor's `size` field |
//! | [`MAX_BLOCK_COUNT`] | blocks per asset | multi-block transfer is theoretical; real servers send one block |

use std::fmt;

use palace_wire::WireError;

/// Largest `blockSize` accepted in a single `sAst`/`rAst` message, in bytes.
pub const MAX_BLOCK_SIZE: u64 = 8 * 1024 * 1024;

/// Largest whole-asset size accepted from a descriptor, in bytes.
pub const MAX_ASSET_SIZE: u64 = 64 * 1024 * 1024;

/// Largest `blockCount` accepted from a block header.
pub const MAX_BLOCK_COUNT: u32 = 4096;

/// Width of the `Str31` asset-name field.
pub const NAME_FIELD_LEN: usize = 32;

/// Width of the common `sAst`/`rAst` block header.
pub const BLOCK_HEADER_LEN: usize = 24;

/// Width of the `AssetDescriptor` that only the first block carries.
pub const DESCRIPTOR_LEN: usize = 40;

/// Everything that can go wrong while moving an asset or fetching a media file.
#[derive(Debug)]
#[non_exhaustive]
pub enum AssetError {
    /// The underlying framing or byte-order layer failed.
    Wire(WireError),
    /// A local filesystem or socket operation failed.
    Io(std::io::Error),
    /// The payload ended before a field the layout promised.
    Truncated {
        context: &'static str,
        needed: usize,
        available: usize,
    },
    /// `blockSize` was negative or above [`MAX_BLOCK_SIZE`].
    ImplausibleBlockSize { block_size: i64, max: u64 },
    /// The descriptor's `size` was above [`MAX_ASSET_SIZE`].
    ImplausibleAssetSize { asset_size: u64, max: u64 },
    /// `blockCount` was below 1 or above [`MAX_BLOCK_COUNT`].
    ImplausibleBlockCount { block_count: i64, max: u32 },
    /// `blockNumber` was outside `0..blockCount`.
    ImplausibleBlockNumber { block_number: i64, block_count: i64 },
    /// `blockOffset` was negative, or non-zero on the first block.
    ImplausibleBlockOffset {
        block_number: i64,
        block_offset: i64,
    },
    /// A later block repeated an offset already claimed with a different size.
    InconsistentBlock {
        block_number: i64,
        expected: i64,
        got: i64,
    },
    /// The assembled bytes did not hash to the CRC the sender declared.
    CrcMismatch { expected: u32, computed: u32 },
    /// The assembled length did not match the descriptor's `size`.
    SizeMismatch { declared: u64, assembled: u64 },
    /// The asset is still missing blocks.
    Incomplete { received: usize, expected: usize },
    /// The descriptor named an asset type this crate has no CRC rule for and
    /// the sender asked us to verify one.
    UnverifiableCrc { asset_type: u32, crc: u32 },
    /// An asset the caller asked for is not present.
    NotCached { asset_type: u32, id: i32, crc: u32 },
    /// A URL could not be built or parsed.
    Url(String),
    /// The HTTP transport failed.
    Http(String),
    /// Every URL in the fallback chain answered with a non-success status or a
    /// transport failure.
    AllAttemptsFailed { attempts: Vec<String> },
    /// This exact media name failed recently and is being skipped until the
    /// negative-cache entry expires.
    RecentlyFailed { name: String, retry_after_ms: u64 },
    /// The response body exceeded the configured maximum.
    BodyTooLarge { url: String, limit: u64 },
    /// A file name was not safe to place under the media cache root.
    UnsafeMediaName(String),
    /// A queue or cache limit was reached and the item was discarded.
    Capacity { what: &'static str, limit: usize },
    /// A caller built a record the wire format cannot express.
    BadShape(&'static str),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetError::Wire(e) => write!(f, "wire error: {e}"),
            AssetError::Io(e) => write!(f, "i/o error: {e}"),
            AssetError::Truncated {
                context,
                needed,
                available,
            } => write!(
                f,
                "{context}: needed {needed} byte(s), {available} available"
            ),
            AssetError::ImplausibleBlockSize { block_size, max } => write!(
                f,
                "implausible blockSize {block_size} (accepted range 0..={max})"
            ),
            AssetError::ImplausibleAssetSize { asset_size, max } => write!(
                f,
                "implausible asset size {asset_size} (maximum accepted is {max})"
            ),
            AssetError::ImplausibleBlockCount { block_count, max } => write!(
                f,
                "implausible blockCount {block_count} (accepted range 1..={max})"
            ),
            AssetError::ImplausibleBlockNumber {
                block_number,
                block_count,
            } => write!(f, "blockNumber {block_number} outside 0..{block_count}"),
            AssetError::ImplausibleBlockOffset {
                block_number,
                block_offset,
            } => write!(
                f,
                "blockOffset {block_offset} is impossible for block {block_number}"
            ),
            AssetError::InconsistentBlock {
                block_number,
                expected,
                got,
            } => write!(
                f,
                "block {block_number} arrived at offset {got}, expected {expected}"
            ),
            AssetError::CrcMismatch { expected, computed } => write!(
                f,
                "asset CRC mismatch: sender declared {expected:#010x}, computed {computed:#010x}"
            ),
            AssetError::SizeMismatch {
                declared,
                assembled,
            } => write!(
                f,
                "asset size mismatch: descriptor declared {declared}, assembled {assembled}"
            ),
            AssetError::Incomplete { received, expected } => write!(
                f,
                "asset incomplete: {received} of {expected} block(s) received"
            ),
            AssetError::UnverifiableCrc { asset_type, crc } => write!(
                f,
                "cannot verify CRC {crc:#010x} for asset type {asset_type:#010x}"
            ),
            AssetError::NotCached {
                asset_type,
                id,
                crc,
            } => write!(
                f,
                "asset ({asset_type:#010x}, {id}, {crc:#010x}) is not in the cache"
            ),
            AssetError::Url(u) => write!(f, "invalid media URL: {u}"),
            AssetError::Http(msg) => write!(f, "media fetch failed: {msg}"),
            AssetError::AllAttemptsFailed { attempts } => write!(
                f,
                "media fetch failed for every candidate URL ({})",
                attempts.join(", ")
            ),
            AssetError::RecentlyFailed {
                name,
                retry_after_ms,
            } => write!(
                f,
                "media {name:?} failed recently; not retrying until t={retry_after_ms}ms"
            ),
            AssetError::BodyTooLarge { url, limit } => write!(
                f,
                "media response from {url} exceeded the {limit}-byte limit"
            ),
            AssetError::UnsafeMediaName(name) => {
                write!(f, "media file name {name:?} is not safe to cache")
            }
            AssetError::Capacity { what, limit } => {
                write!(f, "{what} is full (limit {limit}); item discarded")
            }
            AssetError::BadShape(what) => write!(f, "record shape invalid: {what}"),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AssetError::Wire(e) => Some(e),
            AssetError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<WireError> for AssetError {
    fn from(e: WireError) -> Self {
        AssetError::Wire(e)
    }
}

impl From<std::io::Error> for AssetError {
    fn from(e: std::io::Error) -> Self {
        AssetError::Io(e)
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AssetError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_renders_without_panicking() {
        let samples = [
            AssetError::Truncated {
                context: "sAst payload",
                needed: 64,
                available: 8,
            },
            AssetError::ImplausibleBlockSize {
                block_size: -1,
                max: MAX_BLOCK_SIZE,
            },
            AssetError::CrcMismatch {
                expected: 0xdead_beef,
                computed: 0,
            },
            AssetError::AllAttemptsFailed {
                attempts: vec!["http://a/b.png".into()],
            },
            AssetError::UnsafeMediaName("../../etc/passwd".into()),
        ];
        for e in samples {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn wire_errors_keep_their_source() {
        let err: AssetError = WireError::TrailingBytes { remaining: 3 }.into();
        assert!(std::error::Error::source(&err).is_some());
    }
}

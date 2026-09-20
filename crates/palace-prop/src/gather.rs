//! Gather pipeline: copy a prop seen in a room into My Bag, byte-preserving.
//!
//! "Gathering" is the PalaceChat action that takes a sprite the user can see —
//! a prop in a room, another user's avatar, a loose deco object — and makes it
//! the user's own. The reference client spells this as *Take Prop*, *Copy and
//! Wear* and *Save Avatar to Bag*; all three reduce to the one operation
//! implemented here.
//!
//! # Where a gathered prop lands (decision R1)
//!
//! **Every gather writes into My Bag**, through [`crate::bag_store::BagStore`],
//! the bag's single writer. A gather never touches a shelf, never touches
//! PalaceChat's live data, and never invents a collection: it makes a copy in
//! the one collection the user owns, and the source prop stays where it was.
//!
//! # What the pipeline guarantees
//!
//! * **Byte preservation.** The blob is stored exactly as it arrived: this
//!   module never re-encodes pixels and never normalises the header. The record
//!   written to disk therefore carries the source `crc` and the source bytes,
//!   and a later read returns the identical blob.
//! * **Validation before any write.** The blob must decode through the crate's
//!   own [`crate::decode`] path, its payload CRC must match the `crc` the caller
//!   passed (a record's identity must describe its bytes), and it must fit the
//!   size a `.prp` record can hold. A prop that fails any check comes back as
//!   [`GatherOutcome::Rejected`] and **nothing is written** — a rejected gather
//!   cannot leave a half-written collection or a stray file behind.
//! * **Collision policy.** A prop is identified by `(id, crc)`, never by id
//!   alone. Gathering an `(id, crc)` My Bag already holds is a no-op reported as
//!   [`GatherOutcome::AlreadyPresent`], so repeated clicks cannot stack
//!   duplicates. Gathering the same id with a *different* crc is a legal variant
//!   and is added.
//!
//! # Errors versus rejections
//!
//! [`GatherOutcome::Rejected`] is about the *prop*: bytes that do not decode, a
//! size no record can hold, a CRC that does not match. Storage failures (no bag
//! folder configured, an unreadable or corrupt `My Bag.prp`, a held lock) are
//! [`crate::error::PropError`]s and travel through the `Result` instead: the
//! prop was fine, the disk was not.
//!
//! # Example
//!
//! ```
//! use palace_prop::bag_folder::BagContext;
//! use palace_prop::bag_store::BagStore;
//! use palace_prop::crc::payload_crc;
//! use palace_prop::gather::{gather, GatherOutcome};
//!
//! // A 4x1 8-bit prop: four palette-1 pixels in one run.
//! let mut blob = vec![4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
//! blob.extend_from_slice(&[0x04, 1, 1, 1, 1]);
//! let crc = payload_crc(&blob).expect("the blob has a header");
//!
//! let temp = std::env::temp_dir().join(format!("palace-gather-doc-{}", std::process::id()));
//! let store = BagStore::open(BagContext { bag_root: Some(temp.clone()), home: None });
//! assert_eq!(
//!     gather(&store, 7, crc, &blob, Some("Dot")).expect("gather"),
//!     GatherOutcome::Added
//! );
//! assert_eq!(
//!     gather(&store, 7, crc, &blob, None).expect("gather again"),
//!     GatherOutcome::AlreadyPresent
//! );
//! let _ = std::fs::remove_dir_all(&temp);
//! ```

use std::path::PathBuf;

use crate::bag_folder::my_bag_path;
use crate::bag_store::{BagStore, WriteOutcome};
use crate::crc::payload_crc;
use crate::error::{PropError, Result};
use crate::header::HEADER_LEN;

/// Largest blob a gather will store.
///
/// A `.prp` record stores its blob length in the 32-bit `data_size` field
/// ([`crate::prp::AssetRec::data_size`]), so a longer blob cannot be represented
/// on disk. The limit is defensive: the decoder's own dimension budget already
/// refuses anything within an order of magnitude of it.
pub const MAX_GATHER_BLOB_BYTES: usize = u32::MAX as usize;

/// What a gather attempt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatherOutcome {
    /// The prop was written into My Bag.
    Added,
    /// My Bag already held this exact `(id, crc)`; nothing was written.
    AlreadyPresent,
    /// The prop was refused before anything was written. The string is a
    /// human-readable explanation; when the blob did not decode it wraps the
    /// decoder's own message.
    Rejected(String),
}

/// Gather `blob` into My Bag under the identity `(id, crc)`.
///
/// `blob` is a complete prop blob (12-byte header plus payload) exactly as it
/// arrived and is stored verbatim. `crc` is the payload CRC the source metadata
/// claims (`asset_crc` over `blob[12..]`); a mismatch is rejected, because the
/// stored record's identity must match its bytes. `name` is optional; when given
/// the bag store trims it and refuses an empty one.
///
/// Returns:
///
/// * `Ok(Added)` — written into My Bag.
/// * `Ok(AlreadyPresent)` — the exact `(id, crc)` was already in My Bag.
/// * `Ok(Rejected(reason))` — the prop failed validation; **nothing was
///   written**.
/// * `Err(_)` — the bag folder could not be read or written.
pub fn gather(
    store: &BagStore,
    id: i32,
    crc: u32,
    blob: &[u8],
    name: Option<&str>,
) -> Result<GatherOutcome> {
    if let Some(reason) = size_rejection(blob.len()) {
        return Ok(GatherOutcome::Rejected(reason));
    }
    if let Err(error) = crate::decode(blob) {
        return Ok(GatherOutcome::Rejected(format!(
            "the prop does not decode: {error}"
        )));
    }
    match payload_crc(blob) {
        Some(computed) if computed == crc => {}
        Some(computed) => {
            return Ok(GatherOutcome::Rejected(format!(
                "crc {crc:#010x} does not match the blob's payload crc {computed:#010x}; \
                 the record's identity must match its bytes"
            )));
        }
        // `decode` already guarantees the 12 header bytes, so this is only a
        // total-function guard.
        None => {
            return Ok(GatherOutcome::Rejected(format!(
                "the prop blob is shorter than the {HEADER_LEN}-byte header"
            )));
        }
    }

    let target = gather_target(store)?;
    match store.add_prop(&target, id, crc, blob, name)? {
        WriteOutcome::Added => Ok(GatherOutcome::Added),
        WriteOutcome::AlreadyPresent => Ok(GatherOutcome::AlreadyPresent),
        // `add_prop` only reports the two outcomes above; stay total without
        // claiming a write happened.
        other => Ok(GatherOutcome::Rejected(format!(
            "the bag store reported an unexpected outcome for a gather: {other:?}"
        ))),
    }
}

/// The collection a gather writes to: `<bag root>/My Bag.prp` (decision R1).
///
/// The destination is fixed by construction — a gather is always a copy *into*
/// the user's own bag — so a shelf and PalaceChat's live data are unreachable
/// from this pipeline, and no caller can redirect it.
pub fn gather_target(store: &BagStore) -> Result<PathBuf> {
    let Some(root) = store.context().bag_root.as_deref() else {
        return Err(PropError::BagIo {
            detail: "no bag folder configured: set PALACE_PROP_BAG_DIR or a home directory"
                .to_string(),
        });
    };
    Ok(my_bag_path(root))
}

/// Why `len` bytes cannot be gathered, or `None` when the size is fine.
fn size_rejection(len: usize) -> Option<String> {
    if len > MAX_GATHER_BLOB_BYTES {
        Some(format!(
            "the prop blob is {len} bytes; a .prp record can hold at most \
             {MAX_GATHER_BLOB_BYTES} bytes"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_limit_is_exclusive_and_names_the_length() {
        assert!(size_rejection(0).is_none());
        assert!(size_rejection(MAX_GATHER_BLOB_BYTES).is_none());
        // The rejecting side needs a 4 GiB blob, so exercise the pure check
        // with a length instead of allocating one (32-bit hosts skip it: their
        // `usize` cannot name a length past the limit).
        if let Ok(over) = usize::try_from(u64::from(u32::MAX) + 1) {
            let reason = size_rejection(over).expect("over the limit is rejected");
            assert!(
                reason.contains(&over.to_string()),
                "the reason must name the length: {reason}"
            );
        }
    }
}

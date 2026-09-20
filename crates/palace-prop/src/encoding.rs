//! Encoding-preservation policy and prop identity for `.prp` records.
//!
//! Every edit to a roster record is one of exactly two kinds, and the policy
//! exists to keep them apart (spec `PRP-FORMAT.md` §3.1, §4, §5):
//!
//! * a **metadata-only** edit changes the record's name or other map fields but
//!   **not** the blob. The blob's bytes — and therefore the pixel encoding they
//!   carry — must survive exactly, which is what [`preserves_encoding`] checks;
//! * a **pixel** edit replaces the blob's payload. Only this kind of edit may
//!   change the blob, re-encode it and recompute its CRC. [`encoding_for_pixels`]
//!   is the documented rule for the format such an edit produces.
//!
//! The encoding is not stored on disk: it is a property of the 12-byte blob
//! header's flag word (spec §3.1). [`encoding_of`] therefore reads it back out of
//! the record's header through the codec's existing selector rather than
//! re-deriving the flag rules here.
//!
//! ## Prop flags are not `AssetRec.flags`
//!
//! [`crate::prp::AssetRec::flags`] is the server's **runtime** asset flag word
//! (`0x02` loaded, `0x20` failed validation, …) and is always `0` on disk
//! (spec §6). The prop's own `HEAD`/`GHOST`/`RARE`/`ANIMATE`/`BOUNCE` bits — and
//! the pixel-format bits — live in the 12-byte blob header and are reachable as
//! [`PropRecord::prop_flags`]. **Never copy the blob header flags into
//! `AssetRec.flags`**: the runtime field is left at its safe default
//! ([`default_asset_fields`]) and only preserved as an opaque value.
//!
//! This module is pure policy: model and bytes in, answers out. It performs no
//! filesystem access and never re-encodes a prop as a side effect of loading one.

use crate::crc::payload_crc;
use crate::error::{PropError, Result};
use crate::image::PropImage;
use crate::prp::{PropEncoding, PropHeader, PropKey, PropRecord};

/// Which of the two edits a caller is about to make.
///
/// The distinction *is* the policy: [`EditKind::MetadataOnly`] must leave every
/// blob byte — and so its encoding — alone, while [`EditKind::PixelChange`] is
/// the only edit permitted to re-encode a blob and recompute its CRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditKind {
    /// The blob is untouched; only record metadata (name, timestamps, …) moves.
    MetadataOnly,
    /// The pixel payload is replaced; the blob is re-encoded and its CRC recomputed.
    PixelChange,
}

/// The pixel encoding a record's blob header selects, or `None` when the blob
/// has no header at all.
///
/// Delegates to [`PropHeader::encoding`], which in turn uses the codec's
/// `format()` selector, so the 16-bit special case and the format-bit order have
/// exactly one implementation. A headerless blob — the `Fave` sentinel has
/// `data_size == 0` — has no encoding.
#[must_use]
pub fn encoding_of(record: &PropRecord) -> Option<PropEncoding> {
    record.header.map(PropHeader::encoding)
}

/// Whether an edit kept `original`'s blob bytes and encoding.
///
/// This is the writer's guard for a metadata-only change: a record that passes
/// may be written back with its original blob untouched, byte for byte. A pixel
/// edit fails it by construction, which is the point — the encoding may only
/// change when the pixels change.
///
/// The comparison is deliberately about the **blob**, not the record's runtime
/// fields: [`PropRecord`] keeps the raw bytes precisely so this check is exact.
#[must_use]
pub fn preserves_encoding(original: &PropRecord, updated: &PropRecord) -> bool {
    original.blob == updated.blob && encoding_of(original) == encoding_of(updated)
}

/// The encoding a **pixel** edit must use.
///
/// Rust ships exactly one encoder — S20, via [`crate::encode::encode_s20_blob`] —
/// so S20 is the default and only choice. That is not a shortcut: S20 is the
/// format the reference client uploads (`PalaceProp.as::encodeS20BitProp`), and
/// it is the one format with an encoder/decoder round-trip proof in this crate.
///
/// S20 packs pixels **in pairs** (five bytes per two pixels), so an image with an
/// odd width cannot be represented, and neither can a zero-sized one. Both are
/// refused with [`PropError::UnencodableImage`], whose message names the
/// offending dimensions. This mirrors [`crate::encode::encode_s20_payload`]'s own
/// precondition, so a caller cannot select an encoding the encoder will reject.
pub fn encoding_for_pixels(image: &PropImage) -> Result<PropEncoding> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || !width.is_multiple_of(2) {
        return Err(PropError::UnencodableImage { width, height });
    }
    Ok(PropEncoding::S20Bit)
}

/// The identity of a standalone blob, in the roster's `(id, crc)` shape.
///
/// The CRC is [`payload_crc`] — [`asset_crc`](crate::crc::asset_crc) over
/// `blob[12:]`, the payload only, never the 12-byte header (spec §4; hashing the
/// whole blob is the classic mistake). A bare blob carries no asset id, so the
/// [`PropKey::id`] slot is the neutral `0`; the roster writer pairs the returned
/// `crc` with the record's real id. Use [`PropRecord::key`] when the id is known.
///
/// A blob shorter than the 12-byte header has an empty payload, whose CRC is the
/// seed value; this never panics on a short blob.
#[must_use]
pub fn identity(blob: &[u8]) -> PropKey {
    let crc = payload_crc(blob).unwrap_or(crate::crc::ASSET_CRC_MAGIC);
    PropKey::new(0, crc)
}

/// The safe defaults for the two runtime fields of an
/// [`AssetRec`](crate::prp::AssetRec) on write: `(rHandle, lastUseTime) == (0, 0)`.
///
/// Both fields are runtime state, not data. `rHandle` is a loaded-asset handle
/// that the server zeroes on load and saves as `0` (spec §2.4, §6).
/// `lastUseTime` is a timestamp the server never validates, and the format notes
/// state plainly that writing `0` is safe (spec §7). Neither belongs in identity
/// or in the CRC.
#[must_use]
pub const fn default_asset_fields() -> (i32, i32) {
    (0, 0)
}

/// Check that a written record's blob hashes to the CRC the record declares.
///
/// Recomputes [`payload_crc`] (the asset CRC over `record_blob[12:]`) and
/// compares it with `expected_crc`. `Ok(())` means the record just produced is
/// self-consistent; a mismatch means the declared CRC is stale or the payload
/// was altered.
///
/// # Errors
///
/// * [`PropError::HeaderTooShort`] — the blob is shorter than the 12-byte header,
///   so it has no payload to hash.
/// * [`PropError::PayloadTooShort`] — the payload hashes to a different value
///   than `expected_crc`. This crate's error enum has no dedicated CRC variant
///   and is not owned by this task, so the payload-integrity variant carries the
///   two values: its `needed` field is `expected_crc` and its `available` field
///   is the computed CRC.
pub fn validate_written(record_blob: &[u8], expected_crc: u32) -> Result<()> {
    match payload_crc(record_blob) {
        None => Err(PropError::HeaderTooShort {
            available: record_blob.len(),
        }),
        Some(computed) if computed == expected_crc => Ok(()),
        Some(computed) => Err(PropError::PayloadTooShort {
            format: "record CRC",
            needed: expected_crc as usize,
            available: computed as usize,
        }),
    }
}

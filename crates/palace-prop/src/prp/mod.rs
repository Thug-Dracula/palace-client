//! The `.prp` prop-roster container: the typed data model and public contract.
//!
//! A `.prp` (the server's `pserver.prp` asset file) is the flat container Palace
//! servers load their props from. It is **not** a prop itself — it is a roster:
//!
//! ```text
//! AssetFileHeader (16)   data region (blobs)   asset map (types + recs + names)
//! ```
//!
//! A blob is an ordinary 12-byte prop header followed by a pixel payload, and it
//! lives at file offset `FILE_HEADER_LEN + record.data_offset` — the classic
//! `16 +` mistake is why that is a method ([`AssetRec::blob_file_offset`]) and not
//! left to callers. The record's `crc` is over the payload **after** the 12-byte
//! header; the prop's own `HEAD`/`GHOST`/format bits live in that header, *not* in
//! [`AssetRec::flags`], which is the server's runtime state.
//!
//! The authoritative spec is `PRP-FORMAT.md`; this module mirrors its §2 (`model`)
//! and §3/§5 (`record`).
//!
//! ## Reading, writing and canonicalising
//!
//! [`Roster::parse`] lives in [`read`]; [`Roster::write`] in [`write`] is its
//! literal inverse, so a parse→write round trip is byte-identical. To make that
//! possible the model also keeps the raw data region, because real files leave
//! gaps between blobs and only the region preserves those bytes.
//!
//! [`Roster::canonicalise`], [`Roster::add_prop`], [`Roster::add_fave`] and
//! [`Roster::remove_prop`] ([`mutate`]) are the operations allowed to change the
//! layout: they sort by signed id, repack the data region and recompute CRCs.
//!
//! A record is always inserted into its own type's run. `add_prop` targets
//! `Prop`; `add_fave` targets `Fave`, creating that type when it is absent. The
//! split matters because the server validates only `Prop` (`PRP-FORMAT.md` §6):
//! a favourites payload placed in the `Prop` run is read as a headerless prop.
//!
//! ## Identity: `(id, crc)`, never `id` alone
//!
//! [`PropKey`] is `(id, crc)`. Duplicate ids are legal: several records may share
//! an id and differ only in `crc`, and the server picks the variant by crc. Keep
//! such runs adjacent (the [`PropKey`] ordering does: signed id, then crc) and
//! never deduplicate them.

mod model;
mod mutate;
mod read;
mod record;
mod write;

pub use model::{
    AssetFileHeader, AssetMapHeader, AssetRec, AssetType, AssetTypeRec, FILE_HEADER_LEN,
};
pub use record::{PropEncoding, PropHeader, PropKey, PropRecord, PROP_HEADER_LEN};

use crate::error::Result;

/// A parsed `.prp` file: the file header, the map header, the type table, the
/// records and the resolved names.
///
/// The name lookup is kept as an ordered list of `(byte offset, name)` pairs
/// because the names blob has **no** ordering relationship with the record array
/// (spec §2.5); a record resolves its name strictly through its `name_offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roster {
    file: AssetFileHeader,
    map: AssetMapHeader,
    types: Vec<AssetTypeRec>,
    records: Vec<PropRecord>,
    names: Vec<(u32, String)>,
    data: Vec<u8>,
    dropped: usize,
}

impl Roster {
    /// Parse a complete `.prp` file from its bytes.
    ///
    /// Tolerant by design: the header counts are trusted, trailing bytes after
    /// the map are ignored, and a record whose blob falls outside the data region
    /// is dropped (see [`Self::dropped_records`]) rather than failing the parse.
    /// A file shorter than its own header claims is an error, never a panic.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        read::parse(buf)
    }

    /// Serialise this roster back to bytes, preserving it literally.
    ///
    /// A parsed roster written unchanged reproduces the input byte for byte:
    /// the data region (including the gaps between scattered blobs), every record
    /// field, the type table and the names blob are all carried through as they
    /// were read. In particular the stored `crc` is **not** recomputed here — one
    /// real collection carries a stale CRC and repairing it would break
    /// round-tripping. Use [`Self::canonicalise`], [`Self::add_prop`] or
    /// [`Self::remove_prop`] to produce a normalised roster with fresh CRCs.
    ///
    /// The `filesize == assetMapOffset + assetMapSize` invariant is checked
    /// before returning.
    pub fn write(&self) -> Result<Vec<u8>> {
        write::serialize(self)
    }

    /// Sort each type's records by **signed** `id` and repack the data region
    /// contiguously, producing the canonical form the server's binary search
    /// requires.
    ///
    /// The sort is stable, so records that share an id stay adjacent and keep
    /// their original relative order. Nothing is dropped or deduplicated, and
    /// every CRC is recomputed. This is explicitly *not* a byte-exact operation:
    /// it changes the file's layout on purpose.
    pub fn canonicalise(&mut self) {
        mutate::canonicalise(self);
    }

    /// Every record, in file order (contiguous, sorted by signed id per type).
    #[must_use]
    pub fn records(&self) -> &[PropRecord] {
        &self.records
    }

    /// The type table, one [`AssetTypeRec`] per type.
    #[must_use]
    pub fn types(&self) -> &[AssetTypeRec] {
        &self.types
    }

    /// Names as `(byte offset in the names blob, name)`, in blob order.
    #[must_use]
    pub fn names(&self) -> &[(u32, String)] {
        &self.names
    }

    /// How many records were dropped because their blob slice fell outside the
    /// data region. Truncated or corrupt rosters lose records here instead of
    /// failing, so a non-zero count is the signal that the parse was lossy.
    #[must_use]
    pub fn dropped_records(&self) -> usize {
        self.dropped
    }

    /// The 16-byte file header.
    #[must_use]
    pub fn file_header(&self) -> &AssetFileHeader {
        &self.file
    }

    /// The 24-byte map header.
    #[must_use]
    pub fn map_header(&self) -> &AssetMapHeader {
        &self.map
    }

    /// Find a record by its exact identity `(id, crc)`.
    ///
    /// Exact match only: a `crc` of 0 does **not** wildcard here. The server's
    /// wildcard behaviour belongs to a lookup policy the reader can layer on top,
    /// not to the identity of a stored record.
    #[must_use]
    pub fn record_for(&self, key: PropKey) -> Option<&PropRecord> {
        self.records.iter().find(|record| record.key() == key)
    }

    /// The raw blob of the record with this exact identity, if any.
    #[must_use]
    pub fn blob_for(&self, key: PropKey) -> Option<&[u8]> {
        self.record_for(key).map(|record| record.blob.as_slice())
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Insert a record into the `Prop` type, next to any records sharing its id.
    ///
    /// The record's size and CRC are recomputed from its blob, its name is added
    /// to the names blob, and the data region is repacked so the result is a
    /// valid, non-literal roster. Duplicate ids are expected, not merged.
    pub fn add_prop(&mut self, record: PropRecord) {
        mutate::add_prop(self, record);
    }

    /// Insert a record into the `Fave` type, creating that type when it is absent.
    ///
    /// The `Fave` run is the one slot the format reserves for the favourites
    /// record (`PRP-FORMAT.md` §2.3, §7) and the server never validates it
    /// (§6), so a favourites payload must live there rather than in `Prop`.
    /// Like [`Self::add_prop`], the record's size and CRC are recomputed from
    /// its blob, its name joins the names blob, and the data region is repacked.
    pub fn add_fave(&mut self, record: PropRecord) {
        mutate::add_fave(self, record);
    }

    /// Remove and return the record with this exact identity, if present.
    pub fn remove_prop(&mut self, key: PropKey) -> Option<PropRecord> {
        mutate::remove_prop(self, key)
    }
}

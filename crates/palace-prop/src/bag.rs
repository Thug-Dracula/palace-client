//! Reader for the Palace prop bag: a `.bundle` directory holding the client's
//! own collection of props.
//!
//! The modern Palace client keeps its prop bag in
//! `~/.local/share/PalaceChat/PropBag.bundle/`, as two flat files:
//!
//! ```text
//! PalaceChat.pids    the index
//! PalaceChat.props   the blobs, concatenated
//! ```
//!
//! ## The index
//!
//! `.pids` is a flat array of 16-byte **big-endian** records
//! `(a: u32, b: u32, offset: u32, size: u32)`. `offset`/`size` address a slice of
//! `.props`, which is a pure concatenation of blobs: every adjacent record pair
//! satisfies `offset[i] + size[i] == offset[i+1]`. Records are ordered by
//! non-decreasing `offset`.
//!
//! ## The blob wrapper
//!
//! Each blob is a **fixed 32-byte prefix** followed by a standard prop blob, so
//! the prop header begins at `blob[32]`. This was verified on the full live bag:
//! every one of the 3,846 blobs in the snapshot analysed on 2026-09-17 begins
//! with `00 2c 00 2c` at offset 32 (a big-endian 44×44 header), with **zero**
//! exceptions.
//!
//! The prefix is *not* always zero — that was the earlier working assumption and
//! it is wrong. Only 223 of the 3,844 blobs in the original snapshot have an
//! all-zero prefix; the rest carry ASCII metadata, typically one or two
//! length-prefixed NUL-terminated names (e.g. `\x07NewProp\x00\x00\x16The
//! Colosseum (1 vs 1`). The interior layout of the 32 bytes is **not** decoded
//! here and its meaning is undetermined; this reader only relies on its
//! **fixed length**, which is what the decode path needs. Treat the prefix as
//! opaque.
//!
//! ## `(a, b)` identity
//!
//! `(a, b)` is the bag's identity pair, the same key the client names its
//! `BagThumbCache/<a:08X>_<b:08X>.png` thumbnails with. On the real bag:
//!
//! * `a` is the **asset id** — e.g. a real bag entry has `a = 0x3a3ad1f7 =
//!   976933367`, the same id the prop carries in `pserver.prp`.
//! * `b` is the **asset CRC of the prop payload**, i.e.
//!   [`crate::crc::asset_crc`] over everything after the 12-byte prop header
//!   (`blob[44..]`). It matched for **3,569 of 3,846** records.
//!
//! The 277 records where `b` is *not* a payload CRC are exactly the 277 whose
//! `blob[32..]` [`decode`] rejects — the two sets are identical. They are two
//! shapes of record:
//!
//! * 209 built-ins with synthetic ids `a = 0x80000000 + n`, `b = n + 1`, a
//!   header that claims 32-bit (`flags == 0x0100`) and a payload that is not
//!   zlib — i.e. `b` is a sequence number, not a CRC;
//! * 68 large records whose header sets flag bits `0x0400`/`0x0800` (outside the
//!   documented format mask) and whose payload is not a single 8-bit prop. These
//!   are almost certainly animated/multi-frame or "big prop" containers. Their
//!   internal structure is undetermined.
//!
//! So `a`/`b` are reliable identities for ordinary props and only *candidate*
//! meanings for the two exceptional shapes. This reader reports both without
//! asserting the interpretation.
//!
//! ## Confidence
//!
//! | Aspect | Confidence | Evidence |
//! |---|---|---|
//! | 16-byte big-endian record layout | **Certain** | 3,846 records, 0 out-of-bounds, exact tiling |
//! | 32-byte fixed prefix | **High** | 3,846/3,846 blobs have `00 2c 00 2c` at +32 |
//! | `a` = asset id | **High** | matches a `.prp` record id |
//! | `b` = payload CRC | **High** (ordinary props) / undetermined for the 277 exceptions | 3,569 exact matches |
//! | prefix interior semantics | **Low** | not decoded; opaque bytes |

use std::path::{Path, PathBuf};

use crate::error::{PropError, Result};
use crate::header::PropHeader;
use crate::{decode, decode_header, Prop};

/// Bytes of opaque per-blob metadata that precede the prop in a `.props` blob.
pub const BAG_PREFIX_LEN: usize = 32;

/// Bytes per `.pids` index record.
pub const BAG_INDEX_RECORD_LEN: usize = 16;

/// One prop-bag record: its identity pair, its location and its raw blob.
///
/// `blob()` is the bytes exactly as stored, prefix included. `prop_bytes()` /
/// [`BagEntry::decode`] strip the [`BAG_PREFIX_LEN`]-byte prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagEntry {
    a: u32,
    b: u32,
    offset: u32,
    size: u32,
    blob: Vec<u8>,
}

impl BagEntry {
    /// The first identity word; the asset id for ordinary props.
    #[must_use]
    pub fn a(&self) -> u32 {
        self.a
    }

    /// The second identity word; the payload CRC for ordinary props.
    #[must_use]
    pub fn b(&self) -> u32 {
        self.b
    }

    /// The record's byte offset into `.props`.
    #[must_use]
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// The record's blob length, prefix included.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The whole stored blob, 32-byte prefix included.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The prop bytes: the blob with the fixed 32-byte bag prefix removed.
    ///
    /// `None` when the blob is shorter than the prefix — a malformed record, not
    /// a panic.
    #[must_use]
    pub fn prop_bytes(&self) -> Option<&[u8]> {
        self.blob.get(BAG_PREFIX_LEN..)
    }

    /// Parse just the prop header, without decoding pixels.
    ///
    /// Cheap, and the right call for listing thousands of entries.
    pub fn header(&self) -> Result<PropHeader> {
        decode_header(self.prop_bytes().ok_or(PropError::BagBlobTooShort {
            available: self.blob.len(),
        })?)
    }

    /// Decode this entry's prop to a full image.
    ///
    /// Never panics; a malformed or non-prop payload is a [`PropError`].
    pub fn decode(&self) -> Result<Prop> {
        decode(self.prop_bytes().ok_or(PropError::BagBlobTooShort {
            available: self.blob.len(),
        })?)
    }
}

/// A parsed prop bag: the surviving entries plus the index's quality metrics.
///
/// Out-of-bounds records (one whose `offset + size` runs past the end of
/// `.props`) are dropped rather than turning the whole bag into an error, and
/// counted by [`PropBag::out_of_bounds`]. The real bag has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropBag {
    entries: Vec<BagEntry>,
    records: usize,
    trailing_bytes: usize,
    out_of_bounds: usize,
    tiled_pairs: usize,
    adjacent_pairs: usize,
    props_bytes: usize,
}

impl PropBag {
    /// Open a `.bundle` directory by discovering its `*.pids` / `*.props` pair.
    ///
    /// The `.props` path is the `.pids` path with the extension swapped, so
    /// `PalaceChat.pids` pairs with `PalaceChat.props`. The first such pair in
    /// sorted order wins.
    pub fn open_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let (pids, props) = find_pair(dir).ok_or_else(|| PropError::BagNotABundle {
            path: dir.display().to_string(),
        })?;
        Self::open(pids, props)
    }

    /// Open explicit `.pids` and `.props` paths.
    pub fn open(pids: impl AsRef<Path>, props: impl AsRef<Path>) -> Result<Self> {
        let pids = pids.as_ref();
        let props = props.as_ref();
        let index = read(pids)?;
        let blobs = read(props)?;
        Ok(Self::parse(&index, &blobs))
    }

    /// Parse an in-memory index and blob file.
    ///
    /// This cannot fail: bytes that do not form complete records are counted
    /// ([`PropBag::trailing_bytes`]) and records that do not slice `.props` are
    /// dropped ([`PropBag::out_of_bounds`]), so a corrupted bag yields *some*
    /// entries rather than an error.
    #[must_use]
    pub fn parse(index: &[u8], props: &[u8]) -> Self {
        let records = index.len() / BAG_INDEX_RECORD_LEN;
        let trailing_bytes = index.len() % BAG_INDEX_RECORD_LEN;
        let mut entries = Vec::with_capacity(records);
        let (mut out_of_bounds, mut tiled_pairs, mut adjacent_pairs) = (0usize, 0usize, 0usize);
        let mut previous: Option<(u32, u32)> = None;
        for record in index.chunks_exact(BAG_INDEX_RECORD_LEN) {
            let a = u32::from_be_bytes([record[0], record[1], record[2], record[3]]);
            let b = u32::from_be_bytes([record[4], record[5], record[6], record[7]]);
            let offset = u32::from_be_bytes([record[8], record[9], record[10], record[11]]);
            let size = u32::from_be_bytes([record[12], record[13], record[14], record[15]]);
            if let Some((prev_offset, prev_size)) = previous {
                adjacent_pairs += 1;
                if prev_offset.checked_add(prev_size) == Some(offset) {
                    tiled_pairs += 1;
                }
            }
            previous = Some((offset, size));
            let Some(blob) = blob_at(props, offset, size) else {
                out_of_bounds += 1;
                continue;
            };
            entries.push(BagEntry {
                a,
                b,
                offset,
                size,
                blob: blob.to_vec(),
            });
        }
        PropBag {
            entries,
            records,
            trailing_bytes,
            out_of_bounds,
            tiled_pairs,
            adjacent_pairs,
            props_bytes: props.len(),
        }
    }

    /// The entries whose blobs were in bounds, in index order.
    #[must_use]
    pub fn entries(&self) -> &[BagEntry] {
        &self.entries
    }

    /// Number of usable entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the bag has no usable entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Complete 16-byte records found in `.pids`.
    #[must_use]
    pub fn records(&self) -> usize {
        self.records
    }

    /// Bytes left over in `.pids` after the last complete record; 0 for the real
    /// bag.
    #[must_use]
    pub fn trailing_bytes(&self) -> usize {
        self.trailing_bytes
    }

    /// Records dropped because `offset + size` ran past `.props`.
    #[must_use]
    pub fn out_of_bounds(&self) -> usize {
        self.out_of_bounds
    }

    /// Size of the `.props` file this bag was parsed from.
    #[must_use]
    pub fn props_bytes(&self) -> usize {
        self.props_bytes
    }

    /// Adjacent record pairs satisfying `offset[i] + size[i] == offset[i+1]`.
    #[must_use]
    pub fn tiled_pairs(&self) -> usize {
        self.tiled_pairs
    }

    /// Adjacent record pairs in total.
    #[must_use]
    pub fn adjacent_pairs(&self) -> usize {
        self.adjacent_pairs
    }

    /// Whether the blobs tile `.props` with no gaps or overlaps.
    #[must_use]
    pub fn is_contiguous(&self) -> bool {
        self.tiled_pairs == self.adjacent_pairs
    }

    /// The entry with identity pair `(a, b)`, if the bag has one.
    ///
    /// Linear scan, which is fine at bag scale (thousands of entries) and keeps
    /// the type free of an index map the UI may not need.
    #[must_use]
    pub fn find(&self, a: u32, b: u32) -> Option<&BagEntry> {
        self.entries
            .iter()
            .find(|entry| entry.a == a && entry.b == b)
    }
}

fn blob_at(props: &[u8], offset: u32, size: u32) -> Option<&[u8]> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    props.get(start..end)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| PropError::BagIo {
        detail: format!("{}: {e}", path.display()),
    })
}

fn find_pair(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pids"))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().find_map(|pids| {
        let props = pids.with_extension("props");
        props.is_file().then_some((pids, props))
    })
}

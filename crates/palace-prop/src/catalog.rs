//! A read-only catalog over the user's local PalaceChat prop bag.
//!
//! This module turns the on-disk [`crate::bag::PropBag`] into a flat list of
//! browsable entries plus the two binary endpoints a picker needs: a JSON
//! catalog and a PNG thumbnail per prop. It **only ever reads** the user's
//! files — it never creates, writes or repairs anything under
//! `~/.local/share/PalaceChat/`. A missing or malformed bag yields `None`
//! rather than a panic or an error the caller has to handle.
//!
//! # On-disk format
//!
//! The bag is a `PropBag.bundle/` directory, discovered as `PROP_BAG` (see
//! [`PropCatalog::open_default`]) and read entirely with [`crate::bag`]:
//!
//! * `*.pids` — the index. A flat array of 16-byte **big-endian** records
//!   `(id: u32, crc: u32, offset: u32, size: u32)`. On the live bag observed
//!   2026-09-18: 62192 bytes = 3887 records, 0 trailing bytes, 0 out of bounds.
//!   The first **209** records are placeholders — `id` in
//!   `0x8000_0000..0x8000_0100` with `crc` a 1-based sequence number — and are
//!   skipped, leaving **3678** real ids (all unique; no duplicates seen, though
//!   the reader deduplicates by `id` defensively).
//! * `*.props` — the blobs, concatenated. Each blob is a fixed **32-byte
//!   opaque prefix** followed by a standard prop blob. The prop header (six
//!   16-bit words) begins at `blob[32]` and is **big-endian** on every real
//!   entry (`00 2c 00 2c` = a 44×44 header). `size` in the index **includes**
//!   the 32-byte prefix, so the prop proper is `size - 32` bytes at
//!   `offset + 32`.
//! * `*.favs` — one or more 16-byte big-endian records of the **same shape** as
//!   `.pids`, listing the ids the user has favourited. `Trash.favs` is the
//!   trash can. Observed on the live bag: `PalaceChat.favs` is 192 bytes = 12
//!   records, `Trash.favs` is empty; this matches the task's description.
//!   Favourites are matched by `id` alone.
//!
//! # Names
//!
//! The 32-byte blob prefix carries human-readable text, but its interior
//! layout is **not decoded** by [`crate::bag`] and is not fully mapped here
//! either. Two shapes are observable side by side:
//!
//! * a leading `u8` length followed by that many printable bytes, e.g.
//!   `07 "NewProp"`, `0f "PalaceChat Prop"`; and
//! * a printable string that is simply present in the prefix, e.g.
//!   `00 "balamb_blizzaga03"` or `00 00 00 00 00 00 70 35 00 00 14 "<[ Colosseum
//!   Menu ]>"`.
//!
//! There is no single `u8 len` chain that parses both, and the same text
//! appears with different first bytes in different records, so no length field
//! is trusted. Instead [`extract_name`] takes a deliberately conservative view:
//! it returns the **longest printable UTF-8 run of at least three characters
//! that contains an alphanumeric**, or `None`. This yields the documented
//! reference name for id `976933367` (`"The Colosseum (1 vs 1"`, the tail of
//! the full `"The Colosseum (1 vs 1)"` that the fixed 32-byte prefix truncates)
//! and sensible names for most entries, while recording no name for a prefix
//! that is pure binary. It is a heuristic, not a decoded layout; a caller that
//! needs byte-exact names should treat it as best-effort.
//!
//! # What is read-only
//!
//! `PropBag.bundle/` and both `*.favs` files are the user's live client data.
//! This module opens them read-only and holds the parsed bytes in memory; it
//! must never be pointed at a writable copy and never writes back.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::bag::{BagEntry, PropBag, BAG_INDEX_RECORD_LEN, BAG_PREFIX_LEN};

/// Bundle path under the user's home when `PALACE_PROP_BAG` is unset.
const DEFAULT_BAG_SUBDIR: &str = ".local/share/PalaceChat/PropBag.bundle";

/// First id of the client's synthetic built-in range, skipped by the catalog.
const PLACEHOLDER_ID_START: u32 = 0x8000_0000;
/// One past the last synthetic built-in id.
const PLACEHOLDER_ID_END: u32 = 0x8000_0100;

/// One browsable prop: its identity, dimensions, flags and bag state.
///
/// The fields are the frozen contract the presentation layer consumes. `name`
/// is `None` when the prefix carried no trustworthy text (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The asset id, from the `.pids` index.
    pub id: u32,
    /// The payload CRC, from the `.pids` index.
    pub crc: u32,
    /// A best-effort display name; `None` when the prefix had no usable text.
    pub name: Option<String>,
    /// Prop width in pixels.
    pub width: u16,
    /// Prop height in pixels.
    pub height: u16,
    /// The raw prop flag word (format bits + HEAD/GHOST/RARE/ANIMATE/BOUNCE).
    pub flags: u16,
    /// Whether the id appears in a `*.favs` file other than `Trash.favs`.
    pub favorite: bool,
    /// Whether the id appears in `Trash.favs`.
    pub trash: bool,
}

/// A parsed, read-only view of the user's prop bag.
///
/// Holds the bag's blobs in memory (about 10 MB for the live bag) and a flat,
/// deduplicated entry list. Cheap lookups by id; no filesystem access after
/// construction.
#[derive(Debug)]
pub struct PropCatalog {
    bag: PropBag,
    entries: Vec<CatalogEntry>,
    /// `bag_index[i]` is the [`PropBag`] entry backing `entries[i]`.
    bag_index: Vec<usize>,
}

impl PropCatalog {
    /// Open the bag the environment selects.
    ///
    /// Reads `PALACE_PROP_BAG` when it is set, otherwise
    /// `~/.local/share/PalaceChat/PropBag.bundle`. Returns `None` when the
    /// directory is missing or is not a readable `.pids`/`.props` pair; it
    /// never panics and never creates directories.
    #[must_use]
    pub fn open_default() -> Option<Self> {
        let dir = std::env::var_os("PALACE_PROP_BAG")
            .map(PathBuf::from)
            .or_else(default_dir)?;
        Self::open_dir(dir)
    }

    /// Open a specific bundle directory, for callers and tests that already
    /// know the path.
    ///
    /// Same read-only, `None`-on-failure contract as [`PropCatalog::open_default`].
    #[must_use]
    pub fn open_dir(dir: impl AsRef<Path>) -> Option<Self> {
        let dir = dir.as_ref();
        let bag = PropBag::open_dir(dir).ok()?;
        let (favorites, trash) = read_favs(dir);
        Some(Self::from_bag(bag, &favorites, &trash))
    }

    fn from_bag(bag: PropBag, favorites: &HashSet<u32>, trash: &HashSet<u32>) -> Self {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        let mut bag_index = Vec::new();
        for (index, entry) in bag.entries().iter().enumerate() {
            let id = entry.a();
            if (PLACEHOLDER_ID_START..PLACEHOLDER_ID_END).contains(&id) {
                continue;
            }
            if !seen.insert(id) {
                continue;
            }
            let Ok(header) = entry.header() else {
                continue;
            };
            let name = entry.blob().get(..BAG_PREFIX_LEN).and_then(extract_name);
            entries.push(CatalogEntry {
                id,
                crc: entry.b(),
                name,
                width: header.width.max(0) as u16,
                height: header.height.max(0) as u16,
                flags: header.flags,
                favorite: favorites.contains(&id),
                trash: trash.contains(&id),
            });
            bag_index.push(index);
        }
        PropCatalog {
            bag,
            entries,
            bag_index,
        }
    }

    /// The entries, in `.pids` order.
    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// Number of catalogued entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The prop blob proper for `id`: the 12-byte prop header plus payload,
    /// with the 32-byte bundle prefix stripped.
    ///
    /// `None` for an unknown id or a record whose prefix-only blob has no prop
    /// bytes. The returned bytes are a copy; the catalog stays immutable.
    #[must_use]
    pub fn blob(&self, id: u32) -> Option<Vec<u8>> {
        self.bag_entry(id)?.prop_bytes().map(<[u8]>::to_vec)
    }

    /// A PNG thumbnail for `id`, or `None` when the id is unknown or its prop
    /// cannot be decoded (the bag contains a handful of multi-frame records
    /// that the single-prop decoder rejects).
    #[must_use]
    pub fn thumbnail_png(&self, id: u32) -> Option<Vec<u8>> {
        let prop = self.bag_entry(id)?.decode().ok()?;
        prop.image.to_png_bytes().ok()
    }

    /// The whole catalog as the picker's JSON payload.
    ///
    /// The shape is one object with a `props` array; each entry carries `id`,
    /// `crc`, optional `name`, `w`, `h`, `flags`, `fav` and `trash`. `name` is
    /// omitted entirely when unknown. Values are hand-built (the crate has no
    /// JSON dependency) and strings are escaped.
    #[must_use]
    pub fn catalog_json(&self) -> String {
        let mut out = String::with_capacity(self.entries.len() * 96 + 16);
        out.push_str("{\"props\":[");
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"id\":{},\"crc\":{}", entry.id, entry.crc);
            if let Some(name) = &entry.name {
                out.push_str(",\"name\":\"");
                escape_json_into(name, &mut out);
                out.push('"');
            }
            let _ = write!(
                out,
                ",\"w\":{},\"h\":{},\"flags\":{},\"fav\":{},\"trash\":{}}}",
                entry.width, entry.height, entry.flags, entry.favorite, entry.trash
            );
        }
        out.push_str("]}");
        out
    }

    fn bag_entry(&self, id: u32) -> Option<&BagEntry> {
        let position = self.entries.iter().position(|entry| entry.id == id)?;
        let index = *self.bag_index.get(position)?;
        self.bag.entries().get(index)
    }
}

fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(DEFAULT_BAG_SUBDIR))
}

/// Read every `*.favs` file in `dir`, splitting `Trash.favs` from the rest.
///
/// A missing or unreadable directory or file contributes nothing; this is
/// read-only and must never fail the catalog.
fn read_favs(dir: &Path) -> (HashSet<u32>, HashSet<u32>) {
    let mut favorites = HashSet::new();
    let mut trash = HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (favorites, trash);
    };
    for file in entries.flatten() {
        let path = file.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("favs"))
        {
            continue;
        }
        let is_trash = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("trash"));
        let ids = read_id_records(&path);
        if is_trash {
            trash.extend(ids);
        } else {
            favorites.extend(ids);
        }
    }
    (favorites, trash)
}

/// The first (id) word of each 16-byte big-endian record in a `.favs` file.
fn read_id_records(path: &Path) -> Vec<u32> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    bytes
        .chunks_exact(BAG_INDEX_RECORD_LEN)
        .map(|record| u32::from_be_bytes([record[0], record[1], record[2], record[3]]))
        .collect()
}

/// Best-effort display name from an opaque 32-byte bundle prefix.
///
/// See the module docs: the prefix's interior is not decoded, so this returns
/// the longest printable UTF-8 run of three or more characters that contains
/// an alphanumeric, or `None`. It never invents text and never panics.
fn extract_name(prefix: &[u8]) -> Option<String> {
    printable_runs(prefix)
        .into_iter()
        .filter(|text| text.chars().count() >= 3 && text.chars().any(char::is_alphanumeric))
        .max_by_key(|text| text.chars().count())
}

/// Maximal printable runs in a prefix, with an ASCII-only fallback for runs
/// that are not valid UTF-8 on their own.
fn printable_runs(prefix: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    for &byte in prefix {
        if (0x20..=0x7e).contains(&byte) || byte >= 0x80 {
            run.push(byte);
        } else {
            flush_run(&mut run, &mut out);
        }
    }
    flush_run(&mut run, &mut out);
    out
}

fn flush_run(run: &mut Vec<u8>, out: &mut Vec<String>) {
    if run.is_empty() {
        return;
    }
    if let Ok(text) = std::str::from_utf8(run) {
        push_trimmed(text, out);
    } else {
        for part in run.split(|byte| *byte >= 0x80) {
            if let Ok(text) = std::str::from_utf8(part) {
                push_trimmed(text, out);
            }
        }
    }
    run.clear();
}

fn push_trimmed(text: &str, out: &mut Vec<String>) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
}

/// Append `text` to a JSON string, escaping quote, backslash and control
/// characters. UTF-8 passes through unchanged.
fn escape_json_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
}

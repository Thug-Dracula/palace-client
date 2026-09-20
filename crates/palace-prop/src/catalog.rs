//! A read-only catalog over the props the client can render.
//!
//! This module turns the on-disk [`crate::bag::PropBag`] into a flat list of
//! browsable entries plus the two binary endpoints a picker needs: a JSON
//! catalog and a PNG thumbnail per prop. It **only ever reads** the user's
//! files — it never creates, writes or repairs anything under the client's
//! data directory (`~/.local/share/PalaceChat/` on Unix, a `PalaceChat*`
//! directory under `%APPDATA%`/`%LOCALAPPDATA%` on Windows). A missing or
//! malformed bag yields `None` rather than a panic or an error the caller has
//! to handle.
//!
//! # Cache, not bag
//!
//! PalaceChat's `PropBag.bundle` is the client's **cache**, not the user's bag:
//! it holds whatever the client downloaded to render rooms, including props the
//! user never collected. Every entry this reader produces is therefore tagged
//! [`Provenance::Cache`] and can never satisfy a bag query. A bag listing goes
//! through [`bag_listing`] (or [`PropCatalog::bag_entries`]) and returns only
//! [`Provenance::Bag`] entries, which the bag folder's own `.prp` collections
//! supply. The bundle reader itself is unchanged and remains a rendering
//! source for these props.
//!
//! # Discovery
//!
//! [`PropCatalog::open_default`] reads `PALACE_PROP_BAG` when it is set and
//! otherwise searches the per-platform locations documented on `default_dir`:
//! the Windows client's `PalaceChat*` data directories under `%APPDATA%`, then
//! `%LOCALAPPDATA%`; or `~/.local/share/PalaceChat/PropBag.bundle` on Unix.
//! Discovery is read-only and yields `None` when nothing matches.
//!
//! # On-disk format
//!
//! The bag is a `PropBag.bundle/` directory, selected by `PALACE_PROP_BAG`
//! when set (see [`PropCatalog::open_default`]) and read entirely with
//! [`crate::bag`]:
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

pub use crate::provenance::{bag_listing, merge_entries, Provenance};

/// Bundle path under the user's home on Unix when `PALACE_PROP_BAG` is unset.
const DEFAULT_BAG_SUBDIR: &str = ".local/share/PalaceChat/PropBag.bundle";

/// The bundle directory name inside a PalaceChat data directory.
const BAG_DIR_NAME: &str = "PropBag.bundle";

/// The Windows profile subdirectory that holds the per-user AppData roots,
/// and the two leaf names inside it. Used only when `%APPDATA%` /
/// `%LOCALAPPDATA%` themselves are unset.
const WINDOWS_APPDATA_SUBDIR: &str = "AppData";
/// The roaming leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%APPDATA%`).
const WINDOWS_ROAMING_LEAF: &str = "Roaming";
/// The local leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%LOCALAPPDATA%`).
const WINDOWS_LOCAL_LEAF: &str = "Local";

/// A `PalaceChat*` scan is capped at this many directory names per AppData
/// root, so a crafted directory tree cannot turn discovery into a long walk.
const MAX_PALACE_CHAT_DIRS: usize = 4;

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
    /// Where the entry came from: a user-owned bag collection or the cache.
    ///
    /// Assigned by the source that produced the entry, at load time; it is
    /// never recomputed from `id`/`crc`. Entries from PalaceChat's bundle are
    /// [`Provenance::Cache`], so [`bag_listing`] excludes them.
    pub provenance: Provenance,
}

/// A parsed, read-only view of the props one source supplies.
///
/// Holds the source's blobs in memory (about 10 MB for the live bundle) and a
/// flat, deduplicated entry list. Every entry carries the [`Provenance`] of the
/// source that produced it, so a catalog built by this module's bundle reader
/// is cache-only. Cheap lookups by id; no filesystem access after construction.
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
    /// Reads `PALACE_PROP_BAG` when it is set, otherwise follows the
    /// per-platform defaults documented on `default_dir`: on Windows the
    /// `PalaceChat*` data directories under `%APPDATA%` then `%LOCALAPPDATA%`,
    /// on Unix `~/.local/share/PalaceChat/PropBag.bundle`. Returns `None` when
    /// no readable `.pids`/`.props` pair is found; it never panics and never
    /// creates directories.
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
        let provenance = Provenance::cache(dir.display().to_string());
        Some(Self::from_bag(bag, &favorites, &trash, provenance))
    }

    fn from_bag(
        bag: PropBag,
        favorites: &HashSet<u32>,
        trash: &HashSet<u32>,
        provenance: Provenance,
    ) -> Self {
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
                provenance: provenance.clone(),
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
    ///
    /// This is the renderer's view: it includes cache entries. A bag listing
    /// must use [`PropCatalog::bag_entries`] instead.
    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// Only the bag-sourced entries, in `.pids` order.
    ///
    /// Cache entries are a rendering source and never appear here, so a caller
    /// that lists a bag from this catalog cannot leak a cached prop. The
    /// current catalog is built from PalaceChat's bundle, so this is empty;
    /// bag-folder catalogs fill it.
    #[must_use]
    pub fn bag_entries(&self) -> Vec<&CatalogEntry> {
        bag_listing(&self.entries)
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
    /// This is the renderer's view and includes cache entries. A bag listing
    /// must be serialised from [`PropCatalog::bag_entries`] (or from
    /// [`bag_listing`]) through [`entries_json`], which is the shape below.
    #[must_use]
    pub fn catalog_json(&self) -> String {
        entries_json(self.entries.iter())
    }

    fn bag_entry(&self, id: u32) -> Option<&BagEntry> {
        let position = self.entries.iter().position(|entry| entry.id == id)?;
        let index = *self.bag_index.get(position)?;
        self.bag.entries().get(index)
    }
}

/// Serialise `entries` as the picker's JSON payload.
///
/// The shape is one object with a `props` array; each entry carries `id`,
/// `crc`, optional `name`, `w`, `h`, `flags`, `fav` and `trash`, and a
/// bag-sourced entry additionally carries `collection`. `name` is omitted
/// entirely when unknown, and a cache entry carries no `collection` — a cached
/// prop must never look bag-owned. Values are hand-built (the crate has no JSON
/// dependency) and strings are escaped.
#[must_use]
pub fn entries_json<'a>(entries: impl IntoIterator<Item = &'a CatalogEntry>) -> String {
    let entries = entries.into_iter();
    let mut out = String::with_capacity(entries.size_hint().0 * 96 + 16);
    out.push_str("{\"props\":[");
    for (index, entry) in entries.enumerate() {
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
            ",\"w\":{},\"h\":{},\"flags\":{},\"fav\":{},\"trash\":{}",
            entry.width, entry.height, entry.flags, entry.favorite, entry.trash
        );
        if let Provenance::Bag { collection, .. } = &entry.provenance {
            out.push_str(",\"collection\":\"");
            escape_json_into(collection, &mut out);
            out.push('"');
        }
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// The default bundle directory for the host platform.
///
/// `PALACE_PROP_BAG` is checked by [`PropCatalog::open_default`] before this
/// function is consulted, so the full candidate order is:
///
/// 1. `PALACE_PROP_BAG` (always wins, any platform);
/// 2. **Windows:** the first `PropBag.bundle` inside a `PalaceChat*` directory
///    under `%APPDATA%`, then the same under `%LOCALAPPDATA%`. The bare
///    `PalaceChat` name (the current client) is tried first, then versioned
///    names such as `PalaceChat 4` (the older 4.x client). `%APPDATA%` and
///    `%LOCALAPPDATA%` are derived from `%USERPROFILE%\AppData\Roaming` and
///    `%USERPROFILE%\AppData\Local` when the dedicated variables are unset.
///    Returns `None` when no candidate exists;
/// 3. **Unix only:** `$HOME/.local/share/PalaceChat/PropBag.bundle`, returned
///    whether or not it exists, exactly as before — `open_dir` reports the
///    miss as `None`.
fn default_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        windows_default_dir(
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            std::env::var_os("USERPROFILE").map(PathBuf::from),
        )
    } else {
        unix_default_dir(std::env::var_os("HOME").map(PathBuf::from))
    }
}

/// The Unix default: `$HOME/.local/share/PalaceChat/PropBag.bundle`.
///
/// Returns the path whether or not it exists, matching the pre-Windows
/// behaviour; [`PropCatalog::open_dir`] turns a miss into `None`.
fn unix_default_dir(home: Option<PathBuf>) -> Option<PathBuf> {
    Some(home?.join(DEFAULT_BAG_SUBDIR))
}

/// The Windows default: the first `PropBag.bundle` under `%APPDATA%`, then
/// under `%LOCALAPPDATA%` (first hit wins, roaming before local).
///
/// Each root is taken from its dedicated variable when set, otherwise derived
/// from `user_profile` at the standard `AppData\Roaming` / `AppData\Local`
/// location. Candidates are passed in rather than read from the environment so
/// the Windows rung is testable from any host — the same idiom as
/// `palace_client::runtime::cache_root_from`.
fn windows_default_dir(
    app_data: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
) -> Option<PathBuf> {
    let roaming = app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_ROAMING_LEAF));
    let local = local_app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_LOCAL_LEAF));
    roaming
        .as_deref()
        .and_then(first_bag_under)
        .or_else(|| local.as_deref().and_then(first_bag_under))
}

fn derive_appdata(user_profile: &Option<PathBuf>, leaf: &str) -> Option<PathBuf> {
    user_profile
        .as_ref()
        .map(|profile| profile.join(WINDOWS_APPDATA_SUBDIR).join(leaf))
}

/// The first existing `PropBag.bundle` among the `PalaceChat*` directories of
/// `root`, or `None` when the root is unreadable or holds no such bundle.
fn first_bag_under(root: &Path) -> Option<PathBuf> {
    for name in palace_chat_dir_names(root) {
        let candidate = root.join(name).join(BAG_DIR_NAME);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// The names of `PalaceChat*` directories directly under `root`.
///
/// PalaceChat's Windows data directory has had more than one name: the modern
/// client uses the bare `PalaceChat`, the older 4.x client used a versioned
/// name such as `PalaceChat 4`. Instead of hardcoding one, this lists every
/// match, sorted so the bare name comes first and the versioned names follow
/// in name order, capped at [`MAX_PALACE_CHAT_DIRS`]. Matching is ASCII
/// case-insensitive because Windows file names are; an unreadable root yields
/// an empty list.
fn palace_chat_dir_names(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.to_ascii_lowercase().starts_with("palacechat") {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        names.push(name.to_owned());
    }
    names.sort();
    names.truncate(MAX_PALACE_CHAT_DIRS);
    names
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_s20_blob, PropImage};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temporary directory that deletes itself. Each test builds its own
    /// synthetic bags here and never touches the user's real bag.
    struct TempDir(PathBuf);

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "palace-catalog-default-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SYNTHETIC_ID: u32 = 0x3a3a_d1f7;

    /// Write a one-entry synthetic `PropBag.bundle` and return its path.
    ///
    /// The shape matches the real bag (16-byte big-endian `.pids` records, a
    /// 32-byte opaque prefix before an S20 prop) but the bytes are built here,
    /// never copied from any user's data.
    fn write_synthetic_bag(bundle: &Path) -> PathBuf {
        let image = PropImage::transparent(44, 44);
        let prop = encode_s20_blob(&image, 0, 0, 0).expect("encode synthetic prop");
        let mut blob = vec![0xa5u8; BAG_PREFIX_LEN];
        blob.extend_from_slice(&prop);

        let mut index = Vec::new();
        index.extend_from_slice(&SYNTHETIC_ID.to_be_bytes());
        index.extend_from_slice(&0xfeed_beefu32.to_be_bytes());
        index.extend_from_slice(&0u32.to_be_bytes());
        index.extend_from_slice(&(blob.len() as u32).to_be_bytes());

        std::fs::create_dir_all(bundle).expect("create bundle dir");
        std::fs::write(bundle.join("Test.pids"), index).expect("write .pids");
        std::fs::write(bundle.join("Test.props"), blob).expect("write .props");
        bundle.to_path_buf()
    }

    /// A path assertion is only meaningful if the bag at that path opens.
    fn assert_opens(bundle: &Path) {
        let catalog = PropCatalog::open_dir(bundle).expect("synthetic bag opens");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.entries()[0].id, SYNTHETIC_ID);
        assert_eq!(
            catalog.entries()[0].provenance,
            Provenance::cache(bundle.display().to_string()),
            "the bundle reader is a cache source, not a bag source"
        );
        assert!(
            catalog.bag_entries().is_empty(),
            "a cache-only catalog must list no bag entries"
        );
    }

    #[test]
    fn entries_json_marks_bag_entries_with_their_collection_and_leaves_cache_untagged() {
        let bag = CatalogEntry {
            id: 7,
            crc: 9,
            name: Some("My Prop".to_string()),
            width: 44,
            height: 44,
            flags: 0x0200,
            favorite: false,
            trash: false,
            provenance: Provenance::bag_with_id("My Bag", 7, 9),
        };
        let cache = CatalogEntry {
            id: 8,
            crc: 10,
            name: None,
            width: 1,
            height: 2,
            flags: 0,
            favorite: false,
            trash: false,
            provenance: Provenance::cache("PropBag.bundle:/tmp/example"),
        };
        let json = entries_json([&bag, &cache]);
        assert_eq!(
            json,
            concat!(
                "{\"props\":[",
                "{\"id\":7,\"crc\":9,\"name\":\"My Prop\",\"w\":44,\"h\":44,\"flags\":512,",
                "\"fav\":false,\"trash\":false,\"collection\":\"My Bag\"},",
                "{\"id\":8,\"crc\":10,\"w\":1,\"h\":2,\"flags\":0,\"fav\":false,\"trash\":false}",
                "]}"
            )
        );
    }

    #[test]
    fn the_unix_default_is_the_unchanged_home_relative_path() {
        let root = TempDir::new("unix");
        let home = root.path().to_path_buf();
        let expected = home.join(DEFAULT_BAG_SUBDIR);
        assert_eq!(unix_default_dir(Some(home)), Some(expected));
        assert_eq!(unix_default_dir(None), None);
    }

    #[test]
    fn the_windows_default_prefers_appdata_over_local_appdata() {
        let root = TempDir::new("win-order");
        let roaming = root.path().join("roaming");
        let local = root.path().join("local");
        let in_roaming = write_synthetic_bag(&roaming.join("PalaceChat").join(BAG_DIR_NAME));
        write_synthetic_bag(&local.join("PalaceChat").join(BAG_DIR_NAME));

        let picked = windows_default_dir(Some(roaming), Some(local), None)
            .expect("a bag exists in %APPDATA%");
        assert_eq!(picked, in_roaming);
        assert_opens(&picked);
    }

    #[test]
    fn the_windows_default_finds_the_older_versioned_client() {
        let root = TempDir::new("win-v4");
        let roaming = root.path().join("roaming");
        let versioned = write_synthetic_bag(&roaming.join("PalaceChat 4").join(BAG_DIR_NAME));

        let picked = windows_default_dir(Some(roaming), None, None)
            .expect("the 4.x directory is a candidate");
        assert_eq!(picked, versioned);
        assert_opens(&picked);
    }

    #[test]
    fn the_windows_default_prefers_the_bare_palacechat_name() {
        let root = TempDir::new("win-bare");
        let roaming = root.path().join("roaming");
        let modern = write_synthetic_bag(&roaming.join("PalaceChat").join(BAG_DIR_NAME));
        write_synthetic_bag(&roaming.join("PalaceChat 4").join(BAG_DIR_NAME));

        assert_eq!(
            windows_default_dir(Some(roaming), None, None),
            Some(modern),
            "the current client's directory must win over a versioned one"
        );
    }

    #[test]
    fn the_windows_default_is_none_when_no_bundle_exists() {
        let root = TempDir::new("win-none");
        let roaming = root.path().join("roaming");
        let local = root.path().join("local");
        // A PalaceChat directory without a bundle inside must not count as a hit.
        std::fs::create_dir_all(roaming.join("PalaceChat")).expect("create decoy");
        std::fs::create_dir_all(&local).expect("create empty local root");

        assert_eq!(windows_default_dir(Some(roaming), Some(local), None), None);
        assert_eq!(windows_default_dir(None, None, None), None);
    }

    #[test]
    fn the_windows_default_derives_appdata_roots_from_user_profile() {
        let root = TempDir::new("win-profile");
        let profile = root.path().to_path_buf();
        let roaming_appdata = profile
            .join(WINDOWS_APPDATA_SUBDIR)
            .join(WINDOWS_ROAMING_LEAF);
        let local_appdata = profile
            .join(WINDOWS_APPDATA_SUBDIR)
            .join(WINDOWS_LOCAL_LEAF);
        let roaming_bag =
            write_synthetic_bag(&roaming_appdata.join("PalaceChat").join(BAG_DIR_NAME));

        assert_eq!(
            windows_default_dir(None, None, Some(profile.clone())),
            Some(roaming_bag),
            "an unset %APPDATA% falls back to %USERPROFILE%\\AppData\\Roaming"
        );

        let local_bag = write_synthetic_bag(&local_appdata.join("PalaceChat").join(BAG_DIR_NAME));
        std::fs::remove_dir_all(roaming_appdata.join("PalaceChat")).expect("remove roaming bag");
        assert_eq!(
            windows_default_dir(None, None, Some(profile)),
            Some(local_bag),
            "an unset %LOCALAPPDATA% falls back to %USERPROFILE%\\AppData\\Local"
        );
    }
}

//! The bag catalog: the user's `.prp` collections aggregated into one browsable
//! list, plus the JSON payload and the on-demand thumbnail pipeline the UI
//! consumes.
//!
//! [T4](crate::provenance) draws the line between the two ways the client learns
//! about a prop: a **bag** prop is listed by a collection the user owns and may
//! be shown; a **cache** prop was merely seen and must never be listed. This
//! module sits on the bag side of that line. It does not discover anything: the
//! caller ([T13]) hands in the collections it already resolved — "My Bag" plus
//! each shelf — as a name and an already-parsed [`Roster`], and every entry this
//! module produces is tagged [`Provenance::Bag`]. Because no cache source can
//! reach these inputs, a cached prop cannot appear in the catalog *structurally*;
//! [`bag_entries_json`] filters by provenance again anyway, so even a hand-built
//! entry list cannot smuggle one through.
//!
//! # Aggregation
//!
//! One collection contributes the records of its declared `Prop` types, in
//! record order. Collections are visited in caller order ("My Bag" first, then
//! shelves) and entries are keyed by the full `(id, crc)` identity — never by id
//! alone, because a roster may legally carry several crc variants of one id.
//! When two collections list the same key, the first collection wins and the
//! repeat is counted ([`BagCatalog::duplicates`]) rather than shown twice; this
//! mirrors [`crate::provenance::merge_entries`].
//!
//! A record the catalog cannot use is **skipped and counted**, never fatal:
//!
//! * a record whose blob the reader dropped because it fell outside the data
//!   region ([`Roster::dropped_records`]); and
//! * a record inside a `Prop` range whose blob is shorter than the 12-byte
//!   header (a truncated blob), so it has no dimensions or flag word.
//!
//! The count is [`BagCatalog::skipped`]. A record whose header parses but whose
//! payload does not decode is still catalogued — its metadata is trustworthy —
//! and only its thumbnail call fails (see below).
//!
//! # Favourites and trash
//!
//! Favourites live *inside* the roster as the `Fave` type's record: the record
//! id is 128 and its payload is a run of 8-byte little-endian `(id: i32, crc:
//! u32)` specs. The size-0 sentinel contributes nothing. This is verified against
//! the checked-in real fixture `fixtures/prp/real_palace_hidden.prp`, whose one
//! 8-byte payload `b2 b3 dd 63 c2 93 51 50` decodes to id `1675473842`, crc
//! `0x505193C2` — exactly its first `Prop` record. Reading is done here so an
//! entry's `favorite` flag is correct without a second source; *writing* the
//! record belongs to [T9](crate::favorites_trash).
//!
//! Trash is not part of a `.prp` at all — it is app-owned storage. The caller
//! supplies the trashed keys with [`BagCatalog::with_trash`]; matching is by the
//! same `(id, crc)` identity.
//!
//! # Thumbnails
//!
//! [`BagCatalog::thumbnail_png`] decodes one prop blob on demand and returns its
//! PNG. It never decodes the whole collection: a thumbnail is produced only when
//! asked for, and once produced it is cached on disk. The cache file is named
//! `<id:08X>_<crc:08X>.png` — the same identity-pair keying PalaceChat's own
//! `BagThumbCache/<a:08X>_<b:08X>.png` uses — under a directory the caller picks
//! ([`BagCatalog::with_cache_dir`]). A second request reads the file without
//! touching the decoder. Writes are atomic (temp file plus rename), so a reader
//! can never see a half-written PNG, and a cache write failure is ignored: the
//! cache is an optimisation and must never fail a request.
//!
//! An undecodable prop yields `None` and increments
//! [`BagCatalog::thumbnail_failures`]; it is skipped, counted, and never fatal to
//! the rest of the catalog.
//!
//! # JSON
//!
//! [`BagCatalog::catalog_json`] is one `{"props":[...]}` object compatible with
//! the frontend's existing `PropEntry`/`PropsCatalog` shape (`src/lib/api.ts`):
//! `id`, `crc`, optional `name`, `w`, `h`, `flags`, `fav` and `trash` are
//! unchanged, and each entry additionally carries `collection` (the collection
//! it came from) and `source:"bag"` (the provenance kind). `name` is omitted
//! entirely when unknown. The string is hand-built, like
//! [`crate::catalog::entries_json`], because the crate has no JSON dependency;
//! strings are escaped.
//!
//! [T13]: crate

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::catalog::CatalogEntry;
use crate::catalog_paging::{self, CatalogPage, ThumbnailCache};
use crate::provenance::Provenance;
use crate::prp::{AssetType, PropKey, Roster};

/// Bytes of one `Fave` payload spec: a little-endian `(id: i32, crc: u32)` pair.
const FAVE_SPEC_LEN: usize = 8;

/// One caller-supplied bag collection: a display name and its parsed roster.
///
/// The catalog never opens a file; [T13] resolves the bag folder, parses each
/// `.prp` and builds these. "My Bag" and every shelf are the same shape here.
///
/// [T13]: crate
#[derive(Debug)]
pub struct BagCollection {
    name: String,
    roster: Roster,
}

impl BagCollection {
    /// A collection named `name` backed by `roster`.
    #[must_use]
    pub fn new(name: impl Into<String>, roster: Roster) -> Self {
        Self {
            name: name.into(),
            roster,
        }
    }

    /// The collection's display name (`"My Bag"`, a shelf's file name, ...).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The collection's parsed roster.
    #[must_use]
    pub fn roster(&self) -> &Roster {
        &self.roster
    }
}

/// The aggregated view of every collection the caller supplied.
///
/// Holds each roster (so blobs can be decoded later without re-reading files),
/// one [`CatalogEntry`] per distinct `(id, crc)`, and the lookup that maps an
/// identity back to the record that backs it. Every entry is bag-sourced by
/// construction: there is no constructor that accepts a cache.
///
/// # Paging and thumbnails
///
/// The entry list is never serialised or cloned wholesale to browse it: a
/// caller asks for a window with [`BagCatalog::page`] /
/// [`BagCatalog::page_in_collection`] and serialises just that window with
/// [`BagCatalog::catalog_page_json`]. Thumbnails stay on-demand — decoding one
/// prop per requested identity — and an optional in-memory
/// [`ThumbnailCache`] bounds how many decoded PNGs are retained (see
/// [`BagCatalog::with_thumbnail_cache`]). [`BagCatalog::props_decoded`] counts
/// the props actually decoded, so a caller can prove a page of `n` thumbnails
/// decoded `n` props, not the whole shelf.
#[derive(Debug)]
pub struct BagCatalog {
    rosters: Vec<Roster>,
    entries: Vec<CatalogEntry>,
    /// `sources[i]` is `(roster index, record index)` of the blob behind
    /// `entries[i]`.
    sources: Vec<(usize, usize)>,
    /// The `(id, crc)` identity of each entry, to its position in `entries`.
    identities: HashMap<PropKey, usize>,
    skipped: usize,
    duplicates: usize,
    cache_dir: Option<PathBuf>,
    thumbnail_failures: AtomicUsize,
    /// Props actually decoded for a thumbnail since construction. Not
    /// incremented by an on-disk or in-memory cache hit.
    decodes: AtomicUsize,
    /// Opt-in bounded LRU of decoded PNGs; `None` when in-memory caching is
    /// disabled (the default), which leaves the on-disk cache behaviour intact.
    thumbnail_cache: Option<Mutex<ThumbnailCache>>,
}

impl BagCatalog {
    /// Aggregate `collections` into one catalog.
    ///
    /// Visits the collections in order and keeps the first entry for each
    /// `(id, crc)`; later duplicates are counted, not repeated. Reads nothing
    /// from disk and never fails: a record without a usable header is skipped
    /// and counted (see [`BagCatalog::skipped`]).
    #[must_use]
    pub fn new(collections: impl IntoIterator<Item = BagCollection>) -> Self {
        let mut rosters: Vec<Roster> = Vec::new();
        let mut entries: Vec<CatalogEntry> = Vec::new();
        let mut sources: Vec<(usize, usize)> = Vec::new();
        let mut identities: HashMap<PropKey, usize> = HashMap::new();
        let mut skipped = 0usize;
        let mut duplicates = 0usize;

        for collection in collections {
            let BagCollection { name, roster } = collection;
            skipped += roster.dropped_records();
            let favorites = favorite_keys(&roster);
            let roster_index = rosters.len();
            for asset_type in roster.types() {
                if asset_type.kind() != AssetType::Prop {
                    continue;
                }
                let start = asset_type.first_asset.max(0) as usize;
                let end = start
                    .saturating_add(asset_type.nbr_assets.max(0) as usize)
                    .min(roster.records().len());
                for record_index in start..end {
                    let record = &roster.records()[record_index];
                    let Some(header) = record.header else {
                        skipped += 1;
                        continue;
                    };
                    let key = record.key();
                    if identities.contains_key(&key) {
                        duplicates += 1;
                        continue;
                    }
                    let id = record.id() as u32;
                    let crc = record.crc();
                    entries.push(CatalogEntry {
                        id,
                        crc,
                        name: record.name.clone(),
                        width: header.width.max(0) as u16,
                        height: header.height.max(0) as u16,
                        flags: header.flags,
                        favorite: favorites.contains(&key),
                        trash: false,
                        provenance: Provenance::bag_with_id(name.clone(), id, crc),
                    });
                    sources.push((roster_index, record_index));
                    identities.insert(key, entries.len() - 1);
                }
            }
            rosters.push(roster);
        }

        BagCatalog {
            rosters,
            entries,
            sources,
            identities,
            skipped,
            duplicates,
            cache_dir: None,
            thumbnail_failures: AtomicUsize::new(0),
            decodes: AtomicUsize::new(0),
            thumbnail_cache: None,
        }
    }

    /// Mark every entry whose `(id, crc)` is in `keys` as trashed.
    ///
    /// Trash is app-owned state, not a `.prp` fact, so the caller supplies it
    /// (see [T9](crate::favorites_trash)). Unknown keys are ignored.
    #[must_use]
    pub fn with_trash(mut self, keys: impl IntoIterator<Item = PropKey>) -> Self {
        let trash: HashSet<PropKey> = keys.into_iter().collect();
        if !trash.is_empty() {
            for entry in &mut self.entries {
                if trash.contains(&PropKey::new(entry.id as i32, entry.crc)) {
                    entry.trash = true;
                }
            }
        }
        self
    }

    /// Cache thumbnails under `dir`, creating it on first use.
    ///
    /// The directory is supplied by the caller — the catalog never assumes a
    /// location. Without it, thumbnails are still produced but not cached.
    #[must_use]
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// Retain decoded thumbnails in a bounded in-memory LRU.
    ///
    /// `capacity_entries` is the maximum number of PNGs held and
    /// `capacity_bytes` their maximum total size; a lookup that reaches either
    /// cap evicts the least-recently-used thumbnails first. Off by default, so
    /// a caller that wants the old on-disk-cache-only behaviour keeps it.
    #[must_use]
    pub fn with_thumbnail_cache(mut self, capacity_entries: usize, capacity_bytes: usize) -> Self {
        self.thumbnail_cache = Some(Mutex::new(ThumbnailCache::with_caps(
            capacity_entries,
            capacity_bytes,
        )));
        self
    }

    /// Retain decoded thumbnails with the module's default caps
    /// ([`crate::catalog_paging::DEFAULT_THUMBNAIL_CACHE_ENTRIES`] and
    /// [`crate::catalog_paging::DEFAULT_THUMBNAIL_CACHE_BYTES`]).
    #[must_use]
    pub fn with_default_thumbnail_cache(self) -> Self {
        self.with_thumbnail_cache(
            crate::catalog_paging::DEFAULT_THUMBNAIL_CACHE_ENTRIES,
            crate::catalog_paging::DEFAULT_THUMBNAIL_CACHE_BYTES,
        )
    }

    /// The entries, in collection order (the caller's order, then record order).
    ///
    /// Every entry is [`Provenance::Bag`]; deduplicated by `(id, crc)` with the
    /// first collection winning.
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

    /// How many records were skipped: dropped by the reader because their blob
    /// fell outside the data region, plus `Prop` records whose blob is shorter
    /// than the 12-byte header.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// How many records were dropped because another collection already listed
    /// the same `(id, crc)`.
    #[must_use]
    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    /// How many thumbnail requests failed to decode since construction.
    ///
    /// A failure is per request and never affects the catalog or other props; a
    /// later request for the same identity retries (a successful decode is
    /// cached, so a retry after a transient failure can still succeed).
    #[must_use]
    pub fn thumbnail_failures(&self) -> usize {
        self.thumbnail_failures.load(Ordering::Relaxed)
    }

    /// A PNG thumbnail for the entry with this exact `(id, crc)`, or `None` when
    /// the catalog has no such entry or its blob cannot be decoded.
    ///
    /// Serves the in-memory LRU first when enabled, then the on-disk cache, and
    /// only then decodes the one prop; a produced PNG is stored back into both
    /// caches. Never decodes the collection.
    #[must_use]
    pub fn thumbnail_png(&self, id: u32, crc: u32) -> Option<Vec<u8>> {
        let key = PropKey::new(id as i32, crc);
        if let Some(bytes) = self.cached_thumbnail(key) {
            return Some(bytes);
        }

        let position = *self.identities.get(&key)?;
        let (roster_index, record_index) = *self.sources.get(position)?;
        let record = self
            .rosters
            .get(roster_index)?
            .records()
            .get(record_index)?;

        let cache_path = self.cache_path(id, crc);
        if let Some(path) = &cache_path {
            if let Ok(cached) = std::fs::read(path) {
                self.remember_thumbnail(key, &cached);
                return Some(cached);
            }
        }

        let png = thumbnail_png(&record.blob);
        let Some(png) = png else {
            self.thumbnail_failures.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        self.decodes.fetch_add(1, Ordering::Relaxed);

        if let Some(path) = &cache_path {
            write_cache_file(path, &png);
        }
        self.remember_thumbnail(key, &png);
        Some(png)
    }

    /// How many props were actually decoded for thumbnails since construction.
    ///
    /// A cache hit — in memory or on disk — does not increment this, so it is
    /// the count of real decode work. Paging a window of `n` entries and asking
    /// for their thumbnails decodes exactly `n` props.
    #[must_use]
    pub fn props_decoded(&self) -> usize {
        self.decodes.load(Ordering::Relaxed)
    }

    /// The number of thumbnails retained in the in-memory LRU, or `None` when
    /// in-memory caching is disabled.
    #[must_use]
    pub fn thumbnail_cache_len(&self) -> Option<usize> {
        self.with_thumbnail_cache_stats(|cache| cache.len())
    }

    /// Bytes of PNG data retained in the in-memory LRU, or `None` when it is
    /// disabled.
    #[must_use]
    pub fn thumbnail_cache_bytes(&self) -> Option<usize> {
        self.with_thumbnail_cache_stats(ThumbnailCache::memory_bytes)
    }

    /// Thumbnails evicted from the in-memory LRU to stay within its caps, or
    /// `None` when it is disabled.
    #[must_use]
    pub fn thumbnail_cache_evictions(&self) -> Option<usize> {
        self.with_thumbnail_cache_stats(ThumbnailCache::evictions)
    }

    /// Lookups served from the in-memory LRU.
    #[must_use]
    pub fn thumbnail_cache_hits(&self) -> Option<usize> {
        self.with_thumbnail_cache_stats(ThumbnailCache::hits)
    }

    /// Lookups that missed the in-memory LRU.
    #[must_use]
    pub fn thumbnail_cache_misses(&self) -> Option<usize> {
        self.with_thumbnail_cache_stats(ThumbnailCache::misses)
    }

    /// Drop every in-memory thumbnail, keeping its counters. A no-op when the
    /// in-memory cache is disabled.
    pub fn clear_thumbnail_cache(&self) {
        if let Some(cache) = self.thumbnail_cache.as_ref() {
            let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
            guard.clear();
        }
    }

    /// A window of entries, in collection then record order.
    ///
    /// Borrows the entries; nothing is copied or decoded. `offset` past the end
    /// yields an empty slice and `limit` is clamped to the remaining entries.
    #[must_use]
    pub fn page(&self, offset: usize, limit: usize) -> &[CatalogEntry] {
        catalog_paging::page_slice(&self.entries, offset, limit).0
    }

    /// A window of the entries that came from `collection`.
    ///
    /// The returned [`CatalogPage`]'s `total` is that collection's entry count,
    /// so a UI can page one shelf independently.
    #[must_use]
    pub fn page_in_collection(
        &self,
        collection: &str,
        offset: usize,
        limit: usize,
    ) -> (Vec<&CatalogEntry>, CatalogPage) {
        catalog_paging::page_in_collection(&self.entries, collection, offset, limit)
    }

    /// How many entries came from `collection`.
    #[must_use]
    pub fn count_in_collection(&self, collection: &str) -> usize {
        catalog_paging::count_in_collection(&self.entries, collection)
    }

    /// Only the entries in one window, as the picker's JSON payload.
    ///
    /// The full-listing form is [`BagCatalog::catalog_json`]; this one
    /// serialises at most `limit` entries, so browsing a 66k-entry shelf never
    /// builds a 66k-entry string.
    #[must_use]
    pub fn catalog_page_json(&self, offset: usize, limit: usize) -> String {
        bag_entries_json(self.page(offset, limit))
    }

    /// One window of one collection, as the picker's JSON payload.
    #[must_use]
    pub fn catalog_page_json_in_collection(
        &self,
        collection: &str,
        offset: usize,
        limit: usize,
    ) -> String {
        let (window, _) = self.page_in_collection(collection, offset, limit);
        bag_entries_json(window)
    }

    fn cached_thumbnail(&self, key: PropKey) -> Option<Vec<u8>> {
        let cache = self.thumbnail_cache.as_ref()?;
        let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.get(key).map(<[u8]>::to_vec)
    }

    fn remember_thumbnail(&self, key: PropKey, png: &[u8]) {
        let Some(cache) = self.thumbnail_cache.as_ref() else {
            return;
        };
        let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        guard.insert(key, png.to_vec());
    }

    fn with_thumbnail_cache_stats(
        &self,
        read: impl FnOnce(&ThumbnailCache) -> usize,
    ) -> Option<usize> {
        let cache = self.thumbnail_cache.as_ref()?;
        let guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        Some(read(&guard))
    }

    /// The whole catalog as the picker's JSON payload (see the module docs for
    /// the exact shape). Only bag-sourced entries can be emitted.
    #[must_use]
    pub fn catalog_json(&self) -> String {
        bag_entries_json(&self.entries)
    }

    fn cache_path(&self, id: u32, crc: u32) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|dir| dir.join(thumbnail_cache_name(id, crc)))
    }
}

/// The cache file name for an identity pair: `<id:08X>_<crc:08X>.png`.
///
/// The same keying PalaceChat's `BagThumbCache/<a:08X>_<b:08X>.png` uses: the
/// pair is the identity, so a rename elsewhere cannot collide two props.
#[must_use]
pub fn thumbnail_cache_name(id: u32, crc: u32) -> String {
    format!("{id:08X}_{crc:08X}.png")
}

/// Serialise `entries` as the picker's JSON payload, bag-sourced entries only.
///
/// The shape is one object with a `props` array; each entry carries `id`, `crc`,
/// optional `name`, `w`, `h`, `flags`, `fav`, `trash`, `collection` and
/// `source:"bag"`. `name` is omitted entirely when unknown. An entry tagged
/// [`Provenance::Cache`] is dropped — the second line of defence behind the
/// structural one, so an accidental cache entry cannot reach the UI even if a
/// caller assembles the slice by hand.
#[must_use]
pub fn bag_entries_json<'a>(entries: impl IntoIterator<Item = &'a CatalogEntry>) -> String {
    let entries = entries.into_iter();
    let mut out = String::with_capacity(entries.size_hint().0 * 120 + 16);
    out.push_str("{\"props\":[");
    let mut emitted = 0usize;
    for entry in entries {
        let Provenance::Bag { collection, .. } = &entry.provenance else {
            continue;
        };
        if emitted > 0 {
            out.push(',');
        }
        emitted += 1;
        let _ = write!(out, "{{\"id\":{},\"crc\":{}", entry.id, entry.crc);
        if let Some(name) = &entry.name {
            out.push_str(",\"name\":\"");
            escape_json_into(name, &mut out);
            out.push('"');
        }
        let _ = write!(
            out,
            ",\"w\":{},\"h\":{},\"flags\":{},\"fav\":{},\"trash\":{},\"collection\":\"",
            entry.width, entry.height, entry.flags, entry.favorite, entry.trash
        );
        escape_json_into(collection, &mut out);
        out.push_str("\",\"source\":\"bag\"}");
    }
    out.push_str("]}");
    out
}

/// The favourite `(id, crc)` keys a roster's `Fave` records carry.
///
/// A `Fave` blob is a run of little-endian `(id: i32, crc: u32)` specs; the
/// size-0 sentinel seen in real files contributes nothing. This only *reads*
/// the record; writing it is T9's job.
fn favorite_keys(roster: &Roster) -> HashSet<PropKey> {
    let mut keys = HashSet::new();
    for asset_type in roster.types() {
        if asset_type.kind() != AssetType::Fave {
            continue;
        }
        let start = asset_type.first_asset.max(0) as usize;
        let end = start
            .saturating_add(asset_type.nbr_assets.max(0) as usize)
            .min(roster.records().len());
        for record in &roster.records()[start..end] {
            for spec in record.blob.chunks_exact(FAVE_SPEC_LEN) {
                let id = i32::from_le_bytes([spec[0], spec[1], spec[2], spec[3]]);
                let crc = u32::from_le_bytes([spec[4], spec[5], spec[6], spec[7]]);
                keys.insert(PropKey::new(id, crc));
            }
        }
    }
    keys
}

/// A thumbnail PNG for one prop blob.
///
/// An ordinary prop decodes directly. A big/animated prop is rendered from its
/// **frame 0** — the embedded still for the still-PNG class, the 44x44 base
/// still for the animated-WebP class — so a multi-frame record gets a
/// thumbnail instead of a decode failure.
fn thumbnail_png(blob: &[u8]) -> Option<Vec<u8>> {
    if crate::animated::is_big_prop(blob) {
        return crate::animated::thumbnail_png(blob);
    }
    crate::decode(blob).ok()?.image.to_png_bytes().ok()
}

/// Write `bytes` to `path` via a temp file and rename, best effort.
///
/// The cache is an optimisation: any failure is ignored so a request still gets
/// its PNG. The rename is what makes a concurrent reader safe — it sees either
/// the old file (absent) or the complete new one, never a partial write.
fn write_cache_file(path: &Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    if std::fs::write(&temp, bytes).is_err() {
        let _ = std::fs::remove_file(&temp);
        return;
    }
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Append `text` to a JSON string, escaping quote, backslash and control
/// characters. UTF-8 passes through unchanged. Mirrors
/// [`crate::catalog`]'s escaping so both payloads escape identically.
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

    fn entry(id: u32, crc: u32, provenance: Provenance) -> CatalogEntry {
        CatalogEntry {
            id,
            crc,
            name: Some(format!("prop-{id:08x}")),
            width: 44,
            height: 44,
            flags: 0x0200,
            favorite: false,
            trash: false,
            provenance,
        }
    }

    #[test]
    fn the_json_payload_drops_cache_sourced_entries() {
        let bag = entry(7, 9, Provenance::bag_with_id("My Bag", 7, 9));
        let cache = entry(8, 10, Provenance::cache("PropBag.bundle:/tmp/example"));
        let json = bag_entries_json([&bag, &cache]);

        assert!(json.contains("{\"id\":7,\"crc\":9"));
        assert!(
            !json.contains("\"id\":8"),
            "a cache entry must never appear"
        );
        assert!(json.contains("\"collection\":\"My Bag\""));
        assert!(json.contains("\"source\":\"bag\""));
        assert!(!json.contains("null"));
    }

    #[test]
    fn the_cache_key_matches_the_bag_thumb_cache_naming() {
        assert_eq!(
            thumbnail_cache_name(0x3a3a_d1f7, 0x5051_93c2),
            "3A3AD1F7_505193C2.png"
        );
    }

    #[test]
    fn fave_records_of_the_real_fixture_mark_their_prop_favourite() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/prp/real_palace_hidden.prp");
        let bytes = std::fs::read(&path).expect("read the real fixture");
        let roster = Roster::parse(&bytes).expect("parse the real fixture");

        let keys = favorite_keys(&roster);
        assert_eq!(
            keys.len(),
            1,
            "the fixture carries exactly one 8-byte Fave spec"
        );
        let expected = PropKey::new(1_675_473_842, 0x5051_93c2);
        assert!(keys.contains(&expected), "the Fave payload names its prop");

        let catalog = BagCatalog::new([BagCollection::new("My Bag", roster)]);
        let favourite = catalog
            .entries()
            .iter()
            .find(|entry| entry.id == 1_675_473_842)
            .expect("the favourited prop is catalogued");
        assert!(favourite.favorite, "the Fave record marks it favourite");
        assert_eq!(
            catalog
                .entries()
                .iter()
                .filter(|entry| entry.favorite)
                .count(),
            1,
            "no other prop is favourite"
        );
    }
}

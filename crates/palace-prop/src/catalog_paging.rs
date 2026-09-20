//! Paged browsing and a bounded thumbnail cache for the bag catalog.
//!
//! [T31] makes the catalog survive a real worst case — the checked-in
//! `$MEDIA/Prop Files/Palace.prp`, 66,885 records / about 74 MB — without
//! decoding every prop or letting derived state grow without a ceiling. Two
//! pieces do the work:
//!
//! * [`CatalogPage`] plus the [`page_slice`] / [`page_in_collection`] helpers
//!   return a **window** of already-aggregated entries. Asking for entries
//!   `[offset, offset + limit)` never materialises the whole listing, never
//!   clones an entry, and never decodes a prop. Thumbnails are still produced
//!   only when the caller asks for one identity (see
//!   [`crate::bag_catalog::BagCatalog::thumbnail_png`]), so serving a page of
//!   `n` thumbnails decodes exactly `n` props — not 66,885.
//! * [`ThumbnailCache`] is an in-memory LRU bounded on **two** axes: a maximum
//!   number of thumbnails and a maximum number of bytes. Inserting past either
//!   cap evicts the least-recently-used thumbnails first. A thumbnail larger
//!   than the byte cap is never admitted, so one oversized entry cannot evict
//!   the whole cache and then sit in it anyway.
//!
//! # Budgets
//!
//! The task's numeric budgets are declared here so the code and its tests share
//! one source of truth:
//!
//! * [`BAG_LISTING_PEAK_MEMORY_BUDGET_BYTES`] — 512 MiB for an initial listing
//!   of the worst-case shelf.
//! * [`FIRST_PAGE_BUDGET_MS`] — 500 ms to serve the first page once the catalog
//!   is built (warm).
//!
//! They are *budgets*, not guarantees the module can enforce on its own: the
//! peak is a property of the whole listing path (reading the file, parsing the
//! roster, aggregating). `tests/catalog_paging.rs` measures the real path
//! against them.
//!
//! [T31]: crate::bag_catalog

use crate::catalog::CatalogEntry;

/// Default number of thumbnails retained in memory by a
/// [`ThumbnailCache`].
///
/// A thumbnail is a PNG of a prop that is at most
/// [`crate::MAX_DIMENSION`] square; 256 of them is a few MiB in the worst case.
pub const DEFAULT_THUMBNAIL_CACHE_ENTRIES: usize = 256;

/// Default byte ceiling for a [`ThumbnailCache`]: 32 MiB.
///
/// The entry cap usually binds first; this is the guard for an unusually large
/// prop (a big 32-bit PNG), so a run of them cannot grow the cache without
/// bound.
pub const DEFAULT_THUMBNAIL_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Peak-memory budget for an initial listing of the worst-case shelf: 512 MiB.
pub const BAG_LISTING_PEAK_MEMORY_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// Warm first-page budget in milliseconds.
pub const FIRST_PAGE_BUDGET_MS: u128 = 500;

/// A window over a catalog listing.
///
/// Carries the request (`offset`, `limit`), the size of the collection the
/// window was cut from (`total`) and how many entries the window actually
/// holds (`returned`). `returned` is smaller than `limit` only at the end of
/// the listing, so `offset + returned == total` marks the last page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogPage {
    /// The requested start index.
    pub offset: usize,
    /// The requested maximum number of entries.
    pub limit: usize,
    /// The number of entries the window was cut from.
    pub total: usize,
    /// The number of entries the window holds.
    pub returned: usize,
}

impl CatalogPage {
    /// A page descriptor. `returned` is clamped to what `offset`/`total` allow.
    #[must_use]
    pub fn new(offset: usize, limit: usize, total: usize, returned: usize) -> Self {
        Self {
            offset,
            limit,
            total,
            returned,
        }
    }

    /// One past the last entry index this window covers.
    #[must_use]
    pub fn end(&self) -> usize {
        self.offset.saturating_add(self.returned)
    }

    /// Whether entries exist after this window.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.end() < self.total
    }

    /// Whether the window holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.returned == 0
    }
}

/// Cut `[offset, offset + limit)` out of `entries`, clamping to its bounds.
///
/// Returns the borrowed window and its [`CatalogPage`] descriptor. No entry is
/// copied and the whole slice is never cloned; the cost is O(1) in the listing
/// size regardless of how many entries it holds.
#[must_use]
pub fn page_slice(
    entries: &[CatalogEntry],
    offset: usize,
    limit: usize,
) -> (&[CatalogEntry], CatalogPage) {
    let total = entries.len();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    (
        &entries[start..end],
        CatalogPage::new(offset, limit, total, end - start),
    )
}

/// How many entries belong to `collection`.
#[must_use]
pub fn count_in_collection(entries: &[CatalogEntry], collection: &str) -> usize {
    entries
        .iter()
        .filter(|entry| entry.provenance.collection() == Some(collection))
        .count()
}

/// Cut a window out of the entries belonging to `collection`.
///
/// `total` in the returned [`CatalogPage`] is the number of entries in that
/// collection, not the size of the whole listing, so a caller can render "page
/// 3 of 12" for one shelf. The window itself holds at most `limit` borrowed
/// entries; nothing else is copied.
#[must_use]
pub fn page_in_collection<'a>(
    entries: &'a [CatalogEntry],
    collection: &str,
    offset: usize,
    limit: usize,
) -> (Vec<&'a CatalogEntry>, CatalogPage) {
    let matching = entries
        .iter()
        .filter(|entry| entry.provenance.collection() == Some(collection));
    let total = matching.clone().count();
    let window: Vec<&CatalogEntry> = matching.skip(offset).take(limit).collect();
    let page = CatalogPage::new(offset, limit, total, window.len());
    (window, page)
}

/// A bounded, in-memory LRU cache of thumbnail PNGs.
///
/// Keyed by the prop's `(id, crc)` identity — the same key the on-disk
/// `BagThumbCache` uses — so two crc variants of one id never collide. Bounded
/// on two axes ([`ThumbnailCache::capacity_entries`] and
/// [`ThumbnailCache::capacity_bytes`]); a hit promotes the entry to
/// most-recently-used, an insert past a cap evicts from the least-recently-used
/// end first.
///
/// The cache is a pure value type: [`crate::bag_catalog::BagCatalog`] owns one
/// behind a lock and opts into it with
/// `with_thumbnail_cache`. It is disabled by default so existing callers that
/// rely on the on-disk cache's exact behaviour are unaffected.
#[derive(Debug, Clone, Default)]
pub struct ThumbnailCache {
    /// Front is least-recently-used, back is most-recently-used.
    entries: Vec<(crate::prp::PropKey, Vec<u8>)>,
    capacity_entries: usize,
    capacity_bytes: usize,
    bytes: usize,
    hits: usize,
    misses: usize,
    evictions: usize,
    rejected: usize,
}

impl ThumbnailCache {
    /// An empty cache with the given caps. A cap of zero disables that axis
    /// (`capacity_entries == 0` disables the cache entirely).
    #[must_use]
    pub fn with_caps(capacity_entries: usize, capacity_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            capacity_entries,
            capacity_bytes,
            bytes: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            rejected: 0,
        }
    }

    /// An empty cache with the default caps.
    #[must_use]
    pub fn new() -> Self {
        Self::with_caps(
            DEFAULT_THUMBNAIL_CACHE_ENTRIES,
            DEFAULT_THUMBNAIL_CACHE_BYTES,
        )
    }

    /// The entry-count cap.
    #[must_use]
    pub fn capacity_entries(&self) -> usize {
        self.capacity_entries
    }

    /// The byte cap.
    #[must_use]
    pub fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    /// Thumbnails currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes of PNG data currently held.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }

    /// Successful lookups since construction.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// Failed lookups since construction.
    #[must_use]
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// Entries evicted to stay within a cap.
    #[must_use]
    pub fn evictions(&self) -> usize {
        self.evictions
    }

    /// Thumbnails refused because one alone exceeded the byte cap.
    #[must_use]
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// Whether `key` is currently cached.
    #[must_use]
    pub fn contains(&self, key: crate::prp::PropKey) -> bool {
        self.entries.iter().any(|(cached, _)| *cached == key)
    }

    /// Look `key` up, promoting it to most-recently-used on a hit.
    pub fn get(&mut self, key: crate::prp::PropKey) -> Option<&[u8]> {
        let Some(position) = self.entries.iter().position(|(cached, _)| *cached == key) else {
            self.misses = self.misses.saturating_add(1);
            return None;
        };
        if position + 1 != self.entries.len() {
            let entry = self.entries.remove(position);
            self.entries.push(entry);
        }
        self.hits = self.hits.saturating_add(1);
        self.entries.last().map(|(_, bytes)| bytes.as_slice())
    }

    /// Insert or refresh `key` with `bytes`, evicting to stay within both caps.
    ///
    /// A thumbnail larger than the byte cap is refused ([`Self::rejected`])
    /// rather than admitted: keeping it would evict everything else and still
    /// leave the cache over budget.
    pub fn insert(&mut self, key: crate::prp::PropKey, bytes: Vec<u8>) {
        if self.capacity_entries == 0 || self.capacity_bytes == 0 {
            return;
        }
        if bytes.len() > self.capacity_bytes {
            self.rejected = self.rejected.saturating_add(1);
            return;
        }
        if let Some(position) = self.entries.iter().position(|(cached, _)| *cached == key) {
            let (_, old) = self.entries.remove(position);
            self.bytes = self.bytes.saturating_sub(old.len());
        }
        self.bytes = self.bytes.saturating_add(bytes.len());
        self.entries.push((key, bytes));
        self.trim();
    }

    /// Drop every cached thumbnail, keeping the counters.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// Evict least-recently-used entries until both caps hold.
    ///
    /// At least one entry is always kept when the caps allow any admission:
    /// eviction only runs while more than one entry is held.
    fn trim(&mut self) {
        while self.entries.len() > 1
            && (self.entries.len() > self.capacity_entries || self.bytes > self.capacity_bytes)
        {
            let (_, evicted) = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(evicted.len());
            self.evictions = self.evictions.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogEntry;
    use crate::provenance::Provenance;
    use crate::prp::PropKey;

    fn entry(id: u32, crc: u32) -> CatalogEntry {
        CatalogEntry {
            id,
            crc,
            name: None,
            width: 1,
            height: 1,
            flags: 0,
            favorite: false,
            trash: false,
            provenance: Provenance::bag_with_id("My Bag", id, crc),
        }
    }

    fn key(id: u32) -> PropKey {
        PropKey::new(id as i32, 1)
    }

    #[test]
    fn page_slice_clamps_at_both_ends_and_reports_more() {
        let entries: Vec<CatalogEntry> = (0..10).map(|id| entry(id, 1)).collect();
        let (window, page) = page_slice(&entries, 3, 4);
        assert_eq!(window.len(), 4);
        assert_eq!(page.total, 10);
        assert_eq!(page.returned, 4);
        assert!(page.has_more());

        let (window, page) = page_slice(&entries, 8, 4);
        assert_eq!(window.len(), 2);
        assert_eq!(page.end(), 10);
        assert!(!page.has_more());

        let (window, page) = page_slice(&entries, 99, 4);
        assert!(window.is_empty());
        assert!(page.is_empty());
    }

    #[test]
    fn page_in_collection_filters_before_cutting() {
        let entries = vec![entry(0, 1), entry(1, 1), entry(2, 1), entry(3, 1)];
        let (window, page) = page_in_collection(&entries, "My Bag", 1, 2);
        assert_eq!(window.len(), 2);
        assert_eq!(page.total, 4);
        assert_eq!(count_in_collection(&entries, "My Bag"), 4);
        assert_eq!(count_in_collection(&entries, "Other"), 0);
    }

    #[test]
    fn the_lru_evicts_the_least_recently_used_at_the_entry_cap() {
        let mut cache = ThumbnailCache::with_caps(2, usize::MAX);
        cache.insert(key(1), vec![1]);
        cache.insert(key(2), vec![2]);
        assert_eq!(cache.get(key(1)), Some(&[1u8][..]), "1 is now most recent");
        cache.insert(key(3), vec![3]);
        assert_eq!(cache.len(), 2);
        assert!(
            !cache.contains(key(2)),
            "the least recent entry was evicted"
        );
        assert!(cache.contains(key(1)));
        assert!(cache.contains(key(3)));
        assert_eq!(cache.evictions(), 1);
    }

    #[test]
    fn an_oversized_thumbnail_is_refused_not_admitted() {
        let mut cache = ThumbnailCache::with_caps(8, 4);
        cache.insert(key(1), vec![0; 5]);
        assert!(cache.is_empty());
        assert_eq!(cache.rejected(), 1);
        cache.insert(key(2), vec![0; 4]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_bytes(), 4);
    }
}

//! Bag provenance: which catalog entries are bag-sourced and which are cache.
//!
//! The client learns about a prop in exactly two ways, and they must never be
//! confused:
//!
//! * **Bag** — the prop is listed by a collection the user owns: `My Bag`, or a
//!   read-only shelf `.prp` in the bag folder. These are the only entries a bag
//!   listing may show, and each one carries the collection it came from plus
//!   the `(id, crc)` key that collection listed.
//! * **Cache** — the prop was merely *seen*: PalaceChat's live
//!   `PropBag.bundle`, a prop fetched to draw a room, or a `BagThumbCache`
//!   thumbnail. The renderer still needs those bytes, but the bag listing must
//!   never show one.
//!
//! Provenance is a fact about the **source**, not about the prop. The same
//! `(id, crc)` can sit in My Bag and in the live cache at the same time, and
//! nothing in the id, the CRC or the pixels can say whether the user owns it.
//! That is why [`Provenance`] is assigned by whatever produced the entry and
//! stored on [`CatalogEntry`]; it is never re-derived after the fact.
//!
//! Two functions carry the policy:
//!
//! * [`bag_listing`] — the only query a bag surface uses; it returns just the
//!   [`Provenance::Bag`] entries.
//! * [`merge_entries`] — combine bag collections with a cache into one render
//!   list; the same `(id, crc)` in both stores resolves to exactly one entry,
//!   the bag one.

use std::collections::HashSet;

use crate::catalog::CatalogEntry;
use crate::prp::PropKey;

/// Where a catalog entry came from: a user-owned bag collection or the cache.
///
/// Assigned once, at load time, by the source that produced the entry (see the
/// module docs). No later step may turn a `Cache` into a `Bag` or the reverse
/// by inspecting `id`/`crc`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// A prop listed by a collection the user owns.
    ///
    /// `collection` names the collection the source read it from (`"My Bag"`,
    /// a shelf's file name, ...). `key` is the `(id, crc)` the collection
    /// itself listed: it is carried through from the source rather than rebuilt
    /// from the catalog fields, so an entry can be traced to its record.
    Bag {
        /// The owning collection's name.
        collection: String,
        /// The `(id, crc)` the collection listed.
        key: PropKey,
    },
    /// A prop the client only has cached: it may be rendered, never listed.
    ///
    /// `origin` describes the cache the entry came from, e.g. the
    /// `PropBag.bundle` directory the live reader opened.
    Cache {
        /// A human-readable description of the cache that supplied the entry.
        origin: String,
    },
}

impl Provenance {
    /// Bag provenance for `collection` and an explicit identity key.
    #[must_use]
    pub fn bag(collection: impl Into<String>, key: PropKey) -> Self {
        Self::Bag {
            collection: collection.into(),
            key,
        }
    }

    /// Bag provenance for `collection`, building the key from the catalog's
    /// unsigned identity `(id, crc)`.
    ///
    /// The id is reinterpreted as the signed id a `.prp` record stores
    /// (`u32 as i32`), which is exactly the signed view [`PropKey`]'s ordering
    /// expects; real bag ids are unaffected.
    #[must_use]
    pub fn bag_with_id(collection: impl Into<String>, id: u32, crc: u32) -> Self {
        Self::bag(collection, PropKey::new(id as i32, crc))
    }

    /// Cache provenance for the cache named by `origin`.
    #[must_use]
    pub fn cache(origin: impl Into<String>) -> Self {
        Self::Cache {
            origin: origin.into(),
        }
    }

    /// Whether this entry is bag-sourced, and so may appear in a bag listing.
    #[must_use]
    pub const fn is_bag(&self) -> bool {
        matches!(self, Self::Bag { .. })
    }

    /// Whether this entry is cache-sourced, and so must stay out of bag
    /// listings.
    #[must_use]
    pub const fn is_cache(&self) -> bool {
        matches!(self, Self::Cache { .. })
    }

    /// The listed `(id, crc)` for a bag entry; `None` for a cache entry.
    #[must_use]
    pub const fn key(&self) -> Option<PropKey> {
        match self {
            Self::Bag { key, .. } => Some(*key),
            Self::Cache { .. } => None,
        }
    }

    /// The collection name for a bag entry; `None` for a cache entry.
    #[must_use]
    pub fn collection(&self) -> Option<&str> {
        match self {
            Self::Bag { collection, .. } => Some(collection.as_str()),
            Self::Cache { .. } => None,
        }
    }

    /// The cache origin for a cache entry; `None` for a bag entry.
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        match self {
            Self::Cache { origin } => Some(origin.as_str()),
            Self::Bag { .. } => None,
        }
    }
}

/// The bag listing: only [`Provenance::Bag`] entries, in input order.
///
/// This is the one filter every bag surface must go through, so a cache-only
/// prop can never be listed even when the caller has merged a cache into the
/// entry set (see [`merge_entries`]).
#[must_use]
pub fn bag_listing(entries: &[CatalogEntry]) -> Vec<&CatalogEntry> {
    entries
        .iter()
        .filter(|entry| entry.provenance.is_bag())
        .collect()
}

/// Combine bag collections and a cache into one render list.
///
/// Bag entries come first, in the order given; cache entries follow. Entries
/// are deduplicated by `(id, crc)`, so when the same key exists in both a bag
/// and the cache exactly one entry survives — the bag one. Cache-only entries
/// are preserved because the renderer still needs them, while [`bag_listing`]
/// keeps them out of every bag query.
#[must_use]
pub fn merge_entries(
    bag: impl IntoIterator<Item = CatalogEntry>,
    cache: impl IntoIterator<Item = CatalogEntry>,
) -> Vec<CatalogEntry> {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for entry in bag.into_iter().chain(cache) {
        if seen.insert((entry.id, entry.crc)) {
            merged.push(entry);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bag_entry(id: u32, crc: u32, collection: &str) -> CatalogEntry {
        CatalogEntry {
            id,
            crc,
            name: Some(format!("prop-{id:08x}")),
            width: 44,
            height: 44,
            flags: 0x0200,
            favorite: false,
            trash: false,
            provenance: Provenance::bag_with_id(collection, id, crc),
        }
    }

    fn cache_entry(id: u32, crc: u32, origin: &str) -> CatalogEntry {
        CatalogEntry {
            id,
            crc,
            name: None,
            width: 44,
            height: 44,
            flags: 0x0200,
            favorite: false,
            trash: false,
            provenance: Provenance::cache(origin),
        }
    }

    #[test]
    fn the_constructors_tag_the_right_side_and_expose_their_fields() {
        let bag = Provenance::bag_with_id("My Bag", 7, 9);
        assert!(bag.is_bag());
        assert!(!bag.is_cache());
        assert_eq!(bag.collection(), Some("My Bag"));
        assert_eq!(bag.origin(), None);
        assert_eq!(bag.key(), Some(PropKey::new(7, 9)));

        let cache = Provenance::cache("PropBag.bundle");
        assert!(cache.is_cache());
        assert!(!cache.is_bag());
        assert_eq!(cache.origin(), Some("PropBag.bundle"));
        assert_eq!(cache.collection(), None);
        assert_eq!(cache.key(), None);
    }

    #[test]
    fn bag_listing_never_promotes_a_lone_cache_entry() {
        let cache = cache_entry(0x0cac_0001, 0xaaaa_0001, "room render");
        assert!(bag_listing(std::slice::from_ref(&cache)).is_empty());
        assert!(cache.provenance.is_cache());
    }

    #[test]
    fn merge_resolves_a_bag_and_cache_collision_to_one_bag_entry() {
        let bag = bag_entry(7, 9, "My Bag");
        let cache = cache_entry(7, 9, "PropBag.bundle:/tmp/example");
        let merged = merge_entries(vec![bag], vec![cache]);

        assert_eq!(merged.len(), 1, "the key must appear exactly once");
        assert!(merged[0].provenance.is_bag());
        assert_eq!(merged[0].provenance.collection(), Some("My Bag"));
        assert_eq!(bag_listing(&merged).len(), 1);
    }

    #[test]
    fn merge_keeps_cache_only_entries_out_of_the_listing_but_in_the_render_list() {
        let merged = merge_entries(
            vec![bag_entry(7, 9, "My Bag")],
            vec![cache_entry(0x0cac_0001, 1, "PropBag.bundle")],
        );
        assert_eq!(merged.len(), 2, "the renderer keeps the cache entry");
        let listing = bag_listing(&merged);
        assert_eq!(listing.len(), 1);
        assert_eq!((listing[0].id, listing[0].crc), (7, 9));
    }

    #[test]
    fn merge_deduplicates_the_same_key_across_two_bag_collections() {
        let merged = merge_entries(
            vec![
                bag_entry(7, 9, "My Bag"),
                bag_entry(7, 9, "shelf:Legacy.PRP"),
            ],
            Vec::new(),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].provenance.collection(), Some("My Bag"));
    }
}

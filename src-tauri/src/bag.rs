//! The prop bag, as the desktop shell sees it.
//!
//! The real work lives in `palace-prop`: the bag folder and its write guard
//! ([`palace_prop::bag_folder`]), the one writer ([`palace_prop::bag_store`]),
//! the browseable catalog ([`palace_prop::bag_catalog`]), shelf discovery
//! ([`palace_prop::shelves`]), favourites and trash
//! ([`palace_prop::favorites_trash`]), the byte-preserving gather pipeline
//! ([`palace_prop::gather`]) and the outfits document
//! ([`palace_prop::outfits`] / [`palace_prop::outfit_ops`]).
//!
//! This module is the thin shell-facing layer over those: it resolves the bag
//! folder once, turns every crate type into a plain serialisable record for the
//! webview, and gives the Tauri commands one place to call. It holds no state
//! beyond the [`BagContext`] — each operation opens the store it needs, so a
//! command never sees a stale handle.
//!
//! # The catalog snapshot
//!
//! The webview browses through one [`BagSnapshot`]: the aggregated
//! [`BagCatalog`] plus the shelf health list. It is rebuilt after every
//! mutation. Every entry it carries is bag-sourced by construction — the
//! catalog has no cache input — so a cached prop cannot reach the UI.
//!
//! Thumbnails are cached under the client's own cache directory
//! (`<cache>/org.palace.client/bag-thumbs`), never inside the bag folder, so a
//! bag root that happens to sit in a protected tree still cannot be written to
//! through a thumbnail.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use palace_prop::bag_catalog::{thumbnail_cache_name, BagCatalog, BagCollection};
use palace_prop::bag_folder::{my_bag_path as bag_file_path, BagContext};
use palace_prop::bag_store::{BagStore, WriteOutcome};
use palace_prop::favorites_trash::{fave_record, Favorites, TrashStore};
use palace_prop::gather::{self, GatherOutcome};
use palace_prop::outfit_ops;
use palace_prop::outfits::{Outfit, OutfitStore};
use palace_prop::prp::{PropKey, Roster};
use palace_prop::shelves::{Shelf, ShelfSet, ShelfStatus};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The catalog plus the shelf health the bag browser shows.
///
/// Cheap to clone (the catalog is behind an [`Arc`]); a command hands one to
/// the shared slot after a mutation.
#[derive(Debug, Clone)]
pub struct BagSnapshot {
    /// The aggregated, bag-only catalog.
    pub catalog: Arc<BagCatalog>,
    /// One record per shelf the bag folder exposes, healthy or not.
    pub shelves: Vec<BagShelfInfo>,
    favourites: HashSet<(u32, u32)>,
    fave_keys: HashSet<(u32, u32)>,
}

impl BagSnapshot {
    /// A snapshot over `catalog` with no favourite bookkeeping.
    ///
    /// The favourites are read out of the `.prp` `Fave` records by
    /// [`BagService::snapshot`]; a snapshot built without them simply reports
    /// no entry as favourite.
    #[must_use]
    pub fn new(catalog: Arc<BagCatalog>, shelves: Vec<BagShelfInfo>) -> Self {
        Self {
            catalog,
            shelves,
            favourites: HashSet::new(),
            fave_keys: HashSet::new(),
        }
    }

    /// An empty snapshot, for a machine with no bag folder at all.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(
            Arc::new(BagCatalog::new(Vec::<BagCollection>::new())),
            Vec::new(),
        )
    }

    /// The browsable entries, favourites resolved.
    ///
    /// A `Fave` record is bookkeeping, not a prop, so it is excluded even when
    /// a rewrite filed it under the `Prop` type. The favourite flag is the union
    /// of what the catalog's type-based reader found and what the tolerant
    /// [`Favorites::read`] resolves, so both real `.prp` favourites and this
    /// app's own writes are reported.
    #[must_use]
    pub fn entries(&self) -> Vec<BagPropEntry> {
        self.catalog
            .entries()
            .iter()
            .filter(|entry| !self.fave_keys.contains(&(entry.id, entry.crc)))
            .map(|entry| {
                let key = (entry.id, entry.crc);
                BagPropEntry {
                    id: entry.id,
                    crc: entry.crc,
                    name: entry.name.clone(),
                    w: entry.width,
                    h: entry.height,
                    flags: entry.flags,
                    fav: entry.favorite || self.favourites.contains(&key),
                    trash: entry.trash,
                    collection: entry
                        .provenance
                        .collection()
                        .unwrap_or("My Bag")
                        .to_string(),
                    source: "bag",
                }
            })
            .collect()
    }

    /// The catalog as the picker's `{"props":[...]}` payload.
    #[must_use]
    pub fn catalog_json(&self) -> String {
        #[derive(Serialize)]
        struct Payload<'a> {
            props: &'a [BagPropEntry],
        }
        let props = self.entries();
        serde_json::to_string(&Payload { props: &props })
            .unwrap_or_else(|_| "{\"props\":[]}".to_string())
    }
}

/// One collection in the bag folder, as the UI lists it.
#[derive(Debug, Clone, Serialize)]
pub struct BagCollectionInfo {
    /// The collection's file path.
    pub path: String,
    /// The display name (`My Bag`, a shelf's file stem).
    pub name: String,
    /// Whether the store may write to it (only My Bag).
    pub writable: bool,
    /// Whether the file exists on disk yet.
    pub exists: bool,
}

/// One shelf's health and location.
#[derive(Debug, Clone, Serialize)]
pub struct BagShelfInfo {
    /// The shelf's display name (its file stem).
    pub name: String,
    /// The shelf's `.prp` path.
    pub path: String,
    /// `healthy`, `degraded` or `unreadable`.
    pub status: String,
    /// Records the roster holds.
    pub records: usize,
    /// Records whose name resolved.
    pub named: usize,
    /// Records with no usable prop header.
    pub undecodable: usize,
    /// Records the parser dropped because their blob fell outside the data.
    pub dropped: usize,
    /// A human-readable reason when the shelf could not be read.
    pub error: Option<String>,
}

/// One browsable bag prop. The shape matches the frontend's `PropEntry`, plus
/// the owning collection and an explicit bag provenance tag.
#[derive(Debug, Clone, Serialize)]
pub struct BagPropEntry {
    /// The asset id (unsigned view of the signed `.prp` id).
    pub id: u32,
    /// The payload CRC that completes the identity.
    pub crc: u32,
    /// The display name when the record has one.
    pub name: Option<String>,
    /// Width in pixels.
    pub w: u16,
    /// Height in pixels.
    pub h: u16,
    /// The prop flag word.
    pub flags: u16,
    /// Whether the collection marks it a favourite.
    pub fav: bool,
    /// Whether the trash document lists it.
    pub trash: bool,
    /// The collection the entry came from.
    pub collection: String,
    /// Always `"bag"`: the catalog only ever holds bag-sourced entries.
    pub source: &'static str,
}

/// One `(id, crc)` identity pair crossing the command boundary.
///
/// The id is the **unsigned** view (`u32`), matching both the catalog and the
/// worn-prop list the protocol uses. Internally a `.prp` record stores a signed
/// id; the conversions reinterpret the bits rather than losing range, exactly as
/// [`palace_prop::provenance::Provenance::bag_with_id`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BagKey {
    /// The asset id, as the catalog lists it.
    pub id: u32,
    /// The payload CRC.
    pub crc: u32,
}

impl BagKey {
    /// The crate identity this pair names.
    #[must_use]
    pub fn to_prop_key(self) -> PropKey {
        PropKey::new(self.id as i32, self.crc)
    }

    /// The boundary pair for a crate identity.
    #[must_use]
    pub fn from_prop_key(key: PropKey) -> Self {
        Self {
            id: key.id as u32,
            crc: key.crc,
        }
    }
}

/// One trashed prop, as the UI lists it.
#[derive(Debug, Clone, Serialize)]
pub struct BagTrashEntry {
    /// The asset id, as the catalog lists it.
    pub id: u32,
    /// The payload CRC.
    pub crc: u32,
    /// The prop's name, when it had one.
    pub name: Option<String>,
    /// The stored blob's length in bytes.
    pub bytes: usize,
}

/// One saved outfit.
#[derive(Debug, Clone, Serialize)]
pub struct BagOutfit {
    /// The outfit's name.
    pub name: String,
    /// The props to wear, in order.
    pub props: Vec<BagKey>,
}

/// The outcome of preparing an outfit for wearing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BagOutfitApplication {
    /// The references to wear, in outfit order.
    pub worn: Vec<BagKey>,
    /// The references the bag does not hold, in outfit order.
    pub missing: Vec<BagKey>,
}

/// What a bag mutation did. Mirrors [`WriteOutcome`] but is serialisable, so a
/// command reports a typed outcome instead of a bare boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BagOutcome {
    /// The prop was written.
    Added,
    /// The exact `(id, crc)` already existed; nothing changed.
    AlreadyPresent,
    /// The prop was removed.
    Removed,
    /// A name changed.
    Renamed,
    /// A prop was copied.
    Duplicated,
    /// A prop moved between collections.
    Moved,
    /// A collection was created.
    Created,
    /// A collection was deleted.
    Deleted,
    /// The request was valid but changed nothing.
    Unchanged,
    /// The addressed record or collection did not exist.
    NotFound,
    /// The collection already existed.
    AlreadyExists,
}

impl From<WriteOutcome> for BagOutcome {
    fn from(outcome: WriteOutcome) -> Self {
        match outcome {
            WriteOutcome::Added => Self::Added,
            WriteOutcome::AlreadyPresent => Self::AlreadyPresent,
            WriteOutcome::Removed => Self::Removed,
            WriteOutcome::Renamed => Self::Renamed,
            WriteOutcome::Duplicated => Self::Duplicated,
            WriteOutcome::Moved => Self::Moved,
            WriteOutcome::Created => Self::Created,
            WriteOutcome::Deleted => Self::Deleted,
            WriteOutcome::Unchanged => Self::Unchanged,
            WriteOutcome::NotFound => Self::NotFound,
            WriteOutcome::AlreadyExists => Self::AlreadyExists,
        }
    }
}

/// Whether a favourite toggle changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FavouriteResult {
    /// Whether the key is a favourite after the call.
    pub favourite: bool,
    /// Whether the stored list changed.
    pub changed: bool,
}

/// What a gather attempt did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GatherResult {
    /// The prop was written into My Bag.
    Added,
    /// My Bag already held this exact `(id, crc)`.
    AlreadyPresent,
    /// The prop was refused before anything was written.
    Rejected {
        /// Why the prop was refused.
        reason: String,
    },
}

/// How many entries a purge removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PurgeResult {
    /// Number of trashed props that were discarded.
    pub purged: usize,
}

/// What rebuilding one thumbnail did.
///
/// Mirrors PalaceChat's "Rebuild thumbnail": the cached PNG for one `(id, crc)`
/// is dropped and produced again from the stored blob. `rebuilt` is false when
/// the bag has no such entry or its blob cannot be decoded, matching
/// [`BagCatalog::thumbnail_png`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThumbnailRebuild {
    /// The asset id addressed.
    pub id: u32,
    /// The payload CRC that completes the identity.
    pub crc: u32,
    /// Whether a thumbnail was produced.
    pub rebuilt: bool,
    /// The regenerated PNG's length in bytes; zero when nothing was produced.
    pub bytes: usize,
}

/// How many cache files a "rebuild all" dropped.
///
/// Regeneration is lazy, so the count is what was removed, not what was
/// immediately redrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ThumbnailRebuildAll {
    /// Cache files removed; their thumbnails regenerate on the next request.
    pub removed: usize,
}

/// The bag folder and the operations the shell exposes over it.
#[derive(Debug, Clone)]
pub struct BagService {
    context: BagContext,
}

impl BagService {
    /// Build a service over an explicit bag context.
    #[must_use]
    pub fn open(context: BagContext) -> Self {
        Self { context }
    }

    /// Build a service over the environment-discovered bag folder.
    #[must_use]
    pub fn discover() -> Self {
        Self::open(BagContext::discover())
    }

    /// The context this service reads and writes through.
    #[must_use]
    pub fn context(&self) -> &BagContext {
        &self.context
    }

    /// The one writer over this context.
    fn store(&self) -> BagStore {
        BagStore::open(self.context.clone())
    }

    /// The bag root, or a clear error when no folder is configured.
    fn bag_root(&self) -> Result<PathBuf, String> {
        self.context.bag_root.clone().ok_or_else(|| {
            "no bag folder configured: set PALACE_PROP_BAG_DIR or a home directory".to_string()
        })
    }

    /// `<bag root>/My Bag.prp`.
    fn my_bag_path(&self) -> Result<PathBuf, String> {
        Ok(bag_file_path(&self.bag_root()?))
    }

    /// Aggregate My Bag and every shelf into the snapshot the UI browses.
    ///
    /// Read-only: no file is created or changed. A missing My Bag or an
    /// unreadable shelf contributes nothing rather than failing the snapshot;
    /// the shelf list still reports the failure so the UI can show it.
    #[must_use]
    pub fn snapshot(&self) -> BagSnapshot {
        self.snapshot_with_cache_dir(bag_thumb_cache_dir().as_deref())
    }

    /// Aggregate the snapshot, caching thumbnails under `cache_dir`.
    ///
    /// [`BagService::snapshot`] resolves the ambient cache directory and calls
    /// this. The explicit form lets a test point the cache at a temp directory
    /// without mutating the process environment.
    fn snapshot_with_cache_dir(&self, cache_dir: Option<&Path>) -> BagSnapshot {
        let Some(root) = self.context.bag_root.clone() else {
            return BagSnapshot::empty();
        };

        let shelf_set = ShelfSet::discover(&root);
        let shelves: Vec<BagShelfInfo> = shelf_set.shelves().iter().map(shelf_info).collect();

        let mut favourites: HashSet<(u32, u32)> = HashSet::new();
        let mut fave_keys: HashSet<(u32, u32)> = HashSet::new();
        let mut collections: Vec<BagCollection> = Vec::new();
        if let Some(roster) = read_roster(&bag_file_path(&root)) {
            collect_favourites(&roster, &mut favourites, &mut fave_keys);
            collections.push(BagCollection::new("My Bag", roster));
        }
        for shelf in shelf_set.shelves() {
            if let Some(roster) = read_roster(&shelf.path) {
                collect_favourites(&roster, &mut favourites, &mut fave_keys);
                collections.push(BagCollection::new(shelf.name.clone(), roster));
            }
        }
        drop(shelf_set);

        let trash_keys = TrashStore::open(self.context.clone()).keys();
        let mut catalog = BagCatalog::new(collections).with_trash(trash_keys);
        if let Some(dir) = cache_dir {
            catalog = catalog.with_cache_dir(dir);
        }

        BagSnapshot {
            catalog: Arc::new(catalog),
            shelves,
            favourites,
            fave_keys,
        }
    }

    /// Forget and regenerate one `(id, crc)`'s cached thumbnail.
    ///
    /// The thumbnail cache is keyed by identity and lives on disk under the
    /// client's cache tree. This drops that one file, discards any in-memory
    /// copy, then decodes the prop again and returns the new PNG. `None` means
    /// the bag has no such entry or its blob cannot be decoded.
    pub fn rebuild_thumbnail(&self, id: u32, crc: u32) -> Result<Option<Vec<u8>>, String> {
        self.rebuild_thumbnail_with(bag_thumb_cache_dir().as_deref(), id, crc)
    }

    /// [`BagService::rebuild_thumbnail`] against an explicit cache directory.
    fn rebuild_thumbnail_with(
        &self,
        cache_dir: Option<&Path>,
        id: u32,
        crc: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        let snapshot = self.snapshot_with_cache_dir(cache_dir);
        if let Some(dir) = cache_dir {
            remove_thumb_file(&dir.join(thumbnail_cache_name(id, crc)))?;
        }
        snapshot.catalog.clear_thumbnail_cache();
        Ok(snapshot.catalog.thumbnail_png(id, crc))
    }

    /// Drop every cached bag thumbnail, returning how many files were removed.
    ///
    /// Regeneration is lazy: the next thumbnail request decodes its prop and
    /// rewrites the cache file. A cache directory that does not exist yet is
    /// zero removals, not an error.
    pub fn rebuild_thumbnails(&self) -> Result<usize, String> {
        match bag_thumb_cache_dir() {
            Some(dir) => clear_thumb_cache_dir(&dir),
            None => Ok(0),
        }
    }

    /// Every collection the bag folder exposes, My Bag first.
    pub fn collections(&self) -> Result<Vec<BagCollectionInfo>, String> {
        let list = self
            .store()
            .list_collections()
            .map_err(|error| error.to_string())?;
        Ok(list
            .into_iter()
            .map(|collection| {
                let writable = collection.is_writable();
                BagCollectionInfo {
                    path: collection.path.display().to_string(),
                    name: collection.name,
                    writable,
                    exists: collection.exists,
                }
            })
            .collect())
    }

    /// Add a prop blob to My Bag. The CRC must match the blob's payload.
    pub fn add_prop(
        &self,
        id: u32,
        crc: u32,
        blob: &[u8],
        name: Option<&str>,
    ) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .add_prop(&target, id as i32, crc, blob, name)
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Remove one `(id, crc)` from My Bag.
    pub fn remove_prop(&self, id: u32, crc: u32) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .remove_prop(&target, PropKey::new(id as i32, crc))
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Move a prop between two writable collections.
    ///
    /// My Bag is the only writable collection, so a same-file move reports
    /// [`BagOutcome::Unchanged`] without touching the disk.
    pub fn move_prop(
        &self,
        from: &Path,
        to: &Path,
        id: u32,
        crc: u32,
    ) -> Result<BagOutcome, String> {
        self.store()
            .move_prop(from, to, PropKey::new(id as i32, crc))
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Copy a prop from any readable collection into My Bag.
    ///
    /// The source may be a shelf, which is never modified.
    pub fn duplicate_prop(
        &self,
        from: &Path,
        id: u32,
        crc: u32,
        name: Option<&str>,
    ) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .duplicate_prop(from, PropKey::new(id as i32, crc), &target, name)
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Set (or clear) one My Bag record's name.
    pub fn rename_prop(&self, id: u32, crc: u32, name: Option<&str>) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .rename_prop(&target, PropKey::new(id as i32, crc), name)
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Create My Bag as a valid empty `.prp`.
    pub fn create_my_bag(&self) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .create_collection(&target)
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Delete My Bag.
    pub fn delete_my_bag(&self) -> Result<BagOutcome, String> {
        let target = self.my_bag_path()?;
        self.store()
            .delete_collection(&target)
            .map(BagOutcome::from)
            .map_err(|error| error.to_string())
    }

    /// Add or remove one `(id, crc)` in the `.prp` `Fave` record.
    pub fn set_favourite(
        &self,
        id: u32,
        crc: u32,
        favourite: bool,
    ) -> Result<FavouriteResult, String> {
        let key = PropKey::new(id as i32, crc);
        let (path, mut roster) = self.ensure_bag_roster()?;
        let mut favourites = Favorites::read(&roster);
        let changed = if favourite {
            favourites.add(key)
        } else {
            favourites.remove(key)
        };
        if !changed {
            return Ok(FavouriteResult {
                favourite: favourites.contains(key),
                changed: false,
            });
        }
        favourites
            .apply_to(&mut roster)
            .map_err(|e| e.to_string())?;
        self.write_roster(&path, &roster)?;
        Ok(FavouriteResult {
            favourite: favourites.contains(key),
            changed: true,
        })
    }

    /// Move one My Bag prop to the trash document.
    ///
    /// The trash is written first, then the roster; a missing prop is a no-op.
    pub fn trash_prop(&self, id: u32, crc: u32) -> Result<BagOutcome, String> {
        let key = PropKey::new(id as i32, crc);
        let (path, mut roster) = self.ensure_bag_roster()?;
        if roster.record_for(key).is_none() {
            return Ok(BagOutcome::NotFound);
        }
        let mut trash = TrashStore::open(self.context.clone());
        if !trash.trash(&mut roster, key).map_err(|e| e.to_string())? {
            return Ok(BagOutcome::NotFound);
        }
        self.write_roster(&path, &roster)?;
        Ok(BagOutcome::Removed)
    }

    /// The trashed props, oldest first.
    #[must_use]
    pub fn trash_list(&self) -> Vec<BagTrashEntry> {
        TrashStore::open(self.context.clone())
            .entries()
            .iter()
            .map(|entry| BagTrashEntry {
                id: entry.key.id as u32,
                crc: entry.key.crc,
                name: entry.name.clone(),
                bytes: entry.blob.len(),
            })
            .collect()
    }

    /// Restore one trashed prop into My Bag.
    pub fn trash_restore(&self, id: u32, crc: u32) -> Result<BagOutcome, String> {
        let key = PropKey::new(id as i32, crc);
        let (path, mut roster) = self.ensure_bag_roster()?;
        let mut trash = TrashStore::open(self.context.clone());
        trash
            .restore_from_trash(&mut roster, key)
            .map_err(|e| e.to_string())?;
        self.write_roster(&path, &roster)?;
        Ok(BagOutcome::Added)
    }

    /// Discard every trashed prop.
    pub fn trash_purge(&self) -> Result<PurgeResult, String> {
        let mut trash = TrashStore::open(self.context.clone());
        let purged = trash.len();
        trash.purge_trash().map_err(|e| e.to_string())?;
        Ok(PurgeResult { purged })
    }

    /// Every stored outfit, in file order.
    #[must_use]
    pub fn outfits(&self) -> Vec<BagOutfit> {
        OutfitStore::open(self.context.clone())
            .outfits()
            .iter()
            .map(bag_outfit)
            .collect()
    }

    /// Record the currently worn keys as the outfit `name`.
    pub fn outfit_save(&self, name: &str, props: &[BagKey]) -> Result<Vec<BagOutfit>, String> {
        let mut store = OutfitStore::open(self.context.clone());
        let keys: Vec<PropKey> = props.iter().map(|key| key.to_prop_key()).collect();
        outfit_ops::save_current_outfit(&mut store, name, &keys).map_err(|e| e.to_string())?;
        Ok(store.outfits().iter().map(bag_outfit).collect())
    }

    /// Split the outfit `name` into what the bag can wear and what it cannot.
    ///
    /// The available set is every key the bag catalog lists, so a reference to
    /// a prop the user does not own is reported as missing rather than failing
    /// the whole apply.
    pub fn outfit_apply(
        &self,
        name: &str,
        snapshot: &BagSnapshot,
    ) -> Result<BagOutfitApplication, String> {
        let store = OutfitStore::open(self.context.clone());
        let available: Vec<PropKey> = snapshot
            .entries()
            .iter()
            .map(|entry| PropKey::new(entry.id as i32, entry.crc))
            .collect();
        let application =
            outfit_ops::apply_outfit(&store, name, &available).map_err(|e| e.to_string())?;
        Ok(BagOutfitApplication {
            worn: application
                .worn
                .iter()
                .copied()
                .map(BagKey::from_prop_key)
                .collect(),
            missing: application
                .missing
                .iter()
                .copied()
                .map(BagKey::from_prop_key)
                .collect(),
        })
    }

    /// Rename an outfit.
    pub fn outfit_rename(&self, from: &str, to: &str) -> Result<Vec<BagOutfit>, String> {
        let mut store = OutfitStore::open(self.context.clone());
        outfit_ops::rename_outfit(&mut store, from, to).map_err(|e| e.to_string())?;
        Ok(store.outfits().iter().map(bag_outfit).collect())
    }

    /// Delete an outfit.
    pub fn outfit_delete(&self, name: &str) -> Result<Vec<BagOutfit>, String> {
        let mut store = OutfitStore::open(self.context.clone());
        outfit_ops::delete_outfit(&mut store, name).map_err(|e| e.to_string())?;
        Ok(store.outfits().iter().map(bag_outfit).collect())
    }

    /// Duplicate an outfit under a new name.
    pub fn outfit_duplicate(&self, source: &str, target: &str) -> Result<Vec<BagOutfit>, String> {
        let mut store = OutfitStore::open(self.context.clone());
        outfit_ops::duplicate_outfit(&mut store, source, target).map_err(|e| e.to_string())?;
        Ok(store.outfits().iter().map(bag_outfit).collect())
    }

    /// Gather a prop's bytes into My Bag, byte-preserving.
    pub fn gather(
        &self,
        id: u32,
        crc: u32,
        blob: &[u8],
        name: Option<&str>,
    ) -> Result<GatherResult, String> {
        match gather::gather(&self.store(), id as i32, crc, blob, name)
            .map_err(|e| e.to_string())?
        {
            GatherOutcome::Added => Ok(GatherResult::Added),
            GatherOutcome::AlreadyPresent => Ok(GatherResult::AlreadyPresent),
            GatherOutcome::Rejected(reason) => Ok(GatherResult::Rejected { reason }),
        }
    }

    /// Read My Bag, creating a valid empty collection first if it is missing.
    fn ensure_bag_roster(&self) -> Result<(PathBuf, Roster), String> {
        let path = self.my_bag_path()?;
        if !path.is_file() {
            self.store()
                .create_collection(&path)
                .map_err(|error| error.to_string())?;
        }
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let roster = Roster::parse(&bytes).map_err(|error| error.to_string())?;
        Ok((path, roster))
    }

    /// Serialise a roster and atomically replace My Bag.
    fn write_roster(&self, path: &Path, roster: &Roster) -> Result<(), String> {
        let bytes = roster.write().map_err(|error| error.to_string())?;
        self.context
            .atomic_write(path, &bytes)
            .map_err(|error| error.to_string())
    }
}

/// One shelf's health, mapped to the boundary shape.
fn shelf_info(shelf: &Shelf) -> BagShelfInfo {
    BagShelfInfo {
        name: shelf.name.clone(),
        path: shelf.path.display().to_string(),
        status: status_name(shelf.health.status).to_string(),
        records: shelf.health.records,
        named: shelf.health.named,
        undecodable: shelf.health.undecodable,
        dropped: shelf.health.dropped,
        error: shelf.error.clone(),
    }
}

/// The wire name for a shelf status.
fn status_name(status: ShelfStatus) -> &'static str {
    match status {
        ShelfStatus::Healthy => "healthy",
        ShelfStatus::Degraded => "degraded",
        ShelfStatus::Unreadable => "unreadable",
    }
}

/// Map a crate outfit to the boundary shape.
fn bag_outfit(outfit: &Outfit) -> BagOutfit {
    BagOutfit {
        name: outfit.name.clone(),
        props: outfit
            .props
            .iter()
            .copied()
            .map(BagKey::from_prop_key)
            .collect(),
    }
}

/// Read and parse a `.prp` file, or `None` when it is absent or unreadable.
fn read_roster(path: &Path) -> Option<Roster> {
    let bytes = fs::read(path).ok()?;
    Roster::parse(&bytes).ok()
}

/// Collect one roster's favourite keys and its `Fave` record's own key.
///
/// [`Favorites::read`] resolves both a real `Fave`-typed record and one a
/// rewrite filed under `Prop`, so a favourite set written by this app is read
/// back correctly. The record's own key is remembered so the listing can
/// exclude it.
fn collect_favourites(
    roster: &Roster,
    favourites: &mut HashSet<(u32, u32)>,
    fave_keys: &mut HashSet<(u32, u32)>,
) {
    for key in Favorites::read(roster).iter() {
        favourites.insert((key.id as u32, key.crc));
    }
    if let Some(record) = fave_record(roster) {
        let key = record.key();
        fave_keys.insert((key.id as u32, key.crc));
    }
}

/// The directory thumbnails are cached in, or `None` when no cache root is
/// known.
///
/// Deliberately **outside** the bag root: the bag root may be any path the user
/// configured (even one the write guard refuses), and a thumbnail is a cache
/// artefact, so it belongs in the client's own cache tree.
fn bag_thumb_cache_dir() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("PALACE_PROP_THUMB_CACHE") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| Some(std::env::temp_dir()))
        .map(|base| base.join("org.palace.client").join("bag-thumbs"))
}

/// Remove one thumbnail cache file. A file already gone is success, so a retry
/// after a concurrent rebuild is not an error.
fn remove_thumb_file(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("could not remove {}: {error}", path.display())),
    }
}

/// Remove every `*.png` from a thumbnail cache directory.
///
/// Only PNGs are touched, so an unrelated file left in the directory survives.
/// A missing directory is zero removals, not an error.
fn clear_thumb_cache_dir(dir: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("could not read {}: {error}", dir.display())),
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
            continue;
        }
        if remove_thumb_file(&path)? {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_prop::prp::{AssetRec, PropHeader, PropRecord};
    use palace_prop::{asset_crc, decode, encode_s20_blob, PropImage};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("palace-app-bag-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn service(root: &Path) -> BagService {
        BagService::open(BagContext {
            bag_root: Some(root.to_path_buf()),
            home: Some(root.to_path_buf()),
        })
    }

    fn prop_blob() -> (Vec<u8>, u32) {
        let mut blob = vec![4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        blob.extend_from_slice(&[0x04, 1, 1, 1, 1]);
        let crc = palace_prop::asset_crc(&blob[12..]);
        (blob, crc)
    }

    fn empty_roster() -> Roster {
        let bytes = fs::read(fixture("empty.prp")).expect("empty.prp is checked in");
        Roster::parse(&bytes).expect("empty.prp parses")
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/palace-prop/fixtures/prp")
            .join(name)
    }

    fn record(id: i32, blob: &[u8], crc: u32, name: Option<&str>) -> PropRecord {
        let header = PropHeader::parse(blob).ok();
        let encoding = header.map(PropHeader::encoding);
        PropRecord {
            rec: AssetRec {
                id,
                r_handle: 0,
                data_offset: 0,
                data_size: blob.len() as u32,
                last_use_time: 0,
                name_offset: -1,
                flags: 0,
                crc,
            },
            header,
            encoding,
            blob: blob.to_vec(),
            name: name.map(str::to_string),
        }
    }

    fn decodable_blob(seed: u8) -> (Vec<u8>, u32) {
        let rgba = vec![seed, 0, 255 - seed, 255, 0, seed, 0, 255];
        let image = PropImage::from_rgba(2, 1, rgba).expect("build a 2x1 image");
        let blob = encode_s20_blob(&image, 0, 0, 0).expect("encode an S20 prop");
        let crc = asset_crc(&blob[12..]);
        (blob, crc)
    }

    fn direct_png(blob: &[u8]) -> Vec<u8> {
        decode(blob)
            .expect("decode the source blob")
            .image
            .to_png_bytes()
            .expect("encode the direct PNG")
    }

    #[test]
    fn adding_then_listing_reports_added_and_already_present() {
        let dir = TempDir::new("add");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();

        assert_eq!(
            bag.add_prop(7, crc, &blob, Some("Dot")).expect("add"),
            BagOutcome::Added
        );
        assert_eq!(
            bag.add_prop(7, crc, &blob, None).expect("add again"),
            BagOutcome::AlreadyPresent
        );

        let snapshot = bag.snapshot();
        let entries = snapshot.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].id, entries[0].crc), (7, crc));
        assert_eq!(entries[0].name.as_deref(), Some("Dot"));
        assert_eq!(entries[0].collection, "My Bag");
        assert_eq!(entries[0].source, "bag");
    }

    #[test]
    fn removing_reports_removed_then_not_found() {
        let dir = TempDir::new("remove");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, None).expect("add");

        assert_eq!(
            bag.remove_prop(7, crc).expect("remove"),
            BagOutcome::Removed
        );
        assert_eq!(
            bag.remove_prop(7, crc).expect("remove again"),
            BagOutcome::NotFound
        );
        assert!(bag.snapshot().catalog.is_empty());
    }

    #[test]
    fn favouriting_round_trips_through_the_fave_record() {
        let dir = TempDir::new("fave");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, None).expect("add");

        assert_eq!(
            bag.set_favourite(7, crc, true).expect("favourite"),
            FavouriteResult {
                favourite: true,
                changed: true
            }
        );
        let snapshot = bag.snapshot();
        assert!(snapshot.entries()[0].fav);

        assert_eq!(
            bag.set_favourite(7, crc, false).expect("unfavourite"),
            FavouriteResult {
                favourite: false,
                changed: true
            }
        );
        let snapshot = bag.snapshot();
        assert!(!snapshot.entries()[0].fav);
    }

    #[test]
    fn the_fave_record_is_never_listed_as_a_prop() {
        let dir = TempDir::new("fave-hidden");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, None).expect("add");

        bag.set_favourite(7, crc, true).expect("favourite");
        bag.set_favourite(9, 0x1234_5678, true)
            .expect("second favourite");

        let snapshot = bag.snapshot();
        let entries = snapshot.entries();
        assert_eq!(
            entries.len(),
            1,
            "the Fave bookkeeping record must not be listed as a prop"
        );
        assert_eq!(entries[0].id, 7);
        assert!(entries[0].fav);
    }

    #[test]
    fn trash_restore_returns_the_prop_to_the_bag() {
        let dir = TempDir::new("trash");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, None).expect("add");

        assert_eq!(bag.trash_prop(7, crc).expect("trash"), BagOutcome::Removed);
        assert!(bag.snapshot().catalog.is_empty());
        assert_eq!(bag.trash_list().len(), 1);

        assert_eq!(
            bag.trash_restore(7, crc).expect("restore"),
            BagOutcome::Added
        );
        assert_eq!(bag.snapshot().catalog.len(), 1);
        assert!(bag.trash_list().is_empty());
    }

    #[test]
    fn an_outfit_saves_applies_and_reports_missing_references() {
        let dir = TempDir::new("outfit");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        bag.add_prop(7, crc, &blob, None).expect("add");

        let present = BagKey { id: 7, crc };
        let absent = BagKey { id: 9, crc: 1 };
        bag.outfit_save("Look", &[present, absent])
            .expect("save outfit");
        assert!(bag.outfits().iter().any(|outfit| outfit.name == "Look"));

        let snapshot = bag.snapshot();
        let application = bag.outfit_apply("Look", &snapshot).expect("apply outfit");
        assert_eq!(application.worn, vec![present]);
        assert_eq!(application.missing, vec![absent]);
    }

    #[test]
    fn a_shelf_is_listed_read_only_and_duplicates_into_my_bag() {
        let dir = TempDir::new("shelf");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();
        let shelf_dir = dir.path().join("shelves");
        fs::create_dir_all(&shelf_dir).expect("shelves dir");
        let shelf_path = shelf_dir.join("Old Gems.prp");
        let mut roster = empty_roster();
        roster.add_prop(record(9, &blob, crc, Some("Shelf Dot")));
        fs::write(&shelf_path, roster.write().expect("shelf writes")).expect("write shelf");
        let before = fs::read(&shelf_path).expect("read shelf");

        let collections = bag.collections().expect("collections");
        assert!(collections
            .iter()
            .any(|collection| collection.name == "My Bag" && collection.writable));
        assert!(collections
            .iter()
            .any(|collection| collection.name == "Old Gems" && !collection.writable));

        let snapshot = bag.snapshot();
        assert_eq!(snapshot.shelves.len(), 1);
        assert_eq!(snapshot.shelves[0].status, "healthy");
        assert!(snapshot
            .entries()
            .iter()
            .any(|entry| entry.id == 9 && entry.collection == "Old Gems"));

        assert_eq!(
            bag.duplicate_prop(&shelf_path, 9, crc, None)
                .expect("duplicate"),
            BagOutcome::Duplicated
        );
        assert_eq!(
            fs::read(&shelf_path).expect("read shelf again"),
            before,
            "a shelf must never be modified"
        );
        assert_eq!(bag.snapshot().catalog.len(), 1);
    }

    #[test]
    fn gather_from_intake_bytes_adds_and_is_idempotent() {
        let dir = TempDir::new("gather");
        let bag = service(dir.path());
        let (blob, crc) = prop_blob();

        assert_eq!(
            bag.gather(7, crc, &blob, None).expect("gather"),
            GatherResult::Added
        );
        assert_eq!(
            bag.gather(7, crc, &blob, None).expect("gather again"),
            GatherResult::AlreadyPresent
        );
        let rejected = bag
            .gather(7, crc ^ 0xffff_ffff, &blob, None)
            .expect("mismatched crc is a rejection, not an error");
        assert!(matches!(rejected, GatherResult::Rejected { .. }));
        assert_eq!(bag.snapshot().catalog.len(), 1);
    }

    #[test]
    fn an_absent_bag_is_an_empty_snapshot_not_a_failure() {
        let dir = TempDir::new("absent");
        let bag = service(&dir.path().join("not-created"));

        let snapshot = bag.snapshot();
        assert!(snapshot.catalog.is_empty());
        assert!(snapshot.shelves.is_empty());

        let collections = bag.collections().expect("a configured root still lists");
        assert_eq!(collections.len(), 1);
        assert!(!collections[0].exists);
    }

    #[test]
    fn rebuilding_one_thumbnail_drops_the_cache_and_regenerates_it() {
        let dir = TempDir::new("rebuild-one");
        let cache = dir.path().join("thumbs");
        fs::create_dir_all(&cache).expect("create the thumb cache dir");
        let (blob, crc) = decodable_blob(0x11);
        let bag = service(dir.path());
        bag.add_prop(7, crc, &blob, None).expect("add");

        let snapshot = bag.snapshot_with_cache_dir(Some(&cache));
        let first = snapshot
            .catalog
            .thumbnail_png(7, crc)
            .expect("a decodable prop has a thumbnail");
        let cache_file = cache.join(thumbnail_cache_name(7, crc));
        assert_eq!(
            fs::read(&cache_file).expect("the cache file was written"),
            first
        );

        // A marker stands in for a stale thumbnail: a rebuild that did not drop
        // the cache would serve it back instead of decoding the blob.
        fs::write(&cache_file, b"stale-marker").expect("plant a stale marker");
        assert_eq!(
            snapshot.catalog.thumbnail_png(7, crc).as_deref(),
            Some(&b"stale-marker"[..]),
            "the planted marker is served from cache before the rebuild"
        );

        let rebuilt = bag
            .rebuild_thumbnail_with(Some(&cache), 7, crc)
            .expect("rebuild")
            .expect("a decodable prop regenerates");
        assert_eq!(
            rebuilt,
            direct_png(&blob),
            "the regenerated PNG matches a direct decode"
        );
        assert_eq!(
            fs::read(&cache_file).expect("the cache file was rewritten"),
            rebuilt,
            "the cache file holds the regenerated PNG"
        );
        assert!(
            bag.rebuild_thumbnail_with(Some(&cache), 999, 1)
                .expect("an unknown identity is not an error")
                .is_none(),
            "an identity the bag does not hold has no thumbnail"
        );
    }

    #[test]
    fn rebuilding_all_clears_the_cache_and_regenerates_on_demand() {
        let dir = TempDir::new("rebuild-all");
        let cache = dir.path().join("thumbs");
        fs::create_dir_all(&cache).expect("create the thumb cache dir");
        let (blob_a, crc_a) = decodable_blob(0x11);
        let (blob_b, crc_b) = decodable_blob(0x22);
        let bag = service(dir.path());
        bag.add_prop(7, crc_a, &blob_a, None).expect("add a");
        bag.add_prop(9, crc_b, &blob_b, None).expect("add b");

        let snapshot = bag.snapshot_with_cache_dir(Some(&cache));
        let png_a = snapshot.catalog.thumbnail_png(7, crc_a).expect("a");
        let png_b = snapshot.catalog.thumbnail_png(9, crc_b).expect("b");
        assert_eq!(png_a, direct_png(&blob_a));
        assert_eq!(png_b, direct_png(&blob_b));
        let file_a = cache.join(thumbnail_cache_name(7, crc_a));
        let file_b = cache.join(thumbnail_cache_name(9, crc_b));
        assert!(file_a.exists() && file_b.exists());

        let removed = clear_thumb_cache_dir(&cache).expect("clear the cache");
        assert_eq!(removed, 2, "both cache files are dropped");
        assert!(
            !file_a.exists() && !file_b.exists(),
            "nothing corrupt or half-written is left behind"
        );

        // A fresh snapshot regenerates lazily and rewrites its cache file.
        let after = bag.snapshot_with_cache_dir(Some(&cache));
        let again_a = after
            .catalog
            .thumbnail_png(7, crc_a)
            .expect("regenerated on demand");
        assert_eq!(again_a, direct_png(&blob_a), "regenerated pixel-identical");
        assert_eq!(
            fs::read(&file_a).expect("the cache file was rewritten"),
            again_a
        );
    }
}

//! Bag store operations: the one writer allowed to change a bag collection.
//!
//! The bag is a folder with **one writable collection** ("My Bag") and any
//! number of read-only shelves (`.prp` files under `shelves/`). Layout, path
//! roles and the atomic write live in [`crate::bag_folder`]; this module is the
//! store built on top of them. It answers the operations the client's bag UI
//! needs:
//!
//! | Operation | Meaning |
//! |---|---|
//! | [`BagStore::list_collections`] | My Bag plus every shelf, with its role |
//! | [`BagStore::add_prop`] | add a prop (id, crc, blob, name) to My Bag |
//! | [`BagStore::remove_prop`] | remove one `(id, crc)` from My Bag |
//! | [`BagStore::move_prop`] | move one prop between writable collections |
//! | [`BagStore::duplicate_prop`] | copy a prop (from any collection) into My Bag |
//! | [`BagStore::rename_prop`] | set or clear one record's name |
//! | [`BagStore::create_collection`] | create My Bag (an empty valid `.prp`) |
//! | [`BagStore::delete_collection`] | delete My Bag |
//!
//! Every mutation is a **parse → mutate → write** through [`Roster`]: the file
//! is read, the typed model is changed with [`Roster::add_prop`] /
//! [`Roster::remove_prop`], and the serialised result replaces the file through
//! [`BagContext::atomic_write`] (write temp, `fsync`, rename). A refused or
//! failed write leaves the original bytes exactly as they were.
//!
//! # Single-writer safety
//!
//! Two writers must never interleave a read-modify-write of the same file, or
//! the later writer overwrites the earlier one's entry ("lost update"). The
//! store enforces one writer at a time in two layers:
//!
//! 1. **A process-wide mutex** ([`PROCESS_LOCK`]) so two threads of one process
//!    cannot race each other. Every mutation holds it for the whole
//!    read-modify-write and nothing else.
//! 2. **A lock file beside the target** (`My Bag.prp.lock`), created with
//!    [`std::fs::OpenOptions::create_new`]. A second process finds the file
//!    already there, waits a bounded time for the first to finish, and then
//!    fails with a clear error naming the lock if it never frees. The lock file
//!    is removed when its guard is dropped, including on an error or a panic.
//!
//! Neither lock is ever held while doing unrelated work: they wrap the parse,
//! the mutation and the atomic write, then release.
//!
//! # Role enforcement
//!
//! Only My Bag is a writable collection. A shelf, an unowned path, PalaceChat
//! data or anything else the [`BagRole`] guard refuses is rejected *before* the
//! file is touched, with a message naming why — so a shelf write is impossible
//! and the shelf's bytes stay identical. `Outfits.prp` is writable to the bag
//! *folder* but is not a prop collection, so the store refuses it too.
//!
//! # Duplicate-add policy
//!
//! A prop is identified by `(id, crc)`, never by id alone: several records may
//! share an id and differ in crc (a variant). Adding a key that already exists
//! in the destination is a **no-op** reported as
//! [`WriteOutcome::AlreadyPresent`] — the file is not rewritten, so it stays
//! byte-identical. Adding the same id with a different crc is a legal variant
//! and is added.
//!
//! Because My Bag is the only writable collection, [`BagStore::move_prop`]
//! between two different collections cannot arise under the current rule: it
//! returns [`WriteOutcome::Unchanged`] for the same file, and it refuses a shelf
//! as either end. Moving a prop *out of* a shelf is impossible by design — a
//! shelf is read-only; [`BagStore::duplicate_prop`] copies one into My Bag
//! instead.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::bag_folder::{my_bag_path, shelves_dir, BagContext, BagRole, BAG_FILE_NAME};
use crate::crc::asset_crc;
use crate::error::{PropError, Result};
use crate::prp::{AssetRec, PropHeader, PropKey, PropRecord, Roster, PROP_HEADER_LEN};

/// How many times a held lock file is re-tried before the write fails.
const LOCK_ATTEMPTS: u32 = 400;

/// Delay between lock-file attempts (400 × 5 ms ≈ 2 s before giving up).
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(5);

/// Suffix appended to the target's file name to name its lock file.
const LOCK_SUFFIX: &str = ".lock";

/// A valid, empty `.prp`: the 16-byte file header, the 24-byte map header and
/// one `Prop` type record with zero assets.
///
/// Parsing this is how the store materialises a brand-new collection; the bytes
/// are the literal content of `fixtures/prp/empty.prp` and a parse→write
/// round-trip of them is byte-identical.
const EMPTY_ROSTER: [u8; 52] = [
    0x10, 0x00, 0x00, 0x00, // data_offset = 16
    0x00, 0x00, 0x00, 0x00, // data_size = 0
    0x10, 0x00, 0x00, 0x00, // asset_map_offset = 16
    0x24, 0x00, 0x00, 0x00, // asset_map_size = 36
    0x01, 0x00, 0x00, 0x00, // nbr_types = 1
    0x00, 0x00, 0x00, 0x00, // nbr_assets = 0
    0x00, 0x00, 0x00, 0x00, // len_names = 0
    0x18, 0x00, 0x00, 0x00, // types_offset = 24
    0x24, 0x00, 0x00, 0x00, // recs_offset = 36
    0x24, 0x00, 0x00, 0x00, // names_offset = 36
    0x70, 0x6f, 0x72, 0x50, // "Prop" 4CC as stored little-endian
    0x00, 0x00, 0x00, 0x00, // nbr_assets = 0
    0x00, 0x00, 0x00, 0x00, // first_asset = 0
];

/// The process-wide writer mutex.
///
/// A single global lock (rather than one per file) is deliberate and cheap:
/// bag mutations are rare, short and always end in a disk write, so serialising
/// every writer in the process removes a whole class of races at no practical
/// cost. The file lock handles the cross-process case.
static PROCESS_LOCK: Mutex<()> = Mutex::new(());

/// One collection in the bag folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// The collection's file path (`<root>/My Bag.prp` or a `shelves/*.prp`).
    pub path: PathBuf,
    /// The display name: the file stem without the `.prp` extension
    /// (`My Bag`, `Old Gems`).
    pub name: String,
    /// The role the folder guard assigns this path.
    pub role: BagRole,
    /// Whether the file currently exists on disk.
    pub exists: bool,
}

impl Collection {
    /// Whether the store may mutate this collection. True only for My Bag.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.role == BagRole::MyBag
    }
}

/// What a store operation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// The prop was written.
    Added,
    /// The exact `(id, crc)` already existed; nothing was written.
    AlreadyPresent,
    /// The prop was removed.
    Removed,
    /// The record's name was changed.
    Renamed,
    /// The prop was copied into the destination.
    Duplicated,
    /// The prop was moved between collections.
    Moved,
    /// A collection was created.
    Created,
    /// A collection was deleted.
    Deleted,
    /// The request was valid but changed nothing (e.g. a same-file move).
    Unchanged,
    /// The addressed record or collection did not exist.
    NotFound,
    /// The collection already existed.
    AlreadyExists,
}

impl WriteOutcome {
    /// Whether the file changed on disk.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(
            self,
            WriteOutcome::Added
                | WriteOutcome::Removed
                | WriteOutcome::Renamed
                | WriteOutcome::Duplicated
                | WriteOutcome::Moved
                | WriteOutcome::Created
                | WriteOutcome::Deleted
        )
    }
}

/// The bag store: the bag folder's one writer.
///
/// Built over an explicit [`BagContext`] so tests and callers can point it at a
/// synthetic tree without touching the environment. `BagStore` itself holds no
/// state beyond the context; the locks are process-wide and per-file.
#[derive(Debug, Clone)]
pub struct BagStore {
    context: BagContext,
}

impl BagStore {
    /// Build a store over an explicit bag context.
    #[must_use]
    pub fn open(context: BagContext) -> Self {
        Self { context }
    }

    /// Build a store over the environment-discovered bag folder.
    #[must_use]
    pub fn discover() -> Self {
        Self::open(BagContext::discover())
    }

    /// The context (bag root plus home) this store writes through.
    #[must_use]
    pub fn context(&self) -> &BagContext {
        &self.context
    }

    /// Every collection the bag folder exposes: My Bag first, then the shelves.
    ///
    /// This reads directory entries only — no `.prp` is parsed, so listing a
    /// folder of huge shelves is cheap. A missing bag folder is an error; a
    /// missing My Bag or `shelves/` directory is simply an absent collection.
    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        let root = self.bag_root()?;
        let bag = my_bag_path(&root);
        let mut collections = vec![Collection {
            path: bag.clone(),
            name: display_name(&bag),
            role: BagRole::MyBag,
            exists: bag.is_file(),
        }];

        if let Ok(entries) = fs::read_dir(shelves_dir(&root)) {
            let mut shelves: Vec<Collection> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && has_prp_extension(path))
                .map(|path| Collection {
                    name: display_name(&path),
                    path,
                    role: BagRole::Shelf,
                    exists: true,
                })
                .collect();
            shelves.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.name.cmp(&b.name))
            });
            collections.extend(shelves);
        }
        Ok(collections)
    }

    /// Add a prop to a collection.
    ///
    /// `blob` is a complete `.prp` prop blob (12-byte header plus payload) and
    /// must be at least the header length; `crc` is its payload CRC, i.e.
    /// [`crate::crc::asset_crc`] over `blob[12..]`. Passing a `crc` that does
    /// not match the blob is refused rather than silently rewritten, because the
    /// stored record's identity must match its bytes.
    ///
    /// Adding an `(id, crc)` already present is a no-op reported as
    /// [`WriteOutcome::AlreadyPresent`] and leaves the file byte-identical.
    /// Adding the same id with a different crc creates a legal variant.
    pub fn add_prop(
        &self,
        collection: &Path,
        id: i32,
        crc: u32,
        blob: &[u8],
        name: Option<&str>,
    ) -> Result<WriteOutcome> {
        self.writable_collection(collection)?;
        if blob.len() < PROP_HEADER_LEN {
            return Err(PropError::BagIo {
                detail: format!(
                    "{}: a prop blob needs at least {PROP_HEADER_LEN} bytes of header, got {}",
                    collection.display(),
                    blob.len()
                ),
            });
        }
        let computed = asset_crc(&blob[PROP_HEADER_LEN..]);
        if computed != crc {
            return Err(PropError::BagIo {
                detail: format!(
                    "{}: crc {crc:#010x} does not match the blob's payload crc {computed:#010x}",
                    collection.display()
                ),
            });
        }
        let name = clean_name(name)?;
        self.ensure_layout()?;

        let key = PropKey::new(id, crc);
        self.with_writers(&[collection], || {
            let mut roster = self.load_or_empty(collection)?;
            if roster.record_for(key).is_some() {
                return Ok(WriteOutcome::AlreadyPresent);
            }
            roster.add_prop(build_record(id, crc, blob, name.as_deref())?);
            self.write_roster(collection, &roster)?;
            Ok(WriteOutcome::Added)
        })
    }

    /// Remove the record with the exact identity `key` from a collection.
    ///
    /// [`WriteOutcome::NotFound`] when it is not there; the file is untouched.
    pub fn remove_prop(&self, collection: &Path, key: PropKey) -> Result<WriteOutcome> {
        self.writable_collection(collection)?;
        self.ensure_layout()?;
        self.with_writers(&[collection], || {
            let mut roster = self.load_or_empty(collection)?;
            if roster.remove_prop(key).is_none() {
                return Ok(WriteOutcome::NotFound);
            }
            self.write_roster(collection, &roster)?;
            Ok(WriteOutcome::Removed)
        })
    }

    /// Move a prop from one writable collection to another.
    ///
    /// Both ends must be writable collections; a shelf is refused because a move
    /// would have to modify it. With My Bag as the only writable collection the
    /// two ends name the same file and the call reports
    /// [`WriteOutcome::Unchanged`] without touching the disk. If the
    /// destination already holds `key`, nothing is removed from the source and
    /// [`WriteOutcome::AlreadyPresent`] is returned, so a move can never drop an
    /// entry.
    pub fn move_prop(&self, from: &Path, to: &Path, key: PropKey) -> Result<WriteOutcome> {
        self.writable_collection(from)?;
        self.writable_collection(to)?;
        if same_target(from, to) {
            return Ok(WriteOutcome::Unchanged);
        }
        self.ensure_layout()?;
        self.with_writers(&[from, to], || {
            let mut source = self.load_or_empty(from)?;
            let Some(record) = source.remove_prop(key) else {
                return Ok(WriteOutcome::NotFound);
            };
            let mut destination = self.load_or_empty(to)?;
            if destination.record_for(key).is_some() {
                // Keep the source intact; a move must not lose the prop.
                return Ok(WriteOutcome::AlreadyPresent);
            }
            destination.add_prop(record);
            self.write_roster(to, &destination)?;
            self.write_roster(from, &source)?;
            Ok(WriteOutcome::Moved)
        })
    }

    /// Copy a prop from `from` into the writable collection `to`.
    ///
    /// The source may be any readable collection, including a shelf, so this is
    /// how a prop is taken off a shelf without modifying it. `name` replaces the
    /// copy's name when given; otherwise the source record's name is kept.
    ///
    /// If `to` already holds the same `(id, crc)`, the copy is a no-op reported
    /// as [`WriteOutcome::AlreadyPresent`].
    pub fn duplicate_prop(
        &self,
        from: &Path,
        key: PropKey,
        to: &Path,
        name: Option<&str>,
    ) -> Result<WriteOutcome> {
        self.writable_collection(to)?;
        let name = clean_name(name)?;
        if !from.is_file() {
            return Ok(WriteOutcome::NotFound);
        }
        let source = self.read_collection(from)?;
        let Some(record) = source.record_for(key).cloned() else {
            return Ok(WriteOutcome::NotFound);
        };
        self.ensure_layout()?;
        self.with_writers(&[to], || {
            let mut destination = self.load_or_empty(to)?;
            if destination.record_for(key).is_some() {
                return Ok(WriteOutcome::AlreadyPresent);
            }
            let mut copy = record;
            if name.is_some() {
                copy.name = name;
            }
            destination.add_prop(copy);
            self.write_roster(to, &destination)?;
            Ok(WriteOutcome::Duplicated)
        })
    }

    /// Set (or with `None`, clear) the name of the record `key` in a collection.
    ///
    /// Renaming to the name it already has reports [`WriteOutcome::Unchanged`]
    /// and writes nothing.
    pub fn rename_prop(
        &self,
        collection: &Path,
        key: PropKey,
        name: Option<&str>,
    ) -> Result<WriteOutcome> {
        self.writable_collection(collection)?;
        let name = clean_name(name)?;
        self.ensure_layout()?;
        self.with_writers(&[collection], || {
            let mut roster = self.load_or_empty(collection)?;
            let Some(existing) = roster.record_for(key) else {
                return Ok(WriteOutcome::NotFound);
            };
            if existing.name.as_deref() == name.as_deref() {
                return Ok(WriteOutcome::Unchanged);
            }
            let Some(mut record) = roster.remove_prop(key) else {
                return Ok(WriteOutcome::NotFound);
            };
            record.name = name;
            roster.add_prop(record);
            self.write_roster(collection, &roster)?;
            Ok(WriteOutcome::Renamed)
        })
    }

    /// Create a writable collection as an empty but valid `.prp`.
    ///
    /// Under the bag's rule the only writable collection is My Bag, so this
    /// initialises `My Bag.prp` (used on first run, or after a reset). A shelf
    /// or any other path is refused by the guard; new shelves are *added by
    /// path* (a later task), never created here. An existing file is reported as
    /// [`WriteOutcome::AlreadyExists`] and left untouched.
    pub fn create_collection(&self, collection: &Path) -> Result<WriteOutcome> {
        self.writable_collection(collection)?;
        self.ensure_layout()?;
        let created = self.with_writers(&[collection], || {
            if collection.is_file() {
                return Ok(false);
            }
            self.context.atomic_write(collection, &EMPTY_ROSTER)?;
            Ok(true)
        })?;
        Ok(if created {
            WriteOutcome::Created
        } else {
            WriteOutcome::AlreadyExists
        })
    }

    /// Delete a writable collection.
    ///
    /// As with [`BagStore::create_collection`], the only deletable collection is
    /// My Bag; shelves are never deleted through the store. A missing file
    /// reports [`WriteOutcome::NotFound`].
    pub fn delete_collection(&self, collection: &Path) -> Result<WriteOutcome> {
        self.writable_collection(collection)?;
        self.with_writers(&[collection], || {
            if !collection.exists() {
                return Ok(WriteOutcome::NotFound);
            }
            fs::remove_file(collection).map_err(|err| io_error(collection, err))?;
            Ok(WriteOutcome::Deleted)
        })
    }

    /// The configured bag root, or a "no bag folder" error.
    fn bag_root(&self) -> Result<PathBuf> {
        self.context.bag_root.clone().ok_or_else(no_bag_folder)
    }

    /// Create the bag root and `shelves/` if missing, before a lock is taken.
    fn ensure_layout(&self) -> Result<()> {
        self.context.ensure_layout().map(|_| ())
    }

    /// Refuse everything that is not the one writable prop collection.
    fn writable_collection(&self, path: &Path) -> Result<()> {
        let role = self.context.classify(path);
        if role != BagRole::MyBag {
            return Err(refusal(path, role));
        }
        if path.file_name().and_then(|name| name.to_str()) != Some(BAG_FILE_NAME) {
            return Err(PropError::BagIo {
                detail: format!(
                    "{}: only the \"{BAG_FILE_NAME}\" collection is writable by the bag store",
                    path.display()
                ),
            });
        }
        Ok(())
    }

    /// Read a collection, treating a missing file as the empty roster.
    fn load_or_empty(&self, path: &Path) -> Result<Roster> {
        match fs::read(path) {
            Ok(bytes) => Roster::parse(&bytes),
            Err(err) if err.kind() == ErrorKind::NotFound => Roster::parse(&EMPTY_ROSTER),
            Err(err) => Err(io_error(path, err)),
        }
    }

    /// Read a collection that must exist.
    fn read_collection(&self, path: &Path) -> Result<Roster> {
        let bytes = fs::read(path).map_err(|err| io_error(path, err))?;
        Roster::parse(&bytes)
    }

    /// Serialise a roster and atomically replace its file.
    fn write_roster(&self, path: &Path, roster: &Roster) -> Result<()> {
        let bytes = roster.write()?;
        self.context.atomic_write(path, &bytes)
    }

    /// Run `action` with the process mutex and one lock file per target held.
    ///
    /// Targets are deduplicated and locked in path order so two multi-file
    /// operations can never deadlock on each other. The locks — including the
    /// process mutex — are released when this function returns.
    fn with_writers<T>(&self, targets: &[&Path], action: impl FnOnce() -> Result<T>) -> Result<T> {
        let _process = PROCESS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut unique: Vec<PathBuf> = targets.iter().map(|path| (*path).to_path_buf()).collect();
        unique.sort();
        unique.dedup();
        let mut locks = Vec::with_capacity(unique.len());
        for path in &unique {
            locks.push(CollectionLock::acquire(path)?);
        }
        action()
    }
}

/// A held lock file; removes it on drop so a failed write cannot wedge the bag.
///
/// The lock *file* is the cross-process arbiter; the process mutex is the
/// in-process one. Creating it with `create_new` is the atomic test-and-set:
/// the OS guarantees exactly one creator even across processes.
#[derive(Debug)]
struct CollectionLock {
    path: PathBuf,
}

impl CollectionLock {
    /// Try to become the writer for `target`, waiting briefly for a peer.
    fn acquire(target: &Path) -> Result<Self> {
        let path = lock_path(target)?;
        for attempt in 0..LOCK_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_handle) => return Ok(Self { path }),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if attempt + 1 < LOCK_ATTEMPTS {
                        std::thread::sleep(LOCK_RETRY_DELAY);
                    }
                }
                Err(err) => return Err(io_error(&path, err)),
            }
        }
        Err(PropError::BagIo {
            detail: format!(
                "{}: another writer holds the lock file {}; retry when it finishes",
                target.display(),
                path.display()
            ),
        })
    }
}

impl Drop for CollectionLock {
    fn drop(&mut self) {
        // Best effort: a lock file that cannot be removed is a leftover, not a
        // reason to fail the operation that already succeeded.
        let _ = fs::remove_file(&self.path);
    }
}

/// `<target file name>.lock`, beside the target so it is same-directory.
fn lock_path(target: &Path) -> Result<PathBuf> {
    let name = target.file_name().ok_or_else(|| PropError::BagIo {
        detail: format!("{}: not a file path", target.display()),
    })?;
    let mut lock_name = name.to_os_string();
    lock_name.push(LOCK_SUFFIX);
    Ok(target.with_file_name(lock_name))
}

/// Build a [`PropRecord`] from the store's inputs.
fn build_record(id: i32, crc: u32, blob: &[u8], name: Option<&str>) -> Result<PropRecord> {
    if blob.len() < PROP_HEADER_LEN {
        return Err(PropError::BagIo {
            detail: format!(
                "a prop blob needs at least {PROP_HEADER_LEN} bytes of header, got {}",
                blob.len()
            ),
        });
    }
    let header = PropHeader::parse(blob).ok();
    let encoding = header.map(PropHeader::encoding);
    Ok(PropRecord {
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
    })
}

/// Trim a caller-supplied name; `None` stays `None`, an empty string is refused.
fn clean_name(name: Option<&str>) -> Result<Option<String>> {
    match name {
        None => Ok(None),
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                Err(PropError::BagIo {
                    detail: "bag store: a prop name must not be empty".to_string(),
                })
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
    }
}

/// Whether two collection paths address the same file.
fn same_target(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The display name of a collection: its file stem, without `.prp`.
fn display_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether a file name ends in `.prp`, ignoring case.
fn has_prp_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("prp"))
}

/// The error for a store with no bag folder configured.
fn no_bag_folder() -> PropError {
    PropError::BagIo {
        detail: "no bag folder configured: set PALACE_PROP_BAG_DIR or a home directory".to_string(),
    }
}

/// The error a refused write returns, naming the role that refused it.
fn refusal(path: &Path, role: BagRole) -> PropError {
    let detail = match role {
        BagRole::MyBag => format!("{}: not writable", path.display()),
        BagRole::Shelf => format!(
            "{}: shelves are read-only; duplicate the prop into \"{BAG_FILE_NAME}\" to change it",
            path.display()
        ),
        BagRole::Forbidden => format!(
            "{}: refusing to write; the path is outside the bag folder or inside protected \
             PalaceChat data",
            path.display()
        ),
    };
    PropError::BagIo { detail }
}

fn io_error(path: &Path, err: std::io::Error) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {err}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_roster_constant_is_a_valid_prp() {
        let roster = Roster::parse(&EMPTY_ROSTER).expect("the empty roster parses");
        assert!(roster.is_empty());
        assert_eq!(roster.types().len(), 1);
        assert_eq!(roster.write().expect("writes"), EMPTY_ROSTER);
    }

    #[test]
    fn lock_paths_sit_beside_their_target() {
        let target = Path::new("/bags/My Bag.prp");
        assert_eq!(
            lock_path(target).expect("a file name"),
            PathBuf::from("/bags/My Bag.prp.lock")
        );
    }
}

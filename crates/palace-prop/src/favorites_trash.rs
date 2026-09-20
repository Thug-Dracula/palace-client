//! Favourites (the `.prp` `Fave` record) and the trash store.
//!
//! Two stores that belong to the prop bag but do not belong *inside* the bag's
//! prop set:
//!
//! * **Favourites** ride in the roster's `Fave` record — the one record the
//!   format reserves for exactly this (`PRP-FORMAT.md` §2.3, §7).
//! * **Trash** is a small app-owned document beside the bag. A deleted prop is
//!   written there, blob and all, *before* it leaves the roster, so a delete is
//!   always reversible and a restore is lossless.
//!
//! # The `Fave` record
//!
//! One record, fixed id [`FAVE_RECORD_ID`] (`128`), name `""`, whose payload is
//! an array of 8-byte `(id, crc)` specs: `id` little-endian `i32`, then `crc`
//! little-endian `u32`. Order is the user's order and is preserved; a repeated
//! pair is stored once.
//!
//! The record's own `crc` field is **not** a payload CRC and is never validated.
//! The real collection shipped as `fixtures/prp/real_palace_hidden.prp` stores
//! `0x1f9c6739` next to an 8-byte payload that hashes to a different value, so
//! every read path decodes the payload and ignores the field.
//!
//! ## Where the record lives in the roster
//!
//! [`Favorites::read`] accepts both shapes a roster can hold:
//!
//! 1. a record inside a `Fave`-typed range — what real collections carry (the
//!    `Palace - Hidden.PRP` fixture has one, with a payload);
//! 2. otherwise any record with id `128` whose payload is a whole number of
//!    8-byte specs, so a roster written by an older build that misplaced the
//!    record under `Prop` still resolves.
//!
//! [`Favorites::apply_to`] removes the existing record and re-inserts it with
//! [`Roster::add_fave`], which targets the `Fave` type and creates that type
//! when it is absent. The record therefore stays inside the `Fave` run, so the
//! type table's `Prop` count keeps excluding it and `Fave.firstAsset` /
//! `nbrAssets` point at it. This is load-bearing, not cosmetic: the server's
//! `ValidateProps` walks only the `Prop` type (`PRP-FORMAT.md` §6), so an 8-byte
//! favourites payload sitting in the `Prop` run would be read as a headerless
//! prop and marked bad on every startup.
//!
//! When the payload is already what the caller wants, `apply_to` changes
//! nothing, so an untouched roster still writes byte for byte and the writer's
//! round-trip guarantee is not disturbed by a no-op favourites call.
//!
//! # The trash document
//!
//! Trash lives in the bag root beside `My Bag.prp` (never inside a roster, never
//! in a shelf) as [`TRASH_FILE_NAME`] (`Trash.store`). It is a small text
//! document, one entry per line, so a stuck delete can be inspected by eye:
//!
//! ```text
//! palace-prop-trash 1
//! 1000 3735928559 466f6f 0c000c00000000000000
//! ```
//!
//! Each entry line is `<id> <crc> <name-hex|-> <blob-hex|->`. The blob is stored
//! in full, which is what makes a restore byte-identical; the name is kept so a
//! restored prop does not lose its label. The store is **bounded**: at most
//! [`MAX_TRASH_ENTRIES`] entries and [`MAX_TRASH_BYTES`] of blob data, oldest
//! entries dropped first when a new delete pushes past a cap. Parsing never
//! panics and never repairs: a corrupt document loads as an empty store plus a
//! [`TrashStore::load_error`], the same policy as the outfits store.
//!
//! # Ordering and atomicity
//!
//! * **Delete** ([`TrashStore::trash`]) writes the entry to the trash document
//!   first and only then removes the record from the in-memory roster. A failed
//!   trash write leaves both the document and the roster alone.
//! * **Restore** ([`TrashStore::restore_from_trash`]) stages the roster change
//!   on a clone, writes the trash document without the entry, and only then
//!   commits the roster. A failed write changes nothing.
//! * Both stores replace their file atomically (temp file, `fsync`, rename). The
//!   bag's own `.prp` is written by the caller with [`BagContext::atomic_write`];
//!   the trash document cannot use that helper because the bag guard admits only
//!   `My Bag.prp` and `Outfits.prp`, so this module performs the identical
//!   sequence itself ([`atomic_replace`]).
//!
//! Nothing here reads or writes a shelf, PalaceChat data or `$MEDIA/`.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bag_folder::BagContext;
use crate::crc::asset_crc;
use crate::error::{PropError, Result};
use crate::prp::{AssetRec, AssetType, PropHeader, PropKey, PropRecord, Roster};

/// The `Fave` record's fixed id (`0x80`), the same value `rebuild_prp.py` and
/// the real collections use.
pub const FAVE_RECORD_ID: i32 = 128;

/// Bytes per favourite entry in the `Fave` payload: `i32` id + `u32` crc.
pub const FAVE_SPEC_LEN: usize = 8;

/// The trash document's file name in the bag root.
pub const TRASH_FILE_NAME: &str = "Trash.store";

/// The trash document's first token, so a wrong file is refused instead of
/// half-parsed.
pub const TRASH_MAGIC: &str = "palace-prop-trash";

/// The only trash document version this build reads and writes.
pub const TRASH_VERSION: u32 = 1;

/// Most entries the trash keeps before the oldest are dropped.
pub const MAX_TRASH_ENTRIES: usize = 1024;

/// Most blob bytes a single trashed prop may carry.
pub const MAX_TRASH_BLOB_BYTES: usize = 16 * 1024 * 1024;

/// Most blob bytes the whole trash may carry; the oldest entries are dropped
/// first when a delete pushes past this budget.
pub const MAX_TRASH_BYTES: usize = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Favourites
// ---------------------------------------------------------------------------

/// The user's favourite prop identities, in the user's order.
///
/// Built by [`Favorites::read`] (tolerant) or [`Favorites::decode_payload`]
/// (strict); changed with [`Favorites::add`] / [`Favorites::remove`]; stored
/// back into a roster with [`Favorites::apply_to`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Favorites {
    keys: Vec<PropKey>,
}

impl Favorites {
    /// An empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self { keys: Vec::new() }
    }

    /// The favourites, in stored order.
    #[must_use]
    pub fn keys(&self) -> &[PropKey] {
        &self.keys
    }

    /// The favourites, in stored order, as an iterator.
    pub fn iter(&self) -> impl Iterator<Item = PropKey> + '_ {
        self.keys.iter().copied()
    }

    /// How many favourites are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no favourite is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether `key` is a favourite.
    #[must_use]
    pub fn contains(&self, key: PropKey) -> bool {
        self.keys.contains(&key)
    }

    /// Append `key` unless it is already a favourite.
    ///
    /// Returns whether the list changed.
    pub fn add(&mut self, key: PropKey) -> bool {
        if self.contains(key) {
            return false;
        }
        self.keys.push(key);
        true
    }

    /// Remove `key` from the list.
    ///
    /// Returns whether the list changed.
    pub fn remove(&mut self, key: PropKey) -> bool {
        let before = self.keys.len();
        self.keys.retain(|stored| *stored != key);
        self.keys.len() != before
    }

    /// Remove every favourite.
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// Encode the list as the `Fave` payload: `(id, crc)` little-endian pairs.
    ///
    /// An empty list encodes to an empty payload, the shape the format's
    /// size-0 sentinel uses.
    #[must_use]
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.keys.len() * FAVE_SPEC_LEN);
        for key in &self.keys {
            out.extend_from_slice(&key.id.to_le_bytes());
            out.extend_from_slice(&key.crc.to_le_bytes());
        }
        out
    }

    /// Decode a `Fave` payload.
    ///
    /// Strict: a payload that is not a whole number of 8-byte specs is an error,
    /// and a repeated pair is stored once (first occurrence wins).
    pub fn decode_payload(payload: &[u8]) -> Result<Self> {
        if !payload.len().is_multiple_of(FAVE_SPEC_LEN) {
            return Err(PropError::BagIo {
                detail: format!(
                    "favourites: the Fave payload is {} byte(s); it must be a whole number of \
                     {FAVE_SPEC_LEN}-byte (id, crc) specs",
                    payload.len()
                ),
            });
        }
        let mut keys = Vec::with_capacity(payload.len() / FAVE_SPEC_LEN);
        for spec in payload.chunks_exact(FAVE_SPEC_LEN) {
            let id = i32::from_le_bytes([spec[0], spec[1], spec[2], spec[3]]);
            let crc = u32::from_le_bytes([spec[4], spec[5], spec[6], spec[7]]);
            let key = PropKey::new(id, crc);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        Ok(Self { keys })
    }

    /// Read the favourites out of a roster.
    ///
    /// Tolerant by design: a roster with no `Fave` record is an empty list, and
    /// a record whose payload is not a whole number of specs is treated as an
    /// empty list rather than failing the caller (use
    /// [`Favorites::decode_payload`] when the strict answer matters).
    #[must_use]
    pub fn read(roster: &Roster) -> Self {
        match fave_record(roster) {
            Some(record) => Self::decode_payload(&record.blob).unwrap_or_default(),
            None => Self::new(),
        }
    }

    /// Write this list into `roster`'s `Fave` record.
    ///
    /// The existing record is removed and re-inserted into the `Fave` type with
    /// [`Roster::add_fave`] (see the module docs). When the payload is already in
    /// place this is a no-op, so a no-op write stays byte-exact. A roster that
    /// never had a `Fave` record and has no favourites is left alone; otherwise
    /// exactly one record is kept — with an empty payload (the format's empty
    /// sentinel) when the list is empty.
    ///
    /// # Errors
    ///
    /// The roster mutation itself cannot fail; the `Result` belongs to the
    /// shared store signature.
    pub fn apply_to(&self, roster: &mut Roster) -> Result<()> {
        let payload = self.encode_payload();
        let existing = match fave_record(roster) {
            Some(record) if record.blob == payload => return Ok(()),
            Some(record) => Some(record.key()),
            None if payload.is_empty() => return Ok(()),
            None => None,
        };
        if let Some(key) = existing {
            roster.remove_prop(key);
        }
        roster.add_fave(fave_record_from_payload(payload));
        Ok(())
    }
}

/// The roster's `Fave` record, if it carries one.
///
/// A `Fave`-typed range wins, so real collections resolve by type; otherwise any
/// id-`128` record whose payload is a whole number of specs is accepted, so a
/// roster written by an older build that placed the record under `Prop` still
/// resolves.
#[must_use]
pub fn fave_record(roster: &Roster) -> Option<&PropRecord> {
    for asset_type in roster.types() {
        if asset_type.kind() != AssetType::Fave {
            continue;
        }
        let start = asset_type.first_asset.max(0) as usize;
        let count = asset_type.nbr_assets.max(0) as usize;
        let end = start.saturating_add(count).min(roster.records().len());
        let run = roster.records().get(start..end).unwrap_or(&[]);
        if let Some(record) = run.iter().find(|record| record.id() == FAVE_RECORD_ID) {
            return Some(record);
        }
    }
    roster.records().iter().find(|record| {
        record.id() == FAVE_RECORD_ID && record.blob.len().is_multiple_of(FAVE_SPEC_LEN)
    })
}

/// Build the `Fave` record for a payload.
///
/// The `crc` field is seeded with `0` and is not a payload CRC; the record
/// insert path rewrites it for payloads of 12 bytes or more, which is fine —
/// nothing ever validates it.
fn fave_record_from_payload(payload: Vec<u8>) -> PropRecord {
    let header = PropHeader::parse(&payload).ok();
    let encoding = header.map(PropHeader::encoding);
    let rec = AssetRec {
        id: FAVE_RECORD_ID,
        r_handle: 0,
        data_offset: 0,
        data_size: payload.len() as u32,
        last_use_time: 0,
        name_offset: -1,
        flags: 0,
        crc: 0,
    };
    PropRecord {
        rec,
        header,
        encoding,
        blob: payload,
        name: Some(String::new()),
    }
}

// ---------------------------------------------------------------------------
// Trash
// ---------------------------------------------------------------------------

/// One prop moved out of the bag to the trash: identity, label and raw blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashEntry {
    /// The prop's `(id, crc)` identity at the moment it was trashed.
    pub key: PropKey,
    /// The prop's name, when it had one.
    pub name: Option<String>,
    /// The prop's raw blob (`data_size` bytes, 12-byte header included).
    pub blob: Vec<u8>,
}

impl TrashEntry {
    /// Capture a roster record.
    fn from_record(record: &PropRecord) -> Self {
        Self {
            key: record.key(),
            name: record.name.clone(),
            blob: record.blob.clone(),
        }
    }

    /// Rebuild a roster record from this entry.
    fn to_record(&self) -> PropRecord {
        let blob = self.blob.clone();
        let header = PropHeader::parse(&blob).ok();
        let encoding = header.map(PropHeader::encoding);
        let rec = AssetRec {
            id: self.key.id,
            r_handle: 0,
            data_offset: 0,
            data_size: blob.len() as u32,
            last_use_time: 0,
            name_offset: -1,
            flags: 0,
            crc: self.key.crc,
        };
        PropRecord {
            rec,
            header,
            encoding,
            blob,
            name: self.name.clone(),
        }
    }
}

/// The trash document plus the entries it parsed.
///
/// Built by [`TrashStore::open`], which never fails: inspect
/// [`TrashStore::load_error`] to tell "no trash yet" from "unreadable trash".
#[derive(Debug, Clone)]
pub struct TrashStore {
    /// `None` when no bag folder is configured; every write then fails and the
    /// attempted load records an error.
    path: Option<PathBuf>,
    /// The entries, oldest first.
    entries: Vec<TrashEntry>,
    /// Why the last load produced an empty list, when it did.
    load_error: Option<PropError>,
}

impl TrashStore {
    /// Open the trash document of `context`'s bag folder and parse it.
    ///
    /// Never fails and never panics: a missing file is an empty store, an
    /// unreadable or corrupt one is an empty store plus a
    /// [`TrashStore::load_error`]. The file is only read, never touched.
    #[must_use]
    pub fn open(context: BagContext) -> Self {
        let path = context.bag_root.as_deref().map(trash_path);
        let mut store = Self {
            path,
            entries: Vec::new(),
            load_error: None,
        };
        store.reload();
        store
    }

    /// [`TrashStore::open`] over the environment-discovered bag folder.
    #[must_use]
    pub fn discover() -> Self {
        Self::open(BagContext::discover())
    }

    /// The document this store reads and writes; `None` when no bag folder is
    /// configured.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The trashed entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[TrashEntry] {
        &self.entries
    }

    /// The trashed identities, oldest first — the shape a catalog needs.
    #[must_use]
    pub fn keys(&self) -> Vec<PropKey> {
        self.entries.iter().map(|entry| entry.key).collect()
    }

    /// How many entries the trash holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the trash is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `key` is in the trash.
    #[must_use]
    pub fn contains(&self, key: PropKey) -> bool {
        self.entries.iter().any(|entry| entry.key == key)
    }

    /// Why the last load produced an empty list, when it did.
    #[must_use]
    pub fn load_error(&self) -> Option<&PropError> {
        self.load_error.as_ref()
    }

    /// Re-read and re-parse the document, replacing the in-memory entries.
    ///
    /// Same policy as [`TrashStore::open`]: never fails, never panics, never
    /// writes.
    pub fn reload(&mut self) {
        self.entries.clear();
        self.load_error = None;

        let Some(path) = self.path.clone() else {
            self.load_error = Some(no_bag_folder());
            return;
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            // No trash document yet is a normal first run, not an error.
            Err(err) if err.kind() == ErrorKind::NotFound => return,
            Err(err) => {
                self.load_error = Some(io_error(&path, err));
                return;
            }
        };
        if bytes.len() > MAX_TRASH_BYTES.saturating_mul(2).saturating_add(4096) {
            self.load_error = Some(bag_error(
                &path,
                format!(
                    "trash document is {} bytes; refusing to parse an oversized store",
                    bytes.len()
                ),
            ));
            return;
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(err) => {
                self.load_error = Some(bag_error(
                    &path,
                    format!("trash document is not valid UTF-8: {err}"),
                ));
                return;
            }
        };
        match parse_trash(text, &path) {
            Ok(entries) => self.entries = entries,
            Err(err) => self.load_error = Some(err),
        }
    }

    /// Move the bag prop `key` to the trash.
    ///
    /// The trash document is written first; only then is the record removed from
    /// the in-memory roster, which the caller persists with
    /// [`BagContext::atomic_write`] (or any atomic save) right after. A missing
    /// key is a no-op that returns `false`. The `Fave` record itself is not a
    /// prop and is refused: clear the list through [`Favorites`] instead.
    pub fn trash(&mut self, roster: &mut Roster, key: PropKey) -> Result<bool> {
        if fave_record(roster).is_some_and(|record| record.key() == key) {
            return Err(bag_error_path(
                self.path.as_deref(),
                "refusing to trash the Fave record; clear the favourite list instead",
            ));
        }
        let Some(record) = roster.record_for(key) else {
            return Ok(false);
        };
        let entry = TrashEntry::from_record(record);
        if entry.blob.len() > MAX_TRASH_BLOB_BYTES {
            return Err(bag_error_path(
                self.path.as_deref(),
                format!(
                    "refusing to trash id {}: its blob is {} bytes, above the {MAX_TRASH_BLOB_BYTES}-byte cap",
                    key.id,
                    entry.blob.len()
                ),
            ));
        }

        let mut candidate = self.entries.clone();
        match candidate.iter_mut().find(|stored| stored.key == key) {
            Some(stored) => *stored = entry,
            None => candidate.push(entry),
        }
        bound_entries(&mut candidate);
        self.write(&candidate)?;
        self.entries = candidate;
        roster.remove_prop(key);
        Ok(true)
    }

    /// Move the trashed prop `key` back into the bag.
    ///
    /// The roster change is staged on a clone, the trash document is written
    /// without the entry, and only then is the staged roster committed, so a
    /// failed write changes nothing. The returned key is the identity actually
    /// restored: for a well-formed prop it equals the trashed key, and for a
    /// record whose stored `crc` was stale the insert path recomputes it from
    /// the payload.
    ///
    /// # Errors
    ///
    /// [`PropError::BagIo`] when `key` is not in the trash, when no bag folder is
    /// configured, or when the document cannot be replaced.
    pub fn restore_from_trash(&mut self, roster: &mut Roster, key: PropKey) -> Result<PropKey> {
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return Err(bag_error_path(
                self.path.as_deref(),
                format!("id {} is not in the trash", key.id),
            ));
        };
        let entry = self.entries[index].clone();

        let mut candidate_roster = roster.clone();
        candidate_roster.add_prop(entry.to_record());
        let restored = restored_key(&entry);

        let mut candidate = self.entries.clone();
        candidate.remove(index);
        self.write(&candidate)?;
        self.entries = candidate;
        *roster = candidate_roster;
        Ok(restored)
    }

    /// Empty the trash: replace the document with a valid empty one.
    ///
    /// Works whether or not the document exists, so an empty trash is always a
    /// valid one; a failed write leaves the in-memory entries alone.
    pub fn purge_trash(&mut self) -> Result<()> {
        self.write(&[])?;
        self.entries.clear();
        Ok(())
    }

    /// Serialise `entries` and replace the document in one atomic step.
    fn write(&self, entries: &[TrashEntry]) -> Result<()> {
        let path = self.path.as_ref().ok_or_else(no_bag_folder)?;
        atomic_replace(path, render(entries).as_bytes())
    }
}

/// `<root>/Trash.store`, the app-owned trash document beside the bag.
#[must_use]
pub fn trash_path(root: &Path) -> PathBuf {
    root.join(TRASH_FILE_NAME)
}

/// The identity a trashed entry gets when it is inserted back into a roster.
///
/// Mirrors the insert path's normalisation: a blob of 12 bytes or more has its
/// CRC recomputed from the payload, a shorter one keeps the stored value.
fn restored_key(entry: &TrashEntry) -> PropKey {
    if entry.blob.len() >= crate::prp::PROP_HEADER_LEN {
        PropKey::new(
            entry.key.id,
            asset_crc(&entry.blob[crate::prp::PROP_HEADER_LEN..]),
        )
    } else {
        entry.key
    }
}

/// Drop oldest entries until the store fits its count and byte budgets.
fn bound_entries(entries: &mut Vec<TrashEntry>) {
    while entries.len() > MAX_TRASH_ENTRIES {
        entries.remove(0);
    }
    while entries.len() > 1 && total_blob_bytes(entries) > MAX_TRASH_BYTES {
        entries.remove(0);
    }
}

fn total_blob_bytes(entries: &[TrashEntry]) -> usize {
    entries
        .iter()
        .map(|entry| entry.blob.len())
        .fold(0usize, usize::saturating_add)
}

/// Serialise the trash document.
fn render(entries: &[TrashEntry]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{TRASH_MAGIC} {TRASH_VERSION}");
    for entry in entries {
        let name = hex_encode(entry.name.as_deref().unwrap_or("").as_bytes());
        let name = if name.is_empty() {
            "-".to_string()
        } else {
            name
        };
        let blob = hex_encode(&entry.blob);
        let blob = if blob.is_empty() {
            "-".to_string()
        } else {
            blob
        };
        let _ = writeln!(out, "{} {} {name} {blob}", entry.key.id, entry.key.crc);
    }
    out
}

/// Parse a trash document. Never panics; every failure names its line.
fn parse_trash(text: &str, path: &Path) -> Result<Vec<TrashEntry>> {
    let mut lines = text.lines();
    let header = lines
        .by_ref()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| bag_error(path, "trash document is empty"))?;
    let mut header_fields = header.split_whitespace();
    if header_fields.next() != Some(TRASH_MAGIC) {
        return Err(bag_error(
            path,
            "trash document does not start with the palace-prop-trash magic",
        ));
    }
    match header_fields.next().map(str::parse::<u32>) {
        Some(Ok(version)) if version == TRASH_VERSION => {}
        Some(Ok(version)) => {
            return Err(bag_error(
                path,
                format!("trash document version {version} is not {TRASH_VERSION}"),
            ));
        }
        _ => return Err(bag_error(path, "trash document has no version")),
    }
    if header_fields.next().is_some() {
        return Err(bag_error(path, "trash document header has trailing fields"));
    }

    let mut entries: Vec<TrashEntry> = Vec::new();
    for (number, line) in lines.enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line_number = number.saturating_add(2);
        if entries.len() >= MAX_TRASH_ENTRIES {
            return Err(bag_error(
                path,
                format!("trash document holds more than {MAX_TRASH_ENTRIES} entries"),
            ));
        }
        let mut fields = line.split_whitespace();
        let (Some(id), Some(crc), Some(name), Some(blob), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(bag_error(
                path,
                format!("line {line_number}: expected exactly 4 fields"),
            ));
        };
        let id: i32 = id
            .parse()
            .map_err(|_| bag_error(path, format!("line {line_number}: bad id {id:?}")))?;
        let crc: u32 = crc
            .parse()
            .map_err(|_| bag_error(path, format!("line {line_number}: bad crc {crc:?}")))?;
        let name_bytes = hex_decode(name)
            .ok_or_else(|| bag_error(path, format!("line {line_number}: bad name hex")))?;
        let name =
            if name_bytes.is_empty() {
                None
            } else {
                Some(String::from_utf8(name_bytes).map_err(|_| {
                    bag_error(path, format!("line {line_number}: name is not UTF-8"))
                })?)
            };
        let blob = hex_decode(blob)
            .ok_or_else(|| bag_error(path, format!("line {line_number}: bad blob hex")))?;
        if blob.len() > MAX_TRASH_BLOB_BYTES {
            return Err(bag_error(
                path,
                format!(
                    "line {line_number}: blob is {} bytes, above the cap",
                    blob.len()
                ),
            ));
        }
        entries.push(TrashEntry {
            key: PropKey::new(id, crc),
            name,
            blob,
        });
    }
    Ok(entries)
}

/// Lowercase hex, the encoding the document uses for names and blobs.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// Strict hex decode: `-` is the empty byte string; anything with an odd length
/// or a non-hex character is `None`.
fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text == "-" {
        return Some(Vec::new());
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Atomic document replacement
// ---------------------------------------------------------------------------

/// One temp-file sequence number per process, so concurrent writes never collide.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replace `path` atomically: unique temp file beside the target, write, flush,
/// rename, then a best-effort directory sync.
///
/// The same sequence [`BagContext::atomic_write`] performs, kept local because
/// the bag guard only admits `My Bag.prp` and `Outfits.prp`. On any failure the
/// temp file is removed and the original is left exactly as it was.
fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => return Err(bag_error(path, "not a file path")),
    };
    let Some(name) = path.file_name() else {
        return Err(bag_error(path, "not a file path"));
    };
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let temp = parent.join(temp_name);

    let outcome = write_temp(&temp, bytes)
        .and_then(|()| fs::rename(&temp, path).map_err(|err| io_error(path, err)));
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    } else if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    outcome
}

fn write_temp(temp: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map_err(|err| io_error(temp, err))?;
    file.write_all(bytes).map_err(|err| io_error(temp, err))?;
    file.sync_all().map_err(|err| io_error(temp, err))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn no_bag_folder() -> PropError {
    PropError::BagIo {
        detail: "trash: no bag folder configured (set PALACE_PROP_BAG_DIR or a home directory)"
            .to_string(),
    }
}

fn bag_error(path: &Path, detail: impl std::fmt::Display) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {detail}", path.display()),
    }
}

/// Same as [`bag_error`] for a store that may not know its path yet.
fn bag_error_path(path: Option<&Path>, detail: impl std::fmt::Display) -> PropError {
    match path {
        Some(path) => bag_error(path, detail),
        None => PropError::BagIo {
            detail: format!("trash: {detail}"),
        },
    }
}

fn io_error(path: &Path, err: std::io::Error) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {err}", path.display()),
    }
}

// ---------------------------------------------------------------------------
// Unit tests (payload codec and document edge cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fave_payload_round_trips_in_order_with_deduplication() {
        let keys = [
            PropKey::new(1_675_473_842, 0x5051_93c2),
            PropKey::new(-7, 0),
            PropKey::new(i32::MAX, u32::MAX),
        ];
        let mut favorites = Favorites::new();
        for key in keys {
            assert!(favorites.add(key));
        }
        assert!(!favorites.add(keys[0]), "duplicate is ignored");
        let payload = favorites.encode_payload();
        assert_eq!(payload.len(), keys.len() * FAVE_SPEC_LEN);
        let decoded = Favorites::decode_payload(&payload).expect("payload decodes");
        assert_eq!(decoded.keys(), &keys);
    }

    #[test]
    fn a_payload_that_is_not_whole_specs_is_rejected() {
        assert!(Favorites::decode_payload(&[0u8; 7]).is_err());
        assert!(Favorites::decode_payload(&[0u8; 9]).is_err());
        assert!(Favorites::decode_payload(&[]).is_ok());
    }

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(hex_encode(&bytes), "000fa5ff");
        assert_eq!(hex_decode("000fa5ff"), Some(bytes.to_vec()));
        assert_eq!(hex_decode("000FA5FF"), Some(bytes.to_vec()));
        assert_eq!(hex_decode("-"), Some(Vec::new()));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn a_rendered_document_parses_back() {
        let entries = vec![
            TrashEntry {
                key: PropKey::new(1000, 3_735_928_559),
                name: Some("Forty Two".to_string()),
                blob: vec![1, 2, 3, 4],
            },
            TrashEntry {
                key: PropKey::new(-1, 0),
                name: None,
                blob: Vec::new(),
            },
        ];
        let text = render(&entries);
        let path = Path::new("Trash.store");
        let parsed = parse_trash(&text, path).expect("rendered document parses");
        assert_eq!(parsed, entries);
    }

    #[test]
    fn a_bad_document_is_a_clean_error() {
        let path = Path::new("Trash.store");
        assert!(parse_trash("", path).is_err());
        assert!(parse_trash("not-a-trash 1\n", path).is_err());
        assert!(parse_trash("palace-prop-trash 2\n", path).is_err());
        assert!(parse_trash("palace-prop-trash 1\n1 2 3\n", path).is_err());
        assert!(parse_trash("palace-prop-trash 1\n1 2 xx 00\n", path).is_err());
    }
}

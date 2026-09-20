//! Shelf discovery and read-only aggregation of `.prp` collections.
//!
//! A **shelf** is a read-only collection the user owns: any `.prp` file under
//! the bag folder's `shelves/` directory, or a collection the user points at by
//! an explicit path (for example a folder inside PalaceChat's own data). The
//! bag folder's layout and its guard live in [`crate::bag_folder`]; this module
//! only *names* the shelves directory ([`crate::bag_folder::shelves_dir`]) and
//! opens the files it finds there.
//!
//! Discovery and loading never write anything. A shelf is opened read-only and
//! its bytes are parsed into metadata; a shelf is **never** modified, moved or
//! repaired, and no thumbnail or pixel is decoded. That matters because a real
//! collection can be 66,885 records / 74 MB: only the roster's headers (width,
//! height, flags, name, `(id, crc)`) leave this module, never an image.
//!
//! # Tolerant by design
//!
//! One corrupt shelf must not cost the user the rest of their collection. Each
//! file is loaded independently, so a truncated or unreadable `.prp` becomes a
//! [`Shelf`] with [`Shelf::error`] set and an empty entry list, while every
//! other shelf still loads. The UI can then show the bad shelf's status without
//! hiding the good ones.
//!
//! # Health
//!
//! Every shelf carries a [`ShelfHealth`] — how many records it holds, how many
//! are named, how many carry no usable prop header, how many the parser had to
//! drop, and a [`ShelfStatus`] derived from those. The status is deliberately
//! coarse so a picker can grey out a shelf at a glance.
//!
//! # Provenance
//!
//! Each aggregated entry is tagged [`Provenance::Bag`] with the shelf's name
//! (its file stem, e.g. `"Old Gems"`), so a merged list can always say which
//! collection a prop came from and [`crate::provenance::bag_listing`] may list
//! it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::bag_folder::{bag_root, shelves_dir};
use crate::catalog::CatalogEntry;
use crate::provenance::Provenance;
use crate::prp::{PropRecord, Roster};

/// How healthy a shelf looked when it was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShelfStatus {
    /// Read and parsed; every record carried a usable prop header and none was
    /// dropped.
    Healthy,
    /// Read and parsed, but at least one record is unusable: it has no prop
    /// header, or its blob fell outside the data region and was dropped.
    Degraded,
    /// The file could not be read, or its container is too corrupt to parse at
    /// all. [`Shelf::entries`] is empty and [`Shelf::error`] explains why.
    #[default]
    Unreadable,
}

/// Counting facts about one shelf, for the picker's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShelfHealth {
    /// Records the roster holds, as counted after the tolerant parse.
    pub records: usize,
    /// Records whose `name_offset` resolved to a name.
    pub named: usize,
    /// Records with no usable prop header (a `Fave` sentinel or an empty blob),
    /// which therefore cannot be shown as a prop.
    pub undecodable: usize,
    /// Records the reader dropped because their blob fell outside the data
    /// region — the signal that a file is truncated or otherwise corrupt.
    pub dropped: usize,
    /// The coarse status derived from the counters above.
    pub status: ShelfStatus,
}

/// One loaded shelf: where it is, what it holds and how healthy it is.
#[derive(Debug, Clone)]
pub struct Shelf {
    /// The `.prp` file's path.
    pub path: PathBuf,
    /// The display name: the file stem (`"Old Gems"`), or the whole path when
    /// the stem is unusable. This is the name written into every entry's
    /// [`Provenance::Bag::collection`].
    pub name: String,
    /// The shelf's entries, in file order. Empty when [`Shelf::error`] is set.
    pub entries: Vec<CatalogEntry>,
    /// The health counters and derived status.
    pub health: ShelfHealth,
    /// `Some` with a human-readable reason when the shelf could not be read or
    /// parsed. The shelf is still reported so the UI can show the failure.
    pub error: Option<String>,
}

impl Shelf {
    /// Whether the shelf was read and parsed without any unusable record.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.error.is_none() && self.health.status == ShelfStatus::Healthy
    }

    /// Whether the shelf could be read at all, regardless of health.
    #[must_use]
    pub fn is_readable(&self) -> bool {
        self.error.is_none()
    }
}

/// A set of shelves plus the entries aggregated across them.
///
/// Build one with [`ShelfSet::discover`] (scan a bag folder's `shelves/`
/// directory), [`ShelfSet::discover_default`] (scan the environment-discovered
/// bag folder) and/or [`ShelfSet::add_shelf`] (add a single read-only `.prp`
/// by explicit path). Shelves are loaded as they are added, so a later query
/// never touches the disk again.
#[derive(Debug, Clone, Default)]
pub struct ShelfSet {
    shelves: Vec<Shelf>,
}

impl ShelfSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shelves: Vec::new(),
        }
    }

    /// Scan `<root>/shelves` for `.prp`/`.PRP` files and load each one.
    ///
    /// Read-only: the directory is listed, each candidate file is opened for
    /// reading, and nothing is created or written. A missing or unreadable
    /// `shelves/` directory yields an empty set rather than an error. The
    /// files are ordered by name (case-insensitive, then exact) so the result
    /// is deterministic.
    #[must_use]
    pub fn discover(root: &Path) -> Self {
        let mut set = Self::new();
        let dir = shelves_dir(root);
        let Ok(entries) = fs::read_dir(&dir) else {
            return set;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_prp_file(path))
            .collect();
        sort_paths(&mut paths);
        for path in paths {
            set.add_shelf(&path);
        }
        set
    }

    /// Scan the bag folder the environment selects ([`bag_root`]).
    ///
    /// Returns an empty set when no bag folder is configured. Read-only, like
    /// [`ShelfSet::discover`].
    #[must_use]
    pub fn discover_default() -> Self {
        match bag_root() {
            Some(root) => Self::discover(&root),
            None => Self::new(),
        }
    }

    /// Load one shelf by explicit path and append it, unless it is already in
    /// the set (paths are compared literally).
    ///
    /// The path may live anywhere the user can read — it is *not* restricted to
    /// the bag folder, because a user may add one of PalaceChat's own
    /// collections as a shelf. It is always opened read-only. A path that does
    /// not exist, or a file that fails to parse, is still reported as an
    /// unreadable [`Shelf`] so the caller can surface it.
    pub fn add_shelf(&mut self, path: impl AsRef<Path>) -> &Shelf {
        let path = path.as_ref().to_path_buf();
        if let Some(index) = self.shelves.iter().position(|shelf| shelf.path == path) {
            return &self.shelves[index];
        }
        let name = shelf_name(&path);
        self.shelves.push(load_shelf(path, name));
        let last = self.shelves.len() - 1;
        &self.shelves[last]
    }

    /// Every shelf, in load order.
    #[must_use]
    pub fn shelves(&self) -> &[Shelf] {
        &self.shelves
    }

    /// Every entry across every readable shelf, in shelf then file order.
    ///
    /// This is the aggregate the UI browses. Each entry keeps the
    /// [`Provenance::Bag`] of the shelf it came from.
    #[must_use]
    pub fn entries(&self) -> Vec<&CatalogEntry> {
        self.shelves
            .iter()
            .flat_map(|shelf| shelf.entries.iter())
            .collect()
    }

    /// Consume the set and return the aggregated entries by value.
    #[must_use]
    pub fn into_entries(self) -> Vec<CatalogEntry> {
        self.shelves
            .into_iter()
            .flat_map(|shelf| shelf.entries)
            .collect()
    }

    /// The shelves that could not be read or parsed.
    pub fn failed_shelves(&self) -> impl Iterator<Item = &Shelf> {
        self.shelves.iter().filter(|shelf| shelf.error.is_some())
    }

    /// Total records across every shelf, including unusable ones.
    #[must_use]
    pub fn total_records(&self) -> usize {
        self.shelves.iter().map(|shelf| shelf.health.records).sum()
    }

    /// Number of shelves in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shelves.len()
    }

    /// Whether the set holds no shelves.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shelves.is_empty()
    }
}

/// Whether `path`'s extension is `prp`, case-insensitively.
fn is_prp_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("prp"))
}

/// Order paths by name the same way the collection list does: case-insensitive
/// first, then exact, so discovery is stable.
fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let a_key = a
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase());
        let b_key = b
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase());
        a_key
            .cmp(&b_key)
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });
}

/// The collection name for a shelf path: the file stem, falling back to the
/// whole display path when the stem is missing or not valid UTF-8.
fn shelf_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

/// Read and parse one shelf, never failing: a bad file becomes an unreadable
/// [`Shelf`] carrying the reason.
fn load_shelf(path: PathBuf, name: String) -> Shelf {
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Shelf {
                error: Some(format!("{}: {err}", path.display())),
                path,
                name,
                entries: Vec::new(),
                health: ShelfHealth::default(),
            };
        }
    };
    let roster = match Roster::parse(&bytes) {
        Ok(roster) => roster,
        Err(err) => {
            return Shelf {
                error: Some(format!("{}: {err}", path.display())),
                path,
                name,
                entries: Vec::new(),
                health: ShelfHealth::default(),
            };
        }
    };

    let mut entries = Vec::with_capacity(roster.records().len());
    let mut named = 0usize;
    let mut undecodable = 0usize;
    for record in roster.records() {
        if record.name.is_some() {
            named += 1;
        }
        if record.header.is_none() {
            undecodable += 1;
            continue;
        }
        entries.push(catalog_entry(record, &name));
    }
    let dropped = roster.dropped_records();
    let status = if dropped == 0 && undecodable == 0 {
        ShelfStatus::Healthy
    } else {
        ShelfStatus::Degraded
    };
    Shelf {
        path,
        name,
        entries,
        health: ShelfHealth {
            records: roster.records().len(),
            named,
            undecodable,
            dropped,
            status,
        },
        error: None,
    }
}

/// Turn one roster record into the catalog entry the picker consumes.
///
/// Metadata only: the record's raw blob is never decoded, so no pixel or
/// thumbnail is materialised. The identity key is carried straight from the
/// record (`(id, crc)`) rather than rebuilt from the catalog fields.
fn catalog_entry(record: &PropRecord, collection: &str) -> CatalogEntry {
    let header = record.header;
    CatalogEntry {
        id: record.rec.id as u32,
        crc: record.rec.crc,
        name: record.name.clone(),
        width: header.map_or(0, |header| header.width.max(0) as u16),
        height: header.map_or(0, |header| header.height.max(0) as u16),
        flags: record.prop_flags(),
        favorite: false,
        trash: false,
        provenance: Provenance::bag(collection, record.key()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_prp_file_matches_both_cases_only_for_prp() {
        assert!(is_prp_file(Path::new("Old Gems.prp")));
        assert!(is_prp_file(Path::new("OLD.PRP")));
        assert!(!is_prp_file(Path::new("notes.txt")));
        assert!(!is_prp_file(Path::new("no-extension")));
        assert!(!is_prp_file(Path::new("almost.prpx")));
    }

    #[test]
    fn shelf_name_is_the_file_stem() {
        assert_eq!(
            shelf_name(Path::new("/bags/shelves/Old Gems.prp")),
            "Old Gems"
        );
        assert_eq!(shelf_name(Path::new("/bags/shelves/LEGACY.PRP")), "LEGACY");
    }

    #[test]
    fn a_missing_file_is_reported_as_unreadable_not_panicked() {
        let path = std::env::temp_dir().join("palace-shelves-does-not-exist.prp");
        let shelf = load_shelf(path, "Missing".to_string());
        assert!(shelf.error.is_some());
        assert_eq!(shelf.health.status, ShelfStatus::Unreadable);
        assert!(shelf.entries.is_empty());
        assert_eq!(shelf.health.records, 0);
    }
}

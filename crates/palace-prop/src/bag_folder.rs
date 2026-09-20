//! Bag folder layout, path roles, safety guards and atomic writes.
//!
//! The prop bag is a folder under the client's **own** data directory — never
//! PalaceChat's live data, which stays read-only forever. It holds:
//!
//! ```text
//! <bag root>/                     e.g. ~/.local/share/org.palace.client/props/
//! ├── My Bag.prp                  the one writable collection
//! ├── Outfits.prp                 saved outfits (format owned by a later task)
//! └── shelves/                    read-only collections
//!     └── Old Gems.prp
//! ```
//!
//! This module owns the layout's *locations* and its *write rules*. It does not
//! parse `.prp` bytes, does not copy files and never creates a directory
//! outside the bag root.
//!
//! # Discovery
//!
//! [`bag_root`] returns `PALACE_PROP_BAG_DIR` when it is set and non-empty,
//! otherwise the platform default built from the same candidate rules as
//! `palace_client::runtime::cache_root_from` and `crate::catalog`:
//!
//! * **Unix:** `$XDG_DATA_HOME/org.palace.client/props`, falling back to
//!   `$HOME/.local/share/org.palace.client/props`.
//! * **Windows:** `%LOCALAPPDATA%\org.palace.client\props`, derived from
//!   `%USERPROFILE%\AppData\Local` when `%LOCALAPPDATA%` is unset, then
//!   `%APPDATA%` (and its `%USERPROFILE%\AppData\Roaming` derivation).
//!
//! Discovery only names a path; nothing is created by it.
//!
//! # The guard
//!
//! [`classify`] sorts a path into one of three [`BagRole`]s: [`BagRole::MyBag`]
//! (a writable file the bag owns), [`BagRole::Shelf`] (a read-only collection
//! under `shelves/`) or [`BagRole::Forbidden`] (everything else). Writes are
//! only ever allowed to `MyBag`. The four protected subtrees below the user's
//! home — `~/.local/share/PalaceChat/`, `~/.config/PalaceChat 5/`,
//! `~/.cache/PalaceChat/` and `$MEDIA/Prop Files/` — are forbidden even
//! when they sit inside a configured bag root.
//!
//! The decision is made in two layers:
//!
//! 1. [`BagContext::classify`] is a **pure function** of the path and the
//!    context's roots. It normalises `..`/`.` lexically and compares path
//!    components; it never touches the filesystem, so it is unit-testable and
//!    can answer for paths that do not exist yet.
//! 2. [`BagContext::open_for_write`] and [`BagContext::atomic_write`] then
//!    canonicalise the deepest existing ancestor (resolving symlinks) and
//!    re-classify, so a symlink cannot smuggle a write into a protected tree.
//!
//! # Atomic writes
//!
//! [`atomic_write`] writes the new bytes to a uniquely named temp file in the
//! destination's directory, flushes and `fsync`s it, then renames it into
//! place. On any failure the temp file is removed and the original is left
//! exactly as it was. [`WriteFault`] is the seam the integration tests use to
//! stop the sequence between the fsync and the rename — the same point a power
//! loss or `SIGKILL` hits.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{PropError, Result};

/// Environment variable that overrides the bag folder.
pub const ENV_BAG_DIR: &str = "PALACE_PROP_BAG_DIR";

/// The client's data-directory name (the Tauri bundle identifier).
pub const APP_ID: &str = "org.palace.client";

/// The bag folder's name inside the client data directory.
pub const PROPS_DIR_NAME: &str = "props";

/// The one writable collection inside the bag folder.
pub const BAG_FILE_NAME: &str = "My Bag.prp";

/// Read-only collections live here, one `.prp` per shelf.
pub const SHELVES_DIR_NAME: &str = "shelves";

/// Saved outfits. The *format* is owned by a later task; the location is fixed
/// here so every task agrees where the file lives.
pub const OUTFITS_FILE_NAME: &str = "Outfits.prp";

/// The files directly in the bag root that [`open_for_write`] and
/// [`atomic_write`] may touch. Everything else in the tree is read-only or
/// forbidden.
pub const WRITABLE_FILE_NAMES: [&str; 2] = [BAG_FILE_NAME, OUTFITS_FILE_NAME];

/// Subtrees below the user's home that are **never** writable. Spelled as
/// component lists so one constant works with either path separator.
const FORBIDDEN_HOME_PATHS: &[&[&str]] = &[
    &[".local", "share", "PalaceChat"],
    &[".config", "PalaceChat 5"],
    &[".cache", "PalaceChat"],
    &["Pictures", "Prop Files"],
];

/// The Windows profile subdirectory holding the AppData roots and its leaves.
const WINDOWS_APPDATA_SUBDIR: &str = "AppData";
/// The roaming leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%APPDATA%`).
const WINDOWS_ROAMING_LEAF: &str = "Roaming";
/// The local leaf under [`WINDOWS_APPDATA_SUBDIR`] (`%LOCALAPPDATA%`).
const WINDOWS_LOCAL_LEAF: &str = "Local";

/// One temp-file sequence number per process, so concurrent writes never
/// collide on a temp name.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What a path means to the bag folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BagRole {
    /// A file the bag folder owns and may rewrite atomically (`My Bag.prp`,
    /// `Outfits.prp`).
    MyBag,
    /// A collection under `shelves/`. Read-only: it may be browsed, but a
    /// write to it is refused.
    Shelf,
    /// Protected client data, anything outside the bag root, or otherwise not
    /// writable.
    Forbidden,
}

/// A bag folder plus the home directory used to recognise protected trees.
///
/// The roots are explicit so the guard is a pure function of the path: tests
/// (and a caller that already resolved its configuration) can build a context
/// over a synthetic tree without touching the environment or the real bag.
#[derive(Debug, Clone, Default)]
pub struct BagContext {
    /// The bag folder: the directory that holds `My Bag.prp`.
    pub bag_root: Option<PathBuf>,
    /// The user's home directory, used only to recognise the protected
    /// subtrees listed in the module docs.
    pub home: Option<PathBuf>,
}

impl BagContext {
    /// Build a context from the environment: [`bag_root`] plus `$HOME`
    /// (falling back to `%USERPROFILE%` on Windows).
    #[must_use]
    pub fn discover() -> Self {
        Self {
            bag_root: bag_root(),
            home: discover_home(),
        }
    }

    /// The role of `path` under this context. Pure: no filesystem access, so
    /// it answers for paths that do not exist and cannot race with the disk.
    #[must_use]
    pub fn classify(&self, path: &Path) -> BagRole {
        classify_path(path, self.bag_root.as_deref(), self.home.as_deref())
    }

    /// Open a bag-owned file for writing, truncating it.
    ///
    /// Refused unless `path` is [`BagRole::MyBag`] both lexically and after
    /// canonicalising its deepest existing ancestor. Prefer [`BagContext::atomic_write`]
    /// for saving a collection; this is the direct handle for callers that
    /// need one.
    pub fn open_for_write(&self, path: &Path) -> Result<File> {
        let target = self.writable_target(path)?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&target)
            .map_err(|err| io_error(&target, err))
    }

    /// Replace `path`'s contents atomically, or leave the original untouched.
    ///
    /// Same guard as [`BagContext::open_for_write`]. See [`atomic_write`] for
    /// the sequence.
    pub fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.atomic_write_faulted(path, bytes, WriteFault::None)
    }

    /// [`BagContext::atomic_write`] with the test-only [`WriteFault`] seam.
    ///
    /// Not part of the stable API: the fault parameter exists so an
    /// integration test can interrupt the write in the one window a real
    /// process kill cannot be made to hit deterministically.
    #[doc(hidden)]
    pub fn atomic_write_faulted(&self, path: &Path, bytes: &[u8], fault: WriteFault) -> Result<()> {
        let target = self.writable_target(path)?;
        let parent = non_empty_parent(&target)?;
        let name = target.file_name().ok_or_else(|| PropError::BagIo {
            detail: format!("{}: not a file path", target.display()),
        })?;
        let temp = temp_path(parent, name);

        let outcome = write_temp(&temp, bytes)
            .and_then(|()| fault.check())
            .and_then(|()| fs::rename(&temp, &target).map_err(|err| io_error(&target, err)));
        if outcome.is_err() {
            // Leave no debris: the original is either untouched or was never
            // there. A failed removal must not mask the real error.
            let _ = fs::remove_file(&temp);
        } else {
            // Best effort: makes the rename itself durable. A failure here
            // means the data is in place but the directory entry may not
            // survive a power cut; that is not an error the caller can act on.
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        outcome
    }

    /// Create the bag root and its `shelves/` directory if they are missing.
    ///
    /// Refused — before anything is created — when the root is not a writable
    /// bag location, which includes any root that sits in (or resolves into)
    /// one of the protected subtrees. `My Bag.prp` itself is not created here:
    /// an empty file is not a valid `.prp`, so it appears on the first
    /// [`BagContext::atomic_write`].
    pub fn ensure_layout(&self) -> Result<PathBuf> {
        let root = self.bag_root.clone().ok_or_else(|| PropError::BagIo {
            detail: "no bag folder configured: set PALACE_PROP_BAG_DIR or a home directory"
                .to_string(),
        })?;
        // Reuse the full write guard (lexical plus canonicalised) on a file the
        // layout owns, so a protected or symlinked root is refused before any
        // directory is created.
        self.writable_target(&root.join(BAG_FILE_NAME))?;
        fs::create_dir_all(&root).map_err(|err| io_error(&root, err))?;
        let shelves = root.join(SHELVES_DIR_NAME);
        fs::create_dir_all(&shelves).map_err(|err| io_error(&shelves, err))?;
        Ok(root)
    }

    /// Resolve `path` through any symlinks and refuse it unless both the
    /// literal and the resolved path are [`BagRole::MyBag`].
    fn writable_target(&self, path: &Path) -> Result<PathBuf> {
        let role = self.classify(path);
        if role != BagRole::MyBag {
            return Err(refusal(path, role));
        }
        let resolved = resolve(path);
        let resolved_root = self.bag_root.as_deref().map(resolve);
        let resolved_home = self.home.as_deref().map(resolve);
        let resolved_role = classify_path(
            &resolved,
            resolved_root.as_deref(),
            resolved_home.as_deref(),
        );
        if resolved_role != BagRole::MyBag {
            return Err(refusal(&resolved, resolved_role));
        }
        Ok(resolved)
    }
}

/// The bag folder the environment selects.
///
/// `PALACE_PROP_BAG_DIR` wins when set and non-empty; otherwise the platform
/// default documented on [`BagContext`]. Returns the path whether or not it
/// exists; this function never creates anything.
#[must_use]
pub fn bag_root() -> Option<PathBuf> {
    std::env::var_os(ENV_BAG_DIR)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(default_bag_root)
}

/// The role of `path` under the environment-discovered bag folder. See
/// [`BagContext::classify`], which is the pure form.
#[must_use]
pub fn classify(path: &Path) -> BagRole {
    BagContext::discover().classify(path)
}

/// Open a bag-owned file for writing under the discovered bag folder.
///
/// Equivalent to [`BagContext::open_for_write`] on [`BagContext::discover`].
pub fn open_for_write(path: &Path) -> Result<File> {
    BagContext::discover().open_for_write(path)
}

/// Atomically replace a bag-owned file under the discovered bag folder.
///
/// Equivalent to [`BagContext::atomic_write`] on [`BagContext::discover`].
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    BagContext::discover().atomic_write(path, bytes)
}

/// Create the discovered bag folder's layout. See [`BagContext::ensure_layout`].
pub fn ensure_layout() -> Result<PathBuf> {
    BagContext::discover().ensure_layout()
}

/// `<root>/My Bag.prp`, the writable collection.
#[must_use]
pub fn my_bag_path(root: &Path) -> PathBuf {
    root.join(BAG_FILE_NAME)
}

/// `<root>/shelves`, the read-only collections directory.
#[must_use]
pub fn shelves_dir(root: &Path) -> PathBuf {
    root.join(SHELVES_DIR_NAME)
}

/// `<root>/Outfits.prp`, the saved-outfits file.
#[must_use]
pub fn outfits_path(root: &Path) -> PathBuf {
    root.join(OUTFITS_FILE_NAME)
}

/// A failure point in the atomic-write sequence.
///
/// Public only because integration tests link the crate without `cfg(test)`;
/// [`atomic_write`] always uses [`WriteFault::None`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WriteFault {
    /// Write, fsync and rename normally.
    #[default]
    None,
    /// Stop after the temp file is written and fsynced, exactly where a power
    /// loss or `SIGKILL` would: the rename never happens.
    BeforeRename,
}

impl WriteFault {
    fn check(self) -> Result<()> {
        match self {
            WriteFault::None => Ok(()),
            WriteFault::BeforeRename => Err(PropError::BagIo {
                detail: "atomic write interrupted before rename (fault injected)".to_string(),
            }),
        }
    }
}

/// The platform default bag root, from the environment candidates.
fn default_bag_root() -> Option<PathBuf> {
    if cfg!(windows) {
        windows_bag_root(
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("USERPROFILE").map(PathBuf::from),
        )
    } else {
        unix_bag_root(
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }
}

/// The Unix default: `$XDG_DATA_HOME/org.palace.client/props`, else
/// `$HOME/.local/share/org.palace.client/props`.
///
/// Candidates are passed in rather than read so the rule is testable from any
/// host, the same idiom as `crate::catalog`'s platform helpers.
fn unix_bag_root(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_data_home
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| home.map(|home| home.join(".local").join("share")))?;
    Some(base.join(APP_ID).join(PROPS_DIR_NAME))
}

/// The Windows default: `%LOCALAPPDATA%\org.palace.client\props`, then
/// `%APPDATA%\org.palace.client\props`.
///
/// Each root is taken from its dedicated variable when set, otherwise derived
/// from `user_profile` at `AppData\Local` / `AppData\Roaming`.
fn windows_bag_root(
    local_app_data: Option<PathBuf>,
    app_data: Option<PathBuf>,
    user_profile: Option<PathBuf>,
) -> Option<PathBuf> {
    let local = local_app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_LOCAL_LEAF));
    let roaming = app_data.or_else(|| derive_appdata(&user_profile, WINDOWS_ROAMING_LEAF));
    local
        .or(roaming)
        .map(|base| base.join(APP_ID).join(PROPS_DIR_NAME))
}

fn derive_appdata(user_profile: &Option<PathBuf>, leaf: &str) -> Option<PathBuf> {
    user_profile
        .as_ref()
        .map(|profile| profile.join(WINDOWS_APPDATA_SUBDIR).join(leaf))
}

/// The user's home directory: `$HOME`, then `%USERPROFILE%` on Windows.
fn discover_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The pure role decision. See the module docs for the rules.
fn classify_path(path: &Path, bag_root: Option<&Path>, home: Option<&Path>) -> BagRole {
    let path = normalize(path);
    if let Some(home) = home {
        let home = normalize(home);
        for relative in FORBIDDEN_HOME_PATHS {
            let mut forbidden = home.clone();
            for part in *relative {
                forbidden.push(part);
            }
            if path.starts_with(&forbidden) {
                return BagRole::Forbidden;
            }
        }
    }
    let Some(root) = bag_root else {
        return BagRole::Forbidden;
    };
    let root = normalize(root);
    if WRITABLE_FILE_NAMES
        .iter()
        .any(|name| path == root.join(name))
    {
        return BagRole::MyBag;
    }
    if path.starts_with(root.join(SHELVES_DIR_NAME)) {
        return BagRole::Shelf;
    }
    BagRole::Forbidden
}

/// Lexically resolve `.` and `..` without touching the filesystem.
///
/// `..` pops the previous component; a `..` at a root is ignored, matching how
/// the filesystem would treat it. This is the pure half of the guard — it
/// catches textual detours like `shelves/../My Bag.prp` before any syscall.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// Canonicalise the deepest existing ancestor of `path` and re-append the
/// missing tail.
///
/// `Path::canonicalize` fails on a path that does not exist yet, but the write
/// guard needs an answer for a brand-new `My Bag.prp`. Resolving the part that
/// does exist catches symlinked directories while leaving the not-yet-created
/// tail intact. When nothing along the path exists, the path is returned
/// unchanged.
fn resolve(path: &Path) -> PathBuf {
    let mut remainder: Vec<OsString> = Vec::new();
    let mut candidate = path.to_path_buf();
    loop {
        match candidate.canonicalize() {
            Ok(mut resolved) => {
                for part in remainder.iter().rev() {
                    resolved.push(part);
                }
                return resolved;
            }
            Err(_) => {
                let Some(name) = candidate.file_name() else {
                    return path.to_path_buf();
                };
                let Some(parent) = candidate.parent() else {
                    return path.to_path_buf();
                };
                if parent.as_os_str().is_empty() {
                    return path.to_path_buf();
                }
                remainder.push(name.to_os_string());
                candidate = parent.to_path_buf();
            }
        }
    }
}

/// The parent directory of a target, rejecting a bare filename with no parent.
fn non_empty_parent(path: &Path) -> Result<&Path> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Ok(parent),
        _ => Err(PropError::BagIo {
            detail: format!("{}: not a file path", path.display()),
        }),
    }
}

/// A unique temp path beside the destination, so the rename is same-directory
/// (and therefore atomic) on every platform.
fn temp_path(parent: &Path, file_name: &OsStr) -> PathBuf {
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(file_name);
    name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    parent.join(name)
}

/// Create the temp file exclusively, write every byte and flush it to disk.
fn write_temp(temp: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)
        .map_err(|err| io_error(temp, err))?;
    file.write_all(bytes).map_err(|err| io_error(temp, err))?;
    file.sync_all().map_err(|err| io_error(temp, err))
}

/// The error a refused write returns, naming the role that refused it.
fn refusal(path: &Path, role: BagRole) -> PropError {
    let detail = match role {
        BagRole::MyBag => format!("{}: not writable", path.display()),
        BagRole::Shelf => format!(
            "{}: shelves are read-only; copy the collection into \"{BAG_FILE_NAME}\" to change it",
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

    fn context(root: &str, home: &str) -> BagContext {
        BagContext {
            bag_root: Some(PathBuf::from(root)),
            home: Some(PathBuf::from(home)),
        }
    }

    #[test]
    fn normalize_resolves_dot_segments_without_the_filesystem() {
        assert_eq!(normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
        assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a/b"));
        assert_eq!(normalize(Path::new("/a/b/../../c")), PathBuf::from("/c"));
    }

    #[test]
    fn classify_sorts_writable_files_shelves_and_protected_trees() {
        let ctx = context("/home/u/.local/share/org.palace.client/props", "/home/u");
        let root = Path::new("/home/u/.local/share/org.palace.client/props");
        assert_eq!(ctx.classify(&root.join(BAG_FILE_NAME)), BagRole::MyBag);
        assert_eq!(ctx.classify(&root.join(OUTFITS_FILE_NAME)), BagRole::MyBag);
        assert_eq!(
            ctx.classify(&root.join(SHELVES_DIR_NAME).join("Old.prp")),
            BagRole::Shelf
        );
        assert_eq!(
            ctx.classify(Path::new(
                "/home/u/.local/share/PalaceChat/PropBag.bundle/Test.pids"
            )),
            BagRole::Forbidden
        );
        assert_eq!(
            ctx.classify(Path::new("/home/u/.config/PalaceChat 5/PropBag.bundle")),
            BagRole::Forbidden
        );
        assert_eq!(
            ctx.classify(Path::new("/home/u/.cache/PalaceChat/PropBag.bundle")),
            BagRole::Forbidden
        );
        assert_eq!(
            ctx.classify(Path::new("/home/u/Pictures/Prop Files/Some.prp")),
            BagRole::Forbidden
        );
        assert_eq!(
            ctx.classify(Path::new("/tmp/loose.prp")),
            BagRole::Forbidden
        );
        assert_eq!(ctx.classify(root), BagRole::Forbidden);
        assert_eq!(ctx.classify(&root.join("Loose.prp")), BagRole::Forbidden);

        // A bag root configured inside protected data does not make it
        // writable: the protected check runs before the root check.
        let trapped = context("/home/u/.local/share/PalaceChat/props", "/home/u");
        assert_eq!(
            trapped.classify(Path::new(
                "/home/u/.local/share/PalaceChat/props/My Bag.prp"
            )),
            BagRole::Forbidden
        );
    }

    #[test]
    fn classify_normalises_lexical_detours() {
        let ctx = context("/home/u/props", "/home/u");
        assert_eq!(
            ctx.classify(Path::new("/home/u/props/shelves/../My Bag.prp")),
            BagRole::MyBag
        );
        assert_eq!(
            ctx.classify(Path::new(
                "/home/u/props/shelves/../../.local/share/PalaceChat/X.prp"
            )),
            BagRole::Forbidden
        );
    }

    #[test]
    fn classify_without_a_bag_root_is_always_forbidden() {
        let ctx = BagContext {
            bag_root: None,
            home: Some(PathBuf::from("/home/u")),
        };
        assert_eq!(
            ctx.classify(Path::new("/home/u/props/My Bag.prp")),
            BagRole::Forbidden
        );
    }

    #[test]
    fn the_unix_default_prefers_xdg_then_home() {
        let home = PathBuf::from("/home/u");
        assert_eq!(
            unix_bag_root(Some(PathBuf::from("/xdg")), Some(home.clone())),
            Some(PathBuf::from("/xdg/org.palace.client/props"))
        );
        assert_eq!(
            unix_bag_root(None, Some(home.clone())),
            Some(PathBuf::from(
                "/home/u/.local/share/org.palace.client/props"
            ))
        );
        assert_eq!(
            unix_bag_root(Some(PathBuf::new()), Some(home)),
            Some(PathBuf::from(
                "/home/u/.local/share/org.palace.client/props"
            )),
            "an empty XDG_DATA_HOME must fall through to HOME"
        );
        assert_eq!(unix_bag_root(None, None), None);
    }

    #[test]
    fn the_windows_default_prefers_local_app_data_and_derives_from_profile() {
        let local = PathBuf::from("C:/Users/u/AppData/Local");
        let roaming = PathBuf::from("C:/Users/u/AppData/Roaming");
        let profile = PathBuf::from("C:/Users/u");
        let expected = Some(PathBuf::from(
            "C:/Users/u/AppData/Local/org.palace.client/props",
        ));
        assert_eq!(
            windows_bag_root(Some(local.clone()), Some(roaming), Some(profile.clone())),
            expected
        );
        assert_eq!(windows_bag_root(None, None, Some(profile)), expected);
        assert_eq!(windows_bag_root(None, None, None), None);
    }

    #[test]
    fn the_layout_helpers_point_directly_into_the_root() {
        let root = Path::new("/bags");
        assert_eq!(my_bag_path(root), root.join(BAG_FILE_NAME));
        assert_eq!(shelves_dir(root), root.join(SHELVES_DIR_NAME));
        assert_eq!(outfits_path(root), root.join(OUTFITS_FILE_NAME));
    }
}

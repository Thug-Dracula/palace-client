//! The two caches.
//!
//! ## In-memory asset cache
//!
//! Keyed by `(assetType, id, crc)` — the same triple that identifies an asset on
//! the wire — and bounded by a least-recently-used eviction order. The
//! reference client bounds its in-memory prop table too
//! (`PalaceConfig.numberPropsToCacheInRAM`) because a busy room can reference
//! thousands of props and each one is a full sprite.
//!
//! There is also a secondary `(assetType, id)` index, because a `qAst` is often
//! sent with `crc == 0` ("don't care") while the reply carries the real CRC;
//! looking an asset up by id alone is the only way to answer "do I already have
//! this prop?" in that case.
//!
//! ## On-disk media cache
//!
//! ```text
//! <root>/
//!   media/
//!     <8 hex of the media-server URL hash>/
//!       <sanitised relative name>
//! ```
//!
//! The URL hash is what keeps two servers that both serve `bg.gif` from
//! overwriting each other — the reference implementations that cache to a flat
//! directory get this wrong.
//!
//! File names are **not** trusted. `animated-backgrounds/rainy-day.gif` is a
//! real background path on a live server, so subdirectories must survive; but
//! `../`, absolute paths, `.`/`..` components, over-long components and control
//! characters are all refused or escaped. See [`sanitise_media_name`].
//!
//! Writes go to a sibling `.part` file and are then renamed, so a crash or a
//! concurrent reader never sees a half-written image.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::assembler::Asset;
use crate::asset::{AssetKey, AssetType};
use crate::error::{AssetError, Result};

/// Default in-memory asset cache capacity.
pub const DEFAULT_ASSET_CACHE_CAPACITY: usize = 512;

/// Longest single path component the media cache will write.
pub const MAX_MEDIA_COMPONENT_LEN: usize = 200;

/// Deepest relative path the media cache will write.
pub const MAX_MEDIA_DEPTH: usize = 8;

/// LRU cache of complete assets, keyed by `(assetType, id, crc)`.
#[derive(Debug)]
pub struct AssetCache {
    capacity: usize,
    entries: HashMap<AssetKey, Asset>,
    order: Vec<AssetKey>,
    by_id: HashMap<(AssetType, i32), AssetKey>,
}

impl AssetCache {
    /// Build a cache holding at most `capacity` assets.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        AssetCache {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: Vec::new(),
            by_id: HashMap::new(),
        }
    }

    /// Insert or replace an asset, returning the evicted asset if the cache was
    /// full.
    pub fn insert(&mut self, asset: Asset) -> Option<Asset> {
        let key = asset.key;
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            let evicted = self.evict_oldest();
            self.entries.insert(key, asset);
            self.order.push(key);
            self.by_id.insert((key.asset_type, key.id), key);
            return evicted;
        }
        self.touch(&key);
        self.entries.insert(key, asset);
        self.by_id.insert((key.asset_type, key.id), key);
        None
    }

    /// Look up by the full `(assetType, id, crc)` triple, marking it recently used.
    pub fn get(&mut self, key: &AssetKey) -> Option<&Asset> {
        if !self.entries.contains_key(key) {
            return None;
        }
        self.touch(key);
        self.entries.get(key)
    }

    /// Look up without reordering. Useful for inspection and tests.
    #[must_use]
    pub fn peek(&self, key: &AssetKey) -> Option<&Asset> {
        self.entries.get(key)
    }

    /// Look up by `(assetType, id)`, ignoring the CRC.
    ///
    /// This is the lookup that matches a `crc == 0` request against a cached
    /// asset whose CRC is known.
    #[must_use]
    pub fn peek_by_id(&self, asset_type: AssetType, id: i32) -> Option<&Asset> {
        let key = self.by_id.get(&(asset_type, id))?;
        self.entries.get(key)
    }

    /// True when the exact triple is cached.
    #[must_use]
    pub fn contains(&self, key: &AssetKey) -> bool {
        self.entries.contains_key(key)
    }

    /// True when any asset with this type and id is cached.
    #[must_use]
    pub fn contains_id(&self, asset_type: AssetType, id: i32) -> bool {
        self.by_id.contains_key(&(asset_type, id))
    }

    /// How many assets are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The configured capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Remove one asset.
    pub fn remove(&mut self, key: &AssetKey) -> Option<Asset> {
        let asset = self.entries.remove(key)?;
        self.order.retain(|k| k != key);
        if self
            .by_id
            .get(&(key.asset_type, key.id))
            .is_some_and(|k| *k == *key)
        {
            self.by_id.remove(&(key.asset_type, key.id));
        }
        Some(asset)
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.by_id.clear();
    }

    /// Change the capacity, evicting down to it.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.entries.len() > self.capacity {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) -> Option<Asset> {
        while let Some(key) = self.order.first().copied() {
            self.order.remove(0);
            if let Some(asset) = self.entries.remove(&key) {
                if self
                    .by_id
                    .get(&(key.asset_type, key.id))
                    .is_some_and(|k| *k == key)
                {
                    self.by_id.remove(&(key.asset_type, key.id));
                }
                return Some(asset);
            }
        }
        None
    }

    fn touch(&mut self, key: &AssetKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push(*key);
    }
}

/// On-disk cache for room backgrounds and hotspot images.
#[derive(Debug, Clone)]
pub struct MediaCache {
    root: PathBuf,
}

impl MediaCache {
    /// Use `root` as the cache directory. Nothing is created until the first
    /// [`MediaCache::put`].
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        MediaCache { root: root.into() }
    }

    /// The cache root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory this URL's files live in.
    #[must_use]
    pub fn namespace_dir(&self, base_url: &str) -> PathBuf {
        self.root
            .join("media")
            .join(format!("{:08x}", url_namespace(base_url)))
    }

    /// Where `name` fetched from `base_url` would be stored, if the name is safe.
    pub fn path_for(&self, base_url: &str, name: &str) -> Result<PathBuf> {
        Ok(self.namespace_dir(base_url).join(sanitise_media_name(name)?))
    }

    /// Read a cached file, if present.
    pub fn get(&self, base_url: &str, name: &str) -> Option<Vec<u8>> {
        let path = self.path_for(base_url, name).ok()?;
        fs::read(path).ok()
    }

    /// Read a cached file without copying it into memory.
    pub fn path_if_cached(&self, base_url: &str, name: &str) -> Option<PathBuf> {
        let path = self.path_for(base_url, name).ok()?;
        path.is_file().then_some(path)
    }

    /// Store a fetched file, creating directories as needed.
    ///
    /// Writes via a `.part` sibling and renames, so the visible file is always
    /// complete.
    pub fn put(&self, base_url: &str, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.path_for(base_url, name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!(
            "{}part",
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| format!("{e}."))
                .unwrap_or_default()
        ));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Remove one cached file.
    pub fn remove(&self, base_url: &str, name: &str) -> Result<bool> {
        let path = self.path_for(base_url, name)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AssetError::Io(e)),
        }
    }

    /// Total size of the cache in bytes, and how many files it holds.
    pub fn stats(&self) -> Result<(u64, usize)> {
        let media = self.root.join("media");
        if !media.is_dir() {
            return Ok((0, 0));
        }
        let mut bytes = 0;
        let mut files = 0;
        let mut stack = vec![media];
        while let Some(dir) = stack.pop() {
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    bytes += meta.len();
                    files += 1;
                }
            }
        }
        Ok((bytes, files))
    }
}

/// Turn a server-supplied media name into a safe relative path.
///
/// Subdirectories are preserved (real servers reference
/// `animated-backgrounds/rainy-day.gif`), but everything dangerous is removed:
///
/// * `\` is treated as a separator so a Windows-style traversal is caught too;
/// * empty, `.` and `..` components are refused;
/// * a leading separator (an absolute path) is refused;
/// * components longer than [`MAX_MEDIA_COMPONENT_LEN`] and paths deeper than
///   [`MAX_MEDIA_DEPTH`] are refused;
/// * every character outside `[A-Za-z0-9._-]` is percent-escaped, so NUL, `:`,
///   control codes and shell metacharacters cannot reach the filesystem.
pub fn sanitise_media_name(name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        return Err(AssetError::UnsafeMediaName(name.to_string()));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(AssetError::UnsafeMediaName(name.to_string()));
    }
    if name.contains('\0') {
        return Err(AssetError::UnsafeMediaName(name.to_string()));
    }
    let normalised = name.replace('\\', "/");
    let parts: Vec<&str> = normalised.split('/').collect();
    if parts.len() > MAX_MEDIA_DEPTH {
        return Err(AssetError::UnsafeMediaName(name.to_string()));
    }
    let mut out = PathBuf::new();
    for part in parts {
        if part.is_empty() || part == "." || part == ".." {
            return Err(AssetError::UnsafeMediaName(name.to_string()));
        }
        if part.len() > MAX_MEDIA_COMPONENT_LEN {
            return Err(AssetError::UnsafeMediaName(name.to_string()));
        }
        out.push(escape_component(part));
    }
    if out.as_os_str().is_empty() {
        return Err(AssetError::UnsafeMediaName(name.to_string()));
    }
    Ok(out)
}

fn escape_component(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for b in part.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_namespace(base_url: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in base_url.trim_end_matches('/').bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::AssetSpec;

    fn asset(id: i32, crc: u32, data: &[u8]) -> Asset {
        Asset {
            key: AssetKey::new(AssetType::PROP, id, crc),
            flags: 0,
            name: String::new(),
            data: data.to_vec(),
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "palace-asset-cache-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_asset_cache_keys_on_the_full_triple() {
        let mut c = AssetCache::new(8);
        assert!(c.insert(asset(1, 2, b"a")).is_none());
        assert!(c.contains(&AssetKey::new(AssetType::PROP, 1, 2)));
        assert!(!c.contains(&AssetKey::new(AssetType::PROP, 1, 3)));
        assert!(c.get(&AssetKey::new(AssetType::PROP, 1, 3)).is_none());
        assert_eq!(c.get(&AssetKey::new(AssetType::PROP, 1, 2)).unwrap().data, b"a");
    }

    #[test]
    fn the_id_index_answers_a_dont_care_request() {
        let mut c = AssetCache::new(4);
        c.insert(asset(9, 0xabc, b"x"));
        assert!(c.contains_id(AssetType::PROP, 9));
        assert_eq!(c.peek_by_id(AssetType::PROP, 9).unwrap().crc(), 0xabc);
        assert!(c.peek_by_id(AssetType::PROP, 10).is_none());
    }

    #[test]
    fn eviction_is_lru_and_bounded() {
        let mut c = AssetCache::new(2);
        c.insert(asset(1, 0, b"1"));
        c.insert(asset(2, 0, b"2"));
        let _ = c.get(&AssetKey::new(AssetType::PROP, 1, 0));
        let evicted = c.insert(asset(3, 0, b"3")).expect("cache was full");
        assert_eq!(evicted.id(), 2, "the least recently used goes first");
        assert_eq!(c.len(), 2);
        assert!(c.contains(&AssetKey::new(AssetType::PROP, 1, 0)));
        assert!(c.contains(&AssetKey::new(AssetType::PROP, 3, 0)));
        assert!(c.peek_by_id(AssetType::PROP, 2).is_none());
    }

    #[test]
    fn replacing_the_same_key_does_not_evict() {
        let mut c = AssetCache::new(1);
        c.insert(asset(1, 0, b"old"));
        assert!(c.insert(asset(1, 0, b"new")).is_none());
        assert_eq!(c.get(&AssetKey::new(AssetType::PROP, 1, 0)).unwrap().data, b"new");
    }

    #[test]
    fn shrinking_the_capacity_evicts_down() {
        let mut c = AssetCache::new(4);
        for id in 0..4 {
            c.insert(asset(id, 0, b"x"));
        }
        c.set_capacity(2);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn removal_clears_both_indexes() {
        let mut c = AssetCache::new(4);
        c.insert(asset(1, 5, b"x"));
        assert!(c.remove(&AssetKey::new(AssetType::PROP, 1, 5)).is_some());
        assert!(c.is_empty());
        assert!(!c.contains_id(AssetType::PROP, 1));
        assert!(c.remove(&AssetKey::new(AssetType::PROP, 1, 5)).is_none());
    }

    #[test]
    fn a_prefix_name_is_kept_and_a_traversal_is_refused() {
        assert_eq!(
            sanitise_media_name("animated-backgrounds/rainy-day.gif").unwrap(),
            PathBuf::from("animated-backgrounds/rainy-day.gif")
        );
        for bad in [
            "../etc/passwd",
            "a/../../b",
            "/etc/passwd",
            "\\windows\\system32",
            "a/./b",
            "a//b",
            "",
            "   ",
            "a\0b",
        ] {
            assert!(
                sanitise_media_name(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn exotic_characters_are_escaped_not_dropped() {
        assert_eq!(sanitise_media_name("a b:c.png").unwrap(), PathBuf::from("a%20b%3Ac.png"));
        assert_eq!(sanitise_media_name("é.png").unwrap(), PathBuf::from("%C3%A9.png"));
    }

    #[test]
    fn over_long_and_over_deep_names_are_refused() {
        let long = "a".repeat(MAX_MEDIA_COMPONENT_LEN + 1);
        assert!(sanitise_media_name(&long).is_err());
        let deep = ["a"; MAX_MEDIA_DEPTH + 1].join("/");
        assert!(sanitise_media_name(&deep).is_err());
    }

    #[test]
    fn two_servers_that_serve_the_same_name_do_not_collide() {
        let cache = MediaCache::new(temp_root("ns"));
        let a = cache.path_for("http://a.example/media", "bg.gif").unwrap();
        let b = cache.path_for("http://b.example/media", "bg.gif").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(cache.root()));
        let c = cache.path_for("http://a.example/media/", "bg.gif").unwrap();
        assert_eq!(a, c, "a trailing slash is the same namespace");
    }

    #[test]
    fn put_then_get_round_trips_and_leaves_no_part_file() {
        let root = temp_root("rt");
        let cache = MediaCache::new(&root);
        let base = "http://media.example/palace/media";
        let path = cache.put(base, "sub/bg.png", b"\x89PNG\r\n").unwrap();
        assert!(path.is_file());
        assert_eq!(cache.get(base, "sub/bg.png").unwrap(), b"\x89PNG\r\n");
        assert_eq!(cache.path_if_cached(base, "sub/bg.png"), Some(path.clone()));
        assert!(cache.get(base, "sub/other.png").is_none());
        assert!(!path.with_extension("png.part").exists());
        assert!(cache.remove(base, "sub/bg.png").unwrap());
        assert!(!cache.remove(base, "sub/bg.png").unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stats_walks_the_namespaces() {
        let root = temp_root("stats");
        let cache = MediaCache::new(&root);
        cache.put("http://a/x", "one.bin", &[0u8; 10]).unwrap();
        cache.put("http://a/x", "deep/two.bin", &[0u8; 5]).unwrap();
        assert_eq!(cache.stats().unwrap(), (15, 2));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_put_with_an_unsafe_name_writes_nothing() {
        let root = temp_root("unsafe");
        let cache = MediaCache::new(&root);
        assert!(cache.put("http://a/x", "../escape.bin", b"x").is_err());
        assert!(!root.exists());
    }

    #[test]
    fn the_spec_helper_is_available_for_requesters() {
        let key = AssetKey::from_spec(AssetType::PROP, AssetSpec::unchecked(3));
        assert_eq!(key.spec().id, 3);
    }
}

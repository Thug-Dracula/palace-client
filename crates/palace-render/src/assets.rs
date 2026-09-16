//! Resolving assets from local files: backgrounds, props and placeholders.
//!
//! Nothing here touches the network — a parallel track owns asset transfer. The
//! renderer is handed directories and a `.prp` roster and reads what it finds.
//!
//! ## Prop lookup
//!
//! A loose prop carries a 32-bit asset id ([`palace_room::LoosePropSpec::id`]).
//! Two local sources can answer that id:
//!
//! * a **directory** of prop blobs, where the file name starts with the decimal
//!   asset id (`1001438438_unnamed.bin`, `976933367.ark`);
//! * a **`.prp` roster** (`pserver.prp`), a container of
//!   `(asset id → blob)` records. The layout is the one `palace-prop`'s corpus
//!   test already uses:
//!
//!   ```text
//!   0   uint32  dataOffset      start of the concatenated blob data
//!   8   uint32  mapOffset       start of the asset-map descriptor
//!   mapOffset +  4  uint32  nAssets
//!   mapOffset + 16  uint32  recsOffset   first 32-byte record, relative to mapOffset
//!   record i:
//!     0   uint32  asset id
//!     8   uint32  dataRel         blob offset, relative to dataOffset
//!    12   uint32  size            blob length
//!   ```
//!
//! ## Never silently skip
//!
//! A missing or undecodable asset yields an [`AssetNote`] — returned alongside a
//! placeholder or a skip — so the caller can print it. The renderer reports what
//! it could not draw; it does not pretend the room was complete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use palace_prop::{decode, PropImage};

use crate::error::{AssetNote, RenderError};

/// Decode a full-colour image (PNG/JPEG/GIF) from bytes into RGBA8.
///
/// Backgrounds and hotspot overlays are ordinary image files, not Palace props,
/// so they go through the `image` crate. An animated GIF decodes to its first
/// frame; animation is the presentation layer's problem in a later milestone.
pub fn decode_image_bytes(bytes: &[u8]) -> Result<PropImage, String> {
    let decoded = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    PropImage::from_rgba(width, height, rgba.into_raw()).map_err(|e| e.to_string())
}

/// Decode a prop blob into RGBA.
///
/// Thin wrapper over [`palace_prop::decode`] so callers do not have to know the
/// prop crate's error type.
pub fn decode_prop_blob(bytes: &[u8]) -> Result<PropImage, String> {
    decode(bytes)
        .map(|prop| prop.image)
        .map_err(|e| e.to_string())
}

/// A background/overlay file store: an index of file names across one or more
/// roots, walked once.
#[derive(Debug, Default)]
pub struct MediaStore {
    by_name: HashMap<String, PathBuf>,
}

impl MediaStore {
    /// Index every file under `roots` by its lower-cased base name.
    ///
    /// Later roots win on a name collision, so list fallbacks first.
    #[must_use]
    pub fn new(roots: &[PathBuf]) -> Self {
        let mut by_name = HashMap::new();
        for root in roots {
            index_dir(root, &mut by_name);
        }
        MediaStore { by_name }
    }

    /// Number of indexed file names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the store indexed anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Find a file by name, applying the reference client's extension fallback.
    ///
    /// A room that names `bg.gif` will be served `bg.png` or `bg.jpg` when those
    /// exist, which is exactly the `tryPngBG`/`tryJpegBG` ladder in
    /// `PalaceRoomView.mxml`. `None` means the name is genuinely absent.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&Path> {
        if name.is_empty() {
            return None;
        }
        let base = Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
            .to_ascii_lowercase();
        if let Some(path) = self.by_name.get(&base) {
            return Some(path);
        }
        let stem = Path::new(&base)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&base);
        for ext in ["png", "jpg", "jpeg", "gif"] {
            if let Some(path) = self.by_name.get(&format!("{stem}.{ext}")) {
                return Some(path);
            }
        }
        None
    }

    /// Resolve and decode a named image.
    pub fn load(&self, name: &str) -> Result<PropImage, RenderError> {
        let Some(path) = self.resolve(name) else {
            return Err(RenderError::BackgroundDecode {
                file: name.to_string(),
                detail: "not present in any media root".to_string(),
            });
        };
        let bytes = std::fs::read(path).map_err(RenderError::Io)?;
        decode_image_bytes(&bytes).map_err(|detail| RenderError::BackgroundDecode {
            file: name.to_string(),
            detail,
        })
    }

    /// Register a file fetched after the store was built, keyed like [`MediaStore::new`].
    ///
    /// Returns whether the name was already indexed.
    pub fn insert_path(&mut self, name: &str, path: PathBuf) -> bool {
        if name.is_empty() {
            return false;
        }
        let base = Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(name)
            .to_ascii_lowercase();
        let existed = self.by_name.contains_key(&base);
        self.by_name.insert(base, path);
        existed
    }
}

fn index_dir(dir: &Path, out: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            index_dir(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let name = name.to_ascii_lowercase();
            out.insert(name.clone(), path.clone());
            // Flattened media sets (e.g. a directory of backgrounds named
            // `apdata__media__bg-x.jpg`) are indexed by their flattened base name
            // as well, so `apdata/media/bg-x.jpg` still resolves.
            if let Some(flattened) = name.rsplit("__").next() {
                if flattened != name {
                    out.insert(flattened.to_string(), path);
                }
            }
        }
    }
}

/// Where a prop blob came from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PropBackend {
    /// A file whose name begins with the decimal asset id.
    File(PathBuf),
    /// A record in a `.prp` roster, read on demand.
    Roster {
        path: PathBuf,
        offset: u64,
        len: u64,
    },
    /// A blob handed over by a live client (asset transfer), held in memory.
    Memory(std::sync::Arc<Vec<u8>>),
}

/// A store of prop blobs keyed by 32-bit asset id.
///
/// Built from any mix of prop directories and `.prp` rosters. The roster is
/// indexed, not loaded: a 177 MB `pserver.prp` costs a few MB of index and only
/// the blobs actually drawn are read.
#[derive(Debug, Default)]
pub struct PropStore {
    blobs: HashMap<u32, PropBackend>,
    entries: Vec<(u32, PropBackend)>,
}

impl PropStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        PropStore::default()
    }

    /// Index every `*.bin` / `*.ark` prop blob in `dir`, keyed by the leading
    /// decimal id in the file name.
    pub fn add_directory(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.add_directory(&path);
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(id) = leading_u32(stem) else {
                continue;
            };
            self.insert(id, PropBackend::File(path.clone()));
        }
    }

    /// Index a `.prp` roster. Returns the number of records indexed.
    pub fn add_roster(&mut self, path: &Path) -> Result<usize, RenderError> {
        let bytes = std::fs::read(path)?;
        let count = self.add_roster_bytes(path, &bytes)?;
        Ok(count)
    }

    fn add_roster_bytes(&mut self, path: &Path, bytes: &[u8]) -> Result<usize, RenderError> {
        let read_u32 = |at: usize| -> Option<u32> {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let (Some(data_offset), Some(map_offset)) = (read_u32(0), read_u32(8)) else {
            return Err(RenderError::NoPayload(format!(
                "{} is too short to be a .prp roster",
                path.display()
            )));
        };
        let data_offset = u64::from(data_offset);
        let Some(n_assets) = read_u32(map_offset as usize + 4) else {
            return Err(RenderError::NoPayload(format!(
                "{}: asset map at {map_offset} is out of range",
                path.display()
            )));
        };
        let Some(recs_offset) = read_u32(map_offset as usize + 16) else {
            return Err(RenderError::NoPayload(format!(
                "{}: record list offset missing",
                path.display()
            )));
        };
        let first = (map_offset as usize).saturating_add(recs_offset as usize);
        let mut indexed = 0usize;
        for i in 0..n_assets as usize {
            let at = first.saturating_add(i.saturating_mul(32));
            let (Some(id), Some(data_rel), Some(size)) =
                (read_u32(at), read_u32(at + 8), read_u32(at + 12))
            else {
                break;
            };
            if size < 12 {
                continue;
            }
            let offset = data_offset.saturating_add(u64::from(data_rel));
            if offset.saturating_add(u64::from(size)) > bytes.len() as u64 {
                continue;
            }
            self.insert(
                id,
                PropBackend::Roster {
                    path: path.to_path_buf(),
                    offset,
                    len: u64::from(size),
                },
            );
            indexed += 1;
        }
        Ok(indexed)
    }

    fn insert(&mut self, id: u32, backend: PropBackend) {
        self.entries.push((id, backend.clone()));
        self.blobs.insert(id, backend);
    }

    /// Number of prop ids in the store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    /// Whether the store holds no props.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }

    /// Whether an id is present.
    #[must_use]
    pub fn contains(&self, id: u32) -> bool {
        self.blobs.contains_key(&id)
    }

    /// A sorted list of `(id, backend-file)` pairs, used by the CLI's `--list`.
    #[must_use]
    pub fn ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .entries
            .iter()
            .map(|(id, _)| *id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Read the raw blob for `id`, or `None` when it is not in the store.
    ///
    /// A read failure (a roster that vanished under us) is also `None`: the
    /// caller reports a missing prop rather than aborting the frame.
    #[must_use]
    pub fn blob(&self, id: u32) -> Option<Vec<u8>> {
        let backend = self.blobs.get(&id)?;
        match backend {
            PropBackend::File(path) => std::fs::read(path).ok(),
            PropBackend::Roster { path, offset, len } => read_roster_blob(path, *offset, *len),
            PropBackend::Memory(bytes) => Some(bytes.as_ref().clone()),
        }
    }

    /// Insert a prop blob handed over live, replacing any existing entry for `id`.
    ///
    /// Returns whether an entry already existed. Used by clients that receive
    /// props over the wire rather than from a directory or roster.
    pub fn insert_blob(&mut self, id: u32, blob: Vec<u8>) -> bool {
        let replaced = self.blobs.contains_key(&id);
        self.insert(id, PropBackend::Memory(std::sync::Arc::new(blob)));
        replaced
    }

    /// Decode `id` into RGBA plus the header facts the renderer needs, recording
    /// a note when it is missing or bad.
    ///
    /// This is the one place the "a missing prop is a placeholder, not a crash"
    /// policy is enforced.
    pub fn prop_or_placeholder(&self, id: u32, notes: &mut Vec<AssetNote>) -> DecodedProp {
        let Some(bytes) = self.blob(id) else {
            notes.push(AssetNote::MissingProp { id });
            return DecodedProp::placeholder(id);
        };
        match decode(&bytes) {
            Ok(prop) => DecodedProp {
                image: prop.image,
                alpha: if prop.header.is_ghost() { 0.5 } else { 1.0 },
                h_offset: prop.header.h_offset,
                v_offset: prop.header.v_offset,
            },
            Err(err) => {
                notes.push(AssetNote::BadProp {
                    id,
                    detail: err.to_string(),
                });
                DecodedProp::placeholder(id)
            }
        }
    }
}

/// A prop resolved from the store, with the header facts the renderer needs.
#[derive(Debug, Clone)]
pub struct DecodedProp {
    /// The decoded RGBA image.
    pub image: PropImage,
    /// `0.5` for a ghost prop, else `1.0` — from the prop header's `GHOST` bit.
    pub alpha: f64,
    /// Horizontal origin offset from the prop header.
    pub h_offset: i16,
    /// Vertical origin offset from the prop header.
    pub v_offset: i16,
}

impl DecodedProp {
    /// The placeholder used when a prop cannot be resolved. Offsets are zero
    /// because there is no header to read.
    #[must_use]
    pub fn placeholder(_id: u32) -> Self {
        DecodedProp {
            image: placeholder(),
            alpha: 1.0,
            h_offset: 0,
            v_offset: 0,
        }
    }
}

/// Read a byte range out of a `.prp` roster without holding the file open.
fn read_roster_blob(path: &Path, offset: u64, len: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; usize::try_from(len).ok()?];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// The leading decimal integer of a file stem, if there is one.
fn leading_u32(stem: &str) -> Option<u32> {
    let digits: String = stem.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// The magenta/black checker placeholder drawn for a prop that could not be
/// resolved, so a gap is visible rather than invisible.
///
/// It is 44×44 — the size of every Type 0 prop in the corpus — and uses a
/// saturated colour that cannot be mistaken for room art.
#[must_use]
pub fn placeholder() -> PropImage {
    const SIZE: u32 = 44;
    const CELL: u32 = 4;
    let mut bytes = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let on = ((x / CELL) + (y / CELL)) % 2 == 0;
            bytes.extend_from_slice(if on {
                &[255, 0, 255, 200]
            } else {
                &[0, 0, 0, 200]
            });
        }
    }
    PropImage::from_rgba(SIZE, SIZE, bytes).unwrap_or_else(|_| PropImage::transparent(SIZE, SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("palace-render-assets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn leading_id_parses_the_corpus_file_names() {
        assert_eq!(leading_u32("1001438438_unnamed"), Some(1001438438));
        assert_eq!(leading_u32("976933367"), Some(976933367));
        assert_eq!(leading_u32("unnamed"), None);
        assert_eq!(leading_u32(""), None);
        assert_eq!(leading_u32("99999999999999999999"), None);
    }

    #[test]
    fn a_directory_is_indexed_by_asset_id_and_blobs_are_read_on_demand() {
        let dir = tempdir("dir");
        // A minimal 8-bit prop: 4x1, all palette index 0.
        let blob = [
            4u8, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x04, 0x01, 0x01, 0x01, 0x01,
        ];
        let mut f = std::fs::File::create(dir.join("4242_unnamed.bin")).expect("write");
        f.write_all(&blob).expect("write blob");
        let mut store = PropStore::new();
        store.add_directory(&dir);
        assert!(store.contains(4242));
        assert!(!store.contains(1));
        assert_eq!(store.blob(4242).as_deref(), Some(blob.as_slice()));
        assert!(store.blob(9999).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_prop_becomes_a_placeholder_and_a_note() {
        let store = PropStore::new();
        let mut notes = Vec::new();
        let decoded = store.prop_or_placeholder(7, &mut notes);
        assert_eq!((decoded.image.width(), decoded.image.height()), (44, 44));
        assert_eq!(decoded.alpha, 1.0);
        assert_eq!((decoded.h_offset, decoded.v_offset), (0, 0));
        assert_eq!(notes, vec![AssetNote::MissingProp { id: 7 }]);
    }

    #[test]
    fn a_real_prop_carries_its_ghost_alpha_and_offsets() {
        // A valid prop built with the S20 encoder rather than hand-written bytes,
        // so the header and payload cannot disagree.
        let image = PropImage::from_rgba(44, 44, vec![200; 44 * 44 * 4]).expect("image");
        let blob = palace_prop::encode_s20_blob(
            &image,
            7,
            7,
            palace_prop::FLAG_GHOST | palace_prop::FLAG_HEAD,
        )
        .expect("s20 encode");
        let dir = tempdir("ghost");
        std::fs::write(dir.join("55_unnamed.bin"), &blob).expect("write");
        let mut store = PropStore::new();
        store.add_directory(&dir);
        let mut notes = Vec::new();
        let decoded = store.prop_or_placeholder(55, &mut notes);
        assert!(notes.is_empty(), "a valid prop produces no note: {notes:?}");
        assert_eq!((decoded.h_offset, decoded.v_offset), (7, 7));
        assert_eq!(decoded.alpha, 0.5, "GHOST bit halves the drawn alpha");
        assert_eq!((decoded.image.width(), decoded.image.height()), (44, 44));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_roster_is_indexed_by_id_and_reads_blobs_back() {
        let dir = tempdir("roster");
        let prop = [
            4u8, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x04, 0x01, 0x01, 0x01, 0x01,
        ];
        let mut file_bytes = vec![0u8; 16];
        // dataOffset = 16
        file_bytes[0..4].copy_from_slice(&16u32.to_le_bytes());
        // mapOffset follows the data.
        let data_offset = 16usize;
        let map_offset = data_offset + prop.len();
        file_bytes[8..12].copy_from_slice(&(map_offset as u32).to_le_bytes());
        file_bytes.extend_from_slice(&prop);
        // map descriptor: nAssets at +4, recsOffset at +16 (records follow it)
        let mut map = vec![0u8; 32];
        map[4..8].copy_from_slice(&1u32.to_le_bytes());
        map[16..20].copy_from_slice(&32u32.to_le_bytes());
        file_bytes.extend_from_slice(&map);
        let mut record = vec![0u8; 32];
        record[0..4].copy_from_slice(&1234u32.to_le_bytes());
        record[8..12].copy_from_slice(&0u32.to_le_bytes());
        record[12..16].copy_from_slice(&(prop.len() as u32).to_le_bytes());
        file_bytes.extend_from_slice(&record);
        let path = dir.join("test.prp");
        std::fs::write(&path, &file_bytes).expect("write roster");

        let mut store = PropStore::new();
        let n = store.add_roster(&path).expect("roster parses");
        assert_eq!(n, 1);
        assert!(store.contains(1234));
        assert_eq!(store.blob(1234).as_deref(), Some(prop.as_slice()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_too_short_roster_is_an_error_not_a_panic() {
        let dir = tempdir("short");
        let path = dir.join("bad.prp");
        std::fs::write(&path, b"not a roster").expect("write");
        let mut store = PropStore::new();
        assert!(store.add_roster(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_store_resolves_names_and_falls_back_on_extension() {
        let dir = tempdir("media");
        std::fs::write(dir.join("Backdrop.png"), b"x").expect("write");
        let store = MediaStore::new(std::slice::from_ref(&dir));
        assert!(store.resolve("Backdrop.png").is_some());
        assert!(store.resolve("backdrop.PNG").is_some(), "case-insensitive");
        assert!(
            store.resolve("Backdrop.gif").is_some(),
            "gif falls back to png"
        );
        assert!(store.resolve("nope.gif").is_none());
        assert!(store.resolve("").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decoding_garbage_is_an_error_not_a_panic() {
        assert!(decode_image_bytes(&[1, 2, 3, 4, 5]).is_err());
        assert!(decode_prop_blob(&[1, 2, 3]).is_err());
    }
}

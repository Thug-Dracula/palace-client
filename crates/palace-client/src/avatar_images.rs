//! PNG-encoded avatar artwork, shared with the URI-scheme handler.
//!
//! The webview draws each avatar piece as an image and fetches it over the
//! `palace://` scheme. Two pieces of art need a stable URL:
//!
//! * a **worn prop**, identified by the art id the roster carries; the bytes
//!   must come from the runtime's own decoded prop store, because a prop the
//!   client received over the wire need not be in the user's local bag (the
//!   older `palace://prop/<id>` catalog route can 404 for it);
//! * a **built-in face cell**, identified by a `(face, color)` pair.
//!
//! Encoding a PNG is not free, so both are cached here and encoded at most once.
//! The cache lives next to [`crate::FrameStore`] in spirit: a small shared
//! structure the runtime fills and the URI handler reads.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use palace_prop::PropImage;
use palace_render::{smiley_cell, COLOR_VARIANTS, FACE_VARIANTS};
use palace_wire::messages::AvatarHash;

use crate::type1::{image_dimensions, Type1Format};

/// A shared cache of encoded avatar art.
///
/// Worn props are keyed by their art id; face cells by their `(face, color)`
/// pair. Both maps are filled lazily and never re-encoded for a key already
/// present.
///
/// Type 1 avatars are cached too, keyed by their content hash: either the raw
/// server bytes (`type1`), a URL the server sent instead (`type1_urls`), or both.
#[derive(Debug, Default)]
pub struct AvatarImageStore {
    props: Mutex<HashMap<u32, Vec<u8>>>,
    faces: Mutex<HashMap<(i16, i16), Vec<u8>>>,
    type1: Mutex<HashMap<AvatarHash, Vec<u8>>>,
    type1_urls: Mutex<HashMap<AvatarHash, String>>,
    type1_dims: Mutex<HashMap<AvatarHash, (u32, u32)>>,
}

impl AvatarImageStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        AvatarImageStore::default()
    }

    /// Encode `image` as PNG and cache it under `id`, unless `id` is already
    /// cached. Returns the cached (or freshly encoded) bytes.
    ///
    /// The roster builder calls this once per referenced prop; the cache check
    /// is what stops a re-encode on every roster emit.
    pub fn cache_prop(&self, id: u32, image: &PropImage) -> Option<Vec<u8>> {
        if let Some(png) = lock(&self.props).get(&id) {
            return Some(png.clone());
        }
        let png = image.to_png_bytes().ok()?;
        let mut props = lock(&self.props);
        Some(props.entry(id).or_insert(png).clone())
    }

    /// The cached PNG for a worn prop, or `None` when the runtime has never
    /// encoded it.
    #[must_use]
    pub fn prop_png(&self, id: u32) -> Option<Vec<u8>> {
        lock(&self.props).get(&id).cloned()
    }

    /// Whether a worn prop has been cached.
    #[must_use]
    pub fn has_prop(&self, id: u32) -> bool {
        lock(&self.props).contains_key(&id)
    }

    /// The PNG for one built-in face cell, encoded and cached on first ask.
    ///
    /// `None` when `(face, color)` is outside the sheet: the route turns that
    /// into a 404 rather than serving a clamped cell the caller did not ask for.
    #[must_use]
    pub fn face_cell_png(&self, face: i16, color: i16) -> Option<Vec<u8>> {
        if let Some(png) = lock(&self.faces).get(&(face, color)) {
            return Some(png.clone());
        }
        let png = encode_face_cell(face, color)?;
        let mut faces = lock(&self.faces);
        Some(faces.entry((face, color)).or_insert(png).clone())
    }

    /// Whether a face cell has been cached.
    #[must_use]
    pub fn has_face_cell(&self, face: i16, color: i16) -> bool {
        lock(&self.faces).contains_key(&(face, color))
    }

    /// Cache a Type 1 avatar's server bytes under its content hash.
    ///
    /// The dimensions are read from the image header so a later roster can size
    /// the sprite without decoding the pixels; an unreadable header stores no
    /// dimensions and the roster falls back to the server's cap. Returns the
    /// dimensions when they were read.
    pub fn cache_type1(&self, hash: AvatarHash, bytes: &[u8]) -> Option<(u32, u32)> {
        let dims = Type1Format::sniff(bytes).and_then(|format| image_dimensions(bytes, format));
        let mut store = lock(&self.type1);
        store.insert(hash, bytes.to_vec());
        drop(store);
        if let Some(size) = dims {
            lock(&self.type1_dims).insert(hash, size);
        }
        dims
    }

    /// The bytes and MIME type for a cached Type 1 avatar.
    #[must_use]
    pub fn type1(&self, hash: &AvatarHash) -> Option<(Vec<u8>, &'static str)> {
        let bytes = lock(&self.type1).get(hash).cloned()?;
        let mime = Type1Format::sniff(&bytes).map_or("application/octet-stream", Type1Format::mime);
        Some((bytes, mime))
    }

    /// Whether a Type 1 avatar's bytes are cached.
    #[must_use]
    pub fn has_type1(&self, hash: &AvatarHash) -> bool {
        lock(&self.type1).contains_key(hash)
    }

    /// Remember a Type 1 avatar's URL, when the server sent a URL instead of bytes.
    pub fn cache_type1_url(&self, hash: AvatarHash, url: String) {
        lock(&self.type1_urls).insert(hash, url);
    }

    /// The URL for a Type 1 avatar, when one was recorded.
    #[must_use]
    pub fn type1_url(&self, hash: &AvatarHash) -> Option<String> {
        lock(&self.type1_urls).get(hash).cloned()
    }

    /// The cached dimensions for a Type 1 avatar, when they were read.
    #[must_use]
    pub fn type1_dims(&self, hash: &AvatarHash) -> Option<(u32, u32)> {
        lock(&self.type1_dims).get(hash).copied()
    }
}

/// Encode one built-in face cell as a PNG.
///
/// Returns `None` when `face` or `color` is outside the sheet. The renderer's
/// [`smiley_cell`] clamps instead of failing, so the range check lives here to
/// keep an out-of-range URL a clean 404.
#[must_use]
pub fn encode_face_cell(face: i16, color: i16) -> Option<Vec<u8>> {
    if !(0..FACE_VARIANTS).contains(&face) || !(0..COLOR_VARIANTS).contains(&color) {
        return None;
    }
    smiley_cell(face, color).to_png_bytes().ok()
}

/// Lock a mutex, recovering the inner value from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_face_cell_encodes_to_a_non_empty_png() {
        let store = AvatarImageStore::new();
        let png = store.face_cell_png(0, 0).expect("cell (0,0) is valid");
        assert_eq!(
            &png[..8],
            b"\x89PNG\r\n\x1a\n",
            "the route serves a real PNG"
        );
        assert!(!png.is_empty());
        assert!(
            store.has_face_cell(0, 0),
            "the cell is cached after the first ask"
        );
    }

    #[test]
    fn an_out_of_range_face_cell_is_absent() {
        let store = AvatarImageStore::new();
        assert!(
            store.face_cell_png(FACE_VARIANTS, 0).is_none(),
            "face at the variant count is out of range"
        );
        assert!(
            store.face_cell_png(0, COLOR_VARIANTS).is_none(),
            "colour at the variant count is out of range"
        );
        assert!(store.face_cell_png(-1, 0).is_none(), "negative face");
        assert!(store.face_cell_png(0, -1).is_none(), "negative colour");
        assert!(
            !store.has_face_cell(FACE_VARIANTS, 0),
            "a rejected pair must not be cached"
        );
        assert!(
            encode_face_cell(FACE_VARIANTS - 1, COLOR_VARIANTS - 1).is_some(),
            "the last cell on the sheet is valid"
        );
    }

    #[test]
    fn a_prop_is_encoded_once_and_served_from_cache_afterwards() {
        let store = AvatarImageStore::new();
        let image = PropImage::from_rgba(2, 2, vec![0x11; 2 * 2 * 4]).expect("a 2x2 RGBA image");
        let first = store.cache_prop(5, &image).expect("the first ask encodes");
        let second = store
            .cache_prop(5, &image)
            .expect("the second ask is cached");
        assert_eq!(first, second);
        assert_eq!(store.prop_png(5).as_deref(), Some(first.as_slice()));
        assert!(store.has_prop(5));
        assert!(store.prop_png(6).is_none(), "an id never cached is absent");
    }

    fn tiny_gif() -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x3B]);
        out
    }

    #[test]
    fn a_type1_avatar_is_cached_with_its_bytes_mime_and_dimensions() {
        let store = AvatarImageStore::new();
        let hash = AvatarHash::new([0x11; 20]);
        assert!(!store.has_type1(&hash));

        let dims = store.cache_type1(hash, &tiny_gif());
        assert_eq!(dims, Some((2, 3)));
        assert!(store.has_type1(&hash));
        assert_eq!(store.type1_dims(&hash), Some((2, 3)));

        let (bytes, mime) = store.type1(&hash).expect("the avatar is served");
        assert_eq!(bytes, tiny_gif());
        assert_eq!(mime, "image/gif");
    }

    #[test]
    fn a_type1_url_is_kept_when_the_server_sends_no_bytes() {
        let store = AvatarImageStore::new();
        let hash = AvatarHash::new([0x22; 20]);
        assert_eq!(store.type1(&hash), None);
        store.cache_type1_url(hash, "https://example.test/a.gif".to_string());
        assert_eq!(
            store.type1_url(&hash).as_deref(),
            Some("https://example.test/a.gif")
        );
        assert!(store.type1(&hash).is_none(), "a URL is not image bytes");
    }
}

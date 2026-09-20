//! The `palace://` URI scheme that serves the current frame as PNG.
//!
//! Frames never travel through a Tauri command: a `Vec<u8>` return is marshalled
//! into a JSON number array, which is orders of magnitude larger than the image
//! itself. The webview fetches one already-encoded PNG per frame version.

use std::sync::{Arc, Mutex};

use crate::bag::BagSnapshot;
use palace_client::{AvatarHash, AvatarImageStore, FrameStore};
use palace_prop::PropCatalog;
use tauri::http::{Request, Response, StatusCode};
use tauri::UriSchemeResponder;

/// A shared handle to whichever runtime's frame store is current.
#[derive(Clone, Default)]
pub struct FrameSlot {
    inner: Arc<Mutex<Option<Arc<FrameStore>>>>,
}

impl FrameSlot {
    /// Point the slot at a runtime's frame store.
    pub fn set(&self, store: Arc<FrameStore>) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = Some(store),
            Err(poisoned) => *poisoned.into_inner() = Some(store),
        }
    }

    /// The current frame store, if a runtime has been started.
    #[must_use]
    pub fn get(&self) -> Option<Arc<FrameStore>> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// A shared handle to the prop catalog loaded at startup.
#[derive(Clone, Default)]
pub struct CatalogSlot {
    inner: Arc<Mutex<Option<Arc<PropCatalog>>>>,
}

impl CatalogSlot {
    /// Point the slot at a loaded catalog.
    pub fn set(&self, catalog: Arc<PropCatalog>) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = Some(catalog),
            Err(poisoned) => *poisoned.into_inner() = Some(catalog),
        }
    }

    /// The loaded catalog, if the user has a readable prop bag.
    #[must_use]
    pub fn get(&self) -> Option<Arc<PropCatalog>> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// A shared handle to the runtime's encoded avatar art (worn props and face
/// cells), for the `avatar-prop/` and `face-cell/` routes.
#[derive(Clone, Default)]
pub struct AvatarImageSlot {
    inner: Arc<Mutex<Option<Arc<AvatarImageStore>>>>,
}

impl AvatarImageSlot {
    /// Point the slot at a runtime's avatar-image cache.
    pub fn set(&self, store: Arc<AvatarImageStore>) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = Some(store),
            Err(poisoned) => *poisoned.into_inner() = Some(store),
        }
    }

    /// The current avatar-image cache, if a runtime has been started.
    #[must_use]
    pub fn get(&self) -> Option<Arc<AvatarImageStore>> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// A shared handle to the bag catalog and shelf health loaded at startup.
///
/// Mirrors [`CatalogSlot`], but for the `.prp` bag rather than PalaceChat's
/// bundle. It is rebuilt after every bag mutation so the routes always serve
/// the current collection.
#[derive(Clone, Default)]
pub struct BagSlot {
    inner: Arc<Mutex<Option<Arc<BagSnapshot>>>>,
}

impl BagSlot {
    /// Point the slot at a freshly built bag snapshot.
    pub fn set(&self, snapshot: Arc<BagSnapshot>) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = Some(snapshot),
            Err(poisoned) => *poisoned.into_inner() = Some(snapshot),
        }
    }

    /// The current snapshot, if the bag folder has been read.
    #[must_use]
    pub fn get(&self) -> Option<Arc<BagSnapshot>> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// Route a `palace://localhost/<path>` request.
pub fn handle(
    slot: &FrameSlot,
    catalog: &CatalogSlot,
    images: &AvatarImageSlot,
    bag: &BagSlot,
    request: &Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let path = request.uri().path().trim_start_matches('/').to_string();
    if path == "frame" {
        handle_frame(slot, responder);
        return;
    }
    responder.respond(resolve(catalog, images, bag, &path));
}

/// Resolve every route except `frame` to a response.
///
/// `frame` is handled separately: its store read happens on its own worker
/// thread. Keeping this part pure makes the 404 paths testable without a live
/// runtime or a responder.
fn resolve(
    catalog: &CatalogSlot,
    images: &AvatarImageSlot,
    bag: &BagSlot,
    path: &str,
) -> Response<Vec<u8>> {
    match path {
        "faces" => png(palace_render::face_sheet_png().to_vec()),
        "faces.json" => json(palace_render::face_grid_json()),
        "props.json" => json(handle_props_json(catalog)),
        "bag-catalog.json" => json(handle_bag_catalog_json(bag)),
        "bag-shelves.json" => json(handle_bag_shelves_json(bag)),
        _ => {
            if let Some(id) = path.strip_prefix("prop/") {
                handle_prop(catalog, id)
            } else if let Some(rest) = path.strip_prefix("bag-prop/") {
                handle_bag_prop(bag, rest)
            } else if let Some(rest) = path.strip_prefix("bag-shelf/") {
                handle_bag_shelf(bag, rest)
            } else if let Some(id) = path.strip_prefix("avatar-prop/") {
                handle_avatar_prop(images, id)
            } else if let Some(hash) = path.strip_prefix("type1-avatar/") {
                handle_type1_avatar(images, hash)
            } else if let Some(pair) = path.strip_prefix("face-cell/") {
                handle_face_cell(images, pair)
            } else {
                status(StatusCode::NOT_FOUND)
            }
        }
    }
}

/// The whole catalog as JSON. An absent bag serves a valid empty catalog rather
/// than an error, so the picker renders an empty state instead of failing.
fn handle_props_json(catalog: &CatalogSlot) -> String {
    match catalog.get() {
        Some(catalog) => catalog.catalog_json(),
        None => "{\"props\":[]}".to_string(),
    }
}

/// The bag catalog as JSON. An absent bag is a valid empty catalog, never an
/// error and never the PalaceChat bundle.
fn handle_bag_catalog_json(bag: &BagSlot) -> String {
    match bag.get() {
        Some(snapshot) => snapshot.catalog_json(),
        None => "{\"props\":[]}".to_string(),
    }
}

/// Every shelf's health as JSON; an absent bag is an empty list.
fn handle_bag_shelves_json(bag: &BagSlot) -> String {
    match bag.get() {
        Some(snapshot) => {
            serde_json::to_string(&snapshot.shelves).unwrap_or_else(|_| "[]".to_string())
        }
        None => "[]".to_string(),
    }
}

/// One bag prop's thumbnail PNG.
///
/// The route is `bag-prop/{collection}/{id}/{crc}`. The identity is the
/// `(id, crc)` pair: the collection segment must be present but is
/// informational, because the catalog deduplicates and keys thumbnails by
/// identity. A prop the bag does not list — including one that exists only in
/// the PalaceChat cache — is a 404, so this route can never serve cache art.
fn handle_bag_prop(bag: &BagSlot, rest: &str) -> Response<Vec<u8>> {
    let mut segments = rest.split('/');
    let collection = segments.next();
    let id = segments.next().and_then(|value| value.parse::<u32>().ok());
    let crc = segments.next().and_then(|value| value.parse::<u32>().ok());
    let trailing = segments.next().is_some();
    if trailing || collection.is_none_or(str::is_empty) {
        return status(StatusCode::NOT_FOUND);
    }
    let (Some(id), Some(crc)) = (id, crc) else {
        return status(StatusCode::NOT_FOUND);
    };
    match bag
        .get()
        .and_then(|snapshot| snapshot.catalog.thumbnail_png(id, crc))
    {
        Some(bytes) => png(bytes),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// One shelf's health as JSON, addressed by its index in the snapshot list.
fn handle_bag_shelf(bag: &BagSlot, rest: &str) -> Response<Vec<u8>> {
    let Some(index) = rest.parse::<usize>().ok() else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(snapshot) = bag.get() else {
        return status(StatusCode::NOT_FOUND);
    };
    match snapshot.shelves.get(index) {
        Some(shelf) => json(serde_json::to_string(shelf).unwrap_or_else(|_| "{}".to_string())),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// One prop's thumbnail PNG, or 404 for an unknown id or an undecodable prop.
fn handle_prop(catalog: &CatalogSlot, id: &str) -> Response<Vec<u8>> {
    let Some(id) = id.parse::<u32>().ok() else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(catalog) = catalog.get() else {
        return status(StatusCode::NOT_FOUND);
    };
    match catalog.thumbnail_png(id) {
        Some(bytes) => png(bytes),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// A worn prop's decoded image, as PNG, from the runtime's own cache. 404 for an
/// unknown id or before a runtime has encoded it.
fn handle_avatar_prop(images: &AvatarImageSlot, id: &str) -> Response<Vec<u8>> {
    let Some(id) = id.parse::<u32>().ok() else {
        return status(StatusCode::NOT_FOUND);
    };
    match images.get().and_then(|store| store.prop_png(id)) {
        Some(bytes) => png(bytes),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// One Type 1 avatar, addressed by its 20-byte content hash as hex.
///
/// A Type 1 avatar is a single server-hosted image of any permitted format, so
/// the response carries the format's real MIME type rather than always PNG. An
/// unknown or malformed hash is a 404. When the runtime only learned a URL (the
/// server answered `sAva` with a link, not bytes) the route redirects to it.
fn handle_type1_avatar(images: &AvatarImageSlot, hash: &str) -> Response<Vec<u8>> {
    let Some(hash) = AvatarHash::from_hex(hash) else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(store) = images.get() else {
        return status(StatusCode::NOT_FOUND);
    };
    if let Some((bytes, mime)) = store.type1(&hash) {
        return image(bytes, mime);
    }
    match store.type1_url(&hash) {
        Some(url) => Response::builder()
            .status(StatusCode::FOUND)
            .header("Location", url)
            .header("Cache-Control", "no-store")
            .header("Access-Control-Allow-Origin", "*")
            .body(Vec::new())
            .unwrap_or_else(|_| Response::new(Vec::new())),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// One built-in face cell as PNG, encoded and cached by the runtime. 404 for an
/// out-of-range pair or a malformed path.
fn handle_face_cell(images: &AvatarImageSlot, pair: &str) -> Response<Vec<u8>> {
    let mut segments = pair.split('/');
    let face = segments.next().and_then(|value| value.parse::<i16>().ok());
    let color = segments.next().and_then(|value| value.parse::<i16>().ok());
    if segments.next().is_some() {
        return status(StatusCode::NOT_FOUND);
    }
    let (Some(face), Some(color)) = (face, color) else {
        return status(StatusCode::NOT_FOUND);
    };
    match images
        .get()
        .and_then(|store| store.face_cell_png(face, color))
    {
        Some(bytes) => png(bytes),
        None => status(StatusCode::NOT_FOUND),
    }
}

fn handle_frame(slot: &FrameSlot, responder: UriSchemeResponder) {
    let slot = slot.clone();
    std::thread::spawn(move || {
        let Some(store) = slot.get() else {
            responder.respond(status(StatusCode::NO_CONTENT));
            return;
        };
        match store.png() {
            Some(bytes) => responder.respond(png(bytes)),
            None => responder.respond(status(StatusCode::NO_CONTENT)),
        }
    });
}

fn png(bytes: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/png")
        .header("Cache-Control", "no-store")
        .header("Access-Control-Allow-Origin", "*")
        .body(bytes)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn image(bytes: Vec<u8>, mime: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("Access-Control-Allow-Origin", "*")
        .body(bytes)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn json(body: String) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .header("Access-Control-Allow-Origin", "*")
        .body(body.into_bytes())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header("Access-Control-Allow-Origin", "*")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bag::BagShelfInfo;
    use palace_prop::bag_catalog::{BagCatalog, BagCollection};
    use palace_prop::prp::{AssetRec, PropHeader, PropRecord, Roster};

    const BLOB_HEADER: [u8; 12] = [4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    fn slots() -> (CatalogSlot, AvatarImageSlot, BagSlot) {
        (
            CatalogSlot::default(),
            AvatarImageSlot::default(),
            BagSlot::default(),
        )
    }

    fn content_type(response: &Response<Vec<u8>>) -> Option<&str> {
        response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
    }

    fn body_text(response: &Response<Vec<u8>>) -> String {
        String::from_utf8_lossy(response.body()).into_owned()
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/palace-prop/fixtures/prp")
            .join(name)
    }

    fn roster_with_prop(id: i32) -> (Roster, u32) {
        let empty = std::fs::read(fixture("empty.prp")).expect("empty.prp is checked in");
        let mut roster = Roster::parse(&empty).expect("empty.prp parses");
        let mut blob = BLOB_HEADER.to_vec();
        blob.extend_from_slice(&[0x04, 1, 1, 1, 1]);
        let crc = palace_prop::asset_crc(&blob[12..]);
        let header = PropHeader::parse(&blob).ok();
        let encoding = header.map(PropHeader::encoding);
        roster.add_prop(PropRecord {
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
            blob,
            name: Some("Dot".to_string()),
        });
        (roster, crc)
    }

    fn bag_with(roster: Option<Roster>, shelves: Vec<BagShelfInfo>) -> BagSlot {
        let collections: Vec<BagCollection> = roster
            .into_iter()
            .map(|roster| BagCollection::new("My Bag", roster))
            .collect();
        let snapshot = BagSnapshot::new(Arc::new(BagCatalog::new(collections)), shelves);
        let slot = BagSlot::default();
        slot.set(Arc::new(snapshot));
        slot
    }

    #[test]
    fn an_unknown_path_is_not_found() {
        let (catalog, images, bag) = slots();
        assert_eq!(
            resolve(&catalog, &images, &bag, "nope").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn an_unknown_avatar_prop_is_not_found() {
        let (catalog, images, bag) = slots();
        assert_eq!(
            resolve(&catalog, &images, &bag, "avatar-prop/123").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            resolve(&catalog, &images, &bag, "avatar-prop/not-a-number").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_cached_avatar_prop_is_served_as_png() {
        let (catalog, images, bag) = slots();
        let store = Arc::new(AvatarImageStore::new());
        let image = palace_prop::PropImage::from_rgba(2, 2, vec![0x22; 16]).expect("a 2x2 image");
        store.cache_prop(42, &image).expect("the prop encodes");
        images.set(store);

        let response = resolve(&catalog, &images, &bag, "avatar-prop/42");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("image/png"));
    }

    #[test]
    fn an_out_of_range_face_cell_is_not_found() {
        let (catalog, images, bag) = slots();
        images.set(Arc::new(AvatarImageStore::new()));
        assert_eq!(
            resolve(&catalog, &images, &bag, "face-cell/13/0").status(),
            StatusCode::NOT_FOUND,
            "face 13 is past the sheet"
        );
        assert_eq!(
            resolve(&catalog, &images, &bag, "face-cell/0/16").status(),
            StatusCode::NOT_FOUND,
            "colour 16 is past the sheet"
        );
        assert_eq!(
            resolve(&catalog, &images, &bag, "face-cell/0").status(),
            StatusCode::NOT_FOUND,
            "a missing colour segment is malformed"
        );
    }

    #[test]
    fn a_valid_face_cell_is_served_as_png() {
        let (catalog, images, bag) = slots();
        images.set(Arc::new(AvatarImageStore::new()));
        let response = resolve(&catalog, &images, &bag, "face-cell/0/0");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("image/png"));
    }

    #[test]
    fn an_empty_bag_serves_a_valid_empty_catalog() {
        let (catalog, images, bag) = slots();
        let response = resolve(&catalog, &images, &bag, "bag-catalog.json");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("application/json"));
        assert_eq!(body_text(&response), "{\"props\":[]}");
    }

    #[test]
    fn a_bag_prop_is_served_as_png() {
        let (roster, crc) = roster_with_prop(7);
        let bag = bag_with(Some(roster), Vec::new());
        let (catalog, images, _) = slots();

        let path = format!("bag-prop/My Bag/7/{crc}");
        let response = resolve(&catalog, &images, &bag, &path);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("image/png"));
    }

    #[test]
    fn a_prop_the_bag_does_not_list_is_not_found() {
        let (roster, _) = roster_with_prop(7);
        let bag = bag_with(Some(roster), Vec::new());
        let (catalog, images, _) = slots();

        let response = resolve(&catalog, &images, &bag, "bag-prop/My Bag/999999/1");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "a cache-only or unknown id must never be served through a bag route"
        );
    }

    #[test]
    fn a_malformed_bag_prop_path_is_not_found() {
        let (roster, crc) = roster_with_prop(7);
        let bag = bag_with(Some(roster), Vec::new());
        let (catalog, images, _) = slots();

        for path in [
            "bag-prop/My Bag/not-a-number/1".to_string(),
            "bag-prop/My Bag/7".to_string(),
            "bag-prop//7/1".to_string(),
            format!("bag-prop/My Bag/7/{crc}/extra"),
        ] {
            assert_eq!(
                resolve(&catalog, &images, &bag, &path).status(),
                StatusCode::NOT_FOUND,
                "{path} must be malformed"
            );
        }
    }

    #[test]
    fn a_shelf_is_served_by_index_and_an_unknown_index_is_not_found() {
        let shelf = BagShelfInfo {
            name: "Old Gems".to_string(),
            path: "/bags/shelves/Old Gems.prp".to_string(),
            status: "healthy".to_string(),
            records: 12,
            named: 12,
            undecodable: 0,
            dropped: 0,
            error: None,
        };
        let bag = bag_with(None, vec![shelf]);
        let (catalog, images, _) = slots();

        let response = resolve(&catalog, &images, &bag, "bag-shelf/0");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("application/json"));
        assert!(body_text(&response).contains("\"Old Gems\""));

        assert_eq!(
            resolve(&catalog, &images, &bag, "bag-shelf/9").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            resolve(&catalog, &images, &bag, "bag-shelf/not-a-number").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn bag_shelves_json_is_empty_without_a_bag() {
        let (catalog, images, bag) = slots();
        let response = resolve(&catalog, &images, &bag, "bag-shelves.json");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(&response), "[]");
    }

    #[test]
    fn a_cached_type1_avatar_is_served_with_its_own_mime() {
        let (catalog, images, bag) = slots();
        let store = Arc::new(AvatarImageStore::new());
        let hash = AvatarHash::new([0x33; 20]);
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0, 0x3B]);
        store.cache_type1(hash, &gif);
        images.set(store);

        let path = format!("type1-avatar/{}", hash.to_hex());
        let response = resolve(&catalog, &images, &bag, &path);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            content_type(&response),
            Some("image/gif"),
            "a GIF is served as a GIF, not re-labelled PNG"
        );
        assert_eq!(response.body(), &gif);
    }

    #[test]
    fn an_unknown_or_malformed_type1_avatar_is_not_found() {
        let (catalog, images, bag) = slots();
        images.set(Arc::new(AvatarImageStore::new()));
        for path in [
            format!("type1-avatar/{}", "00".repeat(20)),
            "type1-avatar/nothex".to_string(),
            format!("type1-avatar/{}", "ab".repeat(19)),
            "type1-avatar/".to_string(),
        ] {
            assert_eq!(
                resolve(&catalog, &images, &bag, &path).status(),
                StatusCode::NOT_FOUND,
                "{path} must not resolve"
            );
        }
    }

    #[test]
    fn a_url_only_type1_avatar_redirects_to_the_server_url() {
        let (catalog, images, bag) = slots();
        let store = Arc::new(AvatarImageStore::new());
        let hash = AvatarHash::new([0x44; 20]);
        store.cache_type1_url(hash, "https://example.test/a.gif".to_string());
        images.set(store);

        let path = format!("type1-avatar/{}", hash.to_hex());
        let response = resolve(&catalog, &images, &bag, &path);
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get("Location")
                .and_then(|value| value.to_str().ok()),
            Some("https://example.test/a.gif")
        );
    }
}

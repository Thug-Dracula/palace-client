//! The `palace://` URI scheme that serves the current frame as PNG.
//!
//! Frames never travel through a Tauri command: a `Vec<u8>` return is marshalled
//! into a JSON number array, which is orders of magnitude larger than the image
//! itself. The webview fetches one already-encoded PNG per frame version.

use std::sync::{Arc, Mutex};

use palace_client::{AvatarImageStore, FrameStore};
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

/// Route a `palace://localhost/<path>` request.
pub fn handle(
    slot: &FrameSlot,
    catalog: &CatalogSlot,
    images: &AvatarImageSlot,
    request: &Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let path = request.uri().path().trim_start_matches('/').to_string();
    if path == "frame" {
        handle_frame(slot, responder);
        return;
    }
    responder.respond(resolve(catalog, images, &path));
}

/// Resolve every route except `frame` to a response.
///
/// `frame` is handled separately: its store read happens on its own worker
/// thread. Keeping this part pure makes the 404 paths testable without a live
/// runtime or a responder.
fn resolve(catalog: &CatalogSlot, images: &AvatarImageSlot, path: &str) -> Response<Vec<u8>> {
    match path {
        "faces" => png(palace_render::face_sheet_png().to_vec()),
        "faces.json" => json(palace_render::face_grid_json()),
        "props.json" => json(handle_props_json(catalog)),
        _ => {
            if let Some(id) = path.strip_prefix("prop/") {
                handle_prop(catalog, id)
            } else if let Some(id) = path.strip_prefix("avatar-prop/") {
                handle_avatar_prop(images, id)
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

    fn slots() -> (CatalogSlot, AvatarImageSlot) {
        (CatalogSlot::default(), AvatarImageSlot::default())
    }

    fn content_type(response: &Response<Vec<u8>>) -> Option<&str> {
        response
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn an_unknown_path_is_not_found() {
        let (catalog, images) = slots();
        assert_eq!(
            resolve(&catalog, &images, "nope").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn an_unknown_avatar_prop_is_not_found() {
        let (catalog, images) = slots();
        assert_eq!(
            resolve(&catalog, &images, "avatar-prop/123").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            resolve(&catalog, &images, "avatar-prop/not-a-number").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_cached_avatar_prop_is_served_as_png() {
        let (catalog, images) = slots();
        let store = Arc::new(AvatarImageStore::new());
        let image = palace_prop::PropImage::from_rgba(2, 2, vec![0x22; 16]).expect("a 2x2 image");
        store.cache_prop(42, &image).expect("the prop encodes");
        images.set(store);

        let response = resolve(&catalog, &images, "avatar-prop/42");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("image/png"));
    }

    #[test]
    fn an_out_of_range_face_cell_is_not_found() {
        let (catalog, images) = slots();
        images.set(Arc::new(AvatarImageStore::new()));
        assert_eq!(
            resolve(&catalog, &images, "face-cell/13/0").status(),
            StatusCode::NOT_FOUND,
            "face 13 is past the sheet"
        );
        assert_eq!(
            resolve(&catalog, &images, "face-cell/0/16").status(),
            StatusCode::NOT_FOUND,
            "colour 16 is past the sheet"
        );
        assert_eq!(
            resolve(&catalog, &images, "face-cell/0").status(),
            StatusCode::NOT_FOUND,
            "a missing colour segment is malformed"
        );
    }

    #[test]
    fn a_valid_face_cell_is_served_as_png() {
        let (catalog, images) = slots();
        images.set(Arc::new(AvatarImageStore::new()));
        let response = resolve(&catalog, &images, "face-cell/0/0");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), Some("image/png"));
    }
}

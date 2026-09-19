//! The `palace://` URI scheme that serves the current frame as PNG.
//!
//! Frames never travel through a Tauri command: a `Vec<u8>` return is marshalled
//! into a JSON number array, which is orders of magnitude larger than the image
//! itself. The webview fetches one already-encoded PNG per frame version.

use std::sync::{Arc, Mutex};

use palace_client::FrameStore;
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

/// Route a `palace://localhost/<path>` request.
pub fn handle(
    slot: &FrameSlot,
    catalog: &CatalogSlot,
    request: &Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let path = request.uri().path().trim_start_matches('/').to_string();
    match path.as_str() {
        "frame" => handle_frame(slot, responder),
        "faces" => responder.respond(png(palace_render::face_sheet_png().to_vec())),
        "faces.json" => responder.respond(json(palace_render::face_grid_json())),
        "props.json" => responder.respond(json(handle_props_json(catalog))),
        _ => match path.strip_prefix("prop/") {
            Some(id) => handle_prop(catalog, id, responder),
            None => responder.respond(status(StatusCode::NOT_FOUND)),
        },
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
fn handle_prop(catalog: &CatalogSlot, id: &str, responder: UriSchemeResponder) {
    let Some(id) = id.parse::<u32>().ok() else {
        responder.respond(status(StatusCode::NOT_FOUND));
        return;
    };
    let Some(catalog) = catalog.get() else {
        responder.respond(status(StatusCode::NOT_FOUND));
        return;
    };
    match catalog.thumbnail_png(id) {
        Some(bytes) => responder.respond(png(bytes)),
        None => responder.respond(status(StatusCode::NOT_FOUND)),
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

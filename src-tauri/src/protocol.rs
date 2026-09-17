//! The `palace://` URI scheme that serves the current frame as PNG.
//!
//! Frames never travel through a Tauri command: a `Vec<u8>` return is marshalled
//! into a JSON number array, which is orders of magnitude larger than the image
//! itself. The webview fetches one already-encoded PNG per frame version.

use std::sync::{Arc, Mutex};

use palace_client::FrameStore;
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

/// Route a `palace://localhost/<path>` request.
pub fn handle(slot: &FrameSlot, request: &Request<Vec<u8>>, responder: UriSchemeResponder) {
    let path = request.uri().path().trim_start_matches('/').to_string();
    match path.as_str() {
        "frame" => handle_frame(slot, responder),
        "faces" => responder.respond(png(palace_render::face_sheet_png().to_vec())),
        _ => responder.respond(status(StatusCode::NOT_FOUND)),
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

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header("Access-Control-Allow-Origin", "*")
        .body(Vec::new())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

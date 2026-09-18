//! Tauri commands. Every one is non-blocking: they hand work to the runtime's
//! channels and return immediately, so the UI thread is never held by network
//! or compositing work.

use palace_client::ClientHandle;
use tauri::{AppHandle, State};

use crate::{start_client, AppState, Settings};

/// The event name runtime events are emitted under.
pub const EVENT_NAME: &str = "palace://event";

/// The settings the app started with.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|error| error.to_string())
}

/// (Re)start the runtime against a server and user name.
#[tauri::command]
pub fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    username: String,
) -> Result<(), String> {
    if host.trim().is_empty() {
        return Err("host must not be empty".to_string());
    }
    let username = if username.trim().is_empty() {
        "Guest".to_string()
    } else {
        username.trim().to_string()
    };
    let settings = Settings {
        host: host.trim().to_string(),
        port,
        username,
    };
    let handle = start_client(&app, &settings)?;
    let mut guard = state.client.lock().map_err(|error| error.to_string())?;
    if let Some(previous) = guard.replace(handle) {
        previous.disconnect();
    }
    drop(guard);
    if let Ok(mut stored) = state.settings.lock() {
        *stored = settings;
    }
    Ok(())
}

/// Sign off and stop the runtime.
#[tauri::command]
pub fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    with_client(&state, |client| client.disconnect())
}

/// Navigate to a room by id.
#[tauri::command]
pub fn goto_room(state: State<'_, AppState>, room_id: i32) -> Result<(), String> {
    with_client(&state, |client| client.goto_room(room_id))
}

/// Send a chat line.
#[tauri::command]
pub fn say(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let text = text.trim_end().to_string();
    if text.is_empty() {
        return Ok(());
    }
    with_client(&state, |client| client.say(text))
}

/// Click the room at a viewport pixel; the runtime hit-tests hotspots.
#[tauri::command]
pub fn click(state: State<'_, AppState>, x: f64, y: f64) -> Result<(), String> {
    with_client(&state, |client| client.click(x, y))
}

/// Report pointer movement in viewport pixels; the runtime dispatches hover.
#[tauri::command]
pub fn mousemove(state: State<'_, AppState>, x: f64, y: f64) -> Result<(), String> {
    with_client(&state, |client| client.move_mouse(x, y))
}

/// Report the pointer leaving the viewport.
#[tauri::command]
pub fn mouse_leave(state: State<'_, AppState>) -> Result<(), String> {
    with_client(&state, |client| client.mouse_leave())
}

/// Show or hide name tags and avatars.
#[tauri::command]
pub fn set_visibility(
    state: State<'_, AppState>,
    names: bool,
    avatars: bool,
) -> Result<(), String> {
    with_client(&state, |client| client.set_visibility(names, avatars))
}

/// Change the signed-in user's face and colour.
#[tauri::command]
pub fn set_avatar(state: State<'_, AppState>, face: i16, color: i16) -> Result<(), String> {
    with_client(&state, |client| client.set_avatar(face, color))
}

/// Report the viewport size, device pixel ratio, zoom and scale mode.
#[tauri::command]
pub fn set_viewport(
    state: State<'_, AppState>,
    width: f64,
    height: f64,
    dpr: f64,
    zoom: f64,
    native: bool,
) -> Result<(), String> {
    with_client(&state, |client| {
        client.set_viewport(width, height, dpr, zoom, native);
    })
}

/// Ask the runtime to recompose the current room.
#[tauri::command]
pub fn refresh(state: State<'_, AppState>) -> Result<(), String> {
    with_client(&state, |client| client.refresh())
}

fn with_client<T>(
    state: &State<'_, AppState>,
    action: impl FnOnce(&ClientHandle) -> T,
) -> Result<T, String> {
    let guard = state.client.lock().map_err(|error| error.to_string())?;
    let client = guard
        .as_ref()
        .ok_or_else(|| "no client is running".to_string())?;
    Ok(action(client))
}

//! Tauri commands. Every one is non-blocking: they hand work to the runtime's
//! channels and return immediately, so the UI thread is never held by network
//! or compositing work.

use palace_client::ClientHandle;
use tauri::{AppHandle, State};

use crate::settings;
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
    let audio = state
        .audio
        .lock()
        .map(|engine| engine.handle())
        .map_err(|error| error.to_string())?;
    let mut settings = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    settings.host = host.trim().to_string();
    settings.port = port;
    settings.username = username;
    persist_best_effort(&app, &settings);
    let handle = start_client(&app, &settings, audio)?;
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

/// Replace the signed-in user's worn props, locally and on the server.
///
/// Accepts more than the nine-prop limit: the runtime keeps the first nine and
/// reports the rest, so the caller does not need to know the cap.
#[tauri::command]
pub fn set_props(state: State<'_, AppState>, props: Vec<u32>) -> Result<(), String> {
    with_client(&state, |client| client.set_props(props))
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

/// What the audio panel shows: mute, volume and the chosen SoundFont.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioState {
    pub enabled: bool,
    pub volume: f32,
    pub soundfont: Option<std::path::PathBuf>,
    pub soundfont_exists: bool,
}

/// The audio engine's state, as the stored settings record it.
#[tauri::command]
pub fn get_audio_state(state: State<'_, AppState>) -> Result<AudioState, String> {
    let settings = state.settings.lock().map_err(|error| error.to_string())?;
    Ok(AudioState {
        enabled: settings.audio_enabled,
        volume: settings.audio_volume,
        soundfont: settings.soundfont.clone(),
        soundfont_exists: settings
            .soundfont
            .as_deref()
            .is_some_and(std::path::Path::is_file),
    })
}

/// Choose the SoundFont MIDI is synthesized with; `None` clears it.
///
/// The path is validated (an existing `.sf2`) before anything changes, then the
/// new setting is persisted and only afterwards handed to the live engine, so a
/// rejected path leaves both the engine and the file untouched.
#[tauri::command]
pub fn set_soundfont(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<(), String> {
    let chosen = settings::validate_soundfont(path.as_deref())?;
    update_settings(&app, &state, |settings| {
        settings.soundfont.clone_from(&chosen);
    })?;
    let audio = state.audio.lock().map_err(|error| error.to_string())?;
    match &chosen {
        Some(path) => audio.handle().set_soundfont(path.clone()),
        None => audio.handle().clear_soundfont(),
    }
    Ok(())
}

/// Mute or unmute the audio engine.
#[tauri::command]
pub fn set_audio_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    update_settings(&app, &state, |settings| settings.audio_enabled = enabled)?;
    state
        .audio
        .lock()
        .map_err(|error| error.to_string())?
        .handle()
        .set_enabled(enabled);
    Ok(())
}

/// Set the master volume, clamped to `0.0..=1.0`; returns what was applied.
#[tauri::command]
pub fn set_volume(app: AppHandle, state: State<'_, AppState>, volume: f32) -> Result<f32, String> {
    let volume = clamp_volume(volume);
    update_settings(&app, &state, |settings| settings.audio_volume = volume)?;
    state
        .audio
        .lock()
        .map_err(|error| error.to_string())?
        .handle()
        .set_volume(volume);
    Ok(volume)
}

fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Apply a settings change, persisting it before the caller touches the engine.
fn update_settings(
    app: &AppHandle,
    state: &State<'_, AppState>,
    mutate: impl FnOnce(&mut Settings),
) -> Result<(), String> {
    let mut settings = state
        .settings
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    mutate(&mut settings);
    let path =
        settings::config_path(app).ok_or_else(|| "no config directory is available".to_string())?;
    settings::save(&path, &settings)?;
    *state.settings.lock().map_err(|error| error.to_string())? = settings;
    Ok(())
}

/// Persist settings without failing the caller; used where connecting matters
/// more than the write.
fn persist_best_effort(app: &AppHandle, settings: &Settings) {
    let Some(path) = settings::config_path(app) else {
        eprintln!("palace: no config directory is available; settings will not persist");
        return;
    };
    if let Err(error) = settings::save(&path, settings) {
        eprintln!("palace: could not save settings: {error}");
    }
}

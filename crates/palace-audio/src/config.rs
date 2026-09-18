//! Engine construction options.

use std::path::PathBuf;

/// Whether the engine should try to open an output device.
///
/// [`DeviceMode::Null`] is the default. A process that only wants the script
/// pipeline to keep working — CI, `live-smoke`, unit tests — gets a fully
/// callable engine that touches no device and makes no noise. The desktop shell
/// selects [`DeviceMode::Open`] because it is the one that wants sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceMode {
    /// Never open a device. Commands are accepted, validated and counted, then
    /// dropped.
    #[default]
    Null,
    /// Try to open the default output device; fall back to null if none opens.
    Open,
}

/// How to build the engine.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Whether to attempt an output device. Defaults to [`DeviceMode::Null`].
    pub device: DeviceMode,
    /// The SoundFont to load. `None` means MIDI plays the fallback tone.
    pub soundfont: Option<PathBuf>,
    /// Media server base URL, if it is already known at construction.
    pub media_base: Option<String>,
    /// Directory for the fetched-media disk cache. `None` uses a default under
    /// the platform cache directory.
    pub cache_dir: Option<PathBuf>,
    /// Start muted when `false`.
    pub enabled: bool,
    /// Master volume, clamped to `0.0..=1.0`.
    pub volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            device: DeviceMode::Null,
            soundfont: None,
            media_base: None,
            cache_dir: None,
            enabled: true,
            volume: 1.0,
        }
    }
}

impl AudioConfig {
    /// The default: no device, no sound, every method safe.
    #[must_use]
    pub fn headless() -> Self {
        AudioConfig::default()
    }

    /// A config that opts into opening the default output device.
    #[must_use]
    pub fn desktop() -> Self {
        AudioConfig {
            device: DeviceMode::Open,
            ..AudioConfig::default()
        }
    }

    /// Choose the SoundFont file.
    #[must_use]
    pub fn with_soundfont(mut self, path: impl Into<PathBuf>) -> Self {
        self.soundfont = Some(path.into());
        self
    }

    /// Set the media server base URL.
    #[must_use]
    pub fn with_media_base(mut self, base: impl Into<String>) -> Self {
        self.media_base = Some(base.into());
        self
    }

    /// Set the fetched-media cache directory.
    #[must_use]
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }
}

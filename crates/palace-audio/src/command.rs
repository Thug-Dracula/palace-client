//! The command channel between the runtime and the audio worker.
//!
//! Everything the engine can be told is one [`AudioCommand`]. The channel is
//! unbounded on purpose: sending must never block the thread that is parsing a
//! room script, and the worker's own rate limit — not backpressure — is what
//! bounds a hostile script.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// What the engine is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    /// `SOUND name` — play a bundled sound, else fetch `<name>.mp3`.
    PlaySound { name: String },
    /// `MIDIPLAY name` — play a `.mid` once, replacing any current MIDI.
    MidiPlay { name: String },
    /// `MIDILOOP name loops` — play it `loops` times. See
    /// [`crate::limits::effective_loops`] for the clamp and what `loops <= 0`
    /// means.
    MidiLoop { name: String, loops: i32 },
    /// `MIDISTOP` — stop MIDI playback.
    MidiStop,
    /// `BEEP` — a short synthesized tone.
    Beep,
    /// Load a SoundFont from this path, replacing the current one. A path that
    /// does not load clears the font and re-enables the fallback tone.
    SetSoundfont(PathBuf),
    /// Drop the current SoundFont and return to the fallback tone. Unlike
    /// [`AudioCommand::SetSoundfont`], this is not a failure: nothing is logged
    /// and no counter moves.
    ClearSoundfont,
    /// The media server base URL, learned from the logon banner. Until it is
    /// known, media-backed names cannot be fetched.
    SetMediaBase(String),
    /// Mute (`false`) or unmute (`true`). Muting stops everything playing.
    SetEnabled(bool),
    /// Master volume, clamped to `0.0..=1.0`.
    SetVolume(f32),
    /// Stop the worker and drop the device.
    Shutdown,
}

/// Counters a caller can read without touching the worker.
#[derive(Debug, Default)]
pub struct AudioStats {
    handled: AtomicU64,
    suppressed: AtomicU64,
    failed: AtomicU64,
    stolen: AtomicU64,
    opened: AtomicU64,
}

impl AudioStats {
    /// Commands received by the worker, including the ones policy refused.
    #[must_use]
    pub fn handled(&self) -> u64 {
        self.handled.load(Ordering::Relaxed)
    }

    /// Commands the rate limiter or the dedupe window dropped.
    #[must_use]
    pub fn suppressed(&self) -> u64 {
        self.suppressed.load(Ordering::Relaxed)
    }

    /// Resolution, fetch, decode or font failures.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }

    /// Sounds that evicted an older one at the concurrency cap.
    #[must_use]
    pub fn stolen(&self) -> u64 {
        self.stolen.load(Ordering::Relaxed)
    }

    /// Output devices successfully opened. Zero means the null path ran.
    #[must_use]
    pub fn opened(&self) -> u64 {
        self.opened.load(Ordering::Relaxed)
    }

    pub(crate) fn bump_handled(&self) {
        self.handled.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bump_suppressed(&self) {
        self.suppressed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bump_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bump_stolen(&self) {
        self.stolen.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn bump_opened(&self) {
        self.opened.fetch_add(1, Ordering::Relaxed);
    }
}

/// The caller's end of the audio channel.
///
/// Cheap to clone; every clone feeds the same worker. All methods are
/// non-blocking.
#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<AudioCommand>,
    stats: Arc<AudioStats>,
}

impl AudioHandle {
    pub(crate) fn new(tx: Sender<AudioCommand>, stats: Arc<AudioStats>) -> Self {
        AudioHandle { tx, stats }
    }

    /// Send a command. Returns `false` once the worker is gone; never blocks.
    pub fn send(&self, command: AudioCommand) -> bool {
        self.tx.send(command).is_ok()
    }

    /// `SOUND name`.
    pub fn play_sound(&self, name: impl Into<String>) {
        let _ = self.send(AudioCommand::PlaySound { name: name.into() });
    }

    /// `MIDIPLAY name`.
    pub fn midi_play(&self, name: impl Into<String>) {
        let _ = self.send(AudioCommand::MidiPlay { name: name.into() });
    }

    /// `MIDILOOP name loops`.
    pub fn midi_loop(&self, name: impl Into<String>, loops: i32) {
        let _ = self.send(AudioCommand::MidiLoop {
            name: name.into(),
            loops,
        });
    }

    /// `MIDISTOP`.
    pub fn midi_stop(&self) {
        let _ = self.send(AudioCommand::MidiStop);
    }

    /// `BEEP`.
    pub fn beep(&self) {
        let _ = self.send(AudioCommand::Beep);
    }

    /// Point the engine at the media server.
    pub fn set_media_base(&self, base: impl Into<String>) {
        let _ = self.send(AudioCommand::SetMediaBase(base.into()));
    }

    /// Choose the SoundFont file.
    pub fn set_soundfont(&self, path: impl Into<PathBuf>) {
        let _ = self.send(AudioCommand::SetSoundfont(path.into()));
    }

    /// Drop the SoundFont and return to the fallback tone.
    pub fn clear_soundfont(&self) {
        let _ = self.send(AudioCommand::ClearSoundfont);
    }

    /// Mute or unmute.
    pub fn set_enabled(&self, enabled: bool) {
        let _ = self.send(AudioCommand::SetEnabled(enabled));
    }

    /// Set the master volume.
    pub fn set_volume(&self, volume: f32) {
        let _ = self.send(AudioCommand::SetVolume(volume));
    }

    /// The worker's counters.
    #[must_use]
    pub fn stats(&self) -> Arc<AudioStats> {
        Arc::clone(&self.stats)
    }
}

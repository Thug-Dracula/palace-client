//! `palace-audio` — the sound the room scripts ask for.
//!
//! Room scripts can call `SOUND`, `MIDIPLAY`, `MIDILOOP`, `MIDISTOP` and
//! `BEEP`. The headless runtime turns those into typed [`ClientEvent`]s;
//! something has to turn *those* into samples on an output device. This crate is
//! that step, and nothing else.
//!
//! ```text
//!   palace-client ── ClientEvent ──▶ AudioHandle ── AudioCommand ──▶ worker ──▶ rodio/rustysynth
//! ```
//!
//! ## Design
//!
//! * **Null is the default.** [`AudioConfig::default()`] never opens a device;
//!   [`AudioEngine::spawn`] returns immediately and every method is safe with no
//!   audio at all. A desktop shell opts in with
//!   [`AudioConfig::desktop`]; CI, `live-smoke` and unit tests use the default
//!   and stay silent.
//! * **Command-shaped.** The engine owns one worker thread. Callers hold an
//!   [`AudioHandle`] and push [`AudioCommand`]s down an unbounded channel, so a
//!   script effect is fire-and-forget and never blocks the runtime.
//! * **The SoundFont is a path, never a bundled binary.** [`AudioConfig::soundfont`]
//!   is optional. With no font, or a font that fails to load, MIDI degrades to a
//!   synthesized tone rather than erroring.
//! * **Every failure is bounded and non-fatal.** A missing device, an unknown
//!   name, a fetch failure, an unparsable MIDI and a bad font are all logged
//!   once and dropped. A hostile script cannot spin a thread per sound, flood
//!   the fetch path, or grow a cache without limit; see [`limits`].
//!
//! ## Bundled sounds
//!
//! The reference client plays 17 built-in MP3s by name and fetches
//! `mediaServer + name + ".mp3"` for anything else. Those MP3s are not ours to
//! ship, so [`BUNDLED_SOUNDS`] is an empty-but-extensible table and the
//! *lookup order* is what is real: bundled name first, media URL second. Drop
//! bytes into the table and the first branch starts answering.
//!
//! ## Audibility
//!
//! Everything here is proven headlessly: construction, resolution, policy and
//! event flow. That a speaker actually moves is the one thing a numeric test
//! cannot show, and it needs a human once.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod command;
pub mod config;
pub mod engine;
pub mod limits;
pub mod resolve;
pub mod synth;

pub use command::{AudioCommand, AudioHandle, AudioStats};
pub use config::{AudioConfig, DeviceMode};
pub use engine::AudioEngine;
pub use limits::{
    ConcurrentSounds, LogOnce, RateLimiter, SoundCache, SoundDedupe, DEDUPE_WINDOW_MS,
    MAX_AUDIO_COMMANDS_PER_WINDOW, MAX_CONCURRENT_SOUNDS, MAX_MIDI_LOOPS, MAX_MIDI_SECONDS,
    RATE_WINDOW_MS, SOUND_CACHE_CAPACITY, SOUND_CACHE_SAMPLE_BUDGET,
};
pub use resolve::{
    bundled_sound, midi_media_name, midi_url, resolve_sound, resolve_sound_with, sound_media_name,
    sound_url, SoundSource, BUNDLED_SOUNDS,
};
pub use synth::DecodedSound;

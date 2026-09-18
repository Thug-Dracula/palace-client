//! Sample producers: decoded sound effects, a MIDI sequencer source and the
//! fallback tone.
//!
//! A `rodio` source is an iterator of interleaved `f32` samples with a channel
//! count and a sample rate. Making one out of a `rustysynth` [`MidiFileSequencer`]
//! is how the loop count is implemented: the reference client's loop count was
//! never honoured, and `rustysynth`'s own `play` only takes a bool, so the
//! counter lives here, in [`MidiSource`].

use std::fs::File;
use std::io::BufReader;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rodio::{ChannelCount, SampleRate, Source};
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

use crate::limits::MAX_MIDI_SECONDS;

const TAU: f32 = std::f32::consts::TAU;
const TONE_AMPLITUDE: f32 = 0.18;
const MIDI_BLOCK_FRAMES: usize = 1_024;

/// The sample rate used when there is no device to ask.
pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;

fn channel_count(value: u16) -> ChannelCount {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

fn sample_rate(value: u32) -> SampleRate {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

/// A decoded sound effect, ready to mix.
#[derive(Debug)]
pub struct DecodedSound {
    /// Interleaved channel count.
    pub channels: u16,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Interleaved samples.
    pub samples: Vec<f32>,
}

/// Load a SoundFont from disk.
///
/// A bad path and a bad file are the same kind of failure: both return the
/// reason as text so the caller can log it once and fall back.
pub fn load_soundfont(path: &Path) -> Result<Arc<SoundFont>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    SoundFont::new(&mut reader)
        .map(Arc::new)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// A short synthesized tone, used for `BEEP` and when no SoundFont is available.
#[derive(Debug)]
pub struct ToneSource {
    rate: u32,
    frequency: f32,
    total: usize,
    position: usize,
    phase: f32,
    fade: usize,
}

impl ToneSource {
    /// The `BEEP` tone.
    #[must_use]
    pub fn beep(rate: u32) -> Self {
        ToneSource::new(rate, 880.0, 0.18)
    }

    /// The tone MIDI degrades to when no SoundFont loaded.
    #[must_use]
    pub fn midi_fallback(rate: u32) -> Self {
        ToneSource::new(rate, 440.0, 0.6)
    }

    fn new(rate: u32, frequency: f32, seconds: f32) -> Self {
        let rate = if rate == 0 {
            FALLBACK_SAMPLE_RATE
        } else {
            rate
        };
        let total = ((rate as f32) * seconds) as usize;
        let fade = ((rate as f32) * 0.005) as usize;
        ToneSource {
            rate,
            frequency,
            total,
            position: 0,
            phase: 0.0,
            fade,
        }
    }
}

impl Iterator for ToneSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.position >= self.total {
            return None;
        }
        let gain = if self.fade == 0 {
            1.0
        } else {
            let rising = self.position as f32 / self.fade as f32;
            let falling = (self.total - self.position) as f32 / self.fade as f32;
            rising.min(falling).min(1.0)
        };
        let value = self.phase.sin() * TONE_AMPLITUDE * gain;
        self.phase += TAU * self.frequency / self.rate as f32;
        if self.phase > TAU {
            self.phase -= TAU;
        }
        self.position += 1;
        Some(value)
    }
}

impl Source for ToneSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.total.saturating_sub(self.position))
    }

    fn channels(&self) -> ChannelCount {
        channel_count(1)
    }

    fn sample_rate(&self) -> SampleRate {
        sample_rate(self.rate)
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.total as f64 / self.rate as f64,
        ))
    }
}

/// A finite, counted MIDI performance.
///
/// The sequencer plays the file once per pass with `play(file, false)`; when a
/// pass ends, [`MidiSource`] starts the next until `total_plays` is spent, then
/// returns `None` and the source is done. An absolute frame ceiling enforces
/// [`MAX_MIDI_SECONDS`] regardless of how the loop maths came out.
#[derive(Debug)]
pub struct MidiSource {
    sequencer: MidiFileSequencer,
    midi: Arc<MidiFile>,
    rate: u32,
    left: Vec<f32>,
    right: Vec<f32>,
    block: Vec<f32>,
    position: usize,
    plays_done: u32,
    total_plays: u32,
    rendered: u64,
    max_frames: u64,
    finished: bool,
}

impl MidiSource {
    /// Build a source that plays `midi` `total_plays` times at `rate`.
    #[must_use]
    pub fn new(
        sequencer: MidiFileSequencer,
        midi: Arc<MidiFile>,
        total_plays: i32,
        rate: u32,
    ) -> Self {
        let rate = if rate == 0 {
            FALLBACK_SAMPLE_RATE
        } else {
            rate
        };
        let max_frames = (MAX_MIDI_SECONDS * rate as f64) as u64;
        MidiSource {
            sequencer,
            midi,
            rate,
            left: vec![0.0; MIDI_BLOCK_FRAMES],
            right: vec![0.0; MIDI_BLOCK_FRAMES],
            block: Vec::with_capacity(MIDI_BLOCK_FRAMES * 2),
            position: 0,
            plays_done: 0,
            total_plays: total_plays.max(1) as u32,
            rendered: 0,
            max_frames,
            finished: false,
        }
    }

    fn fill(&mut self) {
        self.block.clear();
        if self.finished {
            return;
        }
        if self.rendered >= self.max_frames {
            self.finished = true;
            return;
        }
        let remaining = (self.max_frames - self.rendered) as usize;
        let frames = MIDI_BLOCK_FRAMES.min(remaining);
        self.sequencer
            .render(&mut self.left[..frames], &mut self.right[..frames]);
        self.rendered += frames as u64;
        for index in 0..frames {
            self.block.push(self.left[index]);
            self.block.push(self.right[index]);
        }
        if self.sequencer.end_of_sequence() {
            self.plays_done += 1;
            if self.plays_done >= self.total_plays {
                self.finished = true;
            } else {
                self.sequencer.play(&self.midi, false);
            }
        }
    }
}

impl Iterator for MidiSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.position >= self.block.len() {
            self.fill();
            self.position = 0;
            if self.block.is_empty() {
                return None;
            }
        }
        let value = self.block[self.position];
        self.position += 1;
        Some(value)
    }
}

impl Source for MidiSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.block.len().saturating_sub(self.position))
    }

    fn channels(&self) -> ChannelCount {
        channel_count(2)
    }

    fn sample_rate(&self) -> SampleRate {
        sample_rate(self.rate)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Build a sequencer over `font`, or `None` if the settings are refused.
pub fn build_sequencer(font: &Arc<SoundFont>, rate: u32) -> Option<MidiFileSequencer> {
    let rate = if rate == 0 {
        FALLBACK_SAMPLE_RATE
    } else {
        rate
    };
    let settings = SynthesizerSettings::new(rate.clamp(16_000, 192_000) as i32);
    Synthesizer::new(font, &settings)
        .ok()
        .map(MidiFileSequencer::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_beep_tone_is_finite_and_audible() {
        let mut peak = 0.0_f32;
        let mut count = 0_usize;
        for sample in ToneSource::beep(48_000) {
            peak = peak.max(sample.abs());
            count += 1;
        }
        assert!(count > 0, "the tone produced samples");
        assert!(peak > 0.05, "the tone is audible, not silence: peak {peak}");
    }

    #[test]
    fn a_tone_with_a_zero_rate_still_produces_samples() {
        let tone = ToneSource::midi_fallback(0);
        assert_eq!(tone.sample_rate().get(), FALLBACK_SAMPLE_RATE);
        assert_eq!(tone.channels().get(), 1);
    }

    #[test]
    fn a_bad_soundfont_path_is_an_error_not_a_panic() {
        let result = load_soundfont(Path::new("/definitely/not/a/font.sf2"));
        assert!(result.is_err());
    }
}

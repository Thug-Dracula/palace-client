//! The worker thread and its device.
//!
//! One thread owns the output device, the MIDI player, the live sound effects,
//! the decoder cache and the fetcher. Callers only ever push commands.

use std::io::Cursor;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use palace_asset::{MediaCache, MediaConfig, MediaFetcher, UreqTransport};
use rodio::buffer::SamplesBuffer;
use rodio::mixer::Mixer;
use rodio::{Decoder, MixerDeviceSink, Player, Source};
use rustysynth::MidiFile;

use crate::command::{AudioCommand, AudioHandle, AudioStats};
use crate::config::{AudioConfig, DeviceMode};
use crate::limits::{
    loops_within_duration, ConcurrentSounds, LogOnce, RateLimiter, SoundCache, SoundDedupe,
    MAX_CONCURRENT_SOUNDS, SOUND_CACHE_CAPACITY, SOUND_CACHE_SAMPLE_BUDGET,
};
use crate::resolve::{self, SoundSource};
use crate::synth::{self, DecodedSound, MidiSource, ToneSource};

/// How often the worker wakes to reap finished sounds.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

struct Output {
    // Kept solely to hold the cpal stream open.
    _sink: MixerDeviceSink,
    mixer: Mixer,
    sample_rate: u32,
}

struct ActiveSound {
    id: u64,
    player: Player,
}

struct Worker {
    output: Option<Output>,
    midi: Option<Player>,
    sounds: Vec<ActiveSound>,
    slots: ConcurrentSounds,
    soundfont: Option<Arc<rustysynth::SoundFont>>,
    decoded: SoundCache,
    fetcher: MediaFetcher<UreqTransport>,
    media_base: Option<String>,
    rate: RateLimiter,
    dedupe: SoundDedupe,
    logged: LogOnce,
    stats: Arc<AudioStats>,
    enabled: bool,
    volume: f32,
    started: Instant,
}

impl Worker {
    fn new(config: AudioConfig, stats: Arc<AudioStats>) -> Self {
        let cache_dir = config.cache_dir.clone().unwrap_or_else(default_cache_dir);
        let media = MediaConfig::default();
        let transport = UreqTransport::from_config(&media);
        let cache = MediaCache::new(cache_dir);
        let fetcher = MediaFetcher::new(transport, cache, media);
        let output = match config.device {
            DeviceMode::Null => None,
            DeviceMode::Open => open_output(&stats),
        };
        let mut worker = Worker {
            output,
            midi: None,
            sounds: Vec::new(),
            slots: ConcurrentSounds::default(),
            soundfont: None,
            decoded: SoundCache::new(SOUND_CACHE_CAPACITY, SOUND_CACHE_SAMPLE_BUDGET),
            fetcher,
            media_base: config.media_base.filter(|base| !base.trim().is_empty()),
            rate: RateLimiter::default(),
            dedupe: SoundDedupe::default(),
            logged: LogOnce::default(),
            stats,
            enabled: config.enabled,
            volume: clamp_volume(config.volume),
            started: Instant::now(),
        };
        if let Some(path) = config.soundfont {
            worker.set_soundfont(&path);
        }
        worker
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn sample_rate(&self) -> u32 {
        self.output
            .as_ref()
            .map_or(synth::FALLBACK_SAMPLE_RATE, |output| output.sample_rate)
    }

    fn fail(&mut self, key: &str, reason: &str) {
        self.stats.bump_failed();
        if self.logged.should_log(key) {
            eprintln!("palace-audio: {reason}");
        }
    }

    fn handle(&mut self, command: AudioCommand) {
        self.stats.bump_handled();
        match command {
            AudioCommand::PlaySound { name } => {
                if self.admits() {
                    self.play_sound(&name);
                }
            }
            AudioCommand::MidiPlay { name } => {
                if self.admits() {
                    self.start_midi(&name, 1);
                }
            }
            AudioCommand::MidiLoop { name, loops } => {
                if self.admits() {
                    self.start_midi(&name, loops);
                }
            }
            AudioCommand::MidiStop => self.stop_midi(),
            AudioCommand::Beep => {
                if self.admits() {
                    self.beep();
                }
            }
            AudioCommand::SetSoundfont(path) => self.set_soundfont(&path),
            AudioCommand::ClearSoundfont => self.soundfont = None,
            AudioCommand::SetMediaBase(base) => {
                self.media_base = if base.trim().is_empty() {
                    None
                } else {
                    Some(base)
                };
            }
            AudioCommand::SetEnabled(enabled) => {
                self.enabled = enabled;
                if !enabled {
                    self.stop_all();
                }
            }
            AudioCommand::SetVolume(volume) => {
                if volume.is_finite() {
                    self.volume = clamp_volume(volume);
                }
            }
            AudioCommand::Shutdown => {}
        }
    }

    fn admits(&mut self) -> bool {
        if !self.enabled {
            self.stats.bump_suppressed();
            return false;
        }
        let allowed = self.rate.allow(self.now_ms());
        if !allowed {
            self.stats.bump_suppressed();
        }
        allowed
    }

    fn set_soundfont(&mut self, path: &Path) {
        match synth::load_soundfont(path) {
            Ok(font) => self.soundfont = Some(font),
            Err(reason) => {
                self.soundfont = None;
                self.fail(&format!("soundfont:{}", path.display()), &reason);
            }
        }
    }

    fn play_sound(&mut self, name: &str) {
        if self.output.is_none() {
            return;
        }
        if !self.dedupe.accept(name, self.now_ms()) {
            self.stats.bump_suppressed();
            return;
        }
        let Some(source) = resolve::resolve_sound(name) else {
            self.fail(&format!("name:{name}"), "unusable sound name");
            return;
        };
        let Some(decoded) = self.load_decoded(name, source) else {
            return;
        };
        self.start_sound(decoded);
    }

    fn load_decoded(&mut self, name: &str, source: SoundSource<'_>) -> Option<Arc<DecodedSound>> {
        let (key, bundled) = match source {
            SoundSource::Bundled(bytes) => (
                format!("bundled:{}", name.trim().to_ascii_lowercase()),
                Some(bytes.to_vec()),
            ),
            SoundSource::Media(media_name) => (media_name, None),
        };
        if let Some(cached) = self.decoded.get(&key) {
            return Some(cached);
        }
        let bytes = match bundled {
            Some(bytes) => bytes,
            None => {
                let Some(base) = self.media_base.clone() else {
                    self.fail(&format!("media-base:{key}"), "no media base URL known yet");
                    return None;
                };
                let now = self.now_ms();
                match self.fetcher.fetch_single(&base, &key, now) {
                    Ok(fetch) => fetch.bytes,
                    Err(error) => {
                        self.fail(&format!("fetch:{key}"), &error.to_string());
                        return None;
                    }
                }
            }
        };
        match decode_sound(&bytes) {
            Ok(sound) => {
                let decoded = Arc::new(sound);
                self.decoded.insert(key, Arc::clone(&decoded));
                Some(decoded)
            }
            Err(reason) => {
                self.fail(&format!("decode:{key}"), &reason);
                None
            }
        }
    }

    fn start_sound(&mut self, decoded: Arc<DecodedSound>) {
        let Some((id, mixer)) = self.reserve_slot() else {
            return;
        };
        let player = Player::connect_new(&mixer);
        player.set_volume(self.volume);
        player.append(SamplesBuffer::new(
            channel_count(decoded.channels),
            sample_rate(decoded.sample_rate),
            decoded.samples.clone(),
        ));
        self.sounds.push(ActiveSound { id, player });
    }

    fn beep(&mut self) {
        let rate = self.sample_rate();
        let Some((id, mixer)) = self.reserve_slot() else {
            return;
        };
        let player = Player::connect_new(&mixer);
        player.set_volume(self.volume);
        player.append(ToneSource::beep(rate));
        self.sounds.push(ActiveSound { id, player });
    }

    fn reserve_slot(&mut self) -> Option<(u64, Mixer)> {
        let mixer = self.output.as_ref()?.mixer.clone();
        let (id, stolen) = self.slots.admit();
        if let Some(stolen) = stolen {
            self.stats.bump_stolen();
            if let Some(position) = self.sounds.iter().position(|active| active.id == stolen) {
                let active = self.sounds.remove(position);
                active.player.stop();
            }
        }
        Some((id, mixer))
    }

    fn start_midi(&mut self, name: &str, requested_loops: i32) {
        let Some((mixer, rate)) = self
            .output
            .as_ref()
            .map(|output| (output.mixer.clone(), output.sample_rate))
        else {
            return;
        };
        let Some(media_name) = resolve::midi_media_name(name) else {
            self.fail(&format!("midi-name:{name}"), "unusable MIDI name");
            return;
        };
        let Some(base) = self.media_base.clone() else {
            self.fail(
                &format!("media-base:{media_name}"),
                "no media base URL known yet",
            );
            return;
        };
        let now = self.now_ms();
        let bytes = match self.fetcher.fetch_single(&base, &media_name, now) {
            Ok(fetch) => fetch.bytes,
            Err(error) => {
                self.fail(&format!("fetch:{media_name}"), &error.to_string());
                return;
            }
        };
        let mut cursor = Cursor::new(bytes);
        let midi = match MidiFile::new(&mut cursor) {
            Ok(midi) => Arc::new(midi),
            Err(error) => {
                self.fail(&format!("midi:{media_name}"), &error.to_string());
                return;
            }
        };
        let plays = loops_within_duration(requested_loops, midi.get_length());
        if let Some(previous) = self.midi.take() {
            previous.stop();
        }
        let player = Player::connect_new(&mixer);
        player.set_volume(self.volume);
        match self
            .soundfont
            .as_ref()
            .and_then(|font| synth::build_sequencer(font, rate))
        {
            Some(sequencer) => {
                player.append(MidiSource::new(sequencer, midi, plays, rate));
            }
            None => {
                self.fail(
                    &format!("soundfont:{media_name}"),
                    "no SoundFont loaded; using the fallback tone",
                );
                player.append(ToneSource::midi_fallback(rate));
            }
        }
        self.midi = Some(player);
    }

    fn stop_midi(&mut self) {
        if let Some(player) = self.midi.take() {
            player.stop();
        }
    }

    fn stop_all(&mut self) {
        self.stop_midi();
        for active in self.sounds.drain(..) {
            active.player.stop();
        }
        self.slots = ConcurrentSounds::new(MAX_CONCURRENT_SOUNDS);
    }

    fn prune(&mut self) {
        let finished: Vec<u64> = self
            .sounds
            .iter()
            .filter(|active| active.player.empty())
            .map(|active| active.id)
            .collect();
        for id in finished {
            self.slots.finish(id);
        }
        self.sounds.retain(|active| !active.player.empty());
    }
}

fn open_output(stats: &AudioStats) -> Option<Output> {
    match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(sink) => {
            let sample_rate = sink.config().sample_rate().get();
            let mixer = sink.mixer().clone();
            stats.bump_opened();
            Some(Output {
                _sink: sink,
                mixer,
                sample_rate,
            })
        }
        Err(error) => {
            eprintln!("palace-audio: no output device ({error}); running silently");
            None
        }
    }
}

fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn channel_count(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

fn sample_rate(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

fn default_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("palace-audio")
}

fn decode_sound(bytes: &[u8]) -> Result<DecodedSound, String> {
    let cursor = Cursor::new(bytes.to_vec());
    let decoder = Decoder::new(cursor).map_err(|error| error.to_string())?;
    let channels = decoder.channels().get();
    let sample_rate = decoder.sample_rate().get();
    let samples: Vec<f32> = decoder.take(SOUND_CACHE_SAMPLE_BUDGET).collect();
    if samples.is_empty() {
        return Err("the decoder produced no samples".to_string());
    }
    Ok(DecodedSound {
        channels,
        sample_rate,
        samples,
    })
}

fn run_worker(config: AudioConfig, rx: Receiver<AudioCommand>, stats: Arc<AudioStats>) {
    let mut worker = Worker::new(config, stats);
    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(AudioCommand::Shutdown) => break,
            Ok(command) => worker.handle(command),
            Err(RecvTimeoutError::Timeout) => worker.prune(),
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// The audio engine: one worker thread and the device it may hold.
///
/// [`AudioEngine::spawn`] is infallible and never touches a device on the
/// calling thread. Dropping the engine shuts the worker down.
pub struct AudioEngine {
    handle: AudioHandle,
    join: Option<JoinHandle<()>>,
}

impl AudioEngine {
    /// Start the worker. Returns immediately; with the default config it opens
    /// no device at all.
    #[must_use]
    pub fn spawn(config: AudioConfig) -> AudioEngine {
        let stats = Arc::new(AudioStats::default());
        let (tx, rx) = mpsc::channel();
        let worker_stats = Arc::clone(&stats);
        let join = thread::Builder::new()
            .name("palace-audio".to_string())
            .spawn(move || run_worker(config, rx, worker_stats))
            .ok();
        AudioEngine {
            handle: AudioHandle::new(tx, stats),
            join,
        }
    }

    /// A cloneable handle to push commands.
    #[must_use]
    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    /// The worker's counters.
    #[must_use]
    pub fn stats(&self) -> Arc<AudioStats> {
        self.handle.stats()
    }

    /// Stop the worker and wait for it.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.handle.send(AudioCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_handled(stats: &AudioStats, target: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        loop {
            let handled = stats.handled();
            if handled >= target || Instant::now() >= deadline {
                return handled;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn every_method_is_safe_with_no_device() {
        let engine = AudioEngine::spawn(AudioConfig::headless());
        let handle = engine.handle();
        handle.play_sound("garden");
        handle.midi_play("garden");
        handle.midi_loop("garden", 99);
        handle.midi_stop();
        handle.beep();
        handle.set_media_base("https://example.invalid/media");
        handle.set_soundfont("/no/such/font.sf2");
        handle.clear_soundfont();
        handle.set_enabled(false);
        handle.set_enabled(true);
        handle.set_volume(0.5);
        handle.set_volume(f32::NAN);
        let handled = wait_handled(&handle.stats(), 12, Duration::from_secs(5));
        assert!(handled >= 12, "every command reached the worker: {handled}");
        engine.shutdown();
    }

    #[test]
    fn clearing_the_soundfont_is_not_a_failure() {
        let engine = AudioEngine::spawn(AudioConfig::headless());
        let handle = engine.handle();
        handle.set_soundfont("/no/such/font.sf2");
        let deadline = Instant::now() + Duration::from_secs(2);
        while handle.stats().failed() == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(handle.stats().failed(), 1, "a bad font is a failure");
        handle.clear_soundfont();
        let handled = wait_handled(&handle.stats(), 2, Duration::from_secs(2));
        assert!(
            handled >= 2,
            "the clear command reached the worker: {handled}"
        );
        assert_eq!(
            handle.stats().failed(),
            1,
            "clearing deliberately is not a failure"
        );
        engine.shutdown();
    }

    #[test]
    fn the_headless_config_never_opens_a_device() {
        let engine = AudioEngine::spawn(AudioConfig::headless());
        let stats = engine.stats();
        engine.handle().beep();
        let _ = wait_handled(&stats, 1, Duration::from_secs(2));
        assert_eq!(stats.opened(), 0, "null mode touched no device");
        engine.shutdown();
    }

    #[test]
    fn a_second_engine_is_independent() {
        let first = AudioEngine::spawn(AudioConfig::headless());
        let second = AudioEngine::spawn(AudioConfig::headless());
        first.handle().beep();
        second.handle().beep();
        first.shutdown();
        assert!(second.handle().send(AudioCommand::Beep));
        second.shutdown();
    }
}

//! The bounds that keep a hostile room script from hurting the process.
//!
//! Every number here is a deliberate ceiling, not a hint: a script can name a
//! sound it does not have, loop MIDI forever, or fire thousands of commands in
//! a burst, and the engine must stay bounded and responsive.

use std::collections::HashSet;
use std::sync::Arc;

use crate::synth::DecodedSound;

/// Window the command rate limit is measured over.
pub const RATE_WINDOW_MS: u64 = 1_000;
/// Audio commands admitted per window. A room-entry burst is well under this;
/// a loop is not.
pub const MAX_AUDIO_COMMANDS_PER_WINDOW: u32 = 20;
/// At most this many sound effects play at once; a new one steals the oldest.
pub const MAX_CONCURRENT_SOUNDS: usize = 8;
/// The most plays a `MIDILOOP` may ask for. Bigger requests are clamped.
pub const MAX_MIDI_LOOPS: i32 = 8;
/// Total MIDI playback is capped at this many seconds regardless of loop count.
pub const MAX_MIDI_SECONDS: f64 = 600.0;
/// The same sound name is ignored for this long after it starts.
pub const DEDUPE_WINDOW_MS: u64 = 250;
/// Decoded sounds kept in memory.
pub const SOUND_CACHE_CAPACITY: usize = 32;
/// Ceiling on the decoded samples the cache holds in total (120 s stereo @ 48 kHz).
pub const SOUND_CACHE_SAMPLE_BUDGET: usize = 48_000 * 2 * 120;
/// Distinct failure keys remembered before the log-once set is allowed to reset.
pub const LOG_ONCE_CAPACITY: usize = 256;

/// A sliding-window limiter for audio commands.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    window_ms: u64,
    max: u32,
    window_start: u64,
    used: u32,
}

impl RateLimiter {
    /// A limiter admitting `max` commands per `window_ms`.
    #[must_use]
    pub fn new(max: u32, window_ms: u64) -> Self {
        RateLimiter {
            window_ms: window_ms.max(1),
            max,
            window_start: 0,
            used: 0,
        }
    }

    /// Whether one command may run at `now_ms`.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if now_ms >= self.window_start.saturating_add(self.window_ms) {
            self.window_start = now_ms;
            self.used = 0;
        }
        if self.used < self.max {
            self.used += 1;
            true
        } else {
            false
        }
    }

    /// How many commands the current window has admitted.
    #[must_use]
    pub fn used(&self) -> u32 {
        self.used
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        RateLimiter::new(MAX_AUDIO_COMMANDS_PER_WINDOW, RATE_WINDOW_MS)
    }
}

/// Tracks which sound effects are playing, oldest first, and who is stolen at
/// the cap.
#[derive(Debug)]
pub struct ConcurrentSounds {
    cap: usize,
    next_id: u64,
    active: Vec<u64>,
}

impl ConcurrentSounds {
    /// Track at most `cap` concurrent sounds.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        ConcurrentSounds {
            cap: cap.max(1),
            next_id: 0,
            active: Vec::new(),
        }
    }

    /// Admit a sound. Returns its id and, when the cap was already full, the id
    /// of the oldest sound it must replace.
    pub fn admit(&mut self) -> (u64, Option<u64>) {
        let id = self.next_id;
        self.next_id += 1;
        let stolen = if self.active.len() >= self.cap {
            self.active.first().copied()
        } else {
            None
        };
        if stolen.is_some() {
            self.active.remove(0);
        }
        self.active.push(id);
        (id, stolen)
    }

    /// Forget a sound that has finished.
    pub fn finish(&mut self, id: u64) {
        self.active.retain(|active| *active != id);
    }

    /// How many sounds are active.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// True when nothing is active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The configured cap.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }
}

impl Default for ConcurrentSounds {
    fn default() -> Self {
        ConcurrentSounds::new(MAX_CONCURRENT_SOUNDS)
    }
}

/// Suppresses a repeat of the same sound name inside a short window.
#[derive(Debug)]
pub struct SoundDedupe {
    window_ms: u64,
    recent: Vec<(String, u64)>,
}

impl SoundDedupe {
    /// A dedupe window of `window_ms`.
    #[must_use]
    pub fn new(window_ms: u64) -> Self {
        SoundDedupe {
            window_ms,
            recent: Vec::new(),
        }
    }

    /// Whether `name` may play at `now_ms`, recording it when it may.
    ///
    /// Names are compared lower-cased, so `Chime` and `chime` dedupe together.
    /// The list is pruned to the window and capped, so it cannot grow.
    pub fn accept(&mut self, name: &str, now_ms: u64) -> bool {
        self.recent
            .retain(|(_, started)| now_ms.saturating_sub(*started) < self.window_ms);
        let key = name.trim().to_ascii_lowercase();
        if self.recent.iter().any(|(seen, _)| *seen == key) {
            return false;
        }
        self.recent.push((key, now_ms));
        if self.recent.len() > LOG_ONCE_CAPACITY {
            self.recent.remove(0);
        }
        true
    }

    /// How many names are in the window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recent.len()
    }

    /// True when the window is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recent.is_empty()
    }
}

impl Default for SoundDedupe {
    fn default() -> Self {
        SoundDedupe::new(DEDUPE_WINDOW_MS)
    }
}

/// A bounded, insertion-ordered cache of decoded sounds.
#[derive(Debug, Default)]
pub struct SoundCache {
    capacity: usize,
    sample_budget: usize,
    entries: Vec<(String, Arc<DecodedSound>)>,
    samples: usize,
}

impl SoundCache {
    /// A cache holding at most `capacity` sounds and `sample_budget` samples.
    #[must_use]
    pub fn new(capacity: usize, sample_budget: usize) -> Self {
        SoundCache {
            capacity: capacity.max(1),
            sample_budget,
            entries: Vec::new(),
            samples: 0,
        }
    }

    /// Look a sound up, marking it most-recently used.
    pub fn get(&mut self, name: &str) -> Option<Arc<DecodedSound>> {
        let position = self.entries.iter().position(|(key, _)| key == name)?;
        let entry = self.entries.remove(position);
        let sound = Arc::clone(&entry.1);
        self.entries.push(entry);
        Some(sound)
    }

    /// Insert a decoded sound, evicting the oldest entries to stay in bounds.
    ///
    /// A single sound larger than the whole budget is kept (it is already
    /// truncated at decode time); everything else yields until the totals fit.
    pub fn insert(&mut self, name: String, sound: Arc<DecodedSound>) {
        if let Some(position) = self.entries.iter().position(|(key, _)| *key == name) {
            let (_, previous) = self.entries.remove(position);
            self.samples = self.samples.saturating_sub(previous.samples.len());
        }
        self.samples = self.samples.saturating_add(sound.samples.len());
        self.entries.push((name, sound));
        while self.entries.len() > self.capacity
            || (self.samples > self.sample_budget && self.entries.len() > 1)
        {
            let (_, evicted) = self.entries.remove(0);
            self.samples = self.samples.saturating_sub(evicted.samples.len());
        }
    }

    /// How many sounds are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The decoded samples the cache currently holds.
    #[must_use]
    pub fn samples(&self) -> usize {
        self.samples
    }

    /// The configured entry cap.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Logs a key at most once until its set has to be reset.
#[derive(Debug, Default)]
pub struct LogOnce {
    seen: HashSet<String>,
}

impl LogOnce {
    /// Whether `key` has not been logged yet, recording it when so.
    pub fn should_log(&mut self, key: &str) -> bool {
        if self.seen.contains(key) {
            return false;
        }
        if self.seen.len() >= LOG_ONCE_CAPACITY {
            self.seen.clear();
        }
        self.seen.insert(key.to_string());
        true
    }

    /// How many keys are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// True when nothing has been logged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// The number of plays a `MIDILOOP` actually gets.
///
/// `loops <= 0` means a single play: a loop count of zero or less is not a
/// request to go silent and it is not a request to play forever; both are the
/// safe reading of "play it once". Everything above [`MAX_MIDI_LOOPS`] clamps
/// down.
#[must_use]
pub fn effective_loops(requested: i32) -> i32 {
    requested.clamp(1, MAX_MIDI_LOOPS)
}

/// [`effective_loops`] further reduced so the total stays under
/// [`MAX_MIDI_SECONDS`].
///
/// A file whose own length already exceeds the ceiling gets one play; the
/// ceiling is then enforced by sample count in the player.
#[must_use]
pub fn loops_within_duration(loops: i32, midi_seconds: f64) -> i32 {
    let loops = effective_loops(loops);
    if !midi_seconds.is_finite() || midi_seconds <= 0.0 {
        return 1;
    }
    let by_time = (MAX_MIDI_SECONDS / midi_seconds).floor();
    let by_time = i32::try_from(by_time as i64).unwrap_or(i32::MAX).max(1);
    loops.min(by_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(samples: usize) -> Arc<DecodedSound> {
        Arc::new(DecodedSound {
            channels: 2,
            sample_rate: 48_000,
            samples: vec![0.0; samples],
        })
    }

    #[test]
    fn the_rate_limiter_admits_then_refuses_within_a_window() {
        let mut limiter = RateLimiter::new(3, 1_000);
        assert!(limiter.allow(0));
        assert!(limiter.allow(10));
        assert!(limiter.allow(20));
        assert!(
            !limiter.allow(30),
            "the fourth command in the window is refused"
        );
        assert!(!limiter.allow(999));
    }

    #[test]
    fn the_rate_limiter_refills_on_the_next_window() {
        let mut limiter = RateLimiter::new(2, 1_000);
        assert!(limiter.allow(0));
        assert!(limiter.allow(0));
        assert!(!limiter.allow(500));
        assert!(limiter.allow(1_000), "a new window admits again");
        assert_eq!(limiter.used(), 1);
    }

    #[test]
    fn the_concurrency_cap_steals_the_oldest_sound() {
        let mut slots = ConcurrentSounds::new(2);
        let (first, stolen) = slots.admit();
        assert_eq!(stolen, None);
        let (second, stolen) = slots.admit();
        assert_eq!(stolen, None);
        let (third, stolen) = slots.admit();
        assert_eq!(stolen, Some(first), "the oldest is stolen at the cap");
        assert_eq!(slots.len(), 2);
        slots.finish(second);
        slots.finish(third);
        assert!(slots.is_empty());
    }

    #[test]
    fn the_concurrency_cap_is_at_least_one() {
        let mut slots = ConcurrentSounds::new(0);
        let (first, _) = slots.admit();
        assert_eq!(slots.cap(), 1);
        let (_, stolen) = slots.admit();
        assert_eq!(stolen, Some(first));
    }

    #[test]
    fn an_empty_or_zero_loop_request_plays_once() {
        assert_eq!(effective_loops(0), 1);
        assert_eq!(effective_loops(-5), 1);
        assert_eq!(effective_loops(1), 1);
    }

    #[test]
    fn a_loop_request_clamps_to_the_ceiling() {
        assert_eq!(effective_loops(99), MAX_MIDI_LOOPS);
        assert_eq!(effective_loops(i32::MAX), MAX_MIDI_LOOPS);
    }

    #[test]
    fn the_duration_ceiling_reduces_a_long_loop() {
        // 200 s per play against a 600 s ceiling is three plays, not eight.
        assert_eq!(loops_within_duration(8, 200.0), 3);
        // A file already past the ceiling still gets its single play.
        assert_eq!(loops_within_duration(8, 900.0), 1);
        // A zero-length file plays once.
        assert_eq!(loops_within_duration(8, 0.0), 1);
        assert_eq!(loops_within_duration(8, f64::NAN), 1);
        // A short file keeps the requested count.
        assert_eq!(loops_within_duration(4, 10.0), 4);
    }

    #[test]
    fn the_same_sound_is_suppressed_inside_the_window() {
        let mut dedupe = SoundDedupe::new(250);
        assert!(dedupe.accept("Chime", 0));
        assert!(
            !dedupe.accept("chime", 100),
            "case does not reset the window"
        );
        assert!(dedupe.accept("chime", 250), "the window has passed");
        assert!(
            !dedupe.accept("chime", 400),
            "the replayed name is inside the new window"
        );
    }

    #[test]
    fn the_decoded_cache_stays_under_its_entry_cap() {
        let mut cache = SoundCache::new(2, usize::MAX);
        cache.insert("a".to_string(), decoded(10));
        cache.insert("b".to_string(), decoded(10));
        cache.insert("c".to_string(), decoded(10));
        assert_eq!(cache.len(), 2);
        assert!(cache.get("a").is_none(), "the oldest entry was evicted");
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn the_decoded_cache_stays_under_its_sample_budget() {
        let mut cache = SoundCache::new(32, 150);
        cache.insert("a".to_string(), decoded(100));
        cache.insert("b".to_string(), decoded(100));
        assert_eq!(cache.samples(), 100, "the oldest was evicted to fit");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_single_oversized_sound_is_still_kept() {
        let mut cache = SoundCache::new(32, 50);
        cache.insert("big".to_string(), decoded(1_000));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.samples(), 1_000);
    }

    #[test]
    fn a_logged_key_is_not_logged_again() {
        let mut log = LogOnce::default();
        assert!(log.should_log("fetch:garden.mp3"));
        assert!(!log.should_log("fetch:garden.mp3"));
        assert!(log.should_log("fetch:other.mp3"));
    }
}

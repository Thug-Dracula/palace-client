//! The asset request scheduler: 20 requests per flush, 500 ms between flushes.
//!
//! ## Why the pacing is not a tuning knob
//!
//! The reference client carries this comment above the code that sends asset
//! requests:
//!
//! > Assets are requested in packets of up to 20 requests, separated by 500ms
//! > to prevent flooding the server and getting killed.
//!
//! That is a **hard compatibility requirement**, not a heuristic. `pserver`
//! counts message rate and drops the connection of a client it decides is
//! flooding; the failure mode is not "slower", it is "disconnected". So the
//! constants below are clamped rather than merely defaulted:
//!
//! * [`SchedulerConfig::batch_size`] is clamped to at most
//!   [`REQUESTS_PER_BATCH`] (20).
//! * [`SchedulerConfig::batch_interval_ms`] is clamped to **at least**
//!   [`MIN_BATCH_INTERVAL_MS`] (500).
//!
//! A caller can make the scheduler more patient. It cannot make it faster.
//! `a_configuration_cannot_be_made_to_flood` asserts that.
//!
//! ## Time is an argument, never a `sleep`
//!
//! [`AssetScheduler::poll`] takes the current time in milliseconds and returns
//! what to do. The scheduler never sleeps, never spawns, and never reads a
//! clock. That makes the pacing exactly assertable in a test — `tests/pacing.rs`
//! drives 45 requests through a [`VirtualClock`] and checks the flush times are
//! 50, 550 and 1050 ms — and it makes the real driver trivially correct: wake at
//! [`AssetScheduler::next_deadline`], call `poll`.
//!
//! ## State machine
//!
//! ```text
//!                     request()
//!                        |
//!                        v
//!   +--> Queued { not_before } --poll()--> InFlight { sent_at }
//!   |                                        |            |
//!   |            timeout, attempts left      |            | asset received
//!   +<---------------------------------------+            v
//!   |            (not_before += backoff)               dropped
//!   |
//!   |            timeout, attempts exhausted
//!   +---------------------------------------> Failed (event, entry removed)
//! ```
//!
//! Dedup is per [`AssetKey`], i.e. per (assetType, id, crc). An asset that
//! arrives is matched against pending requests by exact key **and** by
//! (assetType, id) ignoring the CRC, because a client that asks with `crc == 0`
//! gets an answer carrying the server's real CRC — see
//! [`AssetScheduler::note_received_any_crc`].

use std::collections::{HashMap, VecDeque};

use crate::asset::{AssetKey, AssetSpec, AssetType};
use crate::query::REQUESTS_PER_BATCH as QUERY_REQUESTS_PER_BATCH;

/// Requests placed in one flush. Never raise this.
pub const REQUESTS_PER_BATCH: usize = QUERY_REQUESTS_PER_BATCH;

/// Milliseconds between flushes. Never lower this.
pub const MIN_BATCH_INTERVAL_MS: u64 = 500;

/// Default delay between a request arriving and the first flush that carries it.
///
/// The reference client debounces for 50 ms so that the several props a room
/// description asks for at once travel together.
pub const DEFAULT_FIRST_FLUSH_MS: u64 = 50;

/// Default per-attempt timeout before a request is considered lost.
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;

/// Default number of sends before a request is given up on.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 4;

/// First retry delay. Doubles per attempt, capped by
/// [`SchedulerConfig::max_backoff_ms`].
pub const DEFAULT_BASE_BACKOFF_MS: u64 = 500;

/// Ceiling for the exponential retry delay.
pub const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;

/// Largest number of simultaneously tracked requests.
pub const DEFAULT_MAX_QUEUE: usize = 4096;

/// Limits and timings for [`AssetScheduler`].
///
/// Build one with [`SchedulerConfig::default`] and edit what you need; the
/// constructor clamps the two fields that must not be made aggressive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// Requests per flush. Clamped down to [`REQUESTS_PER_BATCH`].
    pub batch_size: usize,
    /// Milliseconds between flushes. Clamped up to [`MIN_BATCH_INTERVAL_MS`].
    pub batch_interval_ms: u64,
    /// Debounce before the first flush after a request arrives.
    pub first_flush_ms: u64,
    /// How long an in-flight request may go unanswered.
    pub request_timeout_ms: u64,
    /// How many times a request may be sent before it is failed.
    pub max_attempts: u32,
    /// First retry delay; doubles per attempt.
    pub base_backoff_ms: u64,
    /// Retry delay ceiling.
    pub max_backoff_ms: u64,
    /// Cap on tracked (queued + in-flight) requests.
    pub max_queue: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            batch_size: REQUESTS_PER_BATCH,
            batch_interval_ms: MIN_BATCH_INTERVAL_MS,
            first_flush_ms: DEFAULT_FIRST_FLUSH_MS,
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            base_backoff_ms: DEFAULT_BASE_BACKOFF_MS,
            max_backoff_ms: DEFAULT_MAX_BACKOFF_MS,
            max_queue: DEFAULT_MAX_QUEUE,
        }
    }
}

impl SchedulerConfig {
    /// Apply the flood-safety clamps.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.batch_size = self.batch_size.clamp(1, REQUESTS_PER_BATCH);
        self.batch_interval_ms = self.batch_interval_ms.max(MIN_BATCH_INTERVAL_MS);
        self.max_attempts = self.max_attempts.max(1);
        self.max_queue = self.max_queue.max(1);
        self
    }
}

/// Why a request was abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailureReason {
    /// Every attempt went unanswered within the timeout.
    Timeout,
}

/// Something the scheduler wants the caller to do or know about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchedulerEvent {
    /// Write these requests **as one flush**: one `qAst` frame per key,
    /// back-to-back, then a single socket flush.
    Send {
        /// The requests to send, in order.
        batch: Vec<AssetKey>,
    },
    /// An attempt timed out and the request has been re-queued after a backoff.
    Retry {
        /// The request.
        key: AssetKey,
        /// How many sends have now happened.
        attempt: u32,
        /// When it becomes eligible again, in scheduler milliseconds.
        not_before_ms: u64,
    },
    /// A request ran out of attempts.
    Failed {
        /// The request.
        key: AssetKey,
        /// How many sends happened.
        attempts: u32,
        /// Why.
        reason: FailureReason,
    },
    /// A request was discarded before ever being sent.
    Dropped {
        /// The request.
        key: AssetKey,
        /// A short, log-ready reason.
        reason: &'static str,
    },
}

/// What [`AssetScheduler::request`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    /// The request is tracked and will be flushed.
    Queued {
        /// When the next flush is due, in scheduler milliseconds.
        flush_at_ms: u64,
    },
    /// The key was already queued or in flight; nothing changed.
    AlreadyPending,
    /// The queue is full; the request was refused.
    QueueFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryState {
    Queued { not_before: u64 },
    InFlight { sent_at: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    attempts: u32,
    state: EntryState,
}

/// A deterministic, monotonic time source for driving the scheduler.
///
/// Production code uses the wall clock; tests use this, which is the whole
/// point — the 500 ms spacing is asserted, not waited on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VirtualClock {
    now_ms: u64,
}

impl VirtualClock {
    /// A clock at time zero.
    #[must_use]
    pub fn new() -> Self {
        VirtualClock { now_ms: 0 }
    }

    /// A clock already at `now_ms`.
    #[must_use]
    pub fn at(now_ms: u64) -> Self {
        VirtualClock { now_ms }
    }

    /// The current virtual time.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Advance by `by_ms` and return the new time.
    pub fn advance(&mut self, by_ms: u64) -> u64 {
        self.now_ms = self.now_ms.saturating_add(by_ms);
        self.now_ms
    }

    /// Jump to `now_ms`, clamping to a monotonic minimum.
    pub fn set(&mut self, now_ms: u64) -> u64 {
        self.now_ms = self.now_ms.max(now_ms);
        self.now_ms
    }
}

/// Paces `qAst` requests and tracks their outcomes.
#[derive(Debug)]
pub struct AssetScheduler {
    cfg: SchedulerConfig,
    entries: HashMap<AssetKey, Entry>,
    queue: VecDeque<AssetKey>,
    next_flush_at: Option<u64>,
    last_now: u64,
}

impl AssetScheduler {
    /// Build a scheduler with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(SchedulerConfig::default())
    }

    /// Build a scheduler, clamping the config's pacing to flood-safe values.
    #[must_use]
    pub fn with_config(cfg: SchedulerConfig) -> Self {
        AssetScheduler {
            cfg: cfg.clamped(),
            entries: HashMap::new(),
            queue: VecDeque::new(),
            next_flush_at: None,
            last_now: 0,
        }
    }

    /// The effective (clamped) configuration.
    #[must_use]
    pub fn config(&self) -> SchedulerConfig {
        self.cfg
    }

    /// Ask for an asset.
    ///
    /// Idempotent: asking twice for the same [`AssetKey`] is one request. Use
    /// [`AssetScheduler::note_received_any_crc`] when the answer may carry a
    /// different CRC than the request did.
    pub fn request(&mut self, key: AssetKey, now_ms: u64) -> RequestOutcome {
        let now = self.now(now_ms);
        if self.entries.contains_key(&key) {
            return RequestOutcome::AlreadyPending;
        }
        if self.entries.len() >= self.cfg.max_queue {
            return RequestOutcome::QueueFull;
        }
        self.entries.insert(
            key,
            Entry {
                attempts: 0,
                state: EntryState::Queued { not_before: now },
            },
        );
        self.queue.push_back(key);
        let want = now.saturating_add(self.cfg.first_flush_ms);
        self.next_flush_at = Some(match self.next_flush_at {
            Some(t) => t.min(want),
            None => want,
        });
        RequestOutcome::Queued {
            flush_at_ms: self.next_flush_at.unwrap_or(now),
        }
    }

    /// Record that an asset arrived, cancelling any exact-key request for it.
    ///
    /// Returns true when a tracked request was satisfied.
    pub fn note_received(&mut self, key: &AssetKey) -> bool {
        self.remove(key).is_some()
    }

    /// Record that an asset arrived, cancelling every request for that
    /// (assetType, id) regardless of CRC.
    ///
    /// This is the case that matters in practice: a client asks with `crc == 0`
    /// ("don't care") and the server answers with the asset's real CRC, so an
    /// exact-key match would fail and the client would re-request forever.
    ///
    /// Returns the keys that were satisfied.
    pub fn note_received_any_crc(&mut self, asset_type: AssetType, id: i32) -> Vec<AssetKey> {
        let hit: Vec<AssetKey> = self
            .entries
            .keys()
            .filter(|k| k.asset_type == asset_type && k.id == id)
            .copied()
            .collect();
        for key in &hit {
            self.remove(key);
        }
        hit
    }

    /// Cancel a specific request.
    pub fn cancel(&mut self, key: &AssetKey) -> bool {
        self.remove(key).is_some()
    }

    /// Cancel every request matching a predicate. Returns how many went.
    pub fn cancel_if(&mut self, mut pred: impl FnMut(&AssetKey) -> bool) -> usize {
        let hit: Vec<AssetKey> = self.entries.keys().filter(|k| pred(k)).copied().collect();
        for key in &hit {
            self.remove(key);
        }
        hit.len()
    }

    /// Cancel every request, e.g. on a room change or disconnect.
    ///
    /// Returns how many were cancelled.
    pub fn cancel_all(&mut self) -> usize {
        let n = self.entries.len();
        self.entries.clear();
        self.queue.clear();
        self.next_flush_at = None;
        n
    }

    /// Number of tracked requests.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    /// Number of requests waiting for a flush.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Number of requests already sent and awaiting an answer.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.entries
            .values()
            .filter(|e| matches!(e.state, EntryState::InFlight { .. }))
            .count()
    }

    /// True when nothing is tracked.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.entries.is_empty()
    }

    /// This key's send count, or `None` when it is not tracked.
    #[must_use]
    pub fn attempts(&self, key: &AssetKey) -> Option<u32> {
        self.entries.get(key).map(|e| e.attempts)
    }

    /// The requests currently tracked, for logging.
    pub fn keys(&self) -> impl Iterator<Item = &AssetKey> {
        self.entries.keys()
    }

    /// When [`AssetScheduler::poll`] next has something to do, in the same time
    /// base the caller passes in.
    ///
    /// A real driver wakes at this instant (or on socket input), which is how
    /// the scheduler stays sleep-free.
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
        let mut earliest: Option<u64> = None;
        let mut merge = |t: u64| {
            earliest = Some(match earliest {
                Some(cur) => cur.min(t),
                None => t,
            });
        };
        if !self.queue.is_empty() {
            if let Some(t) = self.next_flush_at {
                merge(t);
            }
        }
        for entry in self.entries.values() {
            match entry.state {
                EntryState::Queued { not_before } => {
                    if not_before > self.last_now {
                        merge(not_before);
                    }
                }
                EntryState::InFlight { sent_at } => {
                    merge(sent_at.saturating_add(self.cfg.request_timeout_ms));
                }
            }
        }
        earliest
    }

    /// Advance the state machine to `now_ms` and report what to do.
    ///
    /// Time must be non-decreasing; a smaller value is clamped forward so a
    /// caller with a coarse clock cannot make a request look timed out early.
    pub fn poll(&mut self, now_ms: u64) -> Vec<SchedulerEvent> {
        let now = self.now(now_ms);
        let mut events = Vec::new();

        let timed_out: Vec<AssetKey> = self
            .entries
            .iter()
            .filter(|(_, e)| match e.state {
                EntryState::InFlight { sent_at } => {
                    now.saturating_sub(sent_at) >= self.cfg.request_timeout_ms
                }
                EntryState::Queued { .. } => false,
            })
            .map(|(k, _)| *k)
            .collect();

        for key in timed_out {
            let attempts = match self.entries.get(&key) {
                Some(entry) => entry.attempts,
                None => continue,
            };
            if attempts >= self.cfg.max_attempts {
                self.remove(&key);
                events.push(SchedulerEvent::Failed {
                    key,
                    attempts,
                    reason: FailureReason::Timeout,
                });
                continue;
            }
            let not_before = now.saturating_add(self.backoff_ms(attempts));
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.state = EntryState::Queued { not_before };
            }
            self.queue.push_back(key);
            self.next_flush_at = Some(match self.next_flush_at {
                Some(t) => t.min(now),
                None => now,
            });
            events.push(SchedulerEvent::Retry {
                key,
                attempt: attempts,
                not_before_ms: not_before,
            });
        }

        if self.queue.is_empty() {
            self.next_flush_at = None;
            return events;
        }
        let Some(due) = self.next_flush_at else {
            self.next_flush_at = Some(now.saturating_add(self.cfg.first_flush_ms));
            return events;
        };
        if now < due {
            return events;
        }

        let scan = self.queue.len();
        let mut eligible: Vec<AssetKey> = Vec::with_capacity(scan);
        let mut deferred: Vec<AssetKey> = Vec::new();
        for _ in 0..scan {
            let Some(key) = self.queue.pop_front() else {
                break;
            };
            match self.entries.get(&key) {
                Some(Entry {
                    state: EntryState::Queued { not_before },
                    ..
                }) if *not_before <= now => eligible.push(key),
                Some(_) => deferred.push(key),
                None => {}
            }
        }

        if eligible.len() > self.cfg.batch_size {
            let tail = eligible.split_off(self.cfg.batch_size);
            self.queue.extend(tail);
        }
        self.queue.extend(deferred);

        if eligible.is_empty() {
            let soonest_backoff = self
                .queue
                .iter()
                .filter_map(|k| match self.entries.get(k) {
                    Some(Entry {
                        state: EntryState::Queued { not_before },
                        ..
                    }) => Some(*not_before),
                    _ => None,
                })
                .min();
            let cadence = now.saturating_add(self.cfg.batch_interval_ms);
            self.next_flush_at = Some(match soonest_backoff {
                Some(t) if t > cadence => t,
                _ => cadence,
            });
            return events;
        }

        for key in &eligible {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.attempts = entry.attempts.saturating_add(1);
                entry.state = EntryState::InFlight { sent_at: now };
            }
        }
        self.next_flush_at = if self.queue.is_empty() {
            None
        } else {
            Some(now.saturating_add(self.cfg.batch_interval_ms))
        };
        events.push(SchedulerEvent::Send { batch: eligible });
        events
    }

    fn now(&mut self, now_ms: u64) -> u64 {
        self.last_now = self.last_now.max(now_ms);
        self.last_now
    }

    fn backoff_ms(&self, attempt: u32) -> u64 {
        let shift = attempt.saturating_sub(1).min(16);
        self.cfg
            .base_backoff_ms
            .saturating_mul(1u64 << shift)
            .min(self.cfg.max_backoff_ms)
    }

    fn remove(&mut self, key: &AssetKey) -> Option<Entry> {
        let entry = self.entries.remove(key)?;
        self.queue.retain(|k| k != key);
        if self.entries.is_empty() {
            self.queue.clear();
            self.next_flush_at = None;
        }
        Some(entry)
    }
}

impl Default for AssetScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience: the specs a `Send` batch asks for.
#[must_use]
pub fn batch_specs(batch: &[AssetKey]) -> Vec<AssetSpec> {
    batch.iter().map(AssetKey::spec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: i32) -> AssetKey {
        AssetKey::new(AssetType::PROP, id, 0)
    }

    fn flush_ids(events: &[SchedulerEvent]) -> Vec<Vec<i32>> {
        events
            .iter()
            .filter_map(|e| match e {
                SchedulerEvent::Send { batch } => Some(batch.iter().map(|k| k.id).collect()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_flood_is_impossible_to_configure() {
        let cfg = SchedulerConfig {
            batch_size: 5000,
            batch_interval_ms: 1,
            ..SchedulerConfig::default()
        }
        .clamped();
        assert_eq!(cfg.batch_size, REQUESTS_PER_BATCH);
        assert_eq!(cfg.batch_interval_ms, MIN_BATCH_INTERVAL_MS);

        let s = AssetScheduler::with_config(SchedulerConfig {
            batch_size: 0,
            batch_interval_ms: 0,
            ..SchedulerConfig::default()
        });
        assert_eq!(s.config().batch_size, 1);
        assert_eq!(s.config().batch_interval_ms, MIN_BATCH_INTERVAL_MS);
    }

    #[test]
    fn a_batch_never_exceeds_twenty_even_with_a_bigger_batch_size() {
        let mut s = AssetScheduler::new();
        for id in 0..200 {
            s.request(key(id), 0);
        }
        let events = s.poll(1000);
        for batch in flush_ids(&events) {
            assert!(batch.len() <= REQUESTS_PER_BATCH);
        }
        assert_eq!(flush_ids(&events)[0].len(), REQUESTS_PER_BATCH);
    }

    #[test]
    fn the_first_flush_is_debounced_by_the_configured_delay() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 0);
        assert!(flush_ids(&s.poll(49)).is_empty());
        assert_eq!(flush_ids(&s.poll(50)), vec![vec![1]]);
    }

    #[test]
    fn dedup_makes_a_repeated_request_a_no_op() {
        let mut s = AssetScheduler::new();
        assert!(matches!(
            s.request(key(7), 0),
            RequestOutcome::Queued { .. }
        ));
        assert_eq!(s.request(key(7), 10), RequestOutcome::AlreadyPending);
        assert_eq!(s.pending(), 1);
        assert_eq!(flush_ids(&s.poll(100)), vec![vec![7]]);
    }

    #[test]
    fn a_satisfied_request_is_not_flushed_again() {
        let mut s = AssetScheduler::new();
        s.request(key(7), 0);
        assert!(s.note_received(&key(7)));
        assert!(flush_ids(&s.poll(100)).is_empty());
        assert!(s.is_idle());
        assert!(!s.note_received(&key(7)));
    }

    #[test]
    fn a_dont_care_request_is_satisfied_by_an_answer_with_a_real_crc() {
        let mut s = AssetScheduler::new();
        s.request(key(7), 0);
        let satisfied = s.note_received_any_crc(AssetType::PROP, 7);
        assert_eq!(satisfied, vec![key(7)]);
        assert!(s.is_idle());
        assert!(s.note_received_any_crc(AssetType::PROP, 8).is_empty());
    }

    #[test]
    fn cancellation_is_immediate_and_total() {
        let mut s = AssetScheduler::new();
        for id in 0..5 {
            s.request(key(id), 0);
        }
        assert!(s.cancel(&key(0)));
        assert!(!s.cancel(&key(0)));
        assert_eq!(s.cancel_if(|k| k.id % 2 == 0), 2);
        assert_eq!(s.pending(), 2);
        assert_eq!(s.cancel_all(), 2);
        assert!(flush_ids(&s.poll(1000)).is_empty());
    }

    #[test]
    fn a_timeout_retries_with_exponential_backoff_then_fails() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 0);
        assert_eq!(flush_ids(&s.poll(50)), vec![vec![1]]);

        let t1 = 50 + DEFAULT_REQUEST_TIMEOUT_MS;
        match s.poll(t1).as_slice() {
            [SchedulerEvent::Retry {
                attempt: 1,
                not_before_ms,
                ..
            }] => assert_eq!(*not_before_ms, t1 + 500),
            other => panic!("expected a Retry backed off to 500ms, got {other:?}"),
        }

        let resend = t1 + 500;
        assert_eq!(flush_ids(&s.poll(resend)), vec![vec![1]]);
        assert_eq!(s.attempts(&key(1)), Some(2));

        let t2 = resend + DEFAULT_REQUEST_TIMEOUT_MS;
        match s.poll(t2).as_slice() {
            [SchedulerEvent::Retry {
                attempt: 2,
                not_before_ms,
                ..
            }] => assert_eq!(*not_before_ms, t2 + 1000),
            other => panic!("expected the second Retry backed off to 1000ms, got {other:?}"),
        }
    }

    #[test]
    fn a_dead_server_eventually_fails_every_request_instead_of_hanging() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 0);
        let mut now = 0;
        let mut failures = 0;
        for _ in 0..200 {
            now += 1000;
            for event in s.poll(now) {
                if let SchedulerEvent::Failed {
                    attempts, reason, ..
                } = event
                {
                    assert_eq!(attempts, DEFAULT_MAX_ATTEMPTS);
                    assert_eq!(reason, FailureReason::Timeout);
                    failures += 1;
                }
            }
            if s.is_idle() {
                break;
            }
        }
        assert_eq!(failures, 1);
        assert!(
            s.is_idle(),
            "the request must be given up on, not retried forever"
        );
    }

    #[test]
    fn a_queue_full_refuses_rather_than_growing_without_bound() {
        let mut s = AssetScheduler::with_config(SchedulerConfig {
            max_queue: 3,
            ..SchedulerConfig::default()
        });
        for id in 0..3 {
            assert!(matches!(
                s.request(key(id), 0),
                RequestOutcome::Queued { .. }
            ));
        }
        assert_eq!(s.request(key(9), 0), RequestOutcome::QueueFull);
        assert_eq!(s.pending(), 3);
    }

    #[test]
    fn time_is_clamped_monotonic_so_a_coarse_clock_cannot_invent_timeouts() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 1_000_000);
        assert!(flush_ids(&s.poll(1_000_050)).len() == 1);
        assert!(flush_ids(&s.poll(0)).is_empty());
        assert!(s.in_flight() == 1);
    }

    #[test]
    fn next_deadline_names_the_first_relevant_instant() {
        let mut s = AssetScheduler::new();
        assert_eq!(s.next_deadline(), None);
        s.request(key(1), 0);
        assert_eq!(s.next_deadline(), Some(50));
        let _ = s.poll(50);
        assert_eq!(s.next_deadline(), Some(50 + DEFAULT_REQUEST_TIMEOUT_MS));
        assert!(s.note_received(&key(1)));
        assert_eq!(s.next_deadline(), None);
    }

    #[test]
    fn deferred_backoff_does_not_starve_the_flush_of_ready_requests() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 0);
        let _ = s.poll(50);
        let _ = s.poll(50 + DEFAULT_REQUEST_TIMEOUT_MS);
        assert_eq!(s.queued(), 1);
        s.request(key(2), 50 + DEFAULT_REQUEST_TIMEOUT_MS);
        let events = s.poll(50 + DEFAULT_REQUEST_TIMEOUT_MS + DEFAULT_FIRST_FLUSH_MS + 1);
        assert_eq!(flush_ids(&events), vec![vec![2]]);
        assert_eq!(s.queued(), 1);
    }

    #[test]
    fn attempts_count_sends_not_timeouts() {
        let mut s = AssetScheduler::new();
        s.request(key(1), 0);
        assert_eq!(s.attempts(&key(1)), Some(0));
        let _ = s.poll(50);
        assert_eq!(s.attempts(&key(1)), Some(1));
        let _ = s.poll(50 + DEFAULT_REQUEST_TIMEOUT_MS);
        assert_eq!(
            s.attempts(&key(1)),
            Some(1),
            "a timeout schedules a retry; it is not itself an attempt"
        );
    }

    #[test]
    fn batch_specs_mirror_the_keys() {
        let batch = vec![AssetKey::new(AssetType::PROP, 4, 0x10)];
        assert_eq!(batch_specs(&batch), vec![AssetSpec::with_crc(4, 0x10)]);
    }

    #[test]
    fn the_virtual_clock_is_monotonic() {
        let mut c = VirtualClock::new();
        assert_eq!(c.now_ms(), 0);
        assert_eq!(c.advance(10), 10);
        assert_eq!(c.set(5), 10);
        assert_eq!(VirtualClock::at(3).now_ms(), 3);
    }
}

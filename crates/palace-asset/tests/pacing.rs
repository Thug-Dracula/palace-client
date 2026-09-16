//! The request pacing, asserted rather than waited on.
//!
//! The reference client's rule is "up to 20 requests per packet, separated by
//! 500 ms to prevent flooding the server and getting killed". Those two numbers
//! are what this file pins down, using a [`VirtualClock`] so the test finishes
//! instantly and the timing is exact instead of "about 500 ms".
//!
//! Every test drives the scheduler the way a real connection driver would: ask
//! [`AssetPipeline::next_deadline`] when to wake, jump the virtual clock there,
//! and `poll`. No test sleeps.

use palace_asset::{
    AssetKey, AssetPipeline, AssetScheduler, AssetType, PipelineConfig, PipelineEvent,
    SchedulerConfig, SchedulerEvent, VirtualClock, DEFAULT_REQUEST_TIMEOUT_MS,
    MIN_BATCH_INTERVAL_MS, REQUESTS_PER_BATCH,
};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;

const ORDER: ByteOrder = ByteOrder::Little;
const BATCH_ITERATION_LIMIT: usize = 10_000;

fn frame_count(events: &[PipelineEvent]) -> usize {
    events
        .iter()
        .filter_map(|e| match e {
            PipelineEvent::Send { frames } => Some(frames.len()),
            _ => None,
        })
        .sum()
}

fn frames_of(events: &[PipelineEvent]) -> Vec<Frame> {
    events
        .iter()
        .flat_map(|e| match e {
            PipelineEvent::Send { frames } => frames.clone(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn forty_five_requests_flush_twenty_at_a_time_every_five_hundred_milliseconds() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    for id in 0..45 {
        pipeline.request_prop(id, clock.now_ms());
    }

    let mut flushes: Vec<(u64, usize)> = Vec::new();
    for _ in 0..BATCH_ITERATION_LIMIT {
        let Some(due) = pipeline.next_deadline() else {
            break;
        };
        clock.set(due);
        let n = frame_count(&pipeline.poll(clock.now_ms()));
        if n > 0 {
            flushes.push((clock.now_ms(), n));
        }
        if pipeline.scheduler().queued() == 0 {
            break;
        }
    }

    assert_eq!(
        flushes,
        vec![(50, 20), (550, 20), (1050, 5)],
        "20, then 500ms later 20, then 500ms later the remaining 5"
    );
    for window in flushes.windows(2) {
        assert_eq!(
            window[1].0 - window[0].0,
            MIN_BATCH_INTERVAL_MS,
            "consecutive flushes must be exactly 500ms apart"
        );
    }
}

#[test]
fn no_flush_ever_carries_more_than_twenty_requests() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    for id in 0..500 {
        pipeline.request_prop(id, clock.now_ms());
    }

    let mut worst = 0usize;
    let mut total = 0usize;
    for _ in 0..BATCH_ITERATION_LIMIT {
        let Some(due) = pipeline.next_deadline() else {
            break;
        };
        clock.set(due);
        let n = frame_count(&pipeline.poll(clock.now_ms()));
        worst = worst.max(n);
        total += n;
        if pipeline.scheduler().queued() == 0 {
            break;
        }
    }
    assert_eq!(worst, REQUESTS_PER_BATCH);
    assert_eq!(total, 500, "every request is sent exactly once");
}

#[test]
fn a_batch_is_twenty_separate_twenty_four_byte_frames_not_one_big_one() {
    // The captures show a 48-byte client packet containing exactly two `tsAq`
    // mnemonics, so a "packet of 20 requests" is 20 frames, not one frame with a
    // 240-byte body.
    let mut pipeline = AssetPipeline::new();
    for id in 0..20 {
        pipeline.request_prop(id, 0);
    }
    let frames = frames_of(&pipeline.poll(50));
    assert_eq!(frames.len(), 20);
    for frame in &frames {
        assert_eq!(frame.encoded_len(), 24);
        assert_eq!(frame.payload.len(), 12);
        assert_eq!(frame.opcode.value(), 0x7141_7374);
        assert_eq!(&frame.encode(ORDER).unwrap()[..4], b"tsAq");
    }
}

#[test]
fn requests_arriving_during_an_idle_period_are_debounced_then_flushed() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    pipeline.request_prop(1, clock.now_ms());
    assert_eq!(pipeline.next_deadline(), Some(50));

    clock.advance(49);
    assert!(pipeline.poll(clock.now_ms()).is_empty());

    clock.advance(1);
    assert_eq!(frame_count(&pipeline.poll(clock.now_ms())), 1);
    assert_eq!(pipeline.scheduler().queued(), 0, "nothing left to flush");
    assert_eq!(pipeline.scheduler().in_flight(), 1);
}

#[test]
fn the_interval_cannot_be_tuned_below_the_compatibility_floor() {
    let cfg = SchedulerConfig {
        batch_size: 1000,
        batch_interval_ms: 1,
        ..SchedulerConfig::default()
    };
    let clamped = cfg.clamped();
    assert_eq!(clamped.batch_size, REQUESTS_PER_BATCH);
    assert_eq!(clamped.batch_interval_ms, MIN_BATCH_INTERVAL_MS);

    let pipeline = AssetPipeline::with_config(PipelineConfig {
        scheduler: cfg,
        ..PipelineConfig::default()
    });
    assert_eq!(pipeline.scheduler().config().batch_interval_ms, 500);
    assert_eq!(pipeline.scheduler().config().batch_size, 20);
}

#[test]
fn a_dead_server_does_not_hang_the_scheduler_forever() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    pipeline.request_prop(1, clock.now_ms());

    let mut failed = 0;
    let mut sends = 0;
    for _ in 0..BATCH_ITERATION_LIMIT {
        let Some(due) = pipeline.next_deadline() else {
            break;
        };
        clock.set(due);
        for event in pipeline.poll(clock.now_ms()) {
            match event {
                PipelineEvent::Send { frames } => sends += frames.len(),
                PipelineEvent::RequestFailed { .. } => failed += 1,
                _ => {}
            }
        }
        if pipeline.scheduler().is_idle() {
            break;
        }
        assert!(
            clock.now_ms() < 10 * 60 * 1000,
            "the scheduler must give up long before ten minutes"
        );
    }
    assert_eq!(failed, 1, "the request is failed, not retried forever");
    assert_eq!(sends, 4, "one send per attempt");
    assert!(pipeline.scheduler().is_idle());
}

#[test]
fn a_slow_server_that_answers_just_in_time_is_not_retried() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    let key = AssetKey::new(AssetType::PROP, 7, 0);
    pipeline.request(key, clock.now_ms());

    clock.advance(50);
    assert_eq!(frame_count(&pipeline.poll(clock.now_ms())), 1);

    clock.advance(DEFAULT_REQUEST_TIMEOUT_MS - 1);
    assert!(
        pipeline.poll(clock.now_ms()).is_empty(),
        "still inside the timeout window"
    );

    let satisfied = pipeline
        .scheduler_mut()
        .note_received_any_crc(AssetType::PROP, 7);
    assert_eq!(satisfied, vec![key]);
    clock.advance(10_000);
    assert!(pipeline.poll(clock.now_ms()).is_empty());
    assert_eq!(pipeline.next_deadline(), None);
}

#[test]
fn a_room_change_cancels_everything_and_stops_the_flushes() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    for id in 0..100 {
        pipeline.request_prop(id, clock.now_ms());
    }
    clock.advance(50);
    assert_eq!(frame_count(&pipeline.poll(clock.now_ms())), 20);
    assert_eq!(pipeline.scheduler().in_flight(), 20);
    assert_eq!(pipeline.scheduler().queued(), 80);

    assert_eq!(pipeline.reset(), 100, "in flight and queued both go");
    clock.advance(5000);
    assert!(pipeline.poll(clock.now_ms()).is_empty());
    assert_eq!(pipeline.next_deadline(), None);
}

#[test]
fn the_scheduler_reports_its_own_deadlines_so_a_driver_never_polls_blind() {
    let mut pipeline = AssetPipeline::new();
    assert_eq!(pipeline.next_deadline(), None);
    pipeline.request_prop(1, 1_000);
    assert_eq!(pipeline.next_deadline(), Some(1_050));
    let _ = pipeline.poll(1_050);
    assert_eq!(
        pipeline.next_deadline(),
        Some(1_050 + DEFAULT_REQUEST_TIMEOUT_MS)
    );
}

#[test]
fn the_raw_scheduler_emits_the_same_cadence_as_the_pipeline() {
    let mut clock = VirtualClock::new();
    let mut scheduler = AssetScheduler::new();
    for id in 0..25 {
        scheduler.request(AssetKey::new(AssetType::PROP, id, 0), clock.now_ms());
    }
    let mut batches: Vec<(u64, usize)> = Vec::new();
    for _ in 0..BATCH_ITERATION_LIMIT {
        let Some(due) = scheduler.next_deadline() else {
            break;
        };
        clock.set(due);
        for event in scheduler.poll(clock.now_ms()) {
            if let SchedulerEvent::Send { batch } = event {
                batches.push((clock.now_ms(), batch.len()));
            }
        }
        if scheduler.queued() == 0 {
            break;
        }
    }
    assert_eq!(batches, vec![(50, 20), (550, 5)]);
}

#[test]
fn a_retry_is_also_paced_and_never_lands_in_the_same_window_as_a_fresh_batch() {
    let mut clock = VirtualClock::new();
    let mut pipeline = AssetPipeline::new();
    pipeline.request_prop(1, clock.now_ms());

    // First send, then let it time out so it re-queues with a backoff.
    clock.set(50);
    assert_eq!(frame_count(&pipeline.poll(clock.now_ms())), 1);
    clock.set(50 + DEFAULT_REQUEST_TIMEOUT_MS);
    assert!(pipeline
        .poll(clock.now_ms())
        .iter()
        .all(|e| !matches!(e, PipelineEvent::Send { .. })));

    // A fresh request arriving now is not starved by the backed-off retry.
    let fresh_at = clock.now_ms();
    pipeline.request_prop(2, fresh_at);
    let mut flushes: Vec<(u64, usize)> = Vec::new();
    for _ in 0..BATCH_ITERATION_LIMIT {
        let Some(due) = pipeline.next_deadline() else {
            break;
        };
        clock.set(due);
        let n = frame_count(&pipeline.poll(clock.now_ms()));
        if n > 0 {
            flushes.push((clock.now_ms(), n));
        }
        let events_len = pipeline.scheduler().queued() + pipeline.scheduler().in_flight();
        if events_len == 0 || flushes.len() >= 2 {
            break;
        }
    }
    assert!(
        flushes.iter().any(|(t, n)| *n == 1 && *t <= fresh_at + 50),
        "the fresh request flushes within the debounce, got {flushes:?}"
    );
    for window in flushes.windows(2) {
        assert!(
            window[1].0 - window[0].0 >= MIN_BATCH_INTERVAL_MS,
            "flushes must stay at least 500ms apart, got {flushes:?}"
        );
    }
}

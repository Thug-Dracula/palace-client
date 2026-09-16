//! The coordinator: one object that owns the assembler, the scheduler and the
//! asset cache, and turns Palace frames into frames-to-send.
//!
//! `palace-client` owns the socket and the connection state machine. It should
//! not also have to know how blocks are reassembled, when the next `qAst` flush
//! is due, or what the CRC rule is. This type is that boundary:
//!
//! ```text
//!    socket read ──► on_frame(&Frame) ──► Vec<PipelineEvent>
//!                                          │  Send  → qAst frames to flush
//!                                          │  Serve → rAst frames to flush
//!                                          │  …      → log / metrics
//!    clock wake ──► poll(now_ms) ─────────┘
//!    next_deadline() tells the driver when to wake
//! ```
//!
//! ## The three opcodes it implements
//!
//! | Opcode | Direction | What this does |
//! |---|---|---|
//! | `qAst` (`ASSETQUERY`) | both | inbound: a peer wants something we may hold → `rAst` reply. outbound: the [scheduler](crate::scheduler) emits these in batches of 20, 500 ms apart |
//! | `sAst` (`ASSETSEND`) | server → client | decode, assemble across blocks, verify the CRC, cache |
//! | `rAst` (`ASSETREGI`) | client → server | serve a prop we already hold, in the peer-propagation path |
//!
//! `HTTP` is handled here too, because the media base URL is asset-layer state.
//!
//! Server-initiated `qAst` is the one message where the client is the *sender*
//! of an asset, and the reference client is picky about it: it refuses to serve
//! anything that is not a 44×44 prop, because a larger prop is a "web service
//! prop" the server does not want. That size gate is deliberately **not**
//! enforced here — this crate does not decode props, so it cannot know the
//! dimensions. The gate belongs in `palace-client`, and
//! [`PipelineEvent::Serve`] makes the decision visible at the call site instead
//! of hiding it behind an `if width == 44`.

use std::collections::HashSet;

use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;
use palace_wire::messages::HttpServer;
use palace_wire::opcode::{ASSETQUERY, ASSETSEND, HTTPSERVER};
use palace_wire::{Opcode, Reader};

use crate::assembler::{Asset, AssetAssembler, AssemblerConfig, AssemblyOutcome};
use crate::asset::{AssetKey, AssetSpec, AssetTransfer, AssetType, OP_REGI};
use crate::cache::{AssetCache, DEFAULT_ASSET_CACHE_CAPACITY};
use crate::error::{AssetError, Result};
use crate::query::{encode_batch_typed, AssetQuery};
use crate::scheduler::{
    AssetScheduler, FailureReason, RequestOutcome, SchedulerConfig, SchedulerEvent,
};

/// Tunables for a whole pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineConfig {
    /// Assembler limits.
    pub assembler: AssemblerConfig,
    /// Scheduler timings.
    pub scheduler: SchedulerConfig,
    /// In-memory asset cache capacity.
    pub asset_cache_capacity: usize,
    /// `refNum` for outgoing `qAst` frames.
    ///
    /// The protocol reference says `qAst`'s `refNum` is unused and should be 0.
    /// Both reference clients put the client's own user id there instead, and
    /// the server ignores it either way; 0 is the protocol-correct default.
    pub query_ref_num: i32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        PipelineConfig {
            assembler: AssemblerConfig::default(),
            scheduler: SchedulerConfig::default(),
            asset_cache_capacity: DEFAULT_ASSET_CACHE_CAPACITY,
            query_ref_num: 0,
        }
    }
}

/// Something the pipeline wants the connection to do, or to know.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipelineEvent {
    /// Write these `qAst` frames back-to-back and flush once.
    Send {
        /// One 24-byte frame per request.
        frames: Vec<Frame>,
    },
    /// Write these `rAst` frames back-to-back and flush once — peer propagation.
    Serve {
        /// One frame per asset served.
        frames: Vec<Frame>,
    },
    /// A complete, CRC-verified asset is in the cache.
    AssetReady {
        /// Its key; look it up in [`AssetPipeline::cache`].
        key: AssetKey,
        /// Its descriptor name, often empty.
        name: String,
        /// Its length in bytes.
        len: usize,
    },
    /// An asset could not be used. Its partial state has been dropped.
    AssetRejected {
        /// Its key.
        key: AssetKey,
        /// Log-ready reason.
        error: String,
    },
    /// A block arrived and more are expected.
    Partial {
        /// The asset.
        key: AssetKey,
        /// Blocks held.
        received: usize,
        /// Blocks expected.
        expected: usize,
    },
    /// A block was dropped — usually a later block whose first block never
    /// arrived.
    Discarded {
        /// The asset.
        key: AssetKey,
        /// Log-ready reason.
        reason: &'static str,
    },
    /// The media server URL arrived via `HTTP`.
    MediaServer {
        /// The base URL, as sent.
        url: String,
    },
    /// The server asked for an asset.
    PeerRequest {
        /// What it asked for.
        key: AssetKey,
        /// True when we answered immediately.
        served: bool,
    },
    /// A request ran out of attempts.
    RequestFailed {
        /// The request.
        key: AssetKey,
        /// Sends attempted.
        attempts: u32,
    },
    /// A request was refused before being sent.
    RequestDropped {
        /// The request.
        key: AssetKey,
        /// Log-ready reason.
        reason: &'static str,
    },
}

/// What [`AssetPipeline::request`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDisposition {
    /// Already cached; nothing to fetch.
    Cached,
    /// Queued for a flush.
    Queued,
    /// Already queued or in flight.
    AlreadyPending,
    /// The scheduler's queue is full.
    QueueFull,
}

/// Owns the asset state for one connection.
#[derive(Debug)]
pub struct AssetPipeline {
    cfg: PipelineConfig,
    assembler: AssetAssembler,
    scheduler: AssetScheduler,
    cache: AssetCache,
    media_server: Option<String>,
    peer_wanted: HashSet<AssetKey>,
    order: ByteOrder,
}

impl AssetPipeline {
    /// Build a pipeline with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(PipelineConfig::default())
    }

    /// Build a pipeline with explicit configuration.
    #[must_use]
    pub fn with_config(cfg: PipelineConfig) -> Self {
        AssetPipeline {
            assembler: AssetAssembler::with_config(cfg.assembler),
            scheduler: AssetScheduler::with_config(cfg.scheduler),
            cache: AssetCache::new(cfg.asset_cache_capacity),
            media_server: None,
            peer_wanted: HashSet::new(),
            order: ByteOrder::Little,
            cfg,
        }
    }

    /// The configuration in force.
    #[must_use]
    pub fn config(&self) -> PipelineConfig {
        self.cfg
    }

    /// The session byte order used to encode outgoing frames.
    #[must_use]
    pub fn byte_order(&self) -> ByteOrder {
        self.order
    }

    /// Fix the session byte order.
    ///
    /// The connection learns this from the `TIYID` banner, long before any
    /// asset traffic; the pipeline defaults to little-endian and also adopts
    /// whatever [`AssetPipeline::on_frame`] was given.
    pub fn set_byte_order(&mut self, order: ByteOrder) {
        self.order = order;
    }

    /// The scheduler.
    #[must_use]
    pub fn scheduler(&self) -> &AssetScheduler {
        &self.scheduler
    }

    /// The scheduler, mutably — for cancellation.
    pub fn scheduler_mut(&mut self) -> &mut AssetScheduler {
        &mut self.scheduler
    }

    /// The assembler.
    #[must_use]
    pub fn assembler(&self) -> &AssetAssembler {
        &self.assembler
    }

    /// The asset cache.
    #[must_use]
    pub fn cache(&self) -> &AssetCache {
        &self.cache
    }

    /// The asset cache, mutably.
    pub fn cache_mut(&mut self) -> &mut AssetCache {
        &mut self.cache
    }

    /// The media server base URL from the last `HTTP` message.
    #[must_use]
    pub fn media_server(&self) -> Option<&str> {
        self.media_server.as_deref()
    }

    /// Set the `refNum` used on outgoing `qAst` frames (the client's user id).
    pub fn set_query_ref_num(&mut self, ref_num: i32) {
        self.cfg.query_ref_num = ref_num;
    }

    /// Ask for an asset, unless it is already cached or already wanted.
    pub fn request(&mut self, key: AssetKey, now_ms: u64) -> RequestDisposition {
        if self.cache.contains(&key) || self.cache.contains_id(key.asset_type, key.id) {
            return RequestDisposition::Cached;
        }
        match self.scheduler.request(key, now_ms) {
            RequestOutcome::Queued { .. } => RequestDisposition::Queued,
            RequestOutcome::AlreadyPending => RequestDisposition::AlreadyPending,
            RequestOutcome::QueueFull => RequestDisposition::QueueFull,
        }
    }

    /// Ask for a prop by id, accepting any CRC.
    pub fn request_prop(&mut self, id: i32, now_ms: u64) -> RequestDisposition {
        self.request(AssetKey::new(AssetType::PROP, id, 0), now_ms)
    }

    /// Advance the clock; emit the flush that is due, if any.
    pub fn poll(&mut self, now_ms: u64) -> Vec<PipelineEvent> {
        let events = self.scheduler.poll(now_ms);
        let mut out = Vec::with_capacity(events.len());
        for event in events {
            match event {
                SchedulerEvent::Send { batch } => {
                    let requests: Vec<AssetQuery> = batch
                        .iter()
                        .map(|k| AssetQuery {
                            asset_type: k.asset_type,
                            spec: k.spec(),
                        })
                        .collect();
                    match encode_batch_typed(&requests, self.cfg.query_ref_num, self.order) {
                        Ok(frames) => out.push(PipelineEvent::Send { frames }),
                        Err(e) => {
                            for key in batch {
                                out.push(PipelineEvent::AssetRejected {
                                    key,
                                    error: format!("failed to encode qAst: {e}"),
                                });
                            }
                        }
                    }
                }
                SchedulerEvent::Retry { .. } => {}
                SchedulerEvent::Failed {
                    key, attempts, ..
                } => out.push(PipelineEvent::RequestFailed { key, attempts }),
                SchedulerEvent::Dropped { key, reason } => {
                    out.push(PipelineEvent::RequestDropped { key, reason });
                }
            }
        }
        out
    }

    /// When [`AssetPipeline::poll`] next has work.
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
        self.scheduler.next_deadline()
    }

    /// Feed an inbound frame in and get back anything the connection should do.
    ///
    /// Frames this layer does not own (`room`, `talk`, `uSta`, …) produce no
    /// events and no error — the caller dispatches them elsewhere.
    pub fn on_frame(&mut self, frame: &Frame, order: ByteOrder, now_ms: u64) -> Vec<PipelineEvent> {
        let _ = now_ms;
        self.order = order;
        let mut out = Vec::new();

        if frame.opcode == ASSETSEND {
            match AssetTransfer::decode(&frame.payload, order) {
                Ok(transfer) => self.on_transfer(transfer, &mut out),
                Err(e) => out.push(PipelineEvent::AssetRejected {
                    key: AssetKey::new(AssetType::PROP, 0, 0),
                    error: format!("malformed sAst: {e}"),
                }),
            }
            return out;
        }

        if frame.opcode == ASSETQUERY {
            if let Ok(query) = AssetQuery::decode(&frame.payload, order) {
                self.on_peer_request(query, &mut out);
            }
            return out;
        }

        if frame.opcode == HTTPSERVER {
            let mut r = Reader::new(&frame.payload, order);
            if let Ok(HttpServer { url }) = HttpServer::decode(&mut r) {
                self.media_server = Some(url.clone());
                out.push(PipelineEvent::MediaServer { url });
            }
        }

        out
    }

    fn on_transfer(&mut self, transfer: AssetTransfer, out: &mut Vec<PipelineEvent>) {
        match self.assembler.ingest(transfer) {
            AssemblyOutcome::Complete(asset) => self.finish(*asset, out),
            AssemblyOutcome::Pending {
                key,
                received,
                expected,
            } => out.push(PipelineEvent::Partial {
                key,
                received,
                expected,
            }),
            AssemblyOutcome::Rejected { key, error } => out.push(PipelineEvent::AssetRejected {
                key,
                error: error.to_string(),
            }),
            AssemblyOutcome::Discarded { key, reason } => {
                out.push(PipelineEvent::Discarded { key, reason });
            }
        }
    }

    fn finish(&mut self, asset: Asset, out: &mut Vec<PipelineEvent>) {
        let key = asset.key;
        let name = asset.name.clone();
        let len = asset.len();

        self.scheduler.note_received(&key);
        self.scheduler.note_received_any_crc(key.asset_type, key.id);

        if self.take_peer_want(&key) {
            let event = self.serve_frames(key, &name, &asset.data);
            out.push(event);
        }

        self.cache.insert(asset);
        out.push(PipelineEvent::AssetReady { key, name, len });
    }

    fn take_peer_want(&mut self, key: &AssetKey) -> bool {
        if self.peer_wanted.remove(key) {
            return true;
        }
        let wildcard = self
            .peer_wanted
            .iter()
            .find(|k| k.asset_type == key.asset_type && k.id == key.id)
            .copied();
        match wildcard {
            Some(k) => self.peer_wanted.remove(&k),
            None => false,
        }
    }

    fn on_peer_request(&mut self, query: AssetQuery, out: &mut Vec<PipelineEvent>) {
        let key = AssetKey::from_spec(query.asset_type, query.spec);
        let held = self
            .cache
            .peek_by_id(query.asset_type, query.spec.id)
            .map(|asset| (asset.name.clone(), asset.data.clone()));
        match held {
            Some((name, data)) => {
                let event = self.serve_frames(key, &name, &data);
                out.push(event);
            }
            None => {
                self.peer_wanted.insert(key);
                out.push(PipelineEvent::PeerRequest { key, served: false });
            }
        }
    }

    fn serve_frames(&self, key: AssetKey, name: &str, data: &[u8]) -> PipelineEvent {
        let transfer = AssetTransfer::single_block(key.asset_type, key.spec(), name, data);
        match transfer.encode_frame(OP_REGI, 0, self.order) {
            Ok(frame) => PipelineEvent::Serve {
                frames: vec![frame],
            },
            Err(e) => PipelineEvent::AssetRejected {
                key,
                error: format!("failed to encode rAst: {e}"),
            },
        }
    }

    /// Serve a cached asset to a peer explicitly.
    pub fn serve(&mut self, key: &AssetKey) -> Result<PipelineEvent> {
        let (name, data) = self
            .cache
            .peek_by_id(key.asset_type, key.id)
            .map(|a| (a.name.clone(), a.data.clone()))
            .ok_or(AssetError::NotCached {
                asset_type: key.asset_type.as_raw(),
                id: key.id,
                crc: key.crc,
            })?;
        Ok(self.serve_frames(*key, &name, &data))
    }

    /// Drop every pending request and partial asset, e.g. on a room change.
    pub fn reset(&mut self) -> usize {
        self.peer_wanted.clear();
        self.assembler.clear();
        self.scheduler.cancel_all()
    }
}

impl Default for AssetPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// True when a failure is worth retrying rather than reporting as terminal.
///
/// Every reason [`FailureReason`] currently has is retryable; the function
/// exists so a caller can be total over the enum without a wildcard match.
#[must_use]
pub fn is_retryable(reason: FailureReason) -> bool {
    match reason {
        FailureReason::Timeout => true,
    }
}

/// Convenience: the `qAst` spec a cache key would use.
#[must_use]
pub fn spec_of(key: &AssetKey) -> AssetSpec {
    key.spec()
}

/// Decode a `qAst` body, for callers that dispatch `ASSETQUERY` themselves.
pub fn decode_query(payload: &[u8], order: ByteOrder) -> Result<AssetQuery> {
    AssetQuery::decode(payload, order)
}

/// True when a decoded opcode is one this layer owns.
#[must_use]
pub fn owns(opcode: Opcode) -> bool {
    opcode == ASSETSEND || opcode == ASSETQUERY || opcode == HTTPSERVER
}

#[cfg(test)]
mod tests {
    use super::*;
    use palace_wire::opcode::{ASSETSEND as OP_SEND, TIYID};

    const ORDER: ByteOrder = ByteOrder::Little;

    fn prop_blob(payload: &[u8]) -> (Vec<u8>, u32) {
        let mut blob = vec![44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 2, 0];
        blob.extend_from_slice(payload);
        let crc = palace_prop::payload_crc(&blob).unwrap();
        (blob, crc)
    }

    fn send_frame(spec: AssetSpec, blob: &[u8]) -> Frame {
        AssetTransfer::single_block(AssetType::PROP, spec, "", blob)
            .encode_frame(OP_SEND, 0, ORDER)
            .unwrap()
    }

    fn send_count(events: &[PipelineEvent]) -> usize {
        events
            .iter()
            .filter_map(|e| match e {
                PipelineEvent::Send { frames } => Some(frames.len()),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn requesting_an_already_cached_asset_is_a_no_op() {
        let mut p = AssetPipeline::new();
        let (blob, crc) = prop_blob(&[1, 2, 3]);
        let spec = AssetSpec::with_crc(77, crc);
        let events = p.on_frame(&send_frame(spec, &blob), ORDER, 0);
        assert!(events
            .iter()
            .any(|e| matches!(e, PipelineEvent::AssetReady { .. })));
        assert_eq!(
            p.request(AssetKey::new(AssetType::PROP, 77, crc), 0),
            RequestDisposition::Cached
        );
        assert_eq!(p.request_prop(77, 0), RequestDisposition::Cached);
    }

    #[test]
    fn a_room_full_of_requests_flushes_in_twenty_frame_batches() {
        let mut p = AssetPipeline::new();
        for id in 0..45 {
            assert_eq!(p.request_prop(id, 0), RequestDisposition::Queued);
        }
        assert_eq!(p.request_prop(0, 5), RequestDisposition::AlreadyPending);

        let first = p.poll(50);
        assert_eq!(send_count(&first), 20, "the first packet holds 20 requests");
        for event in &first {
            if let PipelineEvent::Send { frames } = event {
                for frame in frames {
                    assert_eq!(frame.opcode, ASSETQUERY);
                    assert_eq!(frame.encoded_len(), 24);
                    assert_eq!(frame.ref_num, 0);
                }
            }
        }

        assert!(p.poll(549).is_empty(), "nothing before the 500ms boundary");
        assert_eq!(send_count(&p.poll(550)), 20);
        assert_eq!(send_count(&p.poll(1050)), 5);
        assert!(p.poll(2000).is_empty());
    }

    #[test]
    fn a_dont_care_request_is_satisfied_by_the_servers_real_crc() {
        let mut p = AssetPipeline::new();
        let (blob, crc) = prop_blob(&[4, 5, 6]);
        assert_eq!(p.request_prop(99, 0), RequestDisposition::Queued);
        let _ = p.poll(50);
        let events = p.on_frame(&send_frame(AssetSpec::with_crc(99, crc), &blob), ORDER, 60);
        assert!(events
            .iter()
            .any(|e| matches!(e, PipelineEvent::AssetReady { .. })));
        assert!(p.scheduler().is_idle(), "the queued request must be satisfied");
        assert!(p.cache().contains_id(AssetType::PROP, 99));
    }

    #[test]
    fn a_corrupt_asset_is_rejected_and_logged_not_cached() {
        let mut p = AssetPipeline::new();
        let (blob, crc) = prop_blob(&[7, 8, 9]);
        let events = p.on_frame(
            &send_frame(AssetSpec::with_crc(1, crc ^ 0xff), &blob),
            ORDER,
            0,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            PipelineEvent::AssetRejected { error, .. } => assert!(error.contains("CRC")),
            other => panic!("expected AssetRejected, got {other:?}"),
        }
        assert!(p.cache().is_empty());
    }

    #[test]
    fn a_malformed_sast_does_not_panic_and_is_surfaced() {
        let mut p = AssetPipeline::new();
        let frame = Frame::new(OP_SEND, 0, vec![0xff; 10]);
        let events = p.on_frame(&frame, ORDER, 0);
        assert!(matches!(
            events.as_slice(),
            [PipelineEvent::AssetRejected { .. }]
        ));
    }

    #[test]
    fn a_peer_request_for_a_held_prop_is_answered_with_rast() {
        let mut p = AssetPipeline::new();
        let (blob, crc) = prop_blob(&[1, 1, 1]);
        let _ = p.on_frame(&send_frame(AssetSpec::with_crc(5, crc), &blob), ORDER, 0);

        let query = AssetQuery::prop(5, 0).encode_frame(0, ORDER);
        let events = p.on_frame(&query, ORDER, 0);
        match events.as_slice() {
            [PipelineEvent::Serve { frames }] => {
                assert_eq!(frames.len(), 1);
                assert_eq!(frames[0].opcode, OP_REGI);
                let back = AssetTransfer::decode(&frames[0].payload, ORDER).unwrap();
                assert_eq!(back.data, blob);
                assert_eq!(back.header.block_count, 1);
                assert!(back.descriptor.is_some());
            }
            other => panic!("expected Serve, got {other:?}"),
        }
    }

    #[test]
    fn a_peer_request_for_a_missing_prop_defers_until_it_arrives() {
        let mut p = AssetPipeline::new();
        let query = AssetQuery::prop(1234, 0).encode_frame(0, ORDER);
        let events = p.on_frame(&query, ORDER, 0);
        match events.as_slice() {
            [PipelineEvent::PeerRequest { key, served: false }] => assert_eq!(key.id, 1234),
            other => panic!("expected an unserved PeerRequest, got {other:?}"),
        }

        let (blob, crc) = prop_blob(&[2, 2, 2]);
        let events = p.on_frame(&send_frame(AssetSpec::with_crc(1234, crc), &blob), ORDER, 1);
        assert!(events
            .iter()
            .any(|e| matches!(e, PipelineEvent::Serve { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, PipelineEvent::AssetReady { .. })));
    }

    #[test]
    fn the_http_opcode_sets_the_media_server() {
        let mut p = AssetPipeline::new();
        let payload = b"https://media.palace.example.info/palace/media\0".to_vec();
        let events = p.on_frame(&Frame::new(HTTPSERVER, 0, payload), ORDER, 0);
        assert_eq!(
            events.as_slice(),
            [PipelineEvent::MediaServer {
                url: "https://media.palace.example.info/palace/media".into()
            }]
        );
        assert_eq!(
            p.media_server(),
            Some("https://media.palace.example.info/palace/media")
        );
    }

    #[test]
    fn outgoing_frames_follow_the_session_byte_order() {
        let mut p = AssetPipeline::new();
        p.set_byte_order(ByteOrder::Big);
        p.request_prop(1, 0);
        let events = p.poll(50);
        let frames: Vec<Frame> = events
            .iter()
            .flat_map(|e| match e {
                PipelineEvent::Send { frames } => frames.clone(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(frames.len(), 1);
        let bytes = frames[0].encode(ByteOrder::Big).unwrap();
        assert_eq!(&bytes[..4], b"qAst");
        assert_eq!(p.byte_order(), ByteOrder::Big);
    }

    #[test]
    fn unrelated_frames_pass_through_silently() {
        let mut p = AssetPipeline::new();
        assert!(p.on_frame(&Frame::empty(TIYID, 3), ORDER, 0).is_empty());
        assert!(p.media_server().is_none());
    }

    #[test]
    fn a_failed_request_is_reported_and_can_be_retried_by_the_caller() {
        let mut p = AssetPipeline::new();
        p.request_prop(1, 0);
        let mut now = 0;
        let mut failed = 0;
        for _ in 0..300 {
            now += 1000;
            for event in p.poll(now) {
                if matches!(event, PipelineEvent::RequestFailed { .. }) {
                    failed += 1;
                }
            }
            if p.scheduler().is_idle() {
                break;
            }
        }
        assert_eq!(failed, 1);
    }

    #[test]
    fn next_deadline_tracks_the_scheduler() {
        let mut p = AssetPipeline::new();
        assert_eq!(p.next_deadline(), None);
        p.request_prop(1, 0);
        assert_eq!(p.next_deadline(), Some(50));
        let _ = p.poll(50);
        assert_eq!(
            p.next_deadline(),
            Some(50 + crate::scheduler::DEFAULT_REQUEST_TIMEOUT_MS)
        );
    }

    #[test]
    fn reset_drops_requests_partials_and_peer_wants() {
        let mut p = AssetPipeline::new();
        let (blob, crc) = prop_blob(&[1, 2, 3, 4]);
        p.request_prop(1, 0);
        let query = AssetQuery::prop(2222, 0).encode_frame(0, ORDER);
        let _ = p.on_frame(&query, ORDER, 0);

        let mut transfer =
            AssetTransfer::single_block(AssetType::PROP, AssetSpec::with_crc(2, crc), "", &blob);
        transfer.header.block_count = 2;
        let frame = transfer.encode_frame(OP_SEND, 0, ORDER).unwrap();
        let _ = p.on_frame(&frame, ORDER, 0);
        assert_eq!(p.assembler().pending_count(), 1);

        assert_eq!(p.reset(), 1);
        assert_eq!(p.assembler().pending_count(), 0);
        assert!(p.scheduler().is_idle());

        let events = p.on_frame(&send_frame(AssetSpec::with_crc(2222, crc), &blob), ORDER, 1);
        assert!(!events
            .iter()
            .any(|e| matches!(e, PipelineEvent::Serve { .. })));
    }

    #[test]
    fn serving_something_not_cached_is_an_error_not_a_panic() {
        let mut p = AssetPipeline::new();
        let err = p
            .serve(&AssetKey::new(AssetType::PROP, 4242, 0))
            .unwrap_err();
        assert!(matches!(err, AssetError::NotCached { .. }));
    }

    #[test]
    fn helpers_are_total() {
        assert!(is_retryable(FailureReason::Timeout));
        assert_eq!(
            spec_of(&AssetKey::new(AssetType::PROP, 1, 2)),
            AssetSpec::with_crc(1, 2)
        );
        assert!(owns(ASSETQUERY));
        assert!(owns(ASSETSEND));
        assert!(owns(HTTPSERVER));
        assert!(!owns(TIYID));
        assert!(decode_query(&AssetQuery::prop(1, 0).encode_frame(0, ORDER).payload, ORDER).is_ok());
    }
}

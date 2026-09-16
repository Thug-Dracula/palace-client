//! `palace-asset` — the Palace asset layer.
//!
//! Palace clients do not ship their props, avatars or room backgrounds. They ask
//! the server for them by id and CRC, and every client that already holds one
//! helps serve it to the ones that do not. This crate is that whole layer:
//!
//! | Module | Owns |
//! |---|---|
//! | [`asset`] | the asset types and the `sAst`/`rAst` block record |
//! | [`query`] | the `qAst` request record |
//! | [`assembler`] | multi-block reassembly and per-asset CRC verification |
//! | [`scheduler`] | the 20-per-flush, 500 ms-apart request pacing |
//! | [`media`] | the HTTP fetch for backgrounds and hotspot images |
//! | [`cache`] | the in-memory asset cache and the on-disk media cache |
//! | [`pipeline`] | the coordinator that turns frames into frames-to-send |
//!
//! ## Quick start
//!
//! ```
//! use palace_asset::{AssetKey, AssetPipeline, AssetType, PipelineEvent};
//! use palace_wire::ByteOrder;
//!
//! let mut pipeline = AssetPipeline::new();
//! pipeline.set_byte_order(ByteOrder::Little);
//!
//! // A room description named prop 1234; ask for it.
//! pipeline.request(AssetKey::new(AssetType::PROP, 1234, 0), 0);
//!
//! // Drive the clock. The first flush is debounced by 50 ms and carries one
//! // `qAst` frame per request, 20 at most.
//! let events = pipeline.poll(50);
//! assert!(matches!(events.as_slice(), [PipelineEvent::Send { .. }]));
//! ```
//!
//! ## What this crate deliberately does not do
//!
//! * **Decode props.** The output is a complete, CRC-verified blob; `palace-prop`
//!   turns it into pixels. This crate only knows one thing about prop bytes, and
//!   that is which slice the CRC covers.
//! * **Own the socket.** No I/O except the media HTTP fetch, which is behind the
//!   [`HttpTransport`](media::HttpTransport) trait.
//! * **Sleep.** [`AssetScheduler::poll`](scheduler::AssetScheduler::poll) takes
//!   the time as an argument, so the pacing is asserted in tests rather than
//!   waited on, and a real driver just wakes at
//!   [`next_deadline`](scheduler::AssetScheduler::next_deadline).
//!
//! ## Hard rules encoded here
//!
//! * The 20-per-packet / 500 ms pacing is clamped, not merely defaulted — see
//!   [`scheduler`].
//! * No network-derived size drives an allocation. `blockSize`, `blockCount` and
//!   the descriptor's `size` are all checked against the bytes that actually
//!   arrived and against explicit ceilings before anything is copied.
//! * A corrupt or partial asset is dropped and reported, never rendered and
//!   never a panic.
//! * Media file names are sanitised before they touch the filesystem.
//!
//! ## Evidence
//!
//! `README.md` records, field by field, which parts of this implementation are
//! confirmed against real captures and which are reference-derived only. The
//! short version: the `sAst` layout, the absence of trailing padding and the
//! one-frame-per-request `qAst` shape are all measured; multi-block transfer is
//! implemented from the protocol reference because no capture in the corpus
//! contains one.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod assembler;
pub mod asset;
pub mod cache;
pub mod error;
pub mod media;
pub mod pipeline;
pub mod query;
pub mod scheduler;

pub use assembler::{
    prop_payload_crc, raw_asset_crc, Asset, AssetAssembler, AssemblerConfig, AssemblyOutcome,
    CrcPolicy,
};
pub use asset::{
    AssetDescriptor, AssetKey, AssetSpec, AssetTransfer, AssetType, BlockHeader, OP_REGI, OP_SEND,
};
pub use cache::{sanitise_media_name, AssetCache, MediaCache};
pub use error::{
    AssetError, Result, BLOCK_HEADER_LEN, DESCRIPTOR_LEN, MAX_ASSET_SIZE, MAX_BLOCK_COUNT,
    MAX_BLOCK_SIZE, NAME_FIELD_LEN,
};
pub use media::{
    fallback_chain, media_url, HttpTransport, HttpResponse, MediaConfig, MediaFetch, MediaFetcher,
    UreqTransport,
};
pub use pipeline::{
    decode_query, is_retryable, owns, spec_of, AssetPipeline, PipelineConfig, PipelineEvent,
    RequestDisposition,
};
pub use query::{encode_batch, encode_batch_typed, AssetQuery, QUERY_BODY_LEN, REQUESTS_PER_BATCH};
pub use scheduler::{
    batch_specs, AssetScheduler, FailureReason, RequestOutcome, SchedulerConfig, SchedulerEvent,
    VirtualClock, DEFAULT_FIRST_FLUSH_MS, DEFAULT_MAX_ATTEMPTS, DEFAULT_REQUEST_TIMEOUT_MS,
    MIN_BATCH_INTERVAL_MS,
};

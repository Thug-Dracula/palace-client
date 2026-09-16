//! Hostile-input robustness.
//!
//! Everything in this file comes from a deterministic PRNG or from integers a
//! real server would never send, so the suite is hermetic and reproducible. The
//! properties under test are the ones the task states outright:
//!
//! * no `unwrap`/`panic` on network-derived data — decode the bytes and get a
//!   value;
//! * a corrupt or partial asset is **dropped**, not rendered;
//! * a hostile `blockSize`/`assetSize` must not cause a huge allocation.
//!
//! The last one is measured, not asserted by inspection: the test binary
//! installs a counting global allocator and checks that decoding a frame which
//! *claims* a 2 GiB block grows live memory by well under a megabyte.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use palace_asset::{
    AssetAssembler, AssetKey, AssetPipeline, AssetSpec, AssetTransfer, AssetType, AssemblerConfig,
    AssetScheduler, AssemblyOutcome, MediaCache,
};
use palace_wire::byteorder::ByteOrder;
use palace_wire::frame::Frame;
use palace_wire::opcode::{ASSETQUERY, ASSETSEND};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// xorshift64*: deterministic, no dependency, good enough to shake out panics.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

const ORDER: ByteOrder = ByteOrder::Little;

fn prop_blob(payload: &[u8]) -> (Vec<u8>, u32) {
    let mut blob = vec![44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 2, 0];
    blob.extend_from_slice(payload);
    let crc = palace_prop::payload_crc(&blob).unwrap();
    (blob, crc)
}

fn valid_send() -> Vec<u8> {
    let (blob, crc) = prop_blob(&(0..64u8).collect::<Vec<u8>>());
    AssetTransfer::single_block(AssetType::PROP, AssetSpec::with_crc(1234, crc), "", &blob)
        .encode_frame(ASSETSEND, 0, ORDER)
        .unwrap()
        .payload
}

#[test]
fn a_frame_claiming_a_two_gibibyte_block_allocates_nothing_near_it() {
    let mut w = palace_wire::Writer::new(ORDER);
    w.write_u32(AssetType::PROP.as_raw());
    w.write_i32(7);
    w.write_u32(0);
    w.write_i32(i32::MAX);
    w.write_i32(0);
    w.write_i16(0);
    w.write_i16(1);
    w.write_u32(0);
    w.write_u32(u32::MAX);
    w.write_str31("x");
    let payload = w.into_vec();
    assert!(payload.len() < 128, "the hostile frame is 64 bytes");

    let before = live_bytes();
    let outcome = AssetTransfer::decode(&payload, ORDER);
    let grew = live_bytes().saturating_sub(before);

    assert!(
        outcome.is_err(),
        "a 2 GiB block must be refused, not returned"
    );
    assert!(
        grew < 4 * 1024 * 1024,
        "decoding must not allocate for a claimed 2 GiB block; grew {grew} bytes"
    );
}

#[test]
fn a_huge_descriptor_size_is_refused_before_any_buffer_is_sized() {
    let mut w = palace_wire::Writer::new(ORDER);
    w.write_u32(AssetType::PROP.as_raw());
    w.write_i32(7);
    w.write_u32(0);
    w.write_i32(4);
    w.write_i32(0);
    w.write_i16(0);
    w.write_i16(1);
    w.write_u32(0);
    w.write_u32(u32::MAX);
    w.write_str31("x");
    w.write_bytes(&[1, 2, 3, 4]);

    let before = live_bytes();
    let err = AssetTransfer::decode(&w.into_vec(), ORDER).unwrap_err();
    let grew = live_bytes().saturating_sub(before);
    assert!(
        err.to_string().contains("implausible asset size"),
        "got {err}"
    );
    assert!(grew < 4 * 1024 * 1024, "grew {grew} bytes");
}

#[test]
fn an_absurd_block_count_is_refused() {
    let mut w = palace_wire::Writer::new(ORDER);
    w.write_u32(AssetType::PROP.as_raw());
    w.write_i32(7);
    w.write_u32(0);
    w.write_i32(1);
    w.write_i32(0);
    w.write_i16(0);
    w.write_i16(i16::MAX);
    let err = AssetTransfer::decode(&w.into_vec(), ORDER).unwrap_err();
    assert!(err.to_string().contains("blockCount"), "got {err}");
}

#[test]
fn random_bytes_never_panic_any_decoder() {
    let mut rng = Rng::new(0xfeed_face);
    for _ in 0..20_000 {
        let len = rng.below(200) as usize;
        let payload = rng.bytes(len);

        if let Ok(transfer) = AssetTransfer::decode(&payload, ORDER) {
            let mut assembler = AssetAssembler::new();
            let _ = assembler.ingest(transfer);
        }
        let _ = palace_asset::decode_query(&payload, ORDER);
        let _ = palace_asset::sanitise_media_name(&String::from_utf8_lossy(&payload));
    }
}

#[test]
fn mutated_valid_frames_never_panic_and_never_yield_a_huge_blob() {
    let template = valid_send();
    let mut rng = Rng::new(0x1234_5678);
    let mut pipeline = AssetPipeline::new();

    for round in 0..20_000 {
        let mut payload = template.clone();
        let mutations = 1 + rng.below(4) as usize;
        for _ in 0..mutations {
            let idx = rng.below(payload.len() as u64) as usize;
            payload[idx] = rng.next_u64() as u8;
        }

        let before = live_bytes();
        if let Ok(transfer) = AssetTransfer::decode(&payload, ORDER) {
            assert!(
                transfer.data.len() <= transfer.header.block_size.max(0) as usize,
                "duplicated bytes must never exceed the declared block size"
            );
            let mut assembler = AssetAssembler::new();
            if let AssemblyOutcome::Complete(asset) = assembler.ingest(transfer) {
                assert!(
                    asset.data.len() as u64 <= palace_asset::MAX_ASSET_SIZE,
                    "round {round}: an assembled asset exceeded the size ceiling"
                );
            }
        }
        let grew = live_bytes().saturating_sub(before);
        assert!(
            grew < 4 * 1024 * 1024,
            "round {round}: decoding grew memory by {grew} bytes"
        );

        let frame = Frame::new(ASSETSEND, 0, payload);
        let _ = pipeline.on_frame(&frame, ORDER, round as u64);
        if round % 997 == 0 {
            assert!(pipeline.assembler().pending_bytes() <= 32 * 1024 * 1024);
        }
    }
}

#[test]
fn random_qast_bodies_never_panic() {
    let mut rng = Rng::new(0xabcd_ef01);
    for _ in 0..20_000 {
        let len = rng.below(80) as usize;
        let payload = rng.bytes(len);
        if let Ok(query) = palace_asset::decode_query(&payload, ORDER) {
            let frame = query.encode_frame(0, ORDER);
            assert_eq!(frame.payload.len(), 12);
            assert_eq!(frame.opcode, ASSETQUERY);
        }
    }
}

#[test]
fn random_block_sequences_never_panic_the_assembler() {
    let mut rng = Rng::new(0x5555_aaaa);
    for _ in 0..2_000 {
        let mut assembler = AssetAssembler::with_config(AssemblerConfig {
            max_pending_assets: 4,
            max_pending_bytes: 4096,
            ..AssemblerConfig::default()
        });
        let blocks = rng.below(6) as usize;
        for _ in 0..blocks {
            let data_len = rng.below(64) as usize;
            let transfer = AssetTransfer {
                header: palace_asset::BlockHeader {
                    asset_type: AssetType::PROP,
                    spec: AssetSpec::with_crc(
                        rng.below(4) as i32,
                        rng.next_u64() as u32,
                    ),
                    block_size: data_len as i32,
                    block_offset: rng.below(4096) as i32,
                    block_number: rng.below(6) as i16,
                    block_count: 1 + rng.below(6) as i16,
                },
                descriptor: (rng.below(2) == 0).then(|| palace_asset::AssetDescriptor {
                    flags: rng.next_u64() as u32,
                    size: rng.below(8192) as u32,
                    name: String::new(),
                }),
                data: rng.bytes(data_len),
                padding: 0,
            };
            let _ = assembler.ingest(transfer);
        }
        assert!(assembler.pending_bytes() <= 4096);
        assert!(assembler.pending_count() <= 4);
    }
}

#[test]
fn random_scheduler_interleavings_never_panic_and_never_overfill_a_batch() {
    let mut rng = Rng::new(0x0f0f_0f0f);
    let mut scheduler = AssetScheduler::new();
    let mut now = 0u64;
    for _ in 0..200_000 {
        match rng.below(10) {
            0..=3 => {
                let id = rng.below(64) as i32;
                scheduler.request(AssetKey::new(AssetType::PROP, id, 0), now);
            }
            4 => {
                let id = rng.below(64) as i32;
                scheduler.cancel(&AssetKey::new(AssetType::PROP, id, 0));
            }
            5 => {
                let id = rng.below(64) as i32;
                scheduler.note_received_any_crc(AssetType::PROP, id);
            }
            6 => {
                scheduler.cancel_all();
            }
            7 => {
                now += rng.below(3_000);
            }
            _ => {
                now += rng.below(50);
                for event in scheduler.poll(now) {
                    if let palace_asset::SchedulerEvent::Send { batch } = event {
                        assert!(
                            batch.len() <= 20,
                            "a flush carried {} requests",
                            batch.len()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn media_names_are_never_escaped_out_of_the_cache_root() {
    let root = std::env::temp_dir().join(format!("palace-asset-escape-{}", std::process::id()));
    let cache = MediaCache::new(&root);
    let mut rng = Rng::new(0x9999_1111);
    let mut accepted = 0usize;

    for _ in 0..20_000 {
        let len = rng.below(40) as usize;
        let name: String = (0..len)
            .map(|_| match rng.below(8) {
                0 => '/',
                1 => '\\',
                2 => '.',
                3 => ':',
                4 => '\0',
                5 => 'a',
                6 => '%',
                _ => rng.next_u64() as u8 as char,
            })
            .collect();
        if let Ok(relative) = palace_asset::sanitise_media_name(&name) {
            accepted += 1;
            assert!(relative.is_relative(), "{name:?} produced an absolute path");
            assert!(
                !relative
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))),
                "{name:?} escaped: {relative:?}"
            );
            let full = cache.path_for("http://x/media", &name).unwrap();
            assert!(full.starts_with(&root), "{name:?} escaped the root");
        }
    }
    assert!(accepted > 0, "the generator should produce some safe names");
}

#[test]
fn hostile_asset_names_and_huge_blobs_are_refused_by_the_cache_and_media_paths() {
    let cache = MediaCache::new(std::env::temp_dir().join("palace-asset-hostile"));
    assert!(cache.put("http://x", "../../etc/passwd", b"x").is_err());
    assert!(cache.get("http://x", "../../etc/passwd").is_none());
    assert!(palace_asset::sanitise_media_name(&"a".repeat(10_000)).is_err());
}

#[test]
fn the_prop_crc_never_panics_on_short_or_absent_payloads() {
    assert_eq!(palace_asset::prop_payload_crc(&[]), None);
    assert_eq!(palace_asset::prop_payload_crc(&[0u8; 11]), None);
    assert_eq!(
        palace_asset::prop_payload_crc(&[0u8; 12]),
        Some(palace_asset::raw_asset_crc(&[]))
    );
}

#[test]
fn the_cache_lookup_by_id_never_confuses_two_assets() {
    let mut pipeline = AssetPipeline::new();
    let (blob_a, crc_a) = prop_blob(&[1, 2, 3]);
    let (blob_b, crc_b) = prop_blob(&[9, 9, 9]);
    for (blob, crc, id) in [(&blob_a, crc_a, 1), (&blob_b, crc_b, 2)] {
        let frame = AssetTransfer::single_block(
            AssetType::PROP,
            AssetSpec::with_crc(id, crc),
            "",
            blob,
        )
        .encode_frame(ASSETSEND, 0, ORDER)
        .unwrap();
        let _ = pipeline.on_frame(&frame, ORDER, id as u64);
    }
    let a = pipeline
        .cache()
        .peek_by_id(AssetType::PROP, 1)
        .expect("asset 1");
    let b = pipeline
        .cache()
        .peek_by_id(AssetType::PROP, 2)
        .expect("asset 2");
    assert_eq!(a.data, blob_a);
    assert_eq!(b.data, blob_b);
    assert_ne!(a.key.crc, b.key.crc);
}

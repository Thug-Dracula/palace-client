//! The `sAst` layout, asserted against real captured assets.
//!
//! `fixtures/captured/` holds five prop blobs that arrived over the wire from
//! `chat.animanic.de:9977`, together with the exact TCP payload length of the
//! packet that carried each one. The blobs were extracted by
//! `$CORPUS/tools/palace_walker.py::_store_asset`, the known-good Python
//! parser; the packet lengths come from `$CORPUS/pcap_extract/`.
//!
//! Those two numbers are enough to pin down the one part of the layout that is
//! easy to get wrong — the trailing padding term. A captured `sAst` frame is
//!
//! ```text
//! 12-byte frame header + 24-byte block header + 40-byte descriptor
//!   + blockSize bytes of data + padding
//! ```
//!
//! so the observed identity `tcp_payload_len == 64 + blob_size` for every
//! captured prop means **the padding is zero**. That is exactly what
//! [`AssetTransfer::decode`] reports, and these tests assert it rather than
//! trusting the `size - (blockSize + 64)` formula in OpenPalace's source.
//!
//! The real 327-blob measurement behind this is in the crate README.

use std::fs;
use std::path::PathBuf;

use palace_asset::{
    media, prop_payload_crc, AssetAssembler, AssetKey, AssetPipeline, AssetSpec, AssetTransfer,
    AssetType, AssemblyOutcome, MediaCache, MediaFetcher, PipelineEvent, UreqTransport,
};
use palace_wire::byteorder::ByteOrder;
use palace_wire::opcode::ASSETSEND;
use serde_json::Value;

const ORDER: ByteOrder = ByteOrder::Little;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[derive(Debug)]
struct CapturedProp {
    file: PathBuf,
    asset_id: i32,
    asset_size: usize,
    tcp_payload_len: usize,
}

fn captured() -> Vec<CapturedProp> {
    let dir = fixture_dir().join("captured");
    let doc: Value =
        serde_json::from_slice(&fs::read(dir.join("capture.json")).expect("capture.json"))
            .expect("capture.json is valid JSON");
    let props = doc["props"].as_array().expect("props array");
    assert!(!props.is_empty(), "the capture fixture must not be empty");
    props
        .iter()
        .map(|p| CapturedProp {
            file: dir.join("props").join(p["file"].as_str().expect("file")),
            asset_id: p["asset_id"].as_i64().expect("asset_id") as i32,
            asset_size: p["asset_size"].as_u64().expect("asset_size") as usize,
            tcp_payload_len: p["tcp_payload_len"].as_u64().expect("tcp_payload_len") as usize,
        })
        .collect()
}

#[test]
fn the_fixture_metadata_is_coherent() {
    let doc: Value =
        serde_json::from_slice(&fs::read(fixture_dir().join("captured/capture.json")).unwrap())
            .unwrap();
    assert_eq!(doc["server"], "chat.animanic.de:9977");
    assert_eq!(doc["byte_order"], "little");
    for p in captured() {
        assert!(
            p.file.is_file(),
            "missing captured prop fixture {}",
            p.file.display()
        );
        // The recorded identity that proves padding is zero.
        assert_eq!(
            p.tcp_payload_len,
            12 + 64 + p.asset_size,
            "{} does not satisfy tcp_payload_len == frame header + 64-byte block-0 header + data",
            p.file.display()
        );
    }
}

#[test]
fn decoding_a_reconstructed_capture_agrees_with_the_observed_packet_length() {
    for prop in captured() {
        let blob = fs::read(&prop.file).unwrap();
        assert_eq!(
            blob.len(),
            prop.asset_size,
            "{} changed size",
            prop.file.display()
        );

        let crc = prop_payload_crc(&blob).expect("a prop blob has a payload");
        let transfer = AssetTransfer::single_block(
            AssetType::PROP,
            AssetSpec::with_crc(prop.asset_id, crc),
            "",
            &blob,
        );
        let frame = transfer
            .encode_frame(ASSETSEND, 0, ORDER)
            .expect("encode sAst");

        assert_eq!(
            frame.encoded_len(),
            prop.tcp_payload_len,
            "reconstructed frame length must equal the captured packet length"
        );
        assert_eq!(
            frame.payload.len(),
            64 + prop.asset_size,
            "block 0 payload is exactly 64 bytes of header plus the asset"
        );

        let decoded = AssetTransfer::decode(&frame.payload, ORDER).expect("decode sAst");
        assert_eq!(
            decoded.padding, 0,
            "real captures carry no padding after the block"
        );
        assert_eq!(decoded.header.block_number, 0);
        assert_eq!(decoded.header.block_count, 1);
        assert_eq!(decoded.header.block_size as usize, prop.asset_size);
        assert_eq!(decoded.header.block_offset, 0);
        let descriptor = decoded.descriptor.as_ref().expect("block 0 has a descriptor");
        assert_eq!(descriptor.size as usize, prop.asset_size);
        assert_eq!(decoded.data, blob);
    }
}

#[test]
fn assembling_a_captured_asset_verifies_its_crc() {
    let mut assembler = AssetAssembler::new();
    for prop in captured() {
        let blob = fs::read(&prop.file).unwrap();
        let crc = prop_payload_crc(&blob).unwrap();
        let transfer = AssetTransfer::single_block(
            AssetType::PROP,
            AssetSpec::with_crc(prop.asset_id, crc),
            "",
            &blob,
        );
        let payload = transfer.encode_frame(ASSETSEND, 0, ORDER).unwrap().payload;
        let decoded = AssetTransfer::decode(&payload, ORDER).unwrap();
        match assembler.ingest(decoded) {
            AssemblyOutcome::Complete(asset) => {
                assert_eq!(asset.data, blob);
                assert_eq!(asset.computed_crc(), Some(crc));
                assert_eq!(asset.id(), prop.asset_id);
            }
            other => panic!("expected Complete for {}, got {other:?}", prop.file.display()),
        }
    }
    assert!(assembler.is_idle(), "nothing should be left half-assembled");
}

#[test]
fn a_captured_prop_with_a_doctored_crc_is_dropped() {
    let prop = &captured()[0];
    let blob = fs::read(&prop.file).unwrap();
    let crc = prop_payload_crc(&blob).unwrap();
    let mut assembler = AssetAssembler::new();
    let transfer = AssetTransfer::single_block(
        AssetType::PROP,
        AssetSpec::with_crc(prop.asset_id, crc.rotate_left(1) ^ 1),
        "",
        &blob,
    );
    let payload = transfer.encode_frame(ASSETSEND, 0, ORDER).unwrap().payload;
    let decoded = AssetTransfer::decode(&payload, ORDER).unwrap();
    match assembler.ingest(decoded) {
        AssemblyOutcome::Rejected { error, .. } => {
            assert!(
                error.to_string().contains("CRC"),
                "expected a CRC complaint, got {error}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn a_captured_prop_survives_the_whole_pipeline_and_decodes_as_a_prop() {
    let mut pipeline = AssetPipeline::new();
    let mut now = 0u64;
    for prop in captured() {
        now += 100;
        let blob = fs::read(&prop.file).unwrap();
        let crc = prop_payload_crc(&blob).unwrap();
        let key = AssetKey::new(AssetType::PROP, prop.asset_id, crc);

        // A room description asked for this prop; the scheduler sent a qAst.
        assert!(matches!(
            pipeline.request(key, now),
            palace_asset::RequestDisposition::Queued
        ));
        let sent = pipeline.poll(now + 50);
        let frames = sent
            .iter()
            .flat_map(|e| match e {
                PipelineEvent::Send { frames } => frames.clone(),
                _ => Vec::new(),
            })
            .count();
        assert_eq!(frames, 1, "one request, one frame");

        // The server answers with the captured asset.
        let frame = AssetTransfer::single_block(
            AssetType::PROP,
            AssetSpec::with_crc(prop.asset_id, crc),
            "",
            &blob,
        )
        .encode_frame(ASSETSEND, 0, ORDER)
        .unwrap();
        let events = pipeline.on_frame(&frame, ORDER, now + 60);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PipelineEvent::AssetReady { .. })),
            "expected AssetReady for {}, got {events:?}",
            prop.file.display()
        );
        assert!(
            pipeline.scheduler().is_idle(),
            "the request must be satisfied by the answer"
        );

        // The consumer is `palace-prop`; the blob must be exactly decodable.
        let cached = pipeline.cache().peek(&key).expect("cached asset");
        assert_eq!(cached.data, blob);
        let decoded = palace_prop::decode(&cached.data).expect("the blob is a complete prop");
        assert_eq!(decoded.image.width(), 44);
        assert_eq!(decoded.image.height(), 44);
    }
}

#[test]
fn the_crc_slice_is_the_payload_not_the_whole_blob() {
    // The classic mistake is hashing the whole blob. Prove the two differ for
    // every captured prop, so the assertion above is meaningful.
    for prop in captured() {
        let blob = fs::read(&prop.file).unwrap();
        let payload_crc = prop_payload_crc(&blob).unwrap();
        let whole_blob_crc = palace_asset::raw_asset_crc(&blob);
        assert_ne!(
            payload_crc, whole_blob_crc,
            "a 12-byte prop header must change the hash; otherwise this test proves nothing"
        );
        assert_eq!(whole_blob_crc, palace_prop::asset_crc(&blob));
    }
}

#[test]
fn the_media_fixtures_are_usable_by_the_fetcher() {
    // The media path needs no server to be *type*-correct; this checks the
    // fixture files exist and are what their extensions claim.
    let media_dir = fixture_dir().join("media");
    let gif = fs::read(media_dir.join("bg.gif")).unwrap();
    let png = fs::read(media_dir.join("bg.png")).unwrap();
    let jpg = fs::read(media_dir.join("bg.jpg")).unwrap();
    assert_eq!(&gif[..3], b"GIF");
    assert_eq!(&png[..4], b"\x89PNG");
    assert_eq!(&jpg[..2], b"\xff\xd8");
    assert!(media_dir.join("avatar-editor/photopea.png").is_file());

    // The chain a `.gif` background is chased through.
    assert_eq!(media::fallback_chain("bg.gif"), vec!["bg.png", "bg.jpg", "bg.gif"]);
    assert_eq!(
        media::media_url("https://media.example/palace/media", "bg.png"),
        "https://media.example/palace/media/bg.png"
    );
}

#[test]
fn the_ureq_transport_exists_and_is_constructible_offline() {
    // Construction must not perform any I/O; the agent is lazy.
    let cfg = palace_asset::MediaConfig::default();
    let transport = UreqTransport::from_config(&cfg);
    let cache = MediaCache::new(std::env::temp_dir().join("palace-asset-construct-only"));
    let fetcher = MediaFetcher::new(transport, cache, cfg);
    assert_eq!(fetcher.negative_count(), 0);
}

//! Malformed input: the decoder must return an error, never a panic.
//!
//! A prop is untrusted data (it arrives over the wire, out of an asset roster, or
//! from a 1997 room description), so this is a hard requirement rather than a
//! nicety. Every case below runs inside `catch_unwind` so a panic fails the test
//! with the offending bytes printed instead of aborting the run.
//!
//! The corpus already contains the interesting shapes: seven props in `pserver.prp`
//! are structurally broken and the reference client refuses them too, and five more
//! end one byte inside their final run and the reference renders them. Both
//! behaviours are pinned in `fixtures.rs`.

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

use palace_prop::{decode, decode_header, PropEndian, PropHeader, PropImage, Result};

fn attempt(bytes: Vec<u8>) -> Result<PropImage> {
    let shown = bytes.clone();
    let outcome = catch_unwind(AssertUnwindSafe(|| decode(&bytes).map(|prop| prop.image)));
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic>".into());
            panic!(
                "decode panicked on {} bytes: {message}\nbytes: {shown:02x?}",
                shown.len()
            );
        }
    }
}

/// Deterministic xorshift, so a failure is reproducible without a `rand` dep.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 24) as u8).collect()
    }
}

fn valid_prop() -> Vec<u8> {
    // A 4x1 8-bit prop: four palette-1 pixels in one run.
    let mut blob = vec![4, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    blob.extend_from_slice(&[0x04, 1, 1, 1, 1]);
    blob
}

fn s20_header() -> Vec<u8> {
    vec![
        0x2c, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
    ]
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for len in [0usize, 1, 2, 11, 12, 13, 20, 64, 256, 4096, 65_536] {
        for _ in 0..64 {
            let _ = attempt(rng.bytes(len));
        }
    }
}

#[test]
fn every_prefix_of_a_valid_prop_is_handled_without_panicking() {
    let blob = valid_prop();
    for cut in 0..=blob.len() {
        let result = attempt(blob[..cut].to_vec());
        if cut < 12 {
            assert!(result.is_err(), "a {cut}-byte header should be rejected");
        }
    }
}

#[test]
fn every_single_byte_mutation_of_a_valid_prop_is_handled() {
    let blob = valid_prop();
    for index in 0..blob.len() {
        for delta in [1u8, 0x10, 0x80, 0xff] {
            let mut mutated = blob.clone();
            mutated[index] ^= delta;
            let _ = attempt(mutated);
        }
    }
}

#[test]
fn byte_order_detection_cannot_be_confused_by_a_large_second_byte() {
    // The sniff is `data[1] == 0`; any other value means big-endian, so flipping
    // it reinterprets width as up to 0x7fff. That must be refused, not allocated.
    for second in [0x00u8, 0x01, 0x7f, 0xff] {
        let mut blob = valid_prop();
        blob[1] = second;
        if let Ok(image) = attempt(blob) {
            assert!(u64::from(image.width()) * u64::from(image.height()) <= 16 * 1024 * 1024);
        }
    }
}

#[test]
fn absurd_dimensions_are_refused_before_allocating() {
    for (width, height) in [
        (0i16, 0i16),
        (-1, 44),
        (44, -1),
        (i16::MIN, 44),
        (4097, 44),
        (44, 4097),
        (i16::MAX, i16::MAX),
    ] {
        let raw = PropHeader {
            width,
            height,
            h_offset: 0,
            v_offset: 0,
            script_offset: 0,
            flags: 0,
            endian: PropEndian::Little,
        }
        .encode();
        assert!(
            decode_header(&raw).is_err(),
            "{width}x{height} should be refused"
        );
        assert!(decode(&raw).is_err());
    }
}

#[test]
fn a_decompression_bomb_is_capped_rather_than_inflated() {
    // 8 MiB of zeroes compresses to a few kilobytes. An S20 prop needs 4840 bytes
    // plus a 4 KiB slack, so anything past that is refused instead of buffered.
    let payload = {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&vec![0u8; 8 * 1024 * 1024]).unwrap();
        encoder.finish().unwrap()
    };
    let mut blob = s20_header();
    blob.extend_from_slice(&payload);
    assert!(
        attempt(blob).is_err(),
        "a decompression bomb must be refused"
    );
}

#[test]
fn a_corrupt_zlib_stream_is_an_error_not_a_panic() {
    for tail in [
        vec![],
        vec![0x78],
        vec![0x78, 0x9c],
        vec![0x78, 0x9c, 0xff, 0xff, 0xff, 0xff],
        vec![0x00, 0x00, 0x00, 0x00],
    ] {
        let mut blob = s20_header();
        blob.extend_from_slice(&tail);
        assert!(attempt(blob).is_err());
    }
}

#[test]
fn a_rle_stream_of_zero_control_bytes_terminates_with_an_error() {
    // `0x00` advances neither nibble; the reference spins until its 6000-byte
    // guard. Removing our guard turns this test into a hang rather than a failure.
    let mut blob = vec![44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    blob.extend(std::iter::repeat_n(0x00u8, 4096));
    assert!(attempt(blob).is_err());
}

#[test]
fn a_valid_prop_still_decodes_after_all_of_that() {
    assert!(attempt(valid_prop()).is_ok());
}

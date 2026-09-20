//! Acceptance tests for animated / big props (plan task 35).
//!
//! The container layout is pinned by the synthetic tests, which never touch the
//! user's bag. The gated test proves the reading against a real `PropBag.bundle`
//! copy and asserts a byte-exact re-encode of every big record, matching the
//! task-33 prototype property:
//!
//! ```bash
//! cp -a ~/.local/share/PalaceChat/PropBag.bundle /tmp/task35/PropBag.bundle
//! PALACE_PROP_BAG=/tmp/task35/PropBag.bundle \
//!     cargo test -p palace-prop --test animated -- --ignored --nocapture
//! ```
//!
//! Authoring stays blocked: there is no encoder here, and the crate exposes none.

use std::path::PathBuf;

use palace_prop::animated::{is_big_prop, AnimatedProp, EmbeddedImage};
use palace_prop::bag_catalog::{BagCatalog, BagCollection};
use palace_prop::prp::{AssetRec, PropRecord, Roster};
use palace_prop::{encode_s20_blob, PropImage};

const PNG_FLAGS: u16 = 0x0402;
const WEBP_FLAGS: u16 = 0x0c02;
const BASE_IMAGE_LEN: usize = 4840;

fn test_image(width: u32, height: u32) -> PropImage {
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for (i, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&[
            (i as u8).wrapping_mul(37),
            (i as u8).wrapping_mul(11),
            (i as u8).wrapping_mul(5),
            255,
        ]);
    }
    PropImage::from_rgba(width, height, rgba).expect("build the test image")
}

fn encoded_png(image: &PropImage) -> Vec<u8> {
    image.to_png_bytes().expect("encode the test PNG")
}

/// A 44x44 raw S20 base image: every pixel opaque black.
fn base_raw() -> Vec<u8> {
    [0x00, 0x00, 0x00, 0x00, 0x1f]
        .iter()
        .cycle()
        .take(BASE_IMAGE_LEN)
        .copied()
        .collect()
}

fn descriptor(width: i16, height: i16, flags: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    for value in [width, height, (44 - width) / 2, (44 - height) / 2, 100i16] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&flags.to_be_bytes());
    out
}

fn big_blob(flags: u16, width: i16, height: i16, embedded: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x2c, 0x00, 0x2c]);
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&base_raw());
    out.extend_from_slice(&(embedded.len() as u32).to_be_bytes());
    out.extend_from_slice(&descriptor(width, height, flags));
    out.extend_from_slice(embedded);
    out
}

/// A minimal RIFF/WEBP animation: VP8X + ANIM + three ANMF frames.
fn animated_webp() -> Vec<u8> {
    fn chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(fourcc);
        c.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        c.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            c.push(0);
        }
        c
    }
    let mut vp8x = vec![0x02, 0, 0, 0];
    vp8x.extend_from_slice(&[99, 0, 0]);
    vp8x.extend_from_slice(&[79, 0, 0]);
    let anim = vec![0, 0, 0, 0, 0, 0];
    let anmf = |duration: u32, width: u32, height: u32| -> Vec<u8> {
        let mut f = vec![0u8; 16];
        f[6..9].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
        f[9..12].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
        f[12..15].copy_from_slice(&duration.to_le_bytes()[..3]);
        f
    };

    let mut body = Vec::new();
    body.extend_from_slice(&chunk(b"VP8X", &vp8x));
    body.extend_from_slice(&chunk(b"ANIM", &anim));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(40, 10, 8)));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(40, 10, 8)));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(100, 10, 8)));

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

fn empty_roster() -> Roster {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/prp/empty.prp");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    Roster::parse(&bytes).expect("parse the empty fixture")
}

fn roster_with(id: i32, blob: Vec<u8>) -> Roster {
    let mut roster = empty_roster();
    roster.add_prop(PropRecord {
        rec: AssetRec {
            id,
            ..AssetRec::default()
        },
        header: None,
        encoding: None,
        blob,
        name: Some(format!("prop-{id}")),
    });
    roster
}

fn persisted(roster: &Roster) -> Roster {
    let bytes = roster.write().expect("serialise the roster");
    Roster::parse(&bytes).expect("reparse the roster")
}

#[test]
fn a_still_png_big_prop_is_one_frame_that_decodes_to_the_embedded_png() {
    let image = test_image(4, 3);
    let png = encoded_png(&image);
    let blob = big_blob(PNG_FLAGS, 4, 3, &png);
    assert!(is_big_prop(&blob), "the 0x0400 flag selects the container");

    let prop = AnimatedProp::decode(&blob).expect("decode the big prop");
    assert!(!prop.is_animated(), "a still PNG is not multi-frame");
    assert_eq!(prop.frame_count(), 1);
    assert_eq!(prop.canvas(), (4, 3));

    let frame = prop.frame_info(0).expect("frame 0 exists");
    assert_eq!((frame.width, frame.height), (4, 3));
    assert_eq!(frame.duration_ms, 0, "a still carries no timing");
    assert!(frame.decoded);
    assert!(
        prop.frame_info(1).is_none(),
        "indexing past the end is None"
    );

    let pixels = prop.decode_frame(0).expect("frame 0 decodes");
    assert_eq!((pixels.width(), pixels.height()), (4, 3));
    assert_eq!(
        pixels.as_rgba(),
        image.as_rgba(),
        "frame 0 is the embedded PNG, pixel for pixel"
    );
    assert!(prop.decode_frame(1).is_none());
    assert_eq!(
        prop.still_image().expect("still decodes").as_rgba(),
        image.as_rgba()
    );

    assert_eq!(
        prop.to_blob(),
        blob,
        "an unmodified re-encode is byte-exact"
    );
}

#[test]
fn an_animated_webp_big_prop_reports_every_frame_and_re_encodes_exactly() {
    let webp = animated_webp();
    let blob = big_blob(WEBP_FLAGS, 100, 80, &webp);

    let prop = AnimatedProp::decode(&blob).expect("decode the animated prop");
    assert!(prop.is_animated());
    assert!(matches!(prop.embedded(), EmbeddedImage::AnimatedWebp(_)));
    assert_eq!(prop.frame_count(), 3);
    assert_eq!(prop.frame_durations_ms(), vec![40, 40, 100]);
    assert_eq!(prop.total_duration_ms(), 180);
    assert_eq!(prop.loop_count(), 0, "ANIM loop 0 means forever");
    assert_eq!(prop.canvas(), (100, 80));

    let frame = prop.frame_info(1).expect("frame 1 exists");
    assert_eq!((frame.width, frame.height), (10, 8), "dims come from ANMF");
    assert_eq!(frame.duration_ms, 40);
    assert!(
        !frame.decoded,
        "later WebP frames are the app layer's to decode"
    );

    let first = prop.decode_frame(0).expect("frame 0 is renderable");
    assert_eq!((first.width(), first.height()), (44, 44));
    assert_eq!(
        first.as_rgba(),
        prop.base_image().as_rgba(),
        "frame 0 of an animated prop is the container's 44x44 still"
    );
    assert!(prop.decode_frame(1).is_none());
    assert!(prop.decode_frame(3).is_none());

    assert_eq!(
        prop.to_blob(),
        blob,
        "an unmodified re-encode is byte-exact"
    );
}

#[test]
fn the_single_prop_decode_path_no_longer_rejects_a_multi_frame_record() {
    let blob = big_blob(WEBP_FLAGS, 100, 80, &animated_webp());

    let prop = palace_prop::decode(&blob).expect("a big prop must not be rejected");
    assert_eq!((prop.image.width(), prop.image.height()), (44, 44));
    assert_eq!(
        prop.image.as_rgba(),
        AnimatedProp::decode(&blob)
            .expect("decode the container")
            .base_image()
            .as_rgba(),
        "the plain path yields the container's 44x44 still"
    );
    assert!(prop.header.flags & 0x0400 != 0);
}

#[test]
fn an_ordinary_prop_still_decodes_unchanged() {
    let blob = encode_s20_blob(&test_image(6, 5), 0, 0, 0).expect("encode an S20 prop");
    assert!(!is_big_prop(&blob));
    let prop = palace_prop::decode(&blob).expect("decode the ordinary prop");
    assert_eq!((prop.image.width(), prop.image.height()), (6, 5));
}

#[test]
fn a_bag_catalog_thumbnail_for_a_big_prop_is_its_frame_zero() {
    let still_image = test_image(8, 8);
    let still_blob = big_blob(PNG_FLAGS, 8, 8, &encoded_png(&still_image));
    let animated_blob = big_blob(WEBP_FLAGS, 100, 80, &animated_webp());

    let roster = persisted(&roster_with(0x51A0_0001u32 as i32, still_blob.clone()));
    let roster = {
        let mut r = roster;
        r.add_prop(PropRecord {
            rec: AssetRec {
                id: 0x51A0_0002u32 as i32,
                ..AssetRec::default()
            },
            header: None,
            encoding: None,
            blob: animated_blob.clone(),
            name: None,
        });
        persisted(&r)
    };
    let catalog = BagCatalog::new([BagCollection::new("My Bag", roster)]);

    let still_entry = catalog
        .entries()
        .iter()
        .find(|e| e.id == 0x51A0_0001)
        .expect("the still big prop is catalogued");
    let still_frame_zero = AnimatedProp::decode(&still_blob)
        .expect("decode the still container")
        .frame_zero()
        .to_png_bytes()
        .expect("encode frame 0");
    assert_eq!(
        catalog.thumbnail_png(still_entry.id, still_entry.crc),
        Some(still_frame_zero),
        "a still big prop's thumbnail is its frame 0 (the PNG)"
    );

    let animated_entry = catalog
        .entries()
        .iter()
        .find(|e| e.id == 0x51A0_0002)
        .expect("the animated big prop is catalogued");
    let animated_prop =
        AnimatedProp::decode(&animated_blob).expect("decode the animated container");
    let animated_frame_zero = animated_prop
        .frame_zero()
        .to_png_bytes()
        .expect("encode frame 0");
    assert_eq!(
        catalog.thumbnail_png(animated_entry.id, animated_entry.crc),
        Some(animated_frame_zero),
        "an animated big prop's thumbnail is its frame 0"
    );
    assert_eq!(
        catalog.thumbnail_png(animated_entry.id, animated_entry.crc),
        Some(
            animated_prop
                .base_image()
                .to_png_bytes()
                .expect("encode the base")
        ),
        "frame 0 of an animated prop is the 44x44 still"
    );
    assert_eq!(catalog.thumbnail_failures(), 0);
}

/// Gated on a real bag copy (read-only):
///
/// ```bash
/// cp -a ~/.local/share/PalaceChat/PropBag.bundle /tmp/task35/PropBag.bundle
/// PALACE_PROP_BAG=/tmp/task35/PropBag.bundle \
///     cargo test -p palace-prop --test animated -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs PALACE_PROP_BAG pointing at a PropBag.bundle copy"]
fn real_bag_big_props_report_frames_and_re_encode_byte_exact() {
    let dir = std::env::var("PALACE_PROP_BAG").expect("set PALACE_PROP_BAG");
    let bag = palace_prop::PropBag::open_dir(&dir).expect("open the bag");

    let (mut big, mut png, mut webp, mut webp_frames, mut still_frames, mut reencodes, mut frame0) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for entry in bag.entries() {
        let Some(blob) = entry.prop_bytes() else {
            continue;
        };
        if !is_big_prop(blob) {
            continue;
        }
        big += 1;
        let prop = AnimatedProp::decode(blob)
            .unwrap_or_else(|e| panic!("big prop {:#010x} failed to decode: {e}", entry.a()));

        match prop.embedded() {
            EmbeddedImage::Png { .. } => {
                png += 1;
                still_frames += prop.frame_count();
            }
            EmbeddedImage::AnimatedWebp(_) => {
                webp += 1;
                webp_frames += prop.frame_count();
                assert!(prop.frame_count() >= 2, "an animated prop is multi-frame");
            }
        }

        assert!(prop.frame_count() >= 1);
        for frame in prop.frame_infos() {
            assert!(
                frame.width >= 1 && frame.width <= 220 && frame.height >= 1 && frame.height <= 220,
                "{:#010x} frame {} has implausible dims {}x{}",
                entry.a(),
                frame.index,
                frame.width,
                frame.height
            );
        }

        let first = prop
            .decode_frame(0)
            .unwrap_or_else(|| panic!("{:#010x} frame 0 did not decode", entry.a()));
        assert!(first.width() > 0 && first.height() > 0);
        frame0 += 1;

        assert_eq!(
            prop.to_blob(),
            blob,
            "byte-exact re-encode failed for {:#010x}",
            entry.a()
        );
        reencodes += 1;
    }

    let total_frames = still_frames + webp_frames;
    println!(
        "big={big} png={png} animated_webp={webp} webp_frames={webp_frames} \
         still_frames={still_frames} total_frames={total_frames} \
         frame0_decoded={frame0} byte_exact_reencodes={reencodes}"
    );
    assert_eq!(
        reencodes, big,
        "every big record must re-encode byte-exactly"
    );
    assert_eq!(frame0, big, "every big record's frame 0 must decode");
    assert_eq!(png + webp, big);
    assert!(big > 0, "the bag contained no big props");
}

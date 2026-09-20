//! Prototype tests for the big-prop / animated-prop container recon (task 33).
//!
//! The synthetic tests pin the layout without needing the live client data; the
//! gated test proves the reading against the real `PropBag.bundle` and asserts a
//! **byte-exact** re-encode of every big record.

use palace_prop::prop_animated_recon::{
    decode_big_prop, encode_big_prop, is_big_prop, BigImageDescriptor, EmbeddedImage,
    BASE_IMAGE_LEN, DESCRIPTOR_OFFSET, IMAGE_LEN_OFFSET, IMAGE_OFFSET,
};
use palace_prop::PropBag;

/// A valid 1x1 RGBA PNG (70 bytes).
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x60, 0x60, 0x60,
    0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0xa5, 0xf6, 0x45, 0x40, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A 5-byte S20 group with pixel 1 fully opaque black.
const S20_OPAQUE_GROUP: [u8; 5] = [0x00, 0x00, 0x00, 0x00, 0x1f];

fn base_image() -> Vec<u8> {
    S20_OPAQUE_GROUP
        .iter()
        .cycle()
        .take(BASE_IMAGE_LEN)
        .copied()
        .collect()
}

fn descriptor_bytes(png: bool) -> Vec<u8> {
    let d = BigImageDescriptor {
        width: 1,
        height: 1,
        x_offset: 21,
        y_offset: 21,
        reserved: 100,
        flags: if png { 0x0402 } else { 0x0c02 },
    };
    // big-endian, matching the client's bag records.
    d.encode(palace_prop::PropEndian::Big).to_vec()
}

fn build_blob(flags: u16, descriptor: &[u8], image: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x00, 0x2c, 0x00, 0x2c]); // width 44, height 44 (BE)
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // offsets, script 0
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&base_image());
    out.extend_from_slice(&(image.len() as u32).to_be_bytes());
    out.extend_from_slice(descriptor);
    out.extend_from_slice(image);
    out
}

#[test]
fn fixed_offsets_are_what_the_container_uses() {
    assert_eq!(IMAGE_LEN_OFFSET, 4852);
    assert_eq!(DESCRIPTOR_OFFSET, 4856);
    assert_eq!(IMAGE_OFFSET, 4868);
    assert_eq!(BASE_IMAGE_LEN, 4840);
}

#[test]
fn a_synthetic_png_big_prop_decodes_and_re_encodes_exactly() {
    let blob = build_blob(0x0402, &descriptor_bytes(true), PNG_1X1);
    assert!(is_big_prop(&blob));

    let prop = decode_big_prop(&blob).expect("decode");
    assert_eq!(prop.header.flags, 0x0402);
    assert_eq!(prop.base_image.width(), 44);
    assert_eq!(prop.base_image.height(), 44);
    assert_eq!(prop.image_len as usize, PNG_1X1.len());
    assert_eq!(
        prop.embedded,
        EmbeddedImage::Png {
            width: 1,
            height: 1
        }
    );
    assert!(!prop.is_animated());
    assert_eq!(prop.descriptor.reserved, 100);

    assert_eq!(encode_big_prop(&prop), blob, "re-encode must be byte-exact");
}

#[test]
fn a_synthetic_animated_webp_big_prop_reports_its_frames() {
    let webp = animated_webp_fixture();
    let blob = build_blob(0x0c02, &descriptor_bytes(false), &webp);

    let prop = decode_big_prop(&blob).expect("decode");
    let EmbeddedImage::AnimatedWebp(anim) = &prop.embedded else {
        panic!("expected animated WebP, got {:?}", prop.embedded);
    };
    assert_eq!(prop.header.flags & 0x0800, 0x0800);
    assert_eq!(anim.canvas_width, 100);
    assert_eq!(anim.canvas_height, 80);
    assert_eq!(anim.frame_count, 3);
    assert_eq!(anim.frame_durations_ms, vec![40, 40, 100]);
    assert_eq!(anim.loop_count, 0);
    assert!(prop.is_animated());

    assert_eq!(encode_big_prop(&prop), blob, "re-encode must be byte-exact");
}

/// Minimal RIFF/WEBP with an animation: VP8X + ANIM + three ANMF frames.
fn animated_webp_fixture() -> Vec<u8> {
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
    // VP8X: flags(1) + reserved(3) + canvas width-1(3) + height-1(3)
    let mut vp8x = vec![0x02, 0, 0, 0];
    vp8x.extend_from_slice(&[99, 0, 0]); // 100 px wide
    vp8x.extend_from_slice(&[79, 0, 0]); // 80 px tall
                                         // ANIM: background(4) + loop(2)
    let anim = vec![0, 0, 0, 0, 0, 0];
    // ANMF: x/2, y/2, w-1, h-1, duration, flags + (no VP8 data needed here)
    let anmf = |duration: u8| -> Vec<u8> {
        let mut f = vec![0u8; 16];
        f[12] = duration; // 24-bit little-endian frame duration
        f
    };

    let mut body = Vec::new();
    body.extend_from_slice(&chunk(b"VP8X", &vp8x));
    body.extend_from_slice(&chunk(b"ANIM", &anim));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(40)));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(40)));
    body.extend_from_slice(&chunk(b"ANMF", &anmf(100)));

    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

/// Gated on the real client bag:
///
/// ```bash
/// cp -a ~/.local/share/PalaceChat/PropBag.bundle /tmp/propbag
/// PALACE_PROP_BAG=/tmp/propbag cargo test -p palace-prop --test animated_recon \
///     -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs PALACE_PROP_BAG pointing at a PropBag.bundle"]
fn real_bag_big_props_decode_and_re_encode_byte_exact() {
    let dir = std::env::var("PALACE_PROP_BAG").expect("set PALACE_PROP_BAG");
    let bag = PropBag::open_dir(&dir).expect("open bag");

    let (mut big, mut png, mut webp, mut frames, mut reencodes) = (0, 0, 0, 0, 0);
    for entry in bag.entries() {
        let Some(pb) = entry.prop_bytes() else {
            continue;
        };
        if !is_big_prop(pb) {
            continue;
        }
        big += 1;
        let prop = decode_big_prop(pb)
            .unwrap_or_else(|e| panic!("big prop {:#010x} failed to decode: {e}", entry.a()));
        match &prop.embedded {
            EmbeddedImage::Png { .. } => png += 1,
            EmbeddedImage::AnimatedWebp(a) => {
                webp += 1;
                frames += a.frame_count;
            }
        }
        let rebuilt = encode_big_prop(&prop);
        assert_eq!(
            rebuilt,
            pb,
            "byte-exact re-encode failed for {:#010x}",
            entry.a()
        );
        reencodes += 1;
    }

    println!(
        "big={big} png={png} animated_webp={webp} webp_frames={frames} byte_exact_reencodes={reencodes}"
    );
    assert_eq!(reencodes, big);
    assert!(big > 0, "bag contained no big props");
}

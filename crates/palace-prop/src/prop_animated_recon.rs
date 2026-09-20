//! **Recon only** — decoder for the modern client's "big prop" container.
//!
//! This module is the prototype produced by the animated-prop reconnaissance
//! (task 33). It decodes the records that [`crate::decode`] rejects: the prop
//! blobs whose header sets flag bit `0x0400` and/or `0x0800`, which the crate
//! README §2.13 described as "multi-frame / big prop containers, structure
//! undetermined".
//!
//! ## What these records actually are
//!
//! They are **big props with an embedded image file**, not a frame list:
//!
//! ```text
//! offset  size   field
//! 0       12     ordinary prop header: 44x44, hOff/vOff/script = 0,
//!                flags = 0x0400 (+ legacy HEAD/GHOST/... bits)
//!                        (+ 0x0800 when the embedded image is animated WebP)
//! 12      4840   a 44x44 **raw, uncompressed S20** base image (2 pixels / 5 bytes)
//! 4852    4      u32, header byte order: length L of the embedded image file
//! 4856    12     big-image descriptor: imgW, imgH, xOff, yOff, reserved, flags
//! 4868    L      the embedded image file:
//!                  - flags & 0x0800 == 0  -> a static PNG
//!                  - flags & 0x0800 != 0  -> an animated WebP (RIFF/VP8X/ANIM/ANMF)
//! ```
//!
//! Total blob length is exactly `4868 + L`. That was verified on **126/126**
//! such records in a live `PropBag.bundle` snapshot (2026-09-19).
//!
//! The two flag bits partition the corpus perfectly:
//!
//! * `0x0400` without `0x0800` — 115 records, every one a static PNG. These are
//!   large still props (a 220x220 death reaper, a Garfield still, ...).
//! * `0x0800` (always with `0x0400`) — 11 records, every one an animated WebP.
//!
//! The multi-frame property therefore applies only to the `0x0800` class, and
//! the frames live inside the **WebP container**, not in a bespoke frame table.
//!
//! The base 44x44 image is a nearest-neighbour downscale of the big image
//! (load-bearing for a legacy client; a modern client draws the embedded file).
//!
//! ## Confidence
//!
//! | Aspect | Confidence | Evidence |
//! |---|---|---|
//! | fixed offsets + length field | **Certain** | 126/126 exact, `len == 4868 + L` |
//! | base image = raw S20 44x44 | **High** | decodes to a recognizable downscale; 125/126 non-empty |
//! | `0x0400` = embedded PNG | **High** | 115/115 |
//! | `0x0800` = embedded animated WebP | **High** | 11/11, all RIFF/WEBP with ANMF |
//! | descriptor w/h/x/y | **High** | w/h equal the image dims; x/y = centred placement |
//! | descriptor `reserved` field | **Low** | values 0/29/93/100/119/200, undetermined |
//! | playback mode `pb_*` | **High** | editor only; ping-pong/reverse are *baked into the frame order* |
//!
//! This module **decodes**. It deliberately does not synthesis a base image or
//! author an animation; see `$CORPUS/PRP-ANIMATED.md` for the
//! GO/NO-GO on that.

use std::fmt;

use crate::codec::s20;
use crate::error::PropError;
use crate::header::{PropEndian, PropHeader, HEADER_LEN};
use crate::image::PropImage;

/// Flag marking a prop as using the big-image container.
pub const FLAG_BIG_IMAGE: u16 = 0x0400;
/// Flag marking the embedded image as animated WebP (rather than static PNG).
pub const FLAG_WEBP: u16 = 0x0800;

/// Both container bits.
pub const BIG_FLAG_MASK: u16 = FLAG_BIG_IMAGE | FLAG_WEBP;

/// Side of the base image in pixels. Every big prop is 44x44.
pub const BASE_SIDE: u32 = 44;

/// Bytes of raw S20 base image (44x44 pixels at 2.5 bytes each).
pub const BASE_IMAGE_LEN: usize = 4840;

/// Offset of the base image (immediately after the 12-byte header).
pub const BASE_IMAGE_OFFSET: usize = HEADER_LEN;

/// Offset of the u32 embedded-image length.
pub const IMAGE_LEN_OFFSET: usize = BASE_IMAGE_OFFSET + BASE_IMAGE_LEN; // 4852

/// Offset of the 12-byte big-image descriptor.
pub const DESCRIPTOR_OFFSET: usize = IMAGE_LEN_OFFSET + 4; // 4856

/// Length of the big-image descriptor.
pub const DESCRIPTOR_LEN: usize = 12;

/// Offset of the embedded image file.
pub const IMAGE_OFFSET: usize = DESCRIPTOR_OFFSET + DESCRIPTOR_LEN; // 4868

/// Why a big prop could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconError {
    /// The header did not carry `0x0400` or `0x0800`; use [`crate::decode`].
    NotABigProp { flags: u16 },
    /// The blob ended before the fixed container prefix.
    TooShort { available: usize, needed: usize },
    /// The embedded length field points past the end of the blob.
    ImageRunsPastEnd { declared: usize, available: usize },
    /// The prop header was not parseable.
    Header(PropError),
    /// The base image did not decode as 44x44 S20.
    BaseImage(PropError),
    /// The embedded file is not the format the flag bits promise.
    EmbeddedNotPng,
    /// The embedded file is not a parseable RIFF/WEBP container.
    EmbeddedNotWebp,
    /// A WebP chunk declared a length that runs past the buffer.
    WebpChunkTruncated { fourcc: [u8; 4], declared: usize },
}

impl fmt::Display for ReconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReconError::NotABigProp { flags } => {
                write!(f, "flags {flags:#06x} do not select the big-prop container")
            }
            ReconError::TooShort { available, needed } => write!(
                f,
                "big prop is {available} byte(s); at least {needed} are required for the fixed prefix"
            ),
            ReconError::ImageRunsPastEnd {
                declared,
                available,
            } => write!(
                f,
                "embedded image declares {declared} byte(s) but only {available} remain"
            ),
            ReconError::Header(e) => write!(f, "prop header: {e}"),
            ReconError::BaseImage(e) => write!(f, "base image: {e}"),
            ReconError::EmbeddedNotPng => write!(f, "0x0400 without 0x0800 but no PNG signature"),
            ReconError::EmbeddedNotWebp => {
                write!(f, "0x0800 set but embedded file is not RIFF/WEBP")
            }
            ReconError::WebpChunkTruncated { fourcc, declared } => write!(
                f,
                "WebP chunk {} declares {declared} byte(s) past the end of the file",
                fourcc
                    .iter()
                    .map(|b| (*b as char).to_string())
                    .collect::<String>()
            ),
        }
    }
}

impl std::error::Error for ReconError {}

impl From<PropError> for ReconError {
    fn from(e: PropError) -> Self {
        ReconError::Header(e)
    }
}

/// The 12-byte descriptor that positions the big image over the 44x44 base.
///
/// `width`/`height` are the embedded image's pixel dimensions; `x_offset`/
/// `y_offset` place its top-left corner relative to the 44x44 cell and are
/// normally `((44 - size) / 2)`. `flags` mirrors the outer header's container
/// bits. `reserved` is the one field whose meaning is undetermined (observed
/// `0`, `29`, `93`, `100`, `119`, `200`) — it is preserved verbatim so a
/// re-encode is byte-exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BigImageDescriptor {
    /// Embedded image width in pixels (29..=220 observed).
    pub width: i16,
    /// Embedded image height in pixels (25..=220 observed).
    pub height: i16,
    /// Horizontal placement relative to the 44x44 cell.
    pub x_offset: i16,
    /// Vertical placement relative to the 44x44 cell.
    pub y_offset: i16,
    /// Undetermined field; observed `0`, `29`, `93`, `100`, `119`, `200`.
    pub reserved: u16,
    /// Container flag bits, mirroring the outer header.
    pub flags: u16,
}

impl BigImageDescriptor {
    /// Parse the 12-byte descriptor (as found at [`DESCRIPTOR_OFFSET`]).
    pub fn parse(bytes: &[u8], endian: PropEndian) -> Result<Self, ReconError> {
        if bytes.len() < DESCRIPTOR_LEN {
            return Err(ReconError::TooShort {
                available: bytes.len(),
                needed: DESCRIPTOR_LEN,
            });
        }
        Ok(Self {
            width: read_i16(bytes, 0, endian),
            height: read_i16(bytes, 2, endian),
            x_offset: read_i16(bytes, 4, endian),
            y_offset: read_i16(bytes, 6, endian),
            reserved: read_i16(bytes, 8, endian) as u16,
            flags: read_i16(bytes, 10, endian) as u16,
        })
    }

    /// Serialise back to 12 bytes in the given byte order.
    #[must_use]
    pub fn encode(&self, endian: PropEndian) -> [u8; DESCRIPTOR_LEN] {
        let mut out = [0u8; DESCRIPTOR_LEN];
        for (i, value) in [
            self.width,
            self.height,
            self.x_offset,
            self.y_offset,
            self.reserved as i16,
            self.flags as i16,
        ]
        .iter()
        .enumerate()
        {
            let pair = match endian {
                PropEndian::Little => value.to_le_bytes(),
                PropEndian::Big => value.to_be_bytes(),
            };
            out[i * 2] = pair[0];
            out[i * 2 + 1] = pair[1];
        }
        out
    }
}

/// Animation metadata read from an embedded animated WebP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebpAnimation {
    /// Canvas width from the `VP8X` chunk.
    pub canvas_width: u32,
    /// Canvas height from the `VP8X` chunk.
    pub canvas_height: u32,
    /// Number of `ANMF` frames.
    pub frame_count: u32,
    /// Loop count from the `ANIM` chunk (`0` = loop forever).
    pub loop_count: u16,
    /// Background colour from the `ANIM` chunk.
    pub background: u32,
    /// Per-frame display durations in milliseconds, in frame order.
    pub frame_durations_ms: Vec<u32>,
}

/// What kind of file is embedded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedImage {
    /// A static PNG (flag `0x0400` only).
    Png {
        /// IHDR width.
        width: u32,
        /// IHDR height.
        height: u32,
    },
    /// An animated WebP (flag `0x0800`).
    AnimatedWebp(WebpAnimation),
}

/// A decoded big prop: the base image, the placement descriptor and the
/// embedded file, all retained so the original blob can be reconstructed byte
/// for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigProp {
    /// The outer 44x44 header.
    pub header: PropHeader,
    /// Raw 4840-byte base image, exactly as stored (uncompressed S20).
    pub base_raw: Vec<u8>,
    /// The base image decoded to RGBA.
    pub base_image: PropImage,
    /// The big-image placement descriptor.
    pub descriptor: BigImageDescriptor,
    /// The embedded file length as declared by the container.
    pub image_len: u32,
    /// The embedded PNG / animated WebP bytes, verbatim.
    pub image: Vec<u8>,
    /// Parsed metadata for the embedded file.
    pub embedded: EmbeddedImage,
}

impl BigProp {
    /// Whether the embedded file is an animated WebP (i.e. a multi-frame prop).
    #[must_use]
    pub fn is_animated(&self) -> bool {
        matches!(self.embedded, EmbeddedImage::AnimatedWebp(_))
    }
}

/// Whether `data` (a prop blob, bag prefix already stripped) looks like a big
/// prop container, without decoding it.
#[must_use]
pub fn is_big_prop(data: &[u8]) -> bool {
    PropHeader::parse(data).is_ok_and(|h| h.flags & BIG_FLAG_MASK != 0)
}

/// Decode a big-prop container (a prop blob with the bag prefix already
/// stripped).
///
/// Never panics on malformed input.
pub fn decode_big_prop(data: &[u8]) -> Result<BigProp, ReconError> {
    let header = PropHeader::parse(data)?;
    if header.flags & BIG_FLAG_MASK == 0 {
        return Err(ReconError::NotABigProp {
            flags: header.flags,
        });
    }
    if data.len() < IMAGE_OFFSET {
        return Err(ReconError::TooShort {
            available: data.len(),
            needed: IMAGE_OFFSET,
        });
    }
    let base_raw = data[BASE_IMAGE_OFFSET..IMAGE_LEN_OFFSET].to_vec();
    let base_image = s20::decode(&base_raw, BASE_SIDE, BASE_SIDE).map_err(ReconError::BaseImage)?;

    let image_len = read_u32(data, IMAGE_LEN_OFFSET, header.endian);
    let image_end =
        IMAGE_OFFSET
            .checked_add(image_len as usize)
            .ok_or(ReconError::ImageRunsPastEnd {
                declared: image_len as usize,
                available: 0,
            })?;
    if image_end > data.len() {
        return Err(ReconError::ImageRunsPastEnd {
            declared: image_len as usize,
            available: data.len() - IMAGE_OFFSET,
        });
    }
    let image = data[IMAGE_OFFSET..image_end].to_vec();
    let descriptor = BigImageDescriptor::parse(
        &data[DESCRIPTOR_OFFSET..DESCRIPTOR_OFFSET + DESCRIPTOR_LEN],
        header.endian,
    )?;

    let embedded = if header.flags & FLAG_WEBP != 0 {
        EmbeddedImage::AnimatedWebp(parse_webp_animation(&image)?)
    } else {
        parse_png(&image)?
    };

    Ok(BigProp {
        header,
        base_raw,
        base_image,
        descriptor,
        image_len,
        image,
        embedded,
    })
}

/// Re-assemble the blob from its parsed parts.
///
/// Because the undetermined descriptor field is preserved verbatim and the base
/// image is kept as raw bytes, this is byte-exact for any blob this module
/// decodes — which is what makes it safe to prove the reading with a
/// round-trip instead of trusting it.
#[must_use]
pub fn encode_big_prop(prop: &BigProp) -> Vec<u8> {
    let mut out = Vec::with_capacity(IMAGE_OFFSET + prop.image.len());
    out.extend_from_slice(&prop.header.encode());
    out.extend_from_slice(&prop.base_raw);
    out.extend_from_slice(&match prop.header.endian {
        PropEndian::Little => prop.image_len.to_le_bytes(),
        PropEndian::Big => prop.image_len.to_be_bytes(),
    });
    out.extend_from_slice(&prop.descriptor.encode(prop.header.endian));
    out.extend_from_slice(&prop.image);
    out
}

/// Parse the PNG signature and IHDR dimensions. Does not decode pixels.
fn parse_png(data: &[u8]) -> Result<EmbeddedImage, ReconError> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if data.len() < 24 || data[..8] != SIG || &data[12..16] != b"IHDR" {
        return Err(ReconError::EmbeddedNotPng);
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Ok(EmbeddedImage::Png { width, height })
}

/// Walk a RIFF/WEBP container, reading the animation chunks. Does not decode
/// VP8 pixels — only the frame table and timing, which is the part the recon
/// needed.
fn parse_webp_animation(data: &[u8]) -> Result<WebpAnimation, ReconError> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(ReconError::EmbeddedNotWebp);
    }
    let mut canvas_width = 0u32;
    let mut canvas_height = 0u32;
    let mut loop_count = 0u16;
    let mut background = 0u32;
    let mut frame_durations_ms = Vec::new();

    let mut i = 12usize;
    while i + 8 <= data.len() {
        let fourcc: [u8; 4] = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        let size = read_u32(data, i + 4, PropEndian::Little) as usize;
        let payload = i + 8;
        let end = payload
            .checked_add(size)
            .ok_or(ReconError::WebpChunkTruncated {
                fourcc,
                declared: size,
            })?;
        if end > data.len() {
            return Err(ReconError::WebpChunkTruncated {
                fourcc,
                declared: size,
            });
        }
        match &fourcc {
            b"VP8X" if size >= 10 => {
                canvas_width = 1 + u24(&data[payload + 4..payload + 7]);
                canvas_height = 1 + u24(&data[payload + 7..payload + 10]);
            }
            b"ANIM" if size >= 6 => {
                background = read_u32(data, payload, PropEndian::Little);
                loop_count = read_u32(data, payload + 4, PropEndian::Little) as u16;
            }
            b"ANMF" if size >= 16 => {
                frame_durations_ms.push(u24(&data[payload + 12..payload + 15]));
            }
            _ => {}
        }
        i = end + (size & 1);
    }

    Ok(WebpAnimation {
        canvas_width,
        canvas_height,
        frame_count: frame_durations_ms.len() as u32,
        loop_count,
        background,
        frame_durations_ms,
    })
}

#[inline]
fn u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn read_i16(data: &[u8], offset: usize, endian: PropEndian) -> i16 {
    let pair = [data[offset], data[offset + 1]];
    match endian {
        PropEndian::Little => i16::from_le_bytes(pair),
        PropEndian::Big => i16::from_be_bytes(pair),
    }
}

fn read_u32(data: &[u8], offset: usize, endian: PropEndian) -> u32 {
    let quad = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
    match endian {
        PropEndian::Little => u32::from_le_bytes(quad),
        PropEndian::Big => u32::from_be_bytes(quad),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_round_trips_in_both_byte_orders() {
        let d = BigImageDescriptor {
            width: 220,
            height: 123,
            x_offset: -88,
            y_offset: -39,
            reserved: 119,
            flags: 0x0c02,
        };
        for endian in [PropEndian::Little, PropEndian::Big] {
            let bytes = d.encode(endian);
            let parsed = BigImageDescriptor::parse(&bytes, endian).unwrap();
            assert_eq!(parsed, d);
        }
    }

    #[test]
    fn a_real_world_descriptor_decodes() {
        // 220x220, centred at -88,-88, reserved 100, flags 0x0402 (big-endian).
        let bytes = [
            0x00, 0xdc, 0x00, 0xdc, 0xff, 0xa8, 0xff, 0xa8, 0x00, 0x64, 0x04, 0x02,
        ];
        let d = BigImageDescriptor::parse(&bytes, PropEndian::Big).unwrap();
        assert_eq!(d.width, 220);
        assert_eq!(d.height, 220);
        assert_eq!((d.x_offset, d.y_offset), (-88, -88));
        assert_eq!(d.reserved, 100);
        assert_eq!(d.flags, 0x0402);
    }

    #[test]
    fn a_non_big_header_is_rejected_not_decoded() {
        // A plain 44x44 S20 header.
        let mut blob = vec![0x00, 0x2c, 0x00, 0x2c];
        blob.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x02, 0x00]);
        blob.extend_from_slice(&[0u8; 4840]);
        blob.extend_from_slice(&[0, 0, 0, 0]);
        blob.extend_from_slice(&[0u8; 12]);
        assert!(matches!(
            decode_big_prop(&blob),
            Err(ReconError::NotABigProp { flags: 0x0200 })
        ));
        assert!(!is_big_prop(&blob));
    }
}

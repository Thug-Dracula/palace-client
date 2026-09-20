//! Animated / "big" props: reading them and rendering their first frame.
//!
//! This is the **shipping** face of the task-33 reconnaissance. The prototype
//! in [`crate::prop_animated_recon`] proved the container layout byte-exactly on
//! all 126 records of a live bag; this module promotes that into a stable,
//! tested API and adds the one thing renderers need: a decoded image.
//!
//! # What the container is
//!
//! A big prop is not a bespoke frame list. It is an ordinary prop whose header
//! sets flag bit `0x0400` and which carries, after a raw 44x44 S20 base image,
//! a length-prefixed **embedded image file**:
//!
//! ```text
//! offset  size   field
//! 0       12     header: 44x44, offsets/script 0, flags 0x0400 (+0x0800)
//! 12      4840   the 44x44 base image, raw (uncompressed) S20
//! 4852    4      u32 length L of the embedded image file
//! 4856    12     big-image descriptor: imgW, imgH, xOff, yOff, reserved, flags
//! 4868    L      the embedded file: a still PNG, or an animated WebP
//! ```
//!
//! `0x0400` alone means the embedded file is a still PNG (115/126 records);
//! `0x0800` (always with `0x0400`) means it is an animated WebP (11/126), and
//! **only those are multi-frame**. The frames and their timing live inside the
//! WebP, not in the container. See `$CORPUS/PRP-ANIMATED.md`.
//!
//! # Frames, pixels and why the crate stops where it does
//!
//! [`AnimatedProp::frame_count`] and [`AnimatedProp::frame_infos`] report the
//! container's frame table: per-frame dimensions and display duration, taken
//! from the PNG header or the WebP `ANMF` chunks.
//!
//! [`AnimatedProp::decode_frame`] returns pixels **where this crate can produce
//! them**:
//!
//! * the still-PNG class — frame 0 is the embedded PNG, decoded through the
//!   crate's existing `png` dependency;
//! * the animated-WebP class — frame 0 is the container's built-in 44x44 S20
//!   still, which is what a legacy client draws and the only frame pixels this
//!   dependency-light crate can decode. The remaining frames are VP8/VP8L
//!   bitstreams inside the WebP, and decoding those is deliberately the app
//!   layer's job (the crate takes no WebP dependency); their metadata is still
//!   reported here.
//!
//! So `frame_count` is exact, dimensions and timing are exact, and frame 0 is
//! always a valid image — which is what the bag's thumbnails and the room's
//! renderer need. It is **not** a claim that every WebP frame can be decoded
//! here.
//!
//! # Authoring is blocked
//!
//! This module only **reads**. There is deliberately no encoder for a new
//! animated prop: three fields/behaviours are still unpinned (descriptor
//! `reserved`, the base-image resampling rule, and live client round-trip), so a
//! writer could corrupt a prop. See `PRP-ANIMATED.md` §9. `to_blob` is a
//! byte-exact re-serialisation of an already-decoded blob, which is how the
//! reading is proven — it is not an authoring path.

use std::fmt;

use crate::error::MAX_PIXELS;
use crate::header::PropHeader;
use crate::image::PropImage;
use crate::prop_animated_recon::{
    decode_big_prop, is_big_prop as recon_is_big_prop, BigProp, BASE_SIDE,
};

// The container's types and constants belong to the prototype; re-export them
// so callers of the shipping module never need to name the recon module.
pub use crate::prop_animated_recon::{
    BigImageDescriptor, EmbeddedImage, ReconError, WebpAnimation, BASE_IMAGE_LEN,
    BASE_IMAGE_OFFSET, BIG_FLAG_MASK, DESCRIPTOR_LEN, DESCRIPTOR_OFFSET, FLAG_BIG_IMAGE, FLAG_WEBP,
    IMAGE_LEN_OFFSET, IMAGE_OFFSET,
};

/// Why an animated / big prop could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnimatedError {
    /// The container itself was malformed or not a big prop.
    Container(ReconError),
    /// A frame's PNG could not be decoded into pixels.
    Png { detail: String },
    /// A decoded image's dimensions were zero or beyond the crate's budget.
    ImageDimensions { width: u32, height: u32 },
}

impl fmt::Display for AnimatedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnimatedError::Container(e) => write!(f, "animated prop container: {e}"),
            AnimatedError::Png { detail } => write!(f, "embedded PNG: {detail}"),
            AnimatedError::ImageDimensions { width, height } => {
                write!(
                    f,
                    "embedded image dimensions {width}x{height} are out of range"
                )
            }
        }
    }
}

impl std::error::Error for AnimatedError {}

impl From<ReconError> for AnimatedError {
    fn from(e: ReconError) -> Self {
        AnimatedError::Container(e)
    }
}

/// One frame of the container's frame table.
///
/// `width`/`height`/`duration_ms` are what the embedded file declares. `decoded`
/// says whether [`AnimatedProp::decode_frame`] can return this frame's pixels
/// in-crate (see the module docs): true for the still-PNG class and for frame 0
/// of the animated-WebP class, false for the later WebP frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Zero-based frame index, in playback order.
    pub index: usize,
    /// Declared frame width in pixels.
    pub width: u32,
    /// Declared frame height in pixels.
    pub height: u32,
    /// Display duration in milliseconds (0 for the still-PNG class).
    pub duration_ms: u32,
    /// Whether [`AnimatedProp::decode_frame`] yields pixels for this frame.
    pub decoded: bool,
}

/// A decoded big prop: the container, its frame table and its renderable first
/// frame.
///
/// Built by [`AnimatedProp::decode`]. It never silently rejects a big-prop
/// record the way the old single-prop path did: a record that parses as a
/// container is always representable, with at least frame 0 renderable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimatedProp {
    container: BigProp,
    frames: Vec<FrameInfo>,
}

impl AnimatedProp {
    /// Decode a big-prop blob (bag prefix already stripped).
    ///
    /// Fails only when the record genuinely is not a big prop or its fixed
    /// prefix is unusable; an animated WebP whose pixels the app layer must
    /// decode still decodes here.
    pub fn decode(data: &[u8]) -> Result<Self, AnimatedError> {
        let container = decode_big_prop(data)?;
        let frames = build_frames(&container);
        Ok(Self { container, frames })
    }

    /// Whether the embedded file is an animated WebP (i.e. genuinely
    /// multi-frame). False for the still-PNG class.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.container.is_animated()
    }

    /// Number of frames the container declares: 1 for a still PNG, the `ANMF`
    /// count for an animated WebP.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// The frame table, in playback order.
    #[must_use]
    pub fn frame_infos(&self) -> &[FrameInfo] {
        &self.frames
    }

    /// One frame's declared metadata, or `None` when `index` is past the end.
    #[must_use]
    pub fn frame_info(&self, index: usize) -> Option<FrameInfo> {
        self.frames.get(index).copied()
    }

    /// Per-frame display durations, in playback order.
    #[must_use]
    pub fn frame_durations_ms(&self) -> Vec<u32> {
        self.frames.iter().map(|frame| frame.duration_ms).collect()
    }

    /// Total playback time the frame table describes, in milliseconds.
    #[must_use]
    pub fn total_duration_ms(&self) -> u32 {
        self.frames
            .iter()
            .fold(0u32, |sum, frame| sum.saturating_add(frame.duration_ms))
    }

    /// The animated WebP's loop count (`0` = forever), or 0 for the still class.
    #[must_use]
    pub fn loop_count(&self) -> u16 {
        match &self.container.embedded {
            EmbeddedImage::AnimatedWebp(anim) => anim.loop_count,
            EmbeddedImage::Png { .. } => 0,
        }
    }

    /// The animated WebP's background colour, or 0 for the still class.
    #[must_use]
    pub fn background(&self) -> u32 {
        match &self.container.embedded {
            EmbeddedImage::AnimatedWebp(anim) => anim.background,
            EmbeddedImage::Png { .. } => 0,
        }
    }

    /// The image canvas the embedded file declares: the PNG's own dimensions, or
    /// the WebP's `VP8X` canvas.
    #[must_use]
    pub fn canvas(&self) -> (u32, u32) {
        match &self.container.embedded {
            EmbeddedImage::Png { width, height } => (*width, *height),
            EmbeddedImage::AnimatedWebp(anim) => (anim.canvas_width, anim.canvas_height),
        }
    }

    /// The 44x44 base image: the container's built-in still, immutable.
    #[must_use]
    pub fn base_image(&self) -> &PropImage {
        &self.container.base_image
    }

    /// The plain-prop image a legacy client draws: the 44x44 base.
    ///
    /// This is what [`crate::decode`] returns for a big prop, so that the
    /// header's 44x44 size and the image it accompanies always agree.
    #[must_use]
    pub fn legacy_image(&self) -> &PropImage {
        &self.container.base_image
    }

    /// The decoded embedded still for the PNG class; `None` for the animated
    /// class (whose pixels the app layer decodes).
    #[must_use]
    pub fn still_image(&self) -> Option<PropImage> {
        match &self.container.embedded {
            EmbeddedImage::Png { .. } => decode_png(&self.container.image).ok(),
            EmbeddedImage::AnimatedWebp(_) => None,
        }
    }

    /// Frame 0's pixels — always a valid image.
    ///
    /// * still-PNG class: the embedded PNG decoded to RGBA;
    /// * animated-WebP class: the container's 44x44 base still.
    #[must_use]
    pub fn frame_zero(&self) -> PropImage {
        self.decode_frame(0)
            .unwrap_or_else(|| self.container.base_image.clone())
    }

    /// A specific frame's decoded pixels, when this crate can produce them.
    ///
    /// Frame 0 always decodes; later frames of an animated WebP return `None`
    /// because decoding VP8/VP8L is the app layer's responsibility (see the
    /// module docs). Indexing past the end is `None`, never a panic.
    #[must_use]
    pub fn decode_frame(&self, index: usize) -> Option<PropImage> {
        if index >= self.frames.len() {
            return None;
        }
        match &self.container.embedded {
            EmbeddedImage::Png { .. } => {
                if index == 0 {
                    decode_png(&self.container.image).ok()
                } else {
                    None
                }
            }
            EmbeddedImage::AnimatedWebp(_) => {
                if index == 0 {
                    Some(self.container.base_image.clone())
                } else {
                    None
                }
            }
        }
    }

    /// Frame 0 encoded as a PNG — what the bag catalog serves as a thumbnail.
    pub fn thumbnail_png(&self) -> Result<Vec<u8>, AnimatedError> {
        self.frame_zero()
            .to_png_bytes()
            .map_err(|e| AnimatedError::Png {
                detail: e.to_string(),
            })
    }

    /// The outer 12-byte header, as parsed.
    #[must_use]
    pub fn header(&self) -> PropHeader {
        self.container.header
    }

    /// The big-image placement descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &BigImageDescriptor {
        &self.container.descriptor
    }

    /// What kind of file is embedded, with its parsed metadata.
    #[must_use]
    pub fn embedded(&self) -> &EmbeddedImage {
        &self.container.embedded
    }

    /// The embedded file bytes, verbatim.
    #[must_use]
    pub fn embedded_bytes(&self) -> &[u8] {
        &self.container.image
    }

    /// The side of the base image in pixels (always 44).
    #[must_use]
    pub fn base_side(&self) -> u32 {
        BASE_SIDE
    }

    /// Re-assemble the blob byte-exactly from the decoded parts.
    ///
    /// This is the read-side proof (the undetermined descriptor field is kept
    /// verbatim), **not** an authoring encoder: it only rewrites a blob this
    /// module already decoded.
    #[must_use]
    pub fn to_blob(&self) -> Vec<u8> {
        crate::prop_animated_recon::encode_big_prop(&self.container)
    }

    /// The container prototype this was promoted from, for callers that need
    /// the raw base bytes or descriptor directly.
    #[must_use]
    pub fn container(&self) -> &BigProp {
        &self.container
    }
}

/// Whether `data` (a prop blob, bag prefix already stripped) is a big prop.
///
/// Cheap: it parses the header and tests the container flag bits only.
#[must_use]
pub fn is_big_prop(data: &[u8]) -> bool {
    recon_is_big_prop(data)
}

/// Decode a big prop and return frame 0 as a PNG, or `None` when it is not a
/// usable big-prop record.
///
/// The bag catalog's thumbnail path uses this so an animated prop's thumbnail is
/// its frame 0 rather than a decode failure.
#[must_use]
pub fn thumbnail_png(data: &[u8]) -> Option<Vec<u8>> {
    AnimatedProp::decode(data).ok()?.thumbnail_png().ok()
}

/// The decoded image [`crate::decode`] should use for a big prop: the 44x44
/// base still. `None` when the record is not a usable big prop.
#[must_use]
pub(crate) fn legacy_image(data: &[u8]) -> Option<PropImage> {
    AnimatedProp::decode(data)
        .ok()
        .map(|prop| prop.container.base_image.clone())
}

/// Build the frame table from the embedded file's own metadata.
fn build_frames(container: &BigProp) -> Vec<FrameInfo> {
    match &container.embedded {
        EmbeddedImage::Png { width, height } => vec![FrameInfo {
            index: 0,
            width: *width,
            height: *height,
            duration_ms: 0,
            decoded: true,
        }],
        EmbeddedImage::AnimatedWebp(anim) => {
            let count = anim.frame_count as usize;
            let declared = parse_webp_frame_meta(&container.image);
            let mut frames = Vec::with_capacity(count);
            for index in 0..count {
                let (width, height, duration_ms) = declared
                    .as_ref()
                    .and_then(|metas| metas.get(index))
                    .map_or_else(
                        || {
                            (
                                anim.canvas_width,
                                anim.canvas_height,
                                anim.frame_durations_ms.get(index).copied().unwrap_or(0),
                            )
                        },
                        |meta| (meta.width, meta.height, meta.duration_ms),
                    );
                frames.push(FrameInfo {
                    index,
                    width,
                    height,
                    duration_ms,
                    decoded: index == 0,
                });
            }
            frames
        }
    }
}

/// Per-frame dimensions and duration read from a WebP's `ANMF` chunks.
struct WebpFrameMeta {
    width: u32,
    height: u32,
    duration_ms: u32,
}

/// Walk a RIFF/WEBP container's `ANMF` chunks, in order.
///
/// The prototype already reads the durations; this adds the per-frame
/// width/height that the `ANMF` header carries (bytes 6..9 and 9..12 of the
/// payload, as `size - 1`). Returns `None` on any structural problem so callers
/// fall back to the canvas dimensions.
fn parse_webp_frame_meta(data: &[u8]) -> Option<Vec<WebpFrameMeta>> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return None;
    }
    let mut frames = Vec::new();
    let mut i = 12usize;
    while i + 8 <= data.len() {
        let fourcc = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        let size =
            u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        let payload = i + 8;
        let end = payload.checked_add(size)?;
        if end > data.len() {
            return None;
        }
        if &fourcc == b"ANMF" && size >= 16 {
            frames.push(WebpFrameMeta {
                width: 1 + u24(&data[payload + 6..payload + 9]),
                height: 1 + u24(&data[payload + 9..payload + 12]),
                duration_ms: u24(&data[payload + 12..payload + 15]),
            });
        }
        i = end.checked_add(size & 1)?;
    }
    Some(frames)
}

#[inline]
fn u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

/// Decode an embedded PNG into RGBA using the crate's existing `png` dependency.
///
/// Never panics on malformed input: a corrupt file is an [`AnimatedError`]. The
/// declared dimensions are checked against the crate's pixel budget *before* the
/// pixel buffer is allocated, so a malformed header cannot make us allocate
/// unboundedly.
fn decode_png(data: &[u8]) -> Result<PropImage, AnimatedError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| AnimatedError::Png {
        detail: e.to_string(),
    })?;
    let (width, height) = reader.info().size();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(AnimatedError::ImageDimensions { width, height });
    }
    let buffer_size = reader
        .output_buffer_size()
        .ok_or(AnimatedError::ImageDimensions { width, height })?;
    let mut buffer = vec![0u8; buffer_size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| AnimatedError::Png {
            detail: e.to_string(),
        })?;
    let pixels = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => expand_channels(pixels, 3),
        png::ColorType::Grayscale => expand_channels(pixels, 1),
        png::ColorType::GrayscaleAlpha => expand_channels(pixels, 2),
        png::ColorType::Indexed => {
            return Err(AnimatedError::Png {
                detail: "palette PNG was not expanded by the decoder".to_string(),
            });
        }
    };
    PropImage::from_rgba(info.width, info.height, rgba).map_err(|e| AnimatedError::Png {
        detail: e.to_string(),
    })
}

/// Widen packed samples to RGBA.
///
/// `channels` is 1 (grey), 2 (grey+alpha) or 3 (RGB); alpha defaults to opaque.
fn expand_channels(pixels: &[u8], channels: usize) -> Vec<u8> {
    debug_assert!((1..=3).contains(&channels));
    let mut rgba = Vec::with_capacity(pixels.len() / channels * 4);
    for chunk in pixels.chunks_exact(channels) {
        match channels {
            1 => rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], 0xff]),
            2 => rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]),
            _ => rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 0xff]),
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u24_is_little_endian() {
        assert_eq!(u24(&[0x01, 0x02, 0x03]), 0x03_0201);
        assert_eq!(u24(&[0xff, 0x00, 0x00]), 0xff);
    }

    #[test]
    fn expand_channels_widens_grey_and_rgb() {
        assert_eq!(
            expand_channels(&[10, 20], 1),
            vec![10, 10, 10, 255, 20, 20, 20, 255]
        );
        assert_eq!(expand_channels(&[1, 2, 3], 3), vec![1, 2, 3, 255]);
        assert_eq!(expand_channels(&[9, 128], 2), vec![9, 9, 9, 128]);
    }
}

//! Type 1 avatars: a single server-hosted image, not a stack of prop tiles.
//!
//! The server advertises what it accepts in the `'AVAT'` block of an
//! `EXTENDEDINFO` (`sInf`) reply: a format bit mask, a payload cap in kilobytes
//! and dimension caps. This module turns that block into a client-side limit,
//! sniffs an image's real format, checks it against the limit, and computes the
//! 20-byte content hash the server identifies the avatar by.
//!
//! Spec note `$CORPUS/TYPE1-AVATARS.md` §2/§3/§5. Format bits
//! and the `ExtendedInfoAvatar` layout are HIGH confidence (PP SDK `mansion.h`);
//! the live server reported `0x7` / 14 KB / 132x132 (spec §6). The hash
//! algorithm is SHA-1, read from the compiled server's own symbols
//! (`computeAvatarHash`, `verifyAvatarHash`, `SHA1_*`); the spec note did not
//! name it, so this is a stronger source than the note, not a deviation from it.

use std::fmt;

use palace_prop::PropImage;
use palace_wire::messages::{
    AvatarHash, ExtendedInfoAvatar, AVATAR_HASH_LEN, AVFORM_FLASH, AVFORM_GIF, AVFORM_JPEG,
    AVFORM_MNG, AVFORM_PNG99A,
};
use serde::Serialize;

/// One image format a Type 1 avatar may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type1Format {
    /// Animated or still GIF (`AVFORM_GIF`).
    Gif,
    /// JPEG (`AVFORM_JPEG`).
    Jpeg,
    /// PNG (`AVFORM_PNG99A`).
    Png,
}

impl Type1Format {
    /// The format's `AVFORM_*` bit.
    #[must_use]
    pub const fn bit(self) -> u32 {
        match self {
            Type1Format::Gif => AVFORM_GIF,
            Type1Format::Jpeg => AVFORM_JPEG,
            Type1Format::Png => AVFORM_PNG99A,
        }
    }

    /// The MIME type a route should serve it with.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Type1Format::Gif => "image/gif",
            Type1Format::Jpeg => "image/jpeg",
            Type1Format::Png => "image/png",
        }
    }

    /// The lowercase name used in messages and URLs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Type1Format::Gif => "GIF",
            Type1Format::Jpeg => "JPEG",
            Type1Format::Png => "PNG",
        }
    }

    /// The file extension for a cached copy.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Type1Format::Gif => "gif",
            Type1Format::Jpeg => "jpg",
            Type1Format::Png => "png",
        }
    }

    /// Sniff the format from a file's leading bytes, regardless of extension.
    ///
    /// A `.png` that actually holds a JPEG is rejected here rather than at the
    /// server, so the client's own message names the real problem.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Type1Format::Gif);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Type1Format::Jpeg);
        }
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Some(Type1Format::Png);
        }
        None
    }
}

impl fmt::Display for Type1Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The server's Type 1 avatar limits, from its `'AVAT'` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Type1AvatarLimits {
    /// Permitted format bits (`AVFORM_*`).
    pub formats: u32,
    /// Maximum payload in kilobytes; `0` means unlimited.
    pub max_payload_kb: u16,
    /// Maximum height in pixels; `0` means unlimited.
    pub max_height: u16,
    /// Maximum width in pixels; `0` means unlimited.
    pub max_width: u16,
}

impl Type1AvatarLimits {
    /// Build from the decoded `'AVAT'` block.
    #[must_use]
    pub const fn from_info(info: ExtendedInfoAvatar) -> Self {
        Type1AvatarLimits {
            formats: info.formats,
            max_payload_kb: info.max_payload,
            max_height: info.max_height,
            max_width: info.max_width,
        }
    }

    /// The payload cap in bytes, or `None` when the server set no cap.
    #[must_use]
    pub fn max_bytes(&self) -> Option<usize> {
        (self.max_payload_kb != 0).then(|| usize::from(self.max_payload_kb) * 1024)
    }

    /// Whether `format` is permitted.
    #[must_use]
    pub const fn permits(&self, format: Type1Format) -> bool {
        self.formats & format.bit() != 0
    }

    /// Whether any format at all is permitted.
    #[must_use]
    pub const fn allows_any(&self) -> bool {
        self.formats & (AVFORM_GIF | AVFORM_JPEG | AVFORM_PNG99A) != 0
    }

    /// The permitted formats, for a message.
    #[must_use]
    pub fn permitted_names(&self) -> String {
        let mut names = Vec::new();
        for (bit, name) in [
            (AVFORM_GIF, "GIF"),
            (AVFORM_JPEG, "JPEG"),
            (AVFORM_PNG99A, "PNG"),
            (AVFORM_MNG, "MNG"),
            (AVFORM_FLASH, "Flash"),
        ] {
            if self.formats & bit != 0 {
                names.push(name);
            }
        }
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }
}

/// Why an image was refused as a Type 1 avatar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type1AvatarError {
    /// The bytes are not a GIF, JPEG or PNG.
    UnknownFormat,
    /// The image's format is understood but the server does not allow it.
    FormatNotAllowed {
        /// The image's actual format.
        format: Type1Format,
        /// The formats the server allows, already rendered.
        allowed: String,
    },
    /// The image is larger than the server's payload cap.
    TooLarge {
        /// The image's size in bytes.
        size: usize,
        /// The server's cap in bytes.
        max: usize,
    },
    /// The image is wider than the server's cap.
    TooWide {
        /// The image's width in pixels.
        width: u32,
        /// The server's cap in pixels.
        max: u32,
    },
    /// The image is taller than the server's cap.
    TooTall {
        /// The image's height in pixels.
        height: u32,
        /// The server's cap in pixels.
        max: u32,
    },
    /// The image's dimensions could not be read from its header.
    UnreadableDimensions {
        /// The image's actual format.
        format: Type1Format,
    },
    /// Re-encoding a prop to an allowed format failed.
    Encode {
        /// The encoder's own message.
        detail: String,
    },
}

impl fmt::Display for Type1AvatarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type1AvatarError::UnknownFormat => {
                write!(f, "that file is not a GIF, JPEG or PNG")
            }
            Type1AvatarError::FormatNotAllowed { format, allowed } => write!(
                f,
                "this server does not allow {format} avatars (it allows {allowed})"
            ),
            Type1AvatarError::TooLarge { size, max } => write!(
                f,
                "the image is {size} bytes; this server's limit is {max} bytes"
            ),
            Type1AvatarError::TooWide { width, max } => write!(
                f,
                "the image is {width} pixels wide; this server's limit is {max}"
            ),
            Type1AvatarError::TooTall { height, max } => write!(
                f,
                "the image is {height} pixels tall; this server's limit is {max}"
            ),
            Type1AvatarError::UnreadableDimensions { format } => write!(
                f,
                "the {format} image's dimensions could not be read from its header"
            ),
            Type1AvatarError::Encode { detail } => {
                write!(f, "the prop could not be re-encoded: {detail}")
            }
        }
    }
}

impl std::error::Error for Type1AvatarError {}

/// Check an image against the server's limits and return its format and size.
///
/// The checks run in the order a user would fix them: format first, then size,
/// then dimensions. A dimension cap of `0` means the server set no cap in that
/// dimension (spec §4, `avatarmaxdimensions`).
pub fn validate_type1(
    bytes: &[u8],
    limits: &Type1AvatarLimits,
) -> Result<(Type1Format, u32, u32), Type1AvatarError> {
    let format = Type1Format::sniff(bytes).ok_or(Type1AvatarError::UnknownFormat)?;
    if !limits.permits(format) {
        return Err(Type1AvatarError::FormatNotAllowed {
            format,
            allowed: limits.permitted_names(),
        });
    }
    if let Some(max) = limits.max_bytes() {
        if bytes.len() > max {
            return Err(Type1AvatarError::TooLarge {
                size: bytes.len(),
                max,
            });
        }
    }
    let (width, height) =
        image_dimensions(bytes, format).ok_or(Type1AvatarError::UnreadableDimensions { format })?;
    if limits.max_width != 0 && width > u32::from(limits.max_width) {
        return Err(Type1AvatarError::TooWide {
            width,
            max: u32::from(limits.max_width),
        });
    }
    if limits.max_height != 0 && height > u32::from(limits.max_height) {
        return Err(Type1AvatarError::TooTall {
            height,
            max: u32::from(limits.max_height),
        });
    }
    Ok((format, width, height))
}

/// Read `(width, height)` from an image's header without decoding its pixels.
#[must_use]
pub fn image_dimensions(bytes: &[u8], format: Type1Format) -> Option<(u32, u32)> {
    match format {
        Type1Format::Png => png_dimensions(bytes),
        Type1Format::Gif => gif_dimensions(bytes),
        Type1Format::Jpeg => jpeg_dimensions(bytes),
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Signature(8) + IHDR length(4) + type(4) = 16, then width and height.
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Logical screen descriptor: little-endian u16 width then height at 6..10.
    let width = u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?);
    let height = u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?);
    Some((u32::from(width), u32::from(height)))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut offset = 2usize;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xFF {
            return None;
        }
        let marker = bytes[offset + 1];
        // Standalone markers carry no length.
        if (0xD0..=0xD9).contains(&marker) {
            offset += 2;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]));
        if length < 2 {
            return None;
        }
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            // SOF: length(2) precision(1) height(2) width(2)
            let height = be_u16_at(bytes, offset + 5)?;
            let width = be_u16_at(bytes, offset + 7)?;
            return Some((u32::from(width), u32::from(height)));
        }
        offset = offset.checked_add(2 + length)?;
    }
    None
}

fn be_u16_at(bytes: &[u8], start: usize) -> Option<u16> {
    let slice = bytes.get(start..start + 2)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

/// Re-encode a decoded prop as a Type 1 avatar, if the server allows PNG.
///
/// A prop is always RGBA art, so PNG is its natural container; the server must
/// have `AVFORM_PNG99A` set. The encoded image is then checked against the same
/// limits a file would be.
pub fn export_prop_as_type1(
    image: &PropImage,
    limits: &Type1AvatarLimits,
) -> Result<Vec<u8>, Type1AvatarError> {
    if !limits.permits(Type1Format::Png) {
        return Err(Type1AvatarError::FormatNotAllowed {
            format: Type1Format::Png,
            allowed: limits.permitted_names(),
        });
    }
    let png = image
        .to_png_bytes()
        .map_err(|error| Type1AvatarError::Encode {
            detail: error.to_string(),
        })?;
    validate_type1(&png, limits)?;
    Ok(png)
}

/// The 20-byte content hash the server identifies a Type 1 avatar by.
///
/// SHA-1, matching the compiled server's `computeAvatarHash`/`verifyAvatarHash`
/// (spec §1 names a 20-byte content hash but not the algorithm; the binary's
/// symbols do).
#[must_use]
pub fn content_hash(bytes: &[u8]) -> AvatarHash {
    AvatarHash::new(sha1(bytes))
}

/// SHA-1, FIPS 180-1.
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; AVATAR_HASH_LEN] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);

    let mut message = Vec::with_capacity(data.len() + 72);
    message.extend_from_slice(data);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let start = index * 4;
            let mut four = [0u8; 4];
            four.copy_from_slice(&chunk[start..start + 4]);
            *word = u32::from_be_bytes(four);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }

    let mut out = [0u8; AVATAR_HASH_LEN];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Type1AvatarLimits {
        Type1AvatarLimits {
            formats: AVFORM_GIF | AVFORM_JPEG | AVFORM_PNG99A,
            max_payload_kb: 14,
            max_height: 132,
            max_width: 132,
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer
                .write_image_data(&vec![0x10u8; (width * height * 4) as usize])
                .expect("png data");
        }
        out
    }

    /// A PNG whose pixels barely compress, so payload-cap tests are meaningful.
    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut state: u32 = 0x1234_5678;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height * 4) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            pixels.push((state & 0xFF) as u8);
        }
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&pixels).expect("png data");
        }
        out
    }

    fn gif() -> Vec<u8> {
        // A 1x1 GIF89a: header + logical screen descriptor + trailer.
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x3B]);
        out
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        // SOI, APP0 (length 4, no payload), SOF0 with the dimensions, EOI.
        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00]);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn sha1_matches_published_known_answers() {
        assert_eq!(
            sha1(b""),
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
                0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
        assert_eq!(
            content_hash(b"abc").to_hex(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn sniff_identifies_the_three_allowed_formats() {
        assert_eq!(Type1Format::sniff(&png(1, 1)), Some(Type1Format::Png));
        assert_eq!(Type1Format::sniff(&gif()), Some(Type1Format::Gif));
        assert_eq!(Type1Format::sniff(&jpeg(1, 1)), Some(Type1Format::Jpeg));
        assert_eq!(Type1Format::sniff(b"not an image"), None);
    }

    #[test]
    fn dimensions_are_read_for_each_format() {
        assert_eq!(image_dimensions(&png(7, 9), Type1Format::Png), Some((7, 9)));
        assert_eq!(image_dimensions(&gif(), Type1Format::Gif), Some((1, 1)));
        assert_eq!(
            image_dimensions(&jpeg(40, 30), Type1Format::Jpeg),
            Some((40, 30))
        );
    }

    #[test]
    fn a_valid_image_passes() {
        let bytes = png(64, 64);
        let (format, width, height) = validate_type1(&bytes, &limits()).expect("valid");
        assert_eq!(format, Type1Format::Png);
        assert_eq!((width, height), (64, 64));
    }

    #[test]
    fn an_unallowed_format_is_refused_with_a_clear_message() {
        // The server allows GIF+JPEG+PNG; an MNG is understood but not allowed.
        let mut mng = vec![0x8A, b'M', b'N', b'G'];
        mng.extend_from_slice(&[0u8; 16]);
        let error = validate_type1(&mng, &limits()).expect_err("MNG is refused");
        assert!(matches!(error, Type1AvatarError::UnknownFormat));

        // A GIF is understood but a PNG-only server refuses it.
        let png_only = Type1AvatarLimits {
            formats: AVFORM_PNG99A,
            ..limits()
        };
        let error = validate_type1(&gif(), &png_only).expect_err("GIF is refused");
        assert_eq!(
            error,
            Type1AvatarError::FormatNotAllowed {
                format: Type1Format::Gif,
                allowed: "PNG".to_string()
            }
        );
        assert!(error.to_string().contains("does not allow GIF"));
    }

    #[test]
    fn an_over_size_image_is_refused_with_a_clear_message() {
        let mut tight = limits();
        tight.max_payload_kb = 1;
        let bytes = noisy_png(128, 128);
        assert!(bytes.len() > 1024, "the fixture is over the 1 KB cap");
        let error = validate_type1(&bytes, &tight).expect_err("over size is refused");
        assert!(matches!(error, Type1AvatarError::TooLarge { .. }));
        assert!(error
            .to_string()
            .contains("this server's limit is 1024 bytes"));
    }

    #[test]
    fn an_over_dimension_image_is_refused_in_both_dimensions() {
        let mut tight = limits();
        tight.max_width = 8;
        tight.max_height = 8;
        let wide = png(16, 4);
        assert_eq!(
            validate_type1(&wide, &tight),
            Err(Type1AvatarError::TooWide { width: 16, max: 8 })
        );
        let tall = png(4, 16);
        assert_eq!(
            validate_type1(&tall, &tight),
            Err(Type1AvatarError::TooTall { height: 16, max: 8 })
        );
    }

    #[test]
    fn a_zero_dimension_cap_means_no_limit() {
        let unlimited = Type1AvatarLimits {
            formats: AVFORM_PNG99A,
            max_payload_kb: 0,
            max_height: 0,
            max_width: 0,
        };
        let bytes = png(1024, 1024);
        assert!(validate_type1(&bytes, &unlimited).is_ok());
        assert_eq!(unlimited.max_bytes(), None);
    }

    #[test]
    fn the_live_server_limits_accept_a_132_png_and_refuse_a_133() {
        // Spec §6: formats 0x7, 14 KB, 132x132.
        assert!(validate_type1(&png(132, 132), &limits()).is_ok());
        assert!(matches!(
            validate_type1(&png(133, 132), &limits()),
            Err(Type1AvatarError::TooWide { .. })
        ));
    }

    #[test]
    fn a_prop_exports_as_a_type1_png_when_the_server_allows_png() {
        let image = PropImage::from_rgba(44, 44, vec![0x33; 44 * 44 * 4]).expect("a 44x44 prop");
        let bytes = export_prop_as_type1(&image, &limits()).expect("PNG is allowed");
        assert_eq!(Type1Format::sniff(&bytes), Some(Type1Format::Png));
        assert_eq!(image_dimensions(&bytes, Type1Format::Png), Some((44, 44)));
    }

    #[test]
    fn a_prop_export_is_refused_when_the_server_disallows_png() {
        let image = PropImage::from_rgba(44, 44, vec![0x33; 44 * 44 * 4]).expect("a 44x44 prop");
        let gif_only = Type1AvatarLimits {
            formats: AVFORM_GIF,
            ..limits()
        };
        assert!(matches!(
            export_prop_as_type1(&image, &gif_only),
            Err(Type1AvatarError::FormatNotAllowed {
                format: Type1Format::Png,
                ..
            })
        ));
    }
}

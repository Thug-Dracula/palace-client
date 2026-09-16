//! The decoded pixel buffer.
//!
//! Every decoder in [`crate::codec`] produces one of these. It is deliberately
//! dependency-free: `RGBA8`, row-major, top-down, tightly packed. PNG export is a
//! separate convenience so that the codec itself never depends on an image crate.

use std::path::Path;

use crate::error::{PropError, Result};

/// A decoded prop image: 8 bits per channel, RGBA, row-major, top-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl PropImage {
    /// Build an image from packed ARGB (`0xAARRGGBB`) words, as the reference
    /// decoders work in.
    ///
    /// # Panics
    ///
    /// Panics if `argb.len() != width * height`. This is an internal helper for
    /// codec output, never for protocol input — every codec computes the buffer
    /// length from the same dimensions it passes here.
    #[must_use]
    pub fn from_argb(width: u32, height: u32, argb: &[u32]) -> Self {
        assert_eq!(
            argb.len(),
            (width as usize) * (height as usize),
            "ARGB buffer must hold exactly one word per pixel"
        );
        let mut rgba = Vec::with_capacity(argb.len() * 4);
        for word in argb {
            rgba.extend_from_slice(&[
                (word >> 16) as u8,
                (word >> 8) as u8,
                *word as u8,
                (word >> 24) as u8,
            ]);
        }
        PropImage {
            width,
            height,
            rgba,
        }
    }

    /// Build an image from raw RGBA bytes.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or(PropError::ImageTooLarge {
                width: width as i16,
                height: height as i16,
            })?;
        if rgba.len() != expected {
            return Err(PropError::PayloadTooShort {
                format: "RGBA image",
                needed: expected,
                available: rgba.len(),
            });
        }
        Ok(PropImage {
            width,
            height,
            rgba,
        })
    }

    /// An all-transparent image of the given size.
    #[must_use]
    pub fn transparent(width: u32, height: u32) -> Self {
        PropImage {
            width,
            height,
            rgba: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    /// Image width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGBA bytes, `width * height * 4` of them.
    #[must_use]
    pub fn as_rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// One pixel as `[r, g, b, a]`, or `None` when out of bounds.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ])
    }

    /// Re-interpret the image as packed ARGB words.
    #[must_use]
    pub fn to_argb(&self) -> Vec<u32> {
        self.rgba
            .chunks_exact(4)
            .map(|p| {
                (u32::from(p[3]) << 24)
                    | (u32::from(p[0]) << 16)
                    | (u32::from(p[1]) << 8)
                    | u32::from(p[2])
            })
            .collect()
    }

    /// Write the image out as a PNG.
    ///
    /// This exists so a human can look at what the decoder produced; nothing in
    /// the decode path calls it.
    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<()> {
        let file = std::fs::File::create(path).map_err(|e| PropError::Png {
            detail: e.to_string(),
        })?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| PropError::Png {
            detail: e.to_string(),
        })?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| PropError::Png {
                detail: e.to_string(),
            })?;
        Ok(())
    }
}

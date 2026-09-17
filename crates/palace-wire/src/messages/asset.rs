//! `MSG_ASSETREGI` (`rAst`) — the client-to-server prop-upload message.
//!
//! Wearing a prop is two messages. `rAst` registers the prop's art with the
//! server (once per prop); [`UserProp`](super::UserProp) then names the prop in
//! the worn list. The reference builds the body in
//! `PalaceProp.as::assetData(endian)` (lines 267-346) and sends it from
//! `PalaceClient.as::sendPropToServer(prop)` (lines 800-818).
//!
//! ## The body is a `sAst`/`rAst` block, not a bare prop
//!
//! `assetData` is the standard asset-transfer body (the protocol reference says
//! `sAst` and `rAst` are byte-identical apart from the opcode): a 24-byte block
//! header, a 40-byte descriptor, then the data. The data itself is a 12-byte prop
//! info block followed by the zlib-compressed S20 pixels `encodeS20BitProp()`
//! produces — i.e. a complete prop blob:
//!
//! ```text
//! offset  size  field                              ref (PalaceProp.as)
//!   0      4    AssetType `'Prop'` 0x50726f70      writeInt(ASSET_TYPE_PROP)   :296
//!   4      4    asset id                           writeInt(asset.id)          :299
//!   8      4    asset CRC = crc(imageData)         writeUnsignedInt(assetCRC)  :300
//!  12      4    blockSize = imageData.len() + 12   writeInt(size)              :303
//!  16      4    blockOffset = 0                    writeInt(0)                 :306
//!  20      2    blockNbr = 0                       writeShort(0)               :309
//!  22      2    nbrBlocks = 1                      writeShort(1)               :312
//!  24      4    descriptor flags = 0               writeUnsignedInt(0)         :316
//!  28      4    descriptor size = blockSize        writeUnsignedInt(size)      :318
//!  32      1    name length                        writeByte(name.length)      :320
//!  33     31    name, space padded                  writeMultiByte(paddedName)  :321-326
//!  64      2    width = 44                         writeShort(44)              :329
//!  66      2    height = 44                        writeShort(44)              :330
//!  68      2    h offset                           writeShort(horizontalOffset) :336
//!  70      2    v offset                           writeShort(verticalOffset)  :337
//!  72      2    script offset = 0                  writeShort(0)               :339
//!  74      2    flags | PROP_FORMAT_S20BIT         writeShort(flags)           :340
//!  76      N    imageData                          writeBytes(imageData)       :343
//! ```
//!
//! Two details that are easy to get wrong:
//!
//! * the CRC covers **`imageData` alone** (`computeCRC(imageData)` at :291), not
//!   the 12-byte prop info and not the whole body. Compute it with
//!   `palace_prop::asset_crc(image_data)`;
//! * the name is padded with **spaces**, not NULs: the reference appends
//!   `" "` until the field is 31 characters wide (:321-326), then writes one
//!   byte per character. `Writer::write_str31` pads with NUL, so this encoder
//!   writes the field itself to stay byte-for-byte with the reference. (The
//!   server ignores the padding; the crate's NUL-padding is otherwise fine.)
//!
//! The 12-byte prop info is written in the **session** byte order (`ba.endian =
//! endian` at :270), not the prop's own sniffed order, so the encoder writes it
//! through the session [`Writer`] rather than reusing a little-endian prop blob.

use crate::byteorder::{ByteOrder, Writer};
use crate::error::{Result, WireError};
use crate::frame::Frame;
use crate::opcode;

/// `'Prop'` — `PalaceAsset.ASSET_TYPE_PROP`, the only type the reference uploads
/// this way.
const ASSET_TYPE_PROP: u32 = 0x5072_6f70;

/// `PROP_FORMAT_S20BIT` — the pixel format `encodeS20BitProp` always emits.
const PROP_FORMAT_S20BIT: u16 = 0x0200;

/// Width and height of a wearable prop, in pixels.
const WEARABLE_DIMENSION: i16 = 44;

/// The reference allows a standard prop's origin to sit a little outside the
/// 44x44 box, for props made by old PalaceChat and the classic Mac client.
const MIN_WEARABLE_OFFSET: i16 = -44;
/// Upper bound of the offset range (see [`MIN_WEARABLE_OFFSET`]).
const MAX_WEARABLE_OFFSET: i16 = 88;

/// Bytes before the prop info: the 24-byte block header plus the 40-byte
/// descriptor.
const ASSET_HEADER_LEN: usize = 64;

/// Width of the 12-byte prop info that opens the data section.
const PROP_INFO_LEN: usize = 12;

/// The body of an `ASSET_REGI` (`rAst`) prop upload.
///
/// This is `PalaceProp.as::assetData()`: the asset registration the client sends
/// in answer to a server `qAst` so it can wear the prop. See the module docs for
/// the exact byte layout.
///
/// `crc` and `image_data` come from `palace-prop`: encode the decoded prop with
/// `palace_prop::encode_s20_payload(&prop.image)`, then pass
/// `palace_prop::asset_crc(&image_data)` as `crc`. The pixel encoder and the CRC
/// live in `palace-prop`; this crate only frames them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropUpload<'a> {
    /// The asset id this prop is registered under (`asset.id`).
    pub id: i32,
    /// CRC of `image_data` — `computeCRC(imageData)`, i.e.
    /// `palace_prop::asset_crc(image_data)`.
    pub crc: u32,
    /// The asset name, written into the 32-byte `Str31` descriptor field.
    pub name: &'a str,
    /// The prop's decoded width, in pixels.
    pub width: i16,
    /// The prop's decoded height, in pixels.
    pub height: i16,
    /// The prop's horizontal origin offset.
    pub h_offset: i16,
    /// The prop's vertical origin offset.
    pub v_offset: i16,
    /// Head/ghost/rare/animate/bounce bits. The S20 format bit is OR-ed in.
    pub flags: u16,
    /// The zlib-compressed S20 pixel payload (`encodeS20BitProp()` output).
    pub image_data: &'a [u8],
}

impl PropUpload<'_> {
    /// True when the prop passes the reference's `sendPropToServer` guard.
    ///
    /// The guard is anti-abuse, not protocol: a "web service big prop" — a prop
    /// larger than the 44x44 avatar cell, as sold through the old web-service
    /// prop store — is refused rather than uploaded, because the server and
    /// other clients cannot render it. Width and height must be exactly 44; the
    /// offsets may sit in `-44..=88` so that props built by older PalaceChat and
    /// the classic Mac client still upload.
    pub fn is_wearable(&self) -> bool {
        self.width == WEARABLE_DIMENSION
            && self.height == WEARABLE_DIMENSION
            && (MIN_WEARABLE_OFFSET..=MAX_WEARABLE_OFFSET).contains(&self.h_offset)
            && (MIN_WEARABLE_OFFSET..=MAX_WEARABLE_OFFSET).contains(&self.v_offset)
    }

    /// Encode an `rAst` body, rejecting a prop the reference would refuse.
    pub fn encode(&self, w: &mut Writer) -> Result<()> {
        if !self.is_wearable() {
            return Err(WireError::UnwearableProp {
                width: self.width,
                height: self.height,
                h_offset: self.h_offset,
                v_offset: self.v_offset,
            });
        }
        let size = self.image_data.len() + PROP_INFO_LEN;
        let size = u32::try_from(size).map_err(|_| WireError::PayloadTooLarge {
            length: self.image_data.len(),
        })?;

        w.write_u32(ASSET_TYPE_PROP);
        w.write_i32(self.id);
        w.write_u32(self.crc);
        w.write_i32(size as i32);
        w.write_i32(0);
        w.write_i16(0);
        w.write_i16(1);
        w.write_u32(0);
        w.write_u32(size);
        write_asset_name(w, self.name);
        w.write_i16(WEARABLE_DIMENSION);
        w.write_i16(WEARABLE_DIMENSION);
        w.write_i16(self.h_offset);
        w.write_i16(self.v_offset);
        w.write_i16(0);
        w.write_u16(self.flags | PROP_FORMAT_S20BIT);
        w.write_bytes(self.image_data);
        Ok(())
    }

    /// The encoded `rAst` body as a standalone buffer.
    pub fn encode_to_vec(&self, order: ByteOrder) -> Result<Vec<u8>> {
        let mut w = Writer::with_capacity(
            order,
            ASSET_HEADER_LEN + PROP_INFO_LEN + self.image_data.len(),
        );
        self.encode(&mut w)?;
        Ok(w.into_vec())
    }

    /// Build a complete `MSG_ASSETREGI` frame. `refNum` is always `0`, as in the
    /// reference (`sendPropToServer` writes `socket.writeInt(0)` after the size).
    pub fn frame(&self, order: ByteOrder) -> Result<Frame> {
        Ok(Frame::new(opcode::ASSETREGI, 0, self.encode_to_vec(order)?))
    }
}

/// Write the 32-byte `Str31` asset name exactly as `assetData` does: a length
/// byte and 31 bytes of Latin-1, space padded. Names beyond 31 bytes are
/// truncated to the field; the reference would write past the descriptor.
fn write_asset_name(w: &mut Writer, name: &str) {
    let bytes = name.as_bytes();
    let n = bytes.len().min(31);
    w.write_u8(n as u8);
    w.write_bytes(&bytes[..n]);
    for _ in n..31 {
        w.write_u8(b' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::Reader;

    fn upload<'a>(image_data: &'a [u8]) -> PropUpload<'a> {
        PropUpload {
            id: 77,
            crc: 0x1234_5678,
            name: "Prop",
            width: 44,
            height: 44,
            h_offset: 0,
            v_offset: 0,
            flags: 0,
            image_data,
        }
    }

    #[test]
    fn asset_regi_frame_matches_hand_built_reference_bytes() {
        // Expected bytes are typed out from PalaceProp.as::assetData (see the
        // module layout table), not produced by the encoder under test.
        let bytes = upload(&[0xde, 0xad, 0xbe, 0xef])
            .frame(ByteOrder::Little)
            .unwrap()
            .encode(ByteOrder::Little)
            .unwrap();
        let mut expected: Vec<u8> = vec![
            0x74, 0x73, 0x41, 0x72, // "rAst" little-endian wire spelling
            0x50, 0x00, 0x00, 0x00, // length = 76 header + 4 imageData
            0x00, 0x00, 0x00, 0x00, // refNum = 0
            0x70, 0x6f, 0x72, 0x50, // AssetType 'Prop'
            0x4d, 0x00, 0x00, 0x00, // id = 77
            0x78, 0x56, 0x34, 0x12, // crc = 0x12345678
            0x10, 0x00, 0x00, 0x00, // blockSize = 4 imageData + 12
            0x00, 0x00, 0x00, 0x00, // blockOffset = 0
            0x00, 0x00, // blockNbr = 0
            0x01, 0x00, // nbrBlocks = 1
            0x00, 0x00, 0x00, 0x00, // descriptor flags = 0
            0x10, 0x00, 0x00, 0x00, // descriptor size = blockSize
            0x04, // name length = 4
            0x50, 0x72, 0x6f, 0x70, // "Prop"
        ];
        expected.extend(std::iter::repeat_n(0x20u8, 27)); // space padded to 31
        expected.extend_from_slice(&[
            0x2c, 0x00, // width = 44
            0x2c, 0x00, // height = 44
            0x00, 0x00, // h offset = 0
            0x00, 0x00, // v offset = 0
            0x00, 0x00, // script offset = 0
            0x00, 0x02, // flags | PROP_FORMAT_S20BIT
            0xde, 0xad, 0xbe, 0xef, // imageData
        ]);
        assert_eq!(bytes, expected);
        assert_eq!(bytes.len(), 12 + 80);
    }

    #[test]
    fn asset_regi_is_big_endian_aware() {
        let bytes = upload(&[0xde, 0xad, 0xbe, 0xef])
            .frame(ByteOrder::Big)
            .unwrap()
            .encode(ByteOrder::Big)
            .unwrap();
        assert_eq!(&bytes[..4], b"rAst");
        assert_eq!(&bytes[4..8], &80u32.to_be_bytes());
        assert_eq!(&bytes[12..16], &[0x50, 0x72, 0x6f, 0x70]);
        assert_eq!(&bytes[16..20], &77i32.to_be_bytes());
        assert_eq!(&bytes[76..78], &44i16.to_be_bytes());
        assert_eq!(&bytes[86..88], &0x0200u16.to_be_bytes());
        assert_eq!(&bytes[88..92], &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn asset_regi_round_trips_through_the_asset_transfer_layout() {
        // The body is a standard sAst/rAst block: header + descriptor + data.
        let body = upload(&[0xde, 0xad, 0xbe, 0xef])
            .encode_to_vec(ByteOrder::Little)
            .unwrap();
        let mut r = Reader::new(&body, ByteOrder::Little);
        assert_eq!(r.read_u32().unwrap(), 0x5072_6f70);
        assert_eq!(r.read_i32().unwrap(), 77);
        assert_eq!(r.read_u32().unwrap(), 0x1234_5678);
        assert_eq!(r.read_i32().unwrap(), 16);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i16().unwrap(), 0);
        assert_eq!(r.read_i16().unwrap(), 1);
        assert_eq!(r.read_u32().unwrap(), 0);
        assert_eq!(r.read_u32().unwrap(), 16);
        assert_eq!(r.read_str31().unwrap(), "Prop");
        assert_eq!(r.read_i16().unwrap(), 44);
        assert_eq!(r.read_i16().unwrap(), 44);
        assert_eq!(r.read_i16().unwrap(), 0);
        assert_eq!(r.read_i16().unwrap(), 0);
        assert_eq!(r.read_i16().unwrap(), 0);
        assert_eq!(r.read_i16().unwrap(), 0x0200);
        assert_eq!(r.read_bytes(4).unwrap(), &[0xde, 0xad, 0xbe, 0xef]);
        assert!(r.is_empty());
    }

    #[test]
    fn asset_regi_rejects_a_prop_that_is_not_44x44() {
        for (width, height) in [(45i16, 44i16), (44, 45), (88, 88), (0, 0)] {
            let mut prop = upload(&[]);
            prop.width = width;
            prop.height = height;
            let mut w = Writer::new(ByteOrder::Little);
            assert!(
                matches!(prop.encode(&mut w), Err(WireError::UnwearableProp { .. })),
                "{width}x{height} is a web service big prop and must be refused"
            );
            assert!(w.is_empty(), "nothing may be written for a rejected prop");
        }
    }

    #[test]
    fn asset_regi_rejects_offsets_outside_minus_44_to_88() {
        for (h_offset, v_offset) in [(89i16, 0i16), (-45, 0), (0, 89), (0, -45), (89, 89)] {
            let mut prop = upload(&[]);
            prop.h_offset = h_offset;
            prop.v_offset = v_offset;
            let mut w = Writer::new(ByteOrder::Little);
            assert!(
                matches!(
                    prop.encode(&mut w),
                    Err(WireError::UnwearableProp {
                        h_offset: got_h,
                        v_offset: got_v,
                        ..
                    }) if got_h == h_offset && got_v == v_offset
                ),
                "offsets h={h_offset}, v={v_offset} must be refused"
            );
            assert!(w.is_empty());
        }
    }

    #[test]
    fn asset_regi_accepts_the_offset_boundaries() {
        // The guard is `> 88 || < -44`, so the endpoints are inside.
        for (h_offset, v_offset) in [(88i16, 88i16), (-44, -44), (-44, 88), (88, -44)] {
            let mut prop = upload(&[]);
            prop.h_offset = h_offset;
            prop.v_offset = v_offset;
            assert!(prop.is_wearable());
            assert!(prop.encode_to_vec(ByteOrder::Little).is_ok());
        }
    }

    #[test]
    fn asset_regi_carries_the_prop_flags_and_the_format_bit() {
        let mut prop = upload(&[]);
        prop.flags = 0x0002 | 0x0004; // head + ghost
        let body = prop.encode_to_vec(ByteOrder::Little).unwrap();
        assert_eq!(
            &body[74..76],
            &[0x06, 0x02],
            "flags | S20BIT, little-endian"
        );
    }

    #[test]
    fn asset_regi_scales_the_size_fields_with_the_image_data() {
        let image = vec![0u8; 100];
        let body = upload(&image).encode_to_vec(ByteOrder::Little).unwrap();
        assert_eq!(body.len(), 76 + 100);
        assert_eq!(&body[12..16], &112i32.to_le_bytes(), "blockSize");
        assert_eq!(&body[28..32], &112u32.to_le_bytes(), "descriptor size");
    }

    #[test]
    fn asset_regi_truncates_a_name_that_would_overflow_the_field() {
        let long = "A".repeat(40);
        let mut prop = upload(&[]);
        prop.name = &long;
        let body = prop.encode_to_vec(ByteOrder::Little).unwrap();
        assert_eq!(body[32], 31);
        assert_eq!(&body[33..64], &[b'A'; 31]);
        assert_eq!(body.len(), 76, "the descriptor stays 32 bytes wide");
    }
}

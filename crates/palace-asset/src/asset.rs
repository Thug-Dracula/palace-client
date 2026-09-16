//! The asset types, and the `sAst` / `rAst` transfer record.
//!
//! `MSG_ASSETSEND` (`sAst`, server → client) and `MSG_ASSETREGI` (`rAst`,
//! client → server) carry the **same** body — the protocol reference says so in
//! as many words ("These two messages are identical except for the message
//! type"). They are one codec here, parameterised only by the opcode at encode
//! time.
//!
//! ## Body layout
//!
//! ```text
//! AssetType   type            uint32   'Prop' / 'User' / 'IUsr'
//! AssetSpec   spec.id         sint32
//!             spec.crc        uint32   0 means "don't care"
//!             blockSize       sint32   bytes of `data` in this message
//!             blockOffset     sint32   where this block starts in the asset
//!             blockNbr        sint16   0-based
//!             nbrBlocks       sint16   1-based
//! [only when blockNbr == 0]
//! AssetDescriptor
//!             flags           uint32
//!             size            uint32   total asset size, bytes
//!             name            Str31    32 bytes: length byte + 31 name bytes
//!             data            uint8[blockSize]
//!             padding         uint8[…]  remainder of the frame payload
//! ```
//!
//! The `AssetDescriptor` is **conditional**. That is the single most
//! error-prone detail in the message, and the reason the frame is longer by 40
//! bytes on block 0 than on every other block.
//!
//! ## The padding term, verified against captures
//!
//! OpenPalace computes `padding = size - (blockSize + 64)` unconditionally,
//! using 64 for *every* block. 64 is right only for block 0 (24-byte block
//! header + 40-byte descriptor). This crate computes the header length from
//! `blockNbr` and rejects a negative remainder instead of silently skipping
//! nothing.
//!
//! That `size - (blockSize + 64)` term had to be checked against real traffic
//! rather than believed. It was, two ways:
//!
//! * `$CORPUS/tools/palace_walker.py::_store_asset` — the known-good
//!   Python parser that produced 41,478 prop files from live servers — reads
//!   the descriptor at offset 24 and the data at offset 64 for block 0, and at
//!   24 for any other block.
//! * 327 prop blobs in `$CORPUS/props_from_capture/` were extracted from
//!   real `sAst` frames, and for every one of them the TCP payload length of
//!   the packet that carried it was exactly `64 + blob_size` — so the real
//!   padding term is **0**.
//!
//! `fixtures/captured/` holds five of those blobs plus the exact observed
//! packet length for each; `tests/captured_sast.rs` asserts the identity.

use palace_wire::byteorder::{ByteOrder, Reader, Writer};
use palace_wire::frame::Frame;
use palace_wire::opcode::{Opcode, ASSETREGI, ASSETSEND};

use crate::error::{
    AssetError, Result, BLOCK_HEADER_LEN, DESCRIPTOR_LEN, MAX_ASSET_SIZE, MAX_BLOCK_COUNT,
    MAX_BLOCK_SIZE,
};

/// A four-character asset type code, e.g. `'Prop'`.
///
/// Unknown codes are preserved rather than rejected: the protocol reference
/// notes the set is not closed, and "log and skip" beats "abort the session".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetType(pub u32);

impl AssetType {
    /// `'Prop'` — the only type the server ever asks a client for.
    pub const PROP: AssetType = AssetType(0x5072_6f70);
    /// `'User'` — a user database record (vestigial).
    pub const USERBASE: AssetType = AssetType(0x5573_6572);
    /// `'IUsr'` — defined but unused; a historical artifact.
    pub const IPUSERBASE: AssetType = AssetType(0x4955_7372);

    /// Wrap a raw 32-bit type code.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        AssetType(raw)
    }

    /// The raw type code.
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }

    /// The protocol name, or `None` for a code outside the documented set.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        match self {
            AssetType::PROP => Some("Prop"),
            AssetType::USERBASE => Some("User"),
            AssetType::IPUSERBASE => Some("IUsr"),
            _ => None,
        }
    }

    /// The type in a form safe to log.
    #[must_use]
    pub fn describe(self) -> String {
        match self.name() {
            Some(name) => format!("{name}({:#010x})", self.0),
            None => format!("{:#010x}", self.0),
        }
    }

    /// True for asset types whose CRC rule this crate knows.
    #[must_use]
    pub fn crc_is_verifiable(self) -> bool {
        self == AssetType::PROP
    }
}

impl std::fmt::Display for AssetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#010x}", self.0),
        }
    }
}

/// What identifies the asset a client wants: an id and a CRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetSpec {
    /// Arbitrary 4-byte asset id.
    pub id: i32,
    /// Expected CRC. **0 means "don't care"** (protocol reference).
    pub crc: u32,
}

impl AssetSpec {
    /// A spec that accepts any CRC the server has.
    #[must_use]
    pub const fn unchecked(id: i32) -> Self {
        AssetSpec { id, crc: 0 }
    }

    /// A spec pinned to a particular CRC.
    #[must_use]
    pub const fn with_crc(id: i32, crc: u32) -> Self {
        AssetSpec { id, crc }
    }

    /// True when this spec asks the sender not to check the CRC.
    #[must_use]
    pub const fn accepts_any_crc(&self) -> bool {
        self.crc == 0
    }
}

/// The cache key: asset **type**, **id** and **crc** together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetKey {
    /// Asset type code.
    pub asset_type: AssetType,
    /// Asset id.
    pub id: i32,
    /// Asset CRC.
    pub crc: u32,
}

impl AssetKey {
    /// Build a key.
    #[must_use]
    pub const fn new(asset_type: AssetType, id: i32, crc: u32) -> Self {
        AssetKey {
            asset_type,
            id,
            crc,
        }
    }

    /// Build a key from a query spec.
    #[must_use]
    pub const fn from_spec(asset_type: AssetType, spec: AssetSpec) -> Self {
        AssetKey::new(asset_type, spec.id, spec.crc)
    }

    /// The spec to put in a `qAst` request for this key.
    #[must_use]
    pub const fn spec(&self) -> AssetSpec {
        AssetSpec {
            id: self.id,
            crc: self.crc,
        }
    }
}

impl std::fmt::Display for AssetKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {:#010x})", self.asset_type, self.id, self.crc)
    }
}

/// Whole-asset metadata, present only on the first block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDescriptor {
    /// Asset flag bits. Client-side only; the protocol does not define them here.
    pub flags: u32,
    /// Total size of the asset in bytes.
    pub size: u32,
    /// Asset name (`Str31`, Latin-1). Client-side only; scripts use it.
    pub name: String,
}

/// The fixed part of an `sAst`/`rAst`, before the conditional descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    /// Asset type code.
    pub asset_type: AssetType,
    /// Asset id / CRC of the asset being moved.
    pub spec: AssetSpec,
    /// Bytes of data in this message.
    pub block_size: i32,
    /// Offset of this block from the start of the asset.
    pub block_offset: i32,
    /// 0-based block index.
    pub block_number: i16,
    /// Total number of blocks.
    pub block_count: i16,
}

/// A decoded `sAst` or `rAst` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetTransfer {
    /// The block header.
    pub header: BlockHeader,
    /// Present exactly when `header.block_number == 0`.
    pub descriptor: Option<AssetDescriptor>,
    /// The block's bytes.
    pub data: Vec<u8>,
    /// Trailing bytes after `data` that the frame carried and the layout does
    /// not explain. Zero in every real capture we have.
    pub padding: usize,
}

impl AssetTransfer {
    /// The cache key of the asset this transfer belongs to.
    #[must_use]
    pub fn key(&self) -> AssetKey {
        AssetKey::from_spec(self.header.asset_type, self.header.spec)
    }

    /// True when this is the first block of an asset.
    #[must_use]
    pub fn is_first_block(&self) -> bool {
        self.header.block_number == 0
    }

    /// Decode an `sAst`/`rAst` body.
    ///
    /// Every size is checked against the bytes that actually arrived before
    /// anything is copied, so a claimed `blockSize` cannot drive an allocation.
    pub fn decode(payload: &[u8], order: ByteOrder) -> Result<AssetTransfer> {
        let available = payload.len();
        let mut r = Reader::new(payload, order);

        let asset_type = AssetType::from_raw(r.read_u32()?);
        let id = r.read_i32()?;
        let crc = r.read_u32()?;
        let block_size = r.read_i32()?;
        let block_offset = r.read_i32()?;
        let block_number = r.read_i16()?;
        let block_count = r.read_i16()?;

        if block_size < 0 || block_size as u64 > MAX_BLOCK_SIZE {
            return Err(AssetError::ImplausibleBlockSize {
                block_size: i64::from(block_size),
                max: MAX_BLOCK_SIZE,
            });
        }
        if block_count <= 0 || block_count as u32 > MAX_BLOCK_COUNT {
            return Err(AssetError::ImplausibleBlockCount {
                block_count: i64::from(block_count),
                max: MAX_BLOCK_COUNT,
            });
        }
        if block_number < 0 || block_number >= block_count {
            return Err(AssetError::ImplausibleBlockNumber {
                block_number: i64::from(block_number),
                block_count: i64::from(block_count),
            });
        }
        if block_offset < 0 || (block_number == 0 && block_offset != 0) {
            return Err(AssetError::ImplausibleBlockOffset {
                block_number: i64::from(block_number),
                block_offset: i64::from(block_offset),
            });
        }

        let descriptor = if block_number == 0 {
            let flags = r.read_u32()?;
            let size = r.read_u32()?;
            if u64::from(size) > MAX_ASSET_SIZE {
                return Err(AssetError::ImplausibleAssetSize {
                    asset_size: u64::from(size),
                    max: MAX_ASSET_SIZE,
                });
            }
            let name = r.read_str31()?;
            Some(AssetDescriptor { flags, size, name })
        } else {
            None
        };

        let header_len = if descriptor.is_some() {
            BLOCK_HEADER_LEN + DESCRIPTOR_LEN
        } else {
            BLOCK_HEADER_LEN
        };
        let block_size = block_size as usize;
        let payload_used = header_len + block_size;
        if available < payload_used {
            return Err(AssetError::Truncated {
                context: "sAst/rAst payload",
                needed: payload_used,
                available,
            });
        }

        let data = r.read_bytes(block_size)?.to_vec();
        let padding = available - payload_used;

        Ok(AssetTransfer {
            header: BlockHeader {
                asset_type,
                spec: AssetSpec { id, crc },
                block_size: block_size as i32,
                block_offset,
                block_number,
                block_count,
            },
            descriptor,
            data,
            padding,
        })
    }

    /// Encode an `sAst` body.
    ///
    /// `descriptor` must be `Some` when this is block 0 and `None` otherwise;
    /// passing the wrong shape is a caller bug, reported as
    /// [`AssetError::Rejected`]-style truncation rather than written to the wire.
    pub fn encode_body(&self, w: &mut Writer) -> Result<()> {
        if self.descriptor.is_some() != self.is_first_block() {
            return Err(AssetError::BadShape(
                "the descriptor is present exactly when blockNumber == 0",
            ));
        }
        if self.data.len() > MAX_BLOCK_SIZE as usize {
            return Err(AssetError::ImplausibleBlockSize {
                block_size: self.data.len() as i64,
                max: MAX_BLOCK_SIZE,
            });
        }

        w.write_u32(self.header.asset_type.as_raw());
        w.write_i32(self.header.spec.id);
        w.write_u32(self.header.spec.crc);
        w.write_i32(self.data.len() as i32);
        w.write_i32(self.header.block_offset);
        w.write_i16(self.header.block_number);
        w.write_i16(self.header.block_count);
        if let Some(desc) = &self.descriptor {
            w.write_u32(desc.flags);
            w.write_u32(desc.size);
            w.write_str31(&desc.name);
        }
        w.write_bytes(&self.data);
        Ok(())
    }

    /// Encode this transfer into a frame with the given opcode.
    ///
    /// Use [`ASSETSEND`] to answer a `qAst` the server sent us as a client
    /// (peer propagation) and [`ASSETREGI`] when serving a prop we hold.
    pub fn encode_frame(&self, opcode: Opcode, ref_num: i32, order: ByteOrder) -> Result<Frame> {
        let mut w =
            Writer::with_capacity(order, BLOCK_HEADER_LEN + DESCRIPTOR_LEN + self.data.len());
        self.encode_body(&mut w)?;
        Ok(Frame::new(opcode, ref_num, w.into_vec()))
    }

    /// Build a single-block `rAst` transfer serving `blob` for `spec`.
    ///
    /// This is the peer-propagation path: the server asks a client for a prop it
    /// does not have, and the client that holds it answers with `rAst`.
    #[must_use]
    pub fn single_block(asset_type: AssetType, spec: AssetSpec, name: &str, blob: &[u8]) -> Self {
        AssetTransfer {
            header: BlockHeader {
                asset_type,
                spec,
                block_size: blob.len().min(i32::MAX as usize) as i32,
                block_offset: 0,
                block_number: 0,
                block_count: 1,
            },
            descriptor: Some(AssetDescriptor {
                flags: 0,
                size: blob.len().min(u32::MAX as usize) as u32,
                name: name.to_string(),
            }),
            data: blob.to_vec(),
            padding: 0,
        }
    }
}

/// The `sAst` opcode, re-exported so callers do not have to reach into
/// `palace-wire`.
pub const OP_SEND: Opcode = ASSETSEND;
/// The `rAst` opcode.
pub const OP_REGI: Opcode = ASSETREGI;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{BLOCK_HEADER_LEN, DESCRIPTOR_LEN};

    fn transfer(block_number: i16, block_count: i16, data: &[u8]) -> AssetTransfer {
        AssetTransfer {
            header: BlockHeader {
                asset_type: AssetType::PROP,
                spec: AssetSpec::with_crc(42, 0xabcd_ef01),
                block_size: data.len() as i32,
                block_offset: 0,
                block_number,
                block_count,
            },
            descriptor: (block_number == 0).then(|| AssetDescriptor {
                flags: 7,
                size: data.len() as u32,
                name: "hello".into(),
            }),
            data: data.to_vec(),
            padding: 0,
        }
    }

    #[test]
    fn round_trips_block_zero_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let original = transfer(0, 1, b"payload");
            let frame = original.encode_frame(OP_SEND, 0, order).unwrap();
            assert_eq!(
                frame.payload.len(),
                BLOCK_HEADER_LEN + DESCRIPTOR_LEN + 7,
                "block 0 carries the 40-byte descriptor"
            );
            let back = AssetTransfer::decode(&frame.payload, order).unwrap();
            assert_eq!(back, original);
            assert_eq!(back.padding, 0);
        }
    }

    #[test]
    fn round_trips_a_later_block_without_a_descriptor() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let original = transfer(1, 2, b"second");
            let frame = original.encode_frame(OP_REGI, 0, order).unwrap();
            assert_eq!(frame.payload.len(), BLOCK_HEADER_LEN + 6);
            let back = AssetTransfer::decode(&frame.payload, order).unwrap();
            assert_eq!(back, original);
            assert!(back.descriptor.is_none());
        }
    }

    #[test]
    fn trailing_padding_is_reported_not_swallowed() {
        let original = transfer(0, 1, b"xy");
        let mut payload = original
            .encode_frame(OP_SEND, 0, ByteOrder::Little)
            .unwrap()
            .payload;
        payload.extend_from_slice(&[0, 0, 0]);
        let decoded = AssetTransfer::decode(&payload, ByteOrder::Little).unwrap();
        assert_eq!(decoded.data, b"xy");
        assert_eq!(decoded.padding, 3);
    }

    #[test]
    fn a_negative_block_size_is_rejected_without_allocating() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_u32(AssetType::PROP.as_raw());
        w.write_i32(1);
        w.write_u32(0);
        w.write_i32(-1);
        w.write_i32(0);
        w.write_i16(0);
        w.write_i16(1);
        let err = AssetTransfer::decode(w.as_slice(), ByteOrder::Little).unwrap_err();
        assert!(matches!(
            err,
            AssetError::ImplausibleBlockSize { block_size: -1, .. }
        ));
    }

    #[test]
    fn a_huge_block_size_is_rejected_before_the_data_read() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_u32(AssetType::PROP.as_raw());
        w.write_i32(1);
        w.write_u32(0);
        w.write_i32(i32::MAX);
        w.write_i32(0);
        w.write_i16(1);
        w.write_i16(2);
        let err = AssetTransfer::decode(w.as_slice(), ByteOrder::Little).unwrap_err();
        assert!(matches!(err, AssetError::ImplausibleBlockSize { .. }));
    }

    #[test]
    fn a_truncated_block_is_reported_rather_than_padded() {
        let original = transfer(0, 1, b"0123456789");
        let mut payload = original
            .encode_frame(OP_SEND, 0, ByteOrder::Little)
            .unwrap()
            .payload;
        payload.truncate(payload.len() - 4);
        let err = AssetTransfer::decode(&payload, ByteOrder::Little).unwrap_err();
        assert!(matches!(err, AssetError::Truncated { .. }));
    }

    #[test]
    fn a_block_number_outside_the_count_is_rejected() {
        let original = transfer(3, 2, b"x");
        let payload = original
            .encode_frame(OP_SEND, 0, ByteOrder::Little)
            .unwrap()
            .payload;
        let err = AssetTransfer::decode(&payload, ByteOrder::Little).unwrap_err();
        assert!(matches!(err, AssetError::ImplausibleBlockNumber { .. }));
    }

    #[test]
    fn an_absurd_block_count_is_rejected() {
        let original = transfer(0, i16::MAX, b"x");
        let payload = original
            .encode_frame(OP_SEND, 0, ByteOrder::Little)
            .unwrap()
            .payload;
        let err = AssetTransfer::decode(&payload, ByteOrder::Little).unwrap_err();
        assert!(matches!(err, AssetError::ImplausibleBlockCount { .. }));
    }

    #[test]
    fn encoding_a_mismatched_descriptor_shape_is_refused() {
        let mut t = transfer(0, 1, b"x");
        t.descriptor = None;
        let mut w = Writer::new(ByteOrder::Little);
        assert!(t.encode_body(&mut w).is_err());
    }

    #[test]
    fn single_block_builder_matches_the_wire_shape() {
        let t = AssetTransfer::single_block(
            AssetType::PROP,
            AssetSpec::with_crc(9, 0x1234),
            "prop",
            b"abcdef",
        );
        let frame = t.encode_frame(OP_REGI, 0, ByteOrder::Little).unwrap();
        assert_eq!(frame.payload.len(), BLOCK_HEADER_LEN + DESCRIPTOR_LEN + 6);
        let back = AssetTransfer::decode(&frame.payload, ByteOrder::Little).unwrap();
        assert_eq!(back.descriptor.unwrap().name, "prop");
        assert_eq!(back.header.block_count, 1);
    }

    #[test]
    fn asset_type_names_and_descriptions() {
        assert_eq!(AssetType::PROP.name(), Some("Prop"));
        assert_eq!(AssetType::from_raw(0x5072_6f70), AssetType::PROP);
        assert_eq!(AssetType(0x1122_3344).name(), None);
        assert!(AssetType(0x1122_3344).describe().contains("0x11223344"));
        assert!(AssetType::PROP.crc_is_verifiable());
        assert!(!AssetType::USERBASE.crc_is_verifiable());
    }

    #[test]
    fn keys_carry_type_id_and_crc() {
        let key = AssetKey::from_spec(AssetType::PROP, AssetSpec::with_crc(5, 6));
        assert_eq!(key.asset_type, AssetType::PROP);
        assert_eq!(key.id, 5);
        assert_eq!(key.crc, 6);
        assert_eq!(key.spec(), AssetSpec::with_crc(5, 6));
        assert!(AssetSpec::unchecked(5).accepts_any_crc());
        assert!(key.to_string().contains("0x00000006"));
    }
}

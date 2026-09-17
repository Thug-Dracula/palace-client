//! Loose-prop messages: `MSG_PROPNEW`, `MSG_PROPMOVE`, `MSG_PROPDEL`.
//!
//! These three are the room's floor props. `PROPNEW` carries a specification but
//! no identity; the server appends it, so its address is the position it lands
//! at, and `PROPMOVE`/`PROPDEL` name that 0-based position (:1454, :1469). The
//! frame `refNum` is unused and is `0` in every captured frame.

use super::user::{AssetSpec, Point};
use crate::byteorder::Reader;
use crate::error::Result;

/// `MSG_PROPNEW` (`nPrp`): an [`AssetSpec`] plus the new prop's [`Point`],
/// 12 bytes. Protocol reference §3.24 (:1479-1488). The prop's flags and
/// `refCon` are not on the wire; the server assigns `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropNew {
    /// The prop asset.
    pub spec: AssetSpec,
    /// Initial screen position.
    pub position: Point,
}

impl PropNew {
    /// Decode an `nPrp` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(PropNew {
            spec: AssetSpec::decode(r)?,
            position: Point::decode(r)?,
        })
    }
}

/// `MSG_PROPMOVE` (`mPrp`): a `sint32` prop index plus the prop's absolute new
/// [`Point`], 8 bytes. Protocol reference §3.25 (:1464-1471).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropMove {
    /// 0-based index into the room's loose props, in the order they were added.
    pub prop_num: i32,
    /// The prop's new position, absolute rather than a delta.
    pub position: Point,
}

impl PropMove {
    /// Decode an `mPrp` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(PropMove {
            prop_num: r.read_i32()?,
            position: Point::decode(r)?,
        })
    }
}

/// `MSG_PROPDEL` (`dPrp`): the `sint32` index of the prop to delete, 4 bytes.
/// Protocol reference §3.23 (:1450-1454).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropDel {
    /// 0-based index into the room's loose props, in the order they were added,
    /// or `-1` to delete every prop in the room.
    pub prop_num: i32,
}

impl PropDel {
    /// Decode a `dPrp` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(PropDel {
            prop_num: r.read_i32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::{ByteOrder, Writer};

    fn prop(id: i32, crc: u32) -> AssetSpec {
        AssetSpec { id, crc }
    }

    #[test]
    fn prop_new_is_eight_bytes_of_spec_plus_four_of_point() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            prop(0x1122_3344, 0xdead_beef).encode(&mut w);
            Point::new(162, 367).encode(&mut w);
            let bytes = w.into_vec();
            assert_eq!(bytes.len(), 12);
            let mut r = Reader::new(&bytes, order);
            let msg = PropNew::decode(&mut r).unwrap();
            assert!(r.is_empty());
            assert_eq!(msg.spec, prop(0x1122_3344, 0xdead_beef));
            assert_eq!(msg.position, Point::new(162, 367), "the point is (v, h)");
        }
    }

    #[test]
    fn prop_move_carries_index_then_absolute_point() {
        let mut w = Writer::new(ByteOrder::Little);
        w.write_i32(3);
        Point::new(200, 100).encode(&mut w);
        let bytes = w.into_vec();
        assert_eq!(bytes.len(), 8);
        let mut r = Reader::new(&bytes, ByteOrder::Little);
        let msg = PropMove::decode(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(msg.prop_num, 3);
        assert_eq!(msg.position, Point::new(200, 100));
    }

    #[test]
    fn prop_del_reads_a_signed_index() {
        for index in [0i32, 7, -1] {
            let mut w = Writer::new(ByteOrder::Little);
            w.write_i32(index);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, ByteOrder::Little);
            assert_eq!(PropDel::decode(&mut r).unwrap().prop_num, index);
        }
    }

    #[test]
    fn truncated_prop_messages_are_errors() {
        let mut r = Reader::new(&[0u8; 3], ByteOrder::Little);
        assert!(PropDel::decode(&mut r).is_err());
        let mut r = Reader::new(&[0u8; 7], ByteOrder::Little);
        assert!(PropMove::decode(&mut r).is_err());
        let mut r = Reader::new(&[0u8; 11], ByteOrder::Little);
        assert!(PropNew::decode(&mut r).is_err());
    }
}

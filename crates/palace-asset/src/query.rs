//! `MSG_ASSETQUERY` (`qAst`) — asking for an asset.
//!
//! ## Body layout
//!
//! ```text
//! AssetType   type    uint32   'Prop' / 'User' / 'IUsr'
//! AssetSpec   id      sint32
//!             crc     uint32   0 means "don't care"
//! ```
//!
//! Twelve bytes, and the frame's `refNum` is unused (0).
//!
//! ## One frame per request, 20 frames per flush
//!
//! The reference client's comment says assets are "requested in packets of up
//! to 20 requests". That sentence is easy to misread, and OpenPalace's own
//! source invites the misreading: the frame header is written *inside* the
//! per-request loop, so a "packet of 20 requests" is **20 complete 24-byte
//! frames** written back-to-back and flushed together — not one frame with a
//! 240-byte body.
//!
//! The captures settle it. In the saved animanic traffic
//! (`$CORPUS/pcap_extract/animanic_pass2_dump.txt`) a client packet whose
//! TCP payload is 24 bytes contains exactly one `qAst`, a 48-byte payload
//! contains exactly two, and a 72-byte payload exactly three — 24 bytes per
//! request, never a shared header.
//!
//! So [`encode_batch`] returns a **`Vec<Frame>`**, and the pacing in
//! [`crate::scheduler`] is the pace between *flushes* of that vector.
//! [`REQUESTS_PER_BATCH`] is the cap on its length.

use palace_wire::byteorder::{ByteOrder, Reader, Writer};
use palace_wire::frame::Frame;
use palace_wire::opcode::ASSETQUERY;

use crate::asset::{AssetSpec, AssetType};
use crate::error::{AssetError, Result};

/// One `qAst` body: 4 bytes of type plus an 8-byte spec.
pub const QUERY_BODY_LEN: usize = 12;

/// Most requests the reference client puts in one flush.
///
/// This is not a tuning knob. See [`crate::scheduler`]: the batch size and the
/// inter-batch delay together are what stop the server treating the client as a
/// flooder and killing the connection.
pub const REQUESTS_PER_BATCH: usize = 20;

/// A decoded `qAst` body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetQuery {
    /// The type being asked for.
    pub asset_type: AssetType,
    /// The id and CRC being asked for.
    pub spec: AssetSpec,
}

impl AssetQuery {
    /// Build a query for a prop id, accepting any CRC.
    #[must_use]
    pub const fn prop(id: i32, crc: u32) -> Self {
        AssetQuery {
            asset_type: AssetType::PROP,
            spec: AssetSpec { id, crc },
        }
    }

    /// Decode a `qAst` body.
    ///
    /// The body must be exactly [`QUERY_BODY_LEN`] bytes: a `qAst` carries a
    /// single request, so extra bytes mean the stream is being read wrong.
    pub fn decode(payload: &[u8], order: ByteOrder) -> Result<AssetQuery> {
        if payload.len() < QUERY_BODY_LEN {
            return Err(AssetError::Truncated {
                context: "qAst payload",
                needed: QUERY_BODY_LEN,
                available: payload.len(),
            });
        }
        if payload.len() > QUERY_BODY_LEN {
            return Err(AssetError::Truncated {
                context: "qAst payload (one request per frame)",
                needed: QUERY_BODY_LEN,
                available: payload.len(),
            });
        }
        let mut r = Reader::new(payload, order);
        let asset_type = AssetType::from_raw(r.read_u32()?);
        let id = r.read_i32()?;
        let crc = r.read_u32()?;
        Ok(AssetQuery {
            asset_type,
            spec: AssetSpec { id, crc },
        })
    }

    /// Encode into a writer (no frame header).
    pub fn encode_body(&self, w: &mut Writer) {
        w.write_u32(self.asset_type.as_raw());
        w.write_i32(self.spec.id);
        w.write_u32(self.spec.crc);
    }

    /// Encode as a complete `qAst` frame. `refNum` is unused by the protocol
    /// and should be 0.
    pub fn encode_frame(&self, ref_num: i32, order: ByteOrder) -> Frame {
        let mut w = Writer::with_capacity(order, QUERY_BODY_LEN);
        self.encode_body(&mut w);
        Frame::new(ASSETQUERY, ref_num, w.into_vec())
    }
}

/// Encode up to [`REQUESTS_PER_BATCH`] requests as one flush's worth of frames.
///
/// Returns one 24-byte frame per spec, in order. Fewer than
/// [`REQUESTS_PER_BATCH`] specs is normal — it is what the tail of a queue
/// looks like.
pub fn encode_batch(specs: &[AssetSpec], ref_num: i32, order: ByteOrder) -> Result<Vec<Frame>> {
    if specs.len() > REQUESTS_PER_BATCH {
        return Err(AssetError::BadShape(
            "qAst flush carries at most 20 requests",
        ));
    }
    Ok(specs
        .iter()
        .map(|spec| AssetQuery::prop(spec.id, spec.crc).encode_frame(ref_num, order))
        .collect())
}

/// Encode typed requests as one flush's worth of frames.
pub fn encode_batch_typed(
    requests: &[AssetQuery],
    ref_num: i32,
    order: ByteOrder,
) -> Result<Vec<Frame>> {
    if requests.len() > REQUESTS_PER_BATCH {
        return Err(AssetError::BadShape(
            "qAst flush carries at most 20 requests",
        ));
    }
    Ok(requests
        .iter()
        .map(|q| q.encode_frame(ref_num, order))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_frame_is_twenty_four_bytes() {
        let frame = AssetQuery::prop(1000, 0xdead_beef).encode_frame(0, ByteOrder::Little);
        assert_eq!(frame.encoded_len(), 12 + QUERY_BODY_LEN);
        assert_eq!(frame.opcode.value(), 0x7141_7374);
        let back = AssetQuery::decode(&frame.payload, ByteOrder::Little).unwrap();
        assert_eq!(back, AssetQuery::prop(1000, 0xdead_beef));
    }

    #[test]
    fn round_trips_in_both_byte_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let q = AssetQuery {
                asset_type: AssetType::USERBASE,
                spec: AssetSpec::with_crc(-5, 7),
            };
            let frame = q.encode_frame(0, order);
            assert_eq!(AssetQuery::decode(&frame.payload, order).unwrap(), q);
        }
    }

    #[test]
    fn a_batch_is_one_frame_per_request() {
        let specs: Vec<AssetSpec> = (0..REQUESTS_PER_BATCH)
            .map(|i| AssetSpec::unchecked(i as i32))
            .collect();
        let frames = encode_batch(&specs, 0, ByteOrder::Little).unwrap();
        assert_eq!(frames.len(), REQUESTS_PER_BATCH);
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame.encoded_len(), 24, "every qAst is its own frame");
            assert_eq!(frame.payload.len(), QUERY_BODY_LEN);
            let q = AssetQuery::decode(&frame.payload, ByteOrder::Little).unwrap();
            assert_eq!(q.spec.id, i as i32);
        }
    }

    #[test]
    fn an_over_full_batch_is_refused_rather_than_silently_split() {
        let specs = vec![AssetSpec::unchecked(1); REQUESTS_PER_BATCH + 1];
        assert!(matches!(
            encode_batch(&specs, 0, ByteOrder::Big),
            Err(AssetError::BadShape(_))
        ));
    }

    #[test]
    fn a_query_body_of_the_wrong_length_is_rejected() {
        assert!(AssetQuery::decode(&[0u8; 8], ByteOrder::Little).is_err());
        assert!(AssetQuery::decode(&[0u8; 16], ByteOrder::Little).is_err());
        assert!(AssetQuery::decode(&[0u8; 12], ByteOrder::Little).is_ok());
    }

    #[test]
    fn typed_batch_encodes_the_requested_type() {
        let reqs = [AssetQuery {
            asset_type: AssetType::USERBASE,
            spec: AssetSpec::unchecked(3),
        }];
        let frames = encode_batch_typed(&reqs, 0, ByteOrder::Little).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            AssetQuery::decode(&frames[0].payload, ByteOrder::Little)
                .unwrap()
                .asset_type,
            AssetType::USERBASE
        );
    }
}

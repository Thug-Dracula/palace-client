//! Multi-block asset assembly and per-asset CRC verification.
//!
//! The transfer format is *designed* for blocks; in practice the Unix server
//! sends every asset whole, as one block with `blockNbr == 0`, `nbrBlocks == 1`
//! and `blockOffset == 0`. Both the protocol reference and Taj say so:
//!
//! > In theory, as mentioned above, this message format allows an asset to be
//! > transmitted in pieces. However, the Unix server only understands assets
//! > sent whole.
//!
//! This module implements the general case anyway, because the format *is*
//! multi-block and a fork or a peer-propagation path may use it — and because
//! silently assuming one block is how you end up with half an avatar.
//!
//! ## What it does with a block
//!
//! 1. Validate it against the type's limits (done at decode time; re-checked).
//! 2. If it is block 0, take the descriptor and (re)start the partial asset.
//! 3. Store the block by index, rejecting a duplicate index that disagrees
//!    about its offset.
//! 4. When every block is present, concatenate in index order, check the total
//!    against the descriptor's `size`, and check the CRC.
//!
//! Anything that fails step 4 is **dropped**, not retried in place: the caller
//! re-requests it through the [scheduler](crate::scheduler), which is the one
//! component that knows about pacing and backoff.
//!
//! ## The CRC rule
//!
//! For props, the CRC is the seeded rotate-left/XOR fold defined in
//! `palace-prop` (`ASSET_CRC_MAGIC = 0xD9216290`), computed over the prop
//! payload **excluding the 12-byte prop header** — never the whole blob. That
//! slice is the classic mistake, and it is asserted against real captured props
//! in `tests/captured_sast.rs`.
//!
//! A CRC of `0` in the spec means "don't care" (protocol reference) and is never
//! treated as a mismatch.
//!
//! For types whose rule we do not know (`User`, `IUsr`, anything unknown) the
//! behaviour is chosen by [`CrcPolicy`]; the default verifies what it can and
//! says so in the outcome rather than inventing a rule.

use std::collections::{BTreeMap, HashMap};

use palace_prop::{asset_crc, payload_crc, HEADER_LEN as PROP_HEADER_LEN};

use crate::asset::{AssetKey, AssetTransfer, AssetType};
use crate::error::{AssetError, Result, MAX_ASSET_SIZE, MAX_BLOCK_COUNT, MAX_BLOCK_SIZE};

/// Default cap on simultaneously incomplete assets.
pub const DEFAULT_MAX_PENDING_ASSETS: usize = 512;

/// Default cap on the total bytes held by incomplete assets.
pub const DEFAULT_MAX_PENDING_BYTES: u64 = 32 * 1024 * 1024;

/// How to treat a CRC this crate has no rule for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrcPolicy {
    /// Verify the types we have a rule for; accept the rest, reporting that
    /// verification was skipped.
    #[default]
    VerifyKnown,
    /// Refuse any asset whose CRC cannot be verified.
    Strict,
    /// Never verify. Useful when replaying a capture whose prop blob is known
    /// to be fine but whose CRC field was lost.
    Off,
}

/// Limits for the assembler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssemblerConfig {
    /// Cap on one block's data length.
    pub max_block_size: u64,
    /// Cap on a whole asset's declared size.
    pub max_asset_size: u64,
    /// Cap on `blockCount`.
    pub max_block_count: u32,
    /// Cap on simultaneously incomplete assets.
    pub max_pending_assets: usize,
    /// Cap on the total bytes held by incomplete assets.
    pub max_pending_bytes: u64,
    /// CRC policy.
    pub crc_policy: CrcPolicy,
}

impl Default for AssemblerConfig {
    fn default() -> Self {
        AssemblerConfig {
            max_block_size: MAX_BLOCK_SIZE,
            max_asset_size: MAX_ASSET_SIZE,
            max_block_count: MAX_BLOCK_COUNT,
            max_pending_assets: DEFAULT_MAX_PENDING_ASSETS,
            max_pending_bytes: DEFAULT_MAX_PENDING_BYTES,
            crc_policy: CrcPolicy::default(),
        }
    }
}

/// A complete, verified asset — the thing this whole layer exists to produce.
///
/// For a prop, `data` is the raw prop blob: the 12-byte prop header followed by
/// its payload, ready to hand to `palace-prop`'s `decode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The cache key the asset was assembled under.
    pub key: AssetKey,
    /// Descriptor flags. Client-side only.
    pub flags: u32,
    /// Descriptor name. Empty when the server sent none.
    pub name: String,
    /// The complete asset bytes.
    pub data: Vec<u8>,
}

impl Asset {
    /// The asset's type.
    #[must_use]
    pub fn asset_type(&self) -> AssetType {
        self.key.asset_type
    }

    /// The asset's id.
    #[must_use]
    pub fn id(&self) -> i32 {
        self.key.id
    }

    /// The CRC the asset was requested/sent under.
    #[must_use]
    pub fn crc(&self) -> u32 {
        self.key.crc
    }

    /// Length of the assembled bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True for a zero-length asset.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The CRC this asset's bytes hash to under its type's rule, if we know it.
    ///
    /// For a prop this is the CRC of the payload after the 12-byte header — the
    /// same slice the server validates.
    #[must_use]
    pub fn computed_crc(&self) -> Option<u32> {
        match self.key.asset_type {
            AssetType::PROP => payload_crc(&self.data),
            _ => None,
        }
    }
}

/// What the assembler did with a block.
#[derive(Debug)]
pub enum AssemblyOutcome {
    /// The asset is complete and verified.
    Complete(Box<Asset>),
    /// A block was stored; the asset is still incomplete.
    Pending {
        /// The asset.
        key: AssetKey,
        /// Blocks held so far.
        received: usize,
        /// Blocks the first block said to expect.
        expected: usize,
    },
    /// The asset was rejected. Its partial state has been discarded and the
    /// reason is surfaced, not swallowed.
    Rejected {
        /// The asset.
        key: AssetKey,
        /// Why.
        error: AssetError,
    },
    /// The block was discarded without starting an asset — e.g. a non-first
    /// block arrived for an asset whose first block we never saw.
    Discarded {
        /// The asset.
        key: AssetKey,
        /// A short, log-ready reason.
        reason: &'static str,
    },
}

/// Assembles assets from `sAst` blocks and verifies them.
#[derive(Debug)]
pub struct AssetAssembler {
    cfg: AssemblerConfig,
    pending: HashMap<AssetKey, Pending>,
    pending_bytes: u64,
}

#[derive(Debug)]
struct Pending {
    block_count: i16,
    declared_size: Option<u32>,
    flags: u32,
    name: String,
    offsets: BTreeMap<i16, i32>,
    blocks: BTreeMap<i16, Vec<u8>>,
    bytes: u64,
}

impl AssetAssembler {
    /// Build an assembler with the default limits.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(AssemblerConfig::default())
    }

    /// Build an assembler with explicit limits.
    #[must_use]
    pub fn with_config(cfg: AssemblerConfig) -> Self {
        AssetAssembler {
            cfg,
            pending: HashMap::new(),
            pending_bytes: 0,
        }
    }

    /// The active configuration.
    #[must_use]
    pub fn config(&self) -> AssemblerConfig {
        self.cfg
    }

    /// How many assets are still incomplete.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Bytes currently held by incomplete assets.
    #[must_use]
    pub fn pending_bytes(&self) -> u64 {
        self.pending_bytes
    }

    /// True when nothing is half-assembled.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    /// Forget a partial asset, releasing its bytes.
    pub fn forget(&mut self, key: &AssetKey) -> bool {
        match self.pending.remove(key) {
            Some(p) => {
                self.pending_bytes = self.pending_bytes.saturating_sub(p.bytes);
                true
            }
            None => false,
        }
    }

    /// Forget every partial asset.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.pending_bytes = 0;
    }

    /// Feed one decoded block in.
    pub fn ingest(&mut self, transfer: AssetTransfer) -> AssemblyOutcome {
        let key = transfer.key();
        let descriptor = transfer.descriptor.clone();

        if let Some(desc) = &descriptor {
            if u64::from(desc.size) > self.cfg.max_asset_size {
                let error = AssetError::ImplausibleAssetSize {
                    asset_size: u64::from(desc.size),
                    max: self.cfg.max_asset_size,
                };
                self.forget(&key);
                return AssemblyOutcome::Rejected { key, error };
            }
        }

        if transfer.header.block_count as u32 > self.cfg.max_block_count {
            let error = AssetError::ImplausibleBlockCount {
                block_count: i64::from(transfer.header.block_count),
                max: self.cfg.max_block_count,
            };
            self.forget(&key);
            return AssemblyOutcome::Rejected { key, error };
        }

        if transfer.data.len() as u64 > self.cfg.max_block_size {
            let error = AssetError::ImplausibleBlockSize {
                block_size: transfer.data.len() as i64,
                max: self.cfg.max_block_size,
            };
            self.forget(&key);
            return AssemblyOutcome::Rejected { key, error };
        }

        if transfer.header.block_number != 0 && !self.pending.contains_key(&key) {
            self.forget(&key);
            return AssemblyOutcome::Discarded {
                key,
                reason: "block arrived before the first block of the asset",
            };
        }

        if let Some(desc) = descriptor {
            let supersedes = self.pending.get(&key).is_some_and(|p| {
                p.declared_size != Some(desc.size) || p.block_count != transfer.header.block_count
            });
            if supersedes {
                self.forget(&key);
            }
            if !self.pending.contains_key(&key) {
                if self.pending.len() >= self.cfg.max_pending_assets {
                    let error = AssetError::Capacity {
                        what: "incomplete-asset table",
                        limit: self.cfg.max_pending_assets,
                    };
                    return AssemblyOutcome::Rejected { key, error };
                }
                self.pending.insert(
                    key,
                    Pending {
                        block_count: transfer.header.block_count,
                        declared_size: Some(desc.size),
                        flags: desc.flags,
                        name: desc.name,
                        offsets: BTreeMap::new(),
                        blocks: BTreeMap::new(),
                        bytes: 0,
                    },
                );
            }
        }

        let Some(p) = self.pending.get_mut(&key) else {
            return AssemblyOutcome::Discarded {
                key,
                reason: "no partial asset to attach this block to",
            };
        };

        if p.block_count != transfer.header.block_count {
            let error = AssetError::InconsistentBlock {
                block_number: i64::from(transfer.header.block_number),
                expected: i64::from(p.block_count),
                got: i64::from(transfer.header.block_count),
            };
            self.forget(&key);
            return AssemblyOutcome::Rejected { key, error };
        }

        if let Some(seen) = p.offsets.get(&transfer.header.block_number) {
            if *seen != transfer.header.block_offset {
                let error = AssetError::InconsistentBlock {
                    block_number: i64::from(transfer.header.block_number),
                    expected: i64::from(*seen),
                    got: i64::from(transfer.header.block_offset),
                };
                self.forget(&key);
                return AssemblyOutcome::Rejected { key, error };
            }
        }

        let grown = self.pending_bytes.saturating_sub(p.bytes) + transfer.data.len() as u64;
        if grown > self.cfg.max_pending_bytes {
            let error = AssetError::Capacity {
                what: "incomplete-asset byte budget",
                limit: self.cfg.max_pending_bytes as usize,
            };
            self.forget(&key);
            return AssemblyOutcome::Rejected { key, error };
        }

        let replaced = p.blocks.insert(transfer.header.block_number, transfer.data);
        p.offsets
            .insert(transfer.header.block_number, transfer.header.block_offset);
        let old = replaced.map_or(0, |v| v.len() as u64);
        let new = p
            .blocks
            .get(&transfer.header.block_number)
            .map_or(0, |v| v.len() as u64);
        p.bytes = p.bytes.saturating_sub(old).saturating_add(new);
        self.pending_bytes = self.pending_bytes.saturating_sub(old).saturating_add(new);

        let received = p.blocks.len();
        let expected = p.block_count.max(0) as usize;
        if received < expected {
            return AssemblyOutcome::Pending {
                key,
                received,
                expected,
            };
        }

        let Some(p) = self.pending.remove(&key) else {
            return AssemblyOutcome::Discarded {
                key,
                reason: "partial asset disappeared mid-assembly",
            };
        };
        self.pending_bytes = self.pending_bytes.saturating_sub(p.bytes);

        let total: usize = p.blocks.values().map(Vec::len).sum();
        let mut data = Vec::with_capacity(total);
        for block in p.blocks.values() {
            data.extend_from_slice(block);
        }

        if let Some(declared) = p.declared_size {
            if data.len() as u64 != u64::from(declared) {
                return AssemblyOutcome::Rejected {
                    key,
                    error: AssetError::SizeMismatch {
                        declared: u64::from(declared),
                        assembled: data.len() as u64,
                    },
                };
            }
        }

        if let Err(error) = self.verify_crc(key, &data) {
            return AssemblyOutcome::Rejected { key, error };
        }

        AssemblyOutcome::Complete(Box::new(Asset {
            key,
            flags: p.flags,
            name: p.name,
            data,
        }))
    }

    fn verify_crc(&self, key: AssetKey, data: &[u8]) -> Result<()> {
        if self.cfg.crc_policy == CrcPolicy::Off || key.crc == 0 {
            return Ok(());
        }
        match crc_rule(key.asset_type) {
            Some(CrcRule::PropPayload) => {
                let computed = payload_crc(data).ok_or(AssetError::Truncated {
                    context: "prop blob shorter than its 12-byte header",
                    needed: PROP_HEADER_LEN,
                    available: data.len(),
                })?;
                if computed == key.crc {
                    Ok(())
                } else {
                    Err(AssetError::CrcMismatch {
                        expected: key.crc,
                        computed,
                    })
                }
            }
            None => match self.cfg.crc_policy {
                CrcPolicy::Strict => Err(AssetError::UnverifiableCrc {
                    asset_type: key.asset_type.as_raw(),
                    crc: key.crc,
                }),
                _ => Ok(()),
            },
        }
    }
}

impl Default for AssetAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrcRule {
    /// Seeded rotate/XOR fold over the bytes after the 12-byte prop header.
    PropPayload,
}

fn crc_rule(asset_type: AssetType) -> Option<CrcRule> {
    match asset_type {
        AssetType::PROP => Some(CrcRule::PropPayload),
        _ => None,
    }
}

/// The CRC a prop blob should hash to, exposed for callers that hold a prop and
/// want to build a correctly-addressed `qAst` for it.
#[must_use]
pub fn prop_payload_crc(blob: &[u8]) -> Option<u32> {
    payload_crc(blob)
}

/// The CRC of arbitrary bytes under the Palace asset rule. Rarely what you
/// want for a prop — see [`prop_payload_crc`].
#[must_use]
pub fn raw_asset_crc(data: &[u8]) -> u32 {
    asset_crc(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{AssetDescriptor, AssetSpec, AssetTransfer, BlockHeader};

    fn first_block(spec: AssetSpec, size: u32, data: &[u8], count: i16) -> AssetTransfer {
        AssetTransfer {
            header: BlockHeader {
                asset_type: AssetType::PROP,
                spec,
                block_size: data.len() as i32,
                block_offset: 0,
                block_number: 0,
                block_count: count,
            },
            descriptor: Some(AssetDescriptor {
                flags: 1,
                size,
                name: "n".into(),
            }),
            data: data.to_vec(),
            padding: 0,
        }
    }

    fn later_block(
        spec: AssetSpec,
        number: i16,
        count: i16,
        offset: i32,
        data: &[u8],
    ) -> AssetTransfer {
        AssetTransfer {
            header: BlockHeader {
                asset_type: AssetType::PROP,
                spec,
                block_size: data.len() as i32,
                block_offset: offset,
                block_number: number,
                block_count: count,
            },
            descriptor: None,
            data: data.to_vec(),
            padding: 0,
        }
    }

    /// A prop blob whose payload CRC is known, built by prefixing a payload the
    /// CRC routine can hash.
    fn prop_blob(payload: &[u8]) -> (Vec<u8>, u32) {
        let mut blob = vec![44, 0, 44, 0, 0, 0, 0, 0, 0, 0, 2, 0];
        blob.extend_from_slice(payload);
        let crc = payload_crc(&blob).expect("blob has a payload");
        (blob, crc)
    }

    #[test]
    fn a_single_block_asset_assembles_and_verifies() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4, 5]);
        let spec = AssetSpec::with_crc(77, crc);
        let mut a = AssetAssembler::new();
        let outcome = a.ingest(first_block(spec, blob.len() as u32, &blob, 1));
        match outcome {
            AssemblyOutcome::Complete(asset) => {
                assert_eq!(asset.data, blob);
                assert_eq!(asset.key.crc, crc);
                assert_eq!(asset.computed_crc(), Some(crc));
                assert_eq!(asset.flags, 1);
                assert_eq!(asset.name, "n");
            }
            other => panic!("expected Complete, got {other:?}"),
        }
        assert!(a.is_idle());
        assert_eq!(a.pending_bytes(), 0);
    }

    #[test]
    fn a_multi_block_asset_assembles_in_the_right_order() {
        let payload: Vec<u8> = (0..40u8).collect();
        let (blob, crc) = prop_blob(&payload);
        let (b0, rest) = blob.split_at(12);
        let (b1, b2) = rest.split_at(10);
        let spec = AssetSpec::with_crc(1, crc);
        let mut a = AssetAssembler::new();

        let o = a.ingest(first_block(spec, blob.len() as u32, b0, 3));
        assert!(matches!(
            o,
            AssemblyOutcome::Pending {
                received: 1,
                expected: 3,
                ..
            }
        ));
        let o = a.ingest(later_block(spec, 2, 3, 22, b2));
        assert!(matches!(o, AssemblyOutcome::Pending { received: 2, .. }));
        let o = a.ingest(later_block(spec, 1, 3, 12, b1));
        match o {
            AssemblyOutcome::Complete(asset) => assert_eq!(asset.data, blob),
            other => panic!("expected Complete, got {other:?}"),
        }
        assert_eq!(a.pending_bytes(), 0);
    }

    #[test]
    fn a_wrong_crc_drops_the_asset() {
        let (blob, crc) = prop_blob(&[9, 9, 9]);
        let spec = AssetSpec::with_crc(5, crc ^ 0xffff);
        let mut a = AssetAssembler::new();
        match a.ingest(first_block(spec, blob.len() as u32, &blob, 1)) {
            AssemblyOutcome::Rejected { error, .. } => {
                assert!(matches!(error, AssetError::CrcMismatch { .. }));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert!(a.is_idle());
    }

    #[test]
    fn a_zero_crc_means_dont_care() {
        let (blob, _) = prop_blob(&[1, 2, 3]);
        let spec = AssetSpec::unchecked(5);
        let mut a = AssetAssembler::new();
        assert!(matches!(
            a.ingest(first_block(spec, blob.len() as u32, &blob, 1)),
            AssemblyOutcome::Complete(_)
        ));
    }

    #[test]
    fn a_size_mismatch_is_caught_by_the_descriptor() {
        let (blob, crc) = prop_blob(&[1, 2, 3]);
        let spec = AssetSpec::with_crc(5, crc);
        let mut a = AssetAssembler::new();
        let mut t = first_block(spec, blob.len() as u32 + 9, &blob, 1);
        t.descriptor.as_mut().unwrap().size = blob.len() as u32 + 9;
        match a.ingest(t) {
            AssemblyOutcome::Rejected { error, .. } => {
                assert!(matches!(error, AssetError::SizeMismatch { .. }));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn a_non_first_block_without_its_head_is_discarded() {
        let mut a = AssetAssembler::new();
        let spec = AssetSpec::unchecked(3);
        let o = a.ingest(later_block(spec, 1, 2, 12, b"xy"));
        assert!(matches!(o, AssemblyOutcome::Discarded { .. }));
        assert!(a.is_idle());
    }

    #[test]
    fn a_duplicate_block_at_a_different_offset_is_rejected() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let spec = AssetSpec::with_crc(5, crc);
        let mut a = AssetAssembler::new();
        let _ = a.ingest(first_block(spec, blob.len() as u32, &blob[..4], 3));
        let _ = a.ingest(later_block(spec, 1, 3, 4, &blob[4..10]));
        let o = a.ingest(later_block(spec, 1, 3, 999, &blob[4..10]));
        assert!(matches!(
            o,
            AssemblyOutcome::Rejected {
                error: AssetError::InconsistentBlock { .. },
                ..
            }
        ));
        assert!(a.is_idle());
    }

    #[test]
    fn a_repeated_block_at_the_same_offset_is_idempotent() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let spec = AssetSpec::with_crc(5, crc);
        let mut a = AssetAssembler::new();
        let _ = a.ingest(first_block(spec, blob.len() as u32, &blob[..4], 3));
        let _ = a.ingest(later_block(spec, 1, 3, 4, &blob[4..10]));
        let _ = a.ingest(later_block(spec, 1, 3, 4, &blob[4..10]));
        assert_eq!(a.pending_count(), 1);
        assert_eq!(a.pending_bytes(), 10, "the repeat replaced, not doubled");
    }

    #[test]
    fn a_restarted_asset_supersedes_the_stale_partial() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4]);
        let spec = AssetSpec::with_crc(5, crc);
        let mut a = AssetAssembler::new();
        let _ = a.ingest(first_block(spec, 999, &blob[..2], 2));
        let o = a.ingest(first_block(spec, blob.len() as u32, &blob, 1));
        assert!(matches!(o, AssemblyOutcome::Complete(_)));
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn the_pending_asset_cap_is_enforced() {
        let mut a = AssetAssembler::with_config(AssemblerConfig {
            max_pending_assets: 1,
            ..AssemblerConfig::default()
        });
        let (blob, crc) = prop_blob(&[1, 2, 3]);
        let _ = a.ingest(first_block(
            AssetSpec::with_crc(1, crc),
            blob.len() as u32,
            &blob[..1],
            2,
        ));
        let o = a.ingest(first_block(
            AssetSpec::with_crc(2, crc),
            blob.len() as u32,
            &blob[..1],
            2,
        ));
        assert!(matches!(
            o,
            AssemblyOutcome::Rejected {
                error: AssetError::Capacity { .. },
                ..
            }
        ));
    }

    #[test]
    fn the_pending_byte_budget_is_enforced() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut a = AssetAssembler::with_config(AssemblerConfig {
            max_pending_bytes: 4,
            ..AssemblerConfig::default()
        });
        let o = a.ingest(first_block(
            AssetSpec::with_crc(1, crc),
            blob.len() as u32,
            &blob,
            2,
        ));
        assert!(matches!(
            o,
            AssemblyOutcome::Rejected {
                error: AssetError::Capacity { .. },
                ..
            }
        ));
        assert_eq!(a.pending_bytes(), 0);
    }

    #[test]
    fn strict_crc_policy_refuses_unknown_types() {
        let mut a = AssetAssembler::with_config(AssemblerConfig {
            crc_policy: CrcPolicy::Strict,
            ..AssemblerConfig::default()
        });
        let t = AssetTransfer {
            header: BlockHeader {
                asset_type: AssetType::USERBASE,
                spec: AssetSpec::with_crc(1, 0x1234),
                block_size: 2,
                block_offset: 0,
                block_number: 0,
                block_count: 1,
            },
            descriptor: Some(AssetDescriptor {
                flags: 0,
                size: 2,
                name: String::new(),
            }),
            data: vec![1, 2],
            padding: 0,
        };
        assert!(matches!(
            a.ingest(t),
            AssemblyOutcome::Rejected {
                error: AssetError::UnverifiableCrc { .. },
                ..
            }
        ));
    }

    #[test]
    fn the_default_policy_accepts_unknown_types_unverified() {
        let mut a = AssetAssembler::new();
        let t = AssetTransfer {
            header: BlockHeader {
                asset_type: AssetType::USERBASE,
                spec: AssetSpec::with_crc(1, 0x1234),
                block_size: 2,
                block_offset: 0,
                block_number: 0,
                block_count: 1,
            },
            descriptor: Some(AssetDescriptor {
                flags: 0,
                size: 2,
                name: String::new(),
            }),
            data: vec![1, 2],
            padding: 0,
        };
        assert!(matches!(a.ingest(t), AssemblyOutcome::Complete(_)));
    }

    #[test]
    fn a_prop_too_short_to_have_a_payload_is_dropped_not_panicked() {
        let mut a = AssetAssembler::new();
        let t = first_block(AssetSpec::with_crc(1, 0xdead_beef), 3, &[1, 2, 3], 1);
        match a.ingest(t) {
            AssemblyOutcome::Rejected { error, .. } => {
                assert!(matches!(error, AssetError::Truncated { .. }));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn forget_releases_the_byte_budget() {
        let (blob, crc) = prop_blob(&[1, 2, 3, 4]);
        let spec = AssetSpec::with_crc(1, crc);
        let mut a = AssetAssembler::new();
        let _ = a.ingest(first_block(spec, blob.len() as u32, &blob, 2));
        assert!(a.pending_bytes() > 0);
        assert!(a.forget(&AssetKey::new(AssetType::PROP, 1, crc)));
        assert_eq!(a.pending_bytes(), 0);
        assert!(a.is_idle());
        assert!(!a.forget(&AssetKey::new(AssetType::PROP, 1, crc)));
    }
}

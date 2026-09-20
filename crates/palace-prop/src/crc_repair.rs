//! Non-destructive repair of stale `Prop` CRCs in a `.prp` roster.
//!
//! The server validates only `"Prop"` records at startup (`ValidateProps`, spec
//! §6): it hashes the blob after its 12-byte header and compares the result with
//! the record's `crc` field. A record whose stored CRC does not match is marked
//! bad and dropped, so the prop never loads even though its bytes are intact.
//!
//! [`audit`] finds those records. [`repair`] and [`repair_bytes`] emit a
//! **repaired copy**: the same container with every mismatching `Prop` record's
//! `crc` field recomputed from its payload. Nothing else changes — the data
//! region, every other record field, the names blob and every non-`Prop` record
//! are carried through as they were read.
//!
//! # The original is never written
//!
//! [`repair_file`] reads the source and writes a **new** file; it refuses when
//! the destination resolves to the source, and there is no in-place repair in
//! this module. [`audit_bytes`] writes nothing at all.
//!
//! # Non-`Prop` records are not payloads
//!
//! The `Fave` sentinel's `crc` field is not a payload CRC (spec §7), and the
//! other types are not validated by the server either. The audit reports them as
//! unvalidated and the repair leaves them byte-for-byte alone. That also means a
//! `Fave` entry that referenced a repaired prop's old `(id, crc)` pair is not
//! rewritten: changing a favourites list is a separate, visible decision.
//!
//! # What repair does not fix
//!
//! A fresh CRC only clears the server's validation gate. A payload that is itself
//! malformed (an RLE stream that does not decode) still will not render; this
//! module reports CRC state, not decodability.

use std::fs;
use std::io::Write as _;
use std::mem::offset_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::crc::asset_crc;
use crate::error::{PropError, Result};
use crate::header::HEADER_LEN;
use crate::prp::{AssetRec, AssetType, PropKey, Roster};

/// Bytes in one on-disk [`AssetRec`].
const ASSET_REC_LEN: usize = std::mem::size_of::<AssetRec>();

/// File-header offset of `asset_map_offset` (spec §2.1).
const FILE_MAP_OFFSET_AT: usize = 8;

/// Map-header offset of `recs_offset` (spec §2.2).
const MAP_RECS_OFFSET_AT: usize = 16;

/// One temp-file sequence number per process, so concurrent repairs never
/// collide on a temp name.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A `Prop` record whose stored CRC does not match its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrcMismatch {
    /// The record's position in file order.
    pub index: usize,
    /// The identity as stored; its `crc` half is the stale one.
    pub key: PropKey,
    /// The record's resolved name, when it has one.
    pub name: Option<String>,
    /// Blob length in bytes, the 12-byte header included.
    pub data_size: u32,
    /// The prop's own flag word, from the blob header.
    pub prop_flags: u16,
    /// The CRC the record declares.
    pub stored_crc: u32,
    /// The CRC recomputed from the blob's payload.
    pub computed_crc: u32,
}

/// What an audit found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrcAudit {
    /// `Prop` records whose payload was hashed.
    pub checked: usize,
    /// Records left unvalidated: every non-`Prop` record, plus a `Prop` blob too
    /// short to carry a payload.
    pub unvalidated: usize,
    /// The mismatching records, in file order.
    pub mismatches: Vec<CrcMismatch>,
}

impl CrcAudit {
    /// Whether every checked `Prop` payload matched its stored CRC.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// How many records a repair would rewrite.
    #[must_use]
    pub fn repaired_count(&self) -> usize {
        self.mismatches.len()
    }
}

/// A repaired roster's bytes, plus the audit that produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairedBytes {
    /// The complete repaired `.prp`.
    pub bytes: Vec<u8>,
    /// The audit of the roster before repair.
    pub audit: CrcAudit,
}

/// What [`repair_file`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairedFile {
    /// The file that was read; never written.
    pub source: PathBuf,
    /// The new file that was written.
    pub destination: PathBuf,
    /// The audit of the source.
    pub audit: CrcAudit,
    /// Size of the repaired copy in bytes.
    pub bytes_written: usize,
}

/// Audit a parsed roster: hash every `Prop` payload and compare it to the CRC
/// the record declares.
///
/// Only records inside the `"Prop"` type's declared range are checked, matching
/// `ValidateProps` (spec §6). Everything else is counted in
/// [`CrcAudit::unvalidated`].
#[must_use]
pub fn audit(roster: &Roster) -> CrcAudit {
    let mut audit = CrcAudit::default();
    for (index, record) in roster.records().iter().enumerate() {
        let is_prop = record_type(roster, index) == Some(AssetType::Prop);
        if !is_prop || record.blob.len() < HEADER_LEN {
            audit.unvalidated += 1;
            continue;
        }
        audit.checked += 1;
        let computed_crc = asset_crc(&record.blob[HEADER_LEN..]);
        if computed_crc != record.crc() {
            audit.mismatches.push(CrcMismatch {
                index,
                key: record.key(),
                name: record.name.clone(),
                data_size: record.rec.data_size,
                prop_flags: record.prop_flags(),
                stored_crc: record.crc(),
                computed_crc,
            });
        }
    }
    audit
}

/// Parse `buf` and audit it. Writes nothing.
pub fn audit_bytes(buf: &[u8]) -> Result<CrcAudit> {
    Ok(audit(&Roster::parse(buf)?))
}

/// Emit a repaired copy of a parsed roster.
///
/// The output is the roster's own serialisation with each mismatching `Prop`
/// record's `crc` field replaced by the payload CRC. When the roster is already
/// clean the output is byte-identical to [`Roster::write`]'s.
///
/// Refused when the parse dropped records: a repaired copy must not silently
/// lose them.
pub fn repair(roster: &Roster) -> Result<RepairedBytes> {
    if roster.dropped_records() != 0 {
        return Err(refused(
            "the parse dropped records; refusing to emit a copy that would silently lose them",
        ));
    }
    let audit = audit(roster);
    let mut bytes = roster.write()?;
    patch_crc_fields(&mut bytes, &audit.mismatches)?;
    Ok(RepairedBytes { bytes, audit })
}

/// Parse `buf` and emit its repaired copy.
///
/// Refused when the container is not self-consistent (`filesize !=
/// assetMapOffset + assetMapSize`) or when the parse dropped records, because
/// the literal writer would then emit a file that is not a faithful copy plus
/// CRC fixes.
pub fn repair_bytes(buf: &[u8]) -> Result<RepairedBytes> {
    let roster = Roster::parse(buf)?;
    if !roster.file_header().invariant_holds(buf.len()) {
        return Err(refused(
            "the file does not satisfy filesize == assetMapOffset + assetMapSize; a \
             repaired copy would not preserve its trailing bytes",
        ));
    }
    repair(&roster)
}

/// Read `source` and write its repaired copy to `destination`.
///
/// The source is only ever read. The destination is written atomically (temp
/// file in its directory, fsync, rename) and may be an existing file, which is
/// replaced; the call is refused when the destination resolves to the source.
/// The destination's directory must already exist.
pub fn repair_file(source: &Path, destination: &Path) -> Result<RepairedFile> {
    if resolve(source) == resolve(destination) {
        return Err(refused(&format!(
            "refusing to write the repaired copy over its own source ({})",
            source.display()
        )));
    }
    let buf = fs::read(source).map_err(|error| io_error(source, error))?;
    let repaired = repair_bytes(&buf)?;
    write_atomic(destination, &repaired.bytes)?;
    Ok(RepairedFile {
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        audit: repaired.audit,
        bytes_written: repaired.bytes.len(),
    })
}

/// The raw 4CC of the type whose declared range covers `index`, or `None`.
fn record_type(roster: &Roster, index: usize) -> Option<AssetType> {
    roster.types().iter().find_map(|asset_type| {
        let first = asset_type.first_asset.max(0) as usize;
        let count = asset_type.nbr_assets.max(0) as usize;
        (index >= first && index < first.saturating_add(count)).then(|| asset_type.kind())
    })
}

/// Overwrite each mismatching record's `crc` field in an already-serialised
/// roster, using the offsets the serialised bytes themselves declare.
fn patch_crc_fields(bytes: &mut [u8], mismatches: &[CrcMismatch]) -> Result<()> {
    if mismatches.is_empty() {
        return Ok(());
    }
    let map_offset = read_u32(bytes, FILE_MAP_OFFSET_AT)? as usize;
    let recs_offset = read_u32(
        bytes,
        map_offset
            .checked_add(MAP_RECS_OFFSET_AT)
            .ok_or_else(|| refused("the asset-map offset overflows"))?,
    )? as usize;
    let recs_start = map_offset
        .checked_add(recs_offset)
        .ok_or_else(|| refused("the record table offset overflows"))?;

    for mismatch in mismatches {
        let at = recs_start
            .checked_add(mismatch.index.saturating_mul(ASSET_REC_LEN))
            .and_then(|at| at.checked_add(offset_of!(AssetRec, crc)))
            .ok_or_else(|| refused("the record table offset overflows"))?;
        let slot = bytes
            .get_mut(at..at.saturating_add(4))
            .ok_or_else(|| refused("the emitted record table is smaller than the audit expects"))?;
        slot.copy_from_slice(&mismatch.computed_crc.to_le_bytes());
    }
    Ok(())
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| refused("an offset overflows"))?;
    let raw = bytes
        .get(at..end)
        .ok_or_else(|| refused("the emitted file header is truncated"))?;
    let mut word = [0u8; 4];
    word.copy_from_slice(raw);
    Ok(u32::from_le_bytes(word))
}

/// Resolve a path through symlinks, or through its deepest existing ancestor
/// when the path itself does not exist yet.
fn resolve(path: &Path) -> PathBuf {
    if let Ok(resolved) = fs::canonicalize(path) {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match fs::canonicalize(parent) {
            Ok(parent) => parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Write `bytes` to `destination` through a temp file in the same directory,
/// fsyncing before the rename. On failure the temp file is removed.
fn write_atomic(destination: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| refused("the destination has no directory"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| refused("the destination is not a file path"))?;
    let temp = parent.join(format!(
        ".{}.repair-{}-{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let outcome = write_temp(&temp, bytes).and_then(|()| {
        fs::rename(&temp, destination).map_err(|error| io_error(destination, error))
    });
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    } else if let Ok(dir) = fs::File::open(parent) {
        // Best effort: makes the rename itself durable.
        let _ = dir.sync_all();
    }
    outcome
}

fn write_temp(temp: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::File::create(temp).map_err(|error| io_error(temp, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error(temp, error))?;
    file.sync_all().map_err(|error| io_error(temp, error))
}

fn refused(detail: &str) -> PropError {
    PropError::BagIo {
        detail: format!(".prp repair refused: {detail}"),
    }
}

fn io_error(path: &Path, error: std::io::Error) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {error}", path.display()),
    }
}

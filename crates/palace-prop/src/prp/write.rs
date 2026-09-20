//! The `.prp` writer: model in, bytes out.
//!
//! [`serialize`] rebuilds the container from the model in the spec's order: the
//! 16-byte file header, the data region, then the asset map (map header, type
//! records, 32-byte records, names blob). It is a *literal* serialiser:
//!
//! * `dataOffset` is always 16 and `dataSize` is the stored data region's length,
//!   so `assetMapOffset = 16 + dataSize`;
//! * the data region is emitted **verbatim** — the real collections leave gaps
//!   between blobs (unreferenced stale bytes) and the region is the only place
//!   those bytes survive, so blobs are not repacked here;
//! * every record field is emitted unchanged, including `crc`. That is what makes
//!   a no-op parse→write byte-identical; one real collection carries a stale CRC
//!   and the writer must not "repair" it. Recomputing CRCs and repacking blobs is
//!   the explicit job of [`crate::prp::Roster::canonicalise`] and of the mutation
//!   methods.
//! * names are re-encoded latin-1, the exact inverse of the reader.
//!
//! The size invariant is checked before returning: `filesize` must equal
//! `assetMapOffset + assetMapSize`.

use crate::error::{PropError, Result};

use super::model::FILE_HEADER_LEN;
use super::record::{latin1_bytes, MAX_NAME_LEN};
use super::Roster;

/// Map-relative offset of the type table: immediately after the 24-byte map header.
const TYPES_OFFSET: usize = std::mem::size_of::<super::AssetMapHeader>();

/// Serialise a roster, preserving the data region and every record field verbatim.
pub(super) fn serialize(roster: &Roster) -> Result<Vec<u8>> {
    let nbr_types = roster.types.len();
    let nbr_assets = roster.records.len();

    let names_blob = encode_names(roster);
    let len_names = names_blob.len();

    let types_offset = TYPES_OFFSET;
    let recs_offset = types_offset + nbr_types * std::mem::size_of::<super::AssetTypeRec>();
    let names_offset = recs_offset + nbr_assets * std::mem::size_of::<super::AssetRec>();
    let map_size = names_offset + len_names;

    let data = &roster.data;
    let data_size = data.len();
    let map_offset = FILE_HEADER_LEN + data_size;

    let mut out = Vec::with_capacity(map_offset + map_size);
    put_u32(&mut out, FILE_HEADER_LEN as u32);
    put_u32(&mut out, data_size as u32);
    put_u32(&mut out, map_offset as u32);
    put_u32(&mut out, map_size as u32);

    out.extend_from_slice(data);

    put_i32(&mut out, nbr_types as i32);
    put_i32(&mut out, nbr_assets as i32);
    put_i32(&mut out, len_names as i32);
    put_u32(&mut out, types_offset as u32);
    put_u32(&mut out, recs_offset as u32);
    put_u32(&mut out, names_offset as u32);

    for asset_type in &roster.types {
        put_i32(&mut out, asset_type.asset_type);
        put_i32(&mut out, asset_type.nbr_assets);
        put_i32(&mut out, asset_type.first_asset);
    }

    for record in &roster.records {
        let rec = &record.rec;
        put_i32(&mut out, rec.id);
        put_i32(&mut out, rec.r_handle);
        put_u32(&mut out, rec.data_offset);
        put_u32(&mut out, rec.data_size);
        put_i32(&mut out, rec.last_use_time);
        put_i32(&mut out, rec.name_offset);
        put_u32(&mut out, rec.flags);
        put_u32(&mut out, rec.crc);
    }

    out.extend_from_slice(&names_blob);

    let filesize = out.len();
    if (map_offset as u64) + (map_size as u64) != filesize as u64 {
        return Err(inconsistent(
            "write produced a file that violates filesize == assetMapOffset + assetMapSize",
        ));
    }
    Ok(out)
}

/// Encode the names blob in stored order: `[len:u8][latin-1 bytes]...`.
///
/// The records carry their own `nameOffset`, which is reproduced because the
/// reader collected the entries in the same order they were stored.
fn encode_names(roster: &Roster) -> Vec<u8> {
    let mut blob = Vec::new();
    for (_, name) in &roster.names {
        let bytes = latin1_bytes(name);
        let bytes = &bytes[..bytes.len().min(MAX_NAME_LEN)];
        blob.push(bytes.len() as u8);
        blob.extend_from_slice(bytes);
    }
    blob
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn inconsistent(detail: &str) -> PropError {
    PropError::BagIo {
        detail: format!(".prp write refused: {detail}"),
    }
}

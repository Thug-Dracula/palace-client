//! The tolerant `.prp` reader: bytes in, [`Roster`] out.
//!
//! The layout is `PRP-FORMAT.md` §2: a 16-byte file header, a data region of
//! blobs, then the asset map (24-byte map header, type records, 32-byte records,
//! names blob). This module is deliberately forgiving, because real rosters are
//! written by a 1990s server and carry stale bytes:
//!
//! * the counts in the map header are trusted — the parser never scans past the
//!   declared map, so trailing garbage after the map is ignored;
//! * a record whose blob slice falls outside the data region is **dropped and
//!   counted** ([`Roster::dropped_records`]), not treated as fatal;
//! * a name whose entry is malformed simply does not resolve; the record keeps
//!   `None` rather than failing the parse.
//!
//! The one thing that *is* fatal is a file too short to contain what its own
//! header claims: a truncated map cannot be sliced safely, so the reader returns
//! [`PropError`] rather than guessing. It never panics and never repairs data.
//!
//! There is no `.prp`-specific [`PropError`] variant yet, so structural failures
//! use [`PropError::BagIo`]'s free-form detail (a file that is shorter than its
//! header) and [`PropError::HeaderTooShort`] for a buffer smaller than the file
//! header itself.
//!
//! No filesystem access lives here: the caller reads the bytes.

use std::collections::HashMap;
use std::mem::size_of;

use crate::error::{PropError, Result};

use super::model::{AssetFileHeader, AssetMapHeader, AssetRec, AssetTypeRec, FILE_HEADER_LEN};
use super::record::{PropHeader, PropRecord};
use super::Roster;

/// Parse a complete `.prp` file.
pub(super) fn parse(buf: &[u8]) -> Result<Roster> {
    let file = parse_file_header(buf)?;

    let map_start = file.asset_map_offset as usize;
    let map_size = file.asset_map_size as usize;
    let map_end = map_start
        .checked_add(map_size)
        .ok_or_else(|| inconsistent("asset_map_offset + asset_map_size overflows"))?;
    if map_end > buf.len() {
        return Err(truncated("asset map", map_end, buf.len()));
    }
    let region = buf
        .get(map_start..map_end)
        .ok_or_else(|| truncated("asset map", map_end, buf.len()))?;

    let map = parse_map_header(region)?;
    if map.nbr_types < 0 || map.nbr_assets < 0 || map.len_names < 0 {
        return Err(inconsistent("a map header count is negative"));
    }
    let nbr_types = map.nbr_types as usize;
    let nbr_assets = map.nbr_assets as usize;

    let types_off = map.types_offset as usize;
    let recs_off = map.recs_offset as usize;
    let names_off = map.names_offset as usize;

    let types_bytes = nbr_types
        .checked_mul(size_of::<AssetTypeRec>())
        .ok_or_else(|| inconsistent("type count overflows"))?;
    let recs_bytes = nbr_assets
        .checked_mul(size_of::<AssetRec>())
        .ok_or_else(|| inconsistent("record count overflows"))?;

    let types_region = section(region, types_off, types_bytes, "type table")?;
    let recs_region = section(region, recs_off, recs_bytes, "record table")?;
    let names_region = section(region, names_off, map.len_names as usize, "names blob")?;

    let types = parse_types(types_region, nbr_types)?;
    let names = parse_names(names_region);
    let by_offset: HashMap<u32, &str> = names.iter().map(|(o, n)| (*o, n.as_str())).collect();

    let region_base = file.data_offset as usize;
    let mut records = Vec::with_capacity(nbr_assets);
    let mut dropped = 0usize;
    for i in 0..nbr_assets {
        let rec = parse_asset_rec(recs_region, i * size_of::<AssetRec>())?;
        let name = if rec.name_offset >= 0 {
            by_offset
                .get(&(rec.name_offset as u32))
                .map(|name| (*name).to_string())
        } else {
            None
        };

        let start = region_base.checked_add(rec.data_offset as usize);
        let end = start.and_then(|s| s.checked_add(rec.data_size as usize));
        let blob = match (start, end) {
            (Some(s), Some(e)) if e <= map_start => buf.get(s..e).map(<[u8]>::to_vec),
            _ => None,
        };
        let Some(blob) = blob else {
            dropped += 1;
            continue;
        };

        let header = PropHeader::parse(&blob).ok();
        let encoding = header.map(PropHeader::encoding);
        records.push(PropRecord {
            rec,
            header,
            encoding,
            blob,
            name,
        });
    }

    let data = buf
        .get(region_base..map_start)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();

    Ok(Roster {
        file,
        map,
        types,
        records,
        names,
        data,
        dropped,
    })
}

fn parse_file_header(buf: &[u8]) -> Result<AssetFileHeader> {
    if buf.len() < FILE_HEADER_LEN {
        return Err(PropError::HeaderTooShort {
            available: buf.len(),
        });
    }
    Ok(AssetFileHeader {
        data_offset: word_u32(buf, 0)?,
        data_size: word_u32(buf, 4)?,
        asset_map_offset: word_u32(buf, 8)?,
        asset_map_size: word_u32(buf, 12)?,
    })
}

fn parse_map_header(region: &[u8]) -> Result<AssetMapHeader> {
    if region.len() < size_of::<AssetMapHeader>() {
        return Err(inconsistent("asset map is smaller than its own header"));
    }
    Ok(AssetMapHeader {
        nbr_types: word_i32(region, 0)?,
        nbr_assets: word_i32(region, 4)?,
        len_names: word_i32(region, 8)?,
        types_offset: word_u32(region, 12)?,
        recs_offset: word_u32(region, 16)?,
        names_offset: word_u32(region, 20)?,
    })
}

fn parse_types(region: &[u8], count: usize) -> Result<Vec<AssetTypeRec>> {
    let mut types = Vec::with_capacity(count);
    for i in 0..count {
        let at = i * size_of::<AssetTypeRec>();
        types.push(AssetTypeRec {
            asset_type: word_i32(region, at)?,
            nbr_assets: word_i32(region, at + 4)?,
            first_asset: word_i32(region, at + 8)?,
        });
    }
    Ok(types)
}

fn parse_asset_rec(region: &[u8], at: usize) -> Result<AssetRec> {
    Ok(AssetRec {
        id: word_i32(region, at)?,
        r_handle: word_i32(region, at + 4)?,
        data_offset: word_u32(region, at + 8)?,
        data_size: word_u32(region, at + 12)?,
        last_use_time: word_i32(region, at + 16)?,
        name_offset: word_i32(region, at + 20)?,
        flags: word_u32(region, at + 24)?,
        crc: word_u32(region, at + 28)?,
    })
}

/// Parse the names blob: `[len:u8][bytes]...`, keyed by the byte offset of each
/// length byte. Bytes are mapped as latin-1, matching the reference reader, so
/// decoding never fails. A malformed trailing entry simply ends the list.
fn parse_names(region: &[u8]) -> Vec<(u32, String)> {
    let mut names = Vec::new();
    let mut at = 0usize;
    while at < region.len() {
        let Some(&len) = region.get(at) else { break };
        let start = at + 1;
        let end = start.saturating_add(len as usize);
        let Some(bytes) = region.get(start..end) else {
            break;
        };
        let name: String = bytes.iter().map(|&b| char::from(b)).collect();
        names.push((at as u32, name));
        at = end;
    }
    names
}

fn section<'a>(region: &'a [u8], offset: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| inconsistent(format!("{what} extent overflows")))?;
    region
        .get(offset..end)
        .ok_or_else(|| inconsistent(format!("{what} overruns the declared map")))
}

fn word_u32(buf: &[u8], at: usize) -> Result<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| inconsistent("offset overflows"))?;
    let bytes = buf
        .get(at..end)
        .ok_or_else(|| inconsistent("asset map is truncated"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn word_i32(buf: &[u8], at: usize) -> Result<i32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| inconsistent("offset overflows"))?;
    let bytes = buf
        .get(at..end)
        .ok_or_else(|| inconsistent("asset map is truncated"))?;
    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// A file shorter than its own header claims. Reported as `BagIo` because there
/// is no `.prp`-specific variant yet; the detail names the shortfall.
fn truncated(what: &str, needed: usize, available: usize) -> PropError {
    PropError::BagIo {
        detail: format!(
            ".prp is truncated: {what} needs {needed} byte(s) of file but only {available} exist"
        ),
    }
}

/// A map that contradicts itself. Reported as `BagIo` for the same reason.
fn inconsistent(detail: impl Into<String>) -> PropError {
    PropError::BagIo {
        detail: format!(".prp map is inconsistent: {}", detail.into()),
    }
}

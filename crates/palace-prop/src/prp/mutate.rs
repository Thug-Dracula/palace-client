//! Roster mutation: `canonicalise`, `add_prop`, `add_fave` and `remove_prop`.
//!
//! These are the operations that *change* a roster, so unlike [`super::write`]
//! they are allowed to move bytes: they re-sort, repack the data region, rebuild
//! the names blob, and recompute every record CRC. The no-op writer stays a
//! literal serialiser; everything semantic happens here.
//!
//! The rules that matter:
//!
//! * **A record goes in its own type's run.** Every insert targets one
//!   [`AssetType`] and is placed inside that type's declared range, so a `Prop`
//!   insert can never displace a `Fave` entry (or the other way round). This is
//!   what keeps the type table honest: `ValidateProps` only walks `Prop`
//!   (`PRP-FORMAT.md` §6), so a favourites payload inserted as a prop would be
//!   read as a headerless prop and marked bad on every startup.
//! * **Variant chains stay adjacent.** Records that share an `id` differ only by
//!   `crc`; an insert places a new record next to its id's run, and the sort is
//!   stable so existing variants keep their relative order.
//! * **Never drop or deduplicate.** Duplicate ids and duplicate `(id, crc)` pairs
//!   are legal and survive.
//! * **Canonical form** is what the server's signed binary search requires: each
//!   type's records ascending by signed `id`, blobs packed contiguously from
//!   offset 0, and CRCs recomputed.

use std::collections::HashMap;

use crate::crc::asset_crc;
use crate::header::HEADER_LEN;

use super::model::{AssetFileHeader, AssetMapHeader, AssetRec, AssetTypeRec, FILE_HEADER_LEN};
use super::record::{latin1_bytes, PropHeader, PropKey, PropRecord, MAX_NAME_LEN};
use super::{AssetType, Roster};

/// Sort each type's records by signed id and repack them into canonical server
/// form. Stable, so duplicate id/crc variants keep their original relative order.
pub(super) fn canonicalise(roster: &mut Roster) {
    let mut ordered = Vec::with_capacity(roster.records.len());
    for asset_type in &roster.types {
        let start = asset_type.first_asset.max(0) as usize;
        let count = asset_type.nbr_assets.max(0) as usize;
        let end = start.saturating_add(count).min(roster.records.len());
        if start >= end {
            continue;
        }
        let mut group = roster.records[start..end].to_vec();
        group.sort_by_key(|record| record.id());
        ordered.extend(group);
    }
    // Records outside every declared type range (a malformed map): keep them, in
    // order, after the typed ones rather than losing them.
    if ordered.len() < roster.records.len() {
        ordered.extend_from_slice(&roster.records[ordered.len()..]);
    }
    roster.records = ordered;
    finish(roster);
}

/// Insert a record into the `Prop` type, adjacent to any records sharing its id.
pub(super) fn add_prop(roster: &mut Roster, record: PropRecord) {
    add_typed(roster, record, AssetType::Prop);
}

/// Insert a record into the `Fave` type, creating that type when it is absent.
pub(super) fn add_fave(roster: &mut Roster, record: PropRecord) {
    add_typed(roster, record, AssetType::Fave);
}

/// Insert `record` into `kind`'s run, creating the type when it does not exist.
///
/// The insert is confined to the target type's declared range, so a `Prop` insert
/// cannot push a `Fave` record (or the reverse) across the type boundary; the
/// type table stays consistent with the record array afterwards.
fn add_typed(roster: &mut Roster, record: PropRecord, kind: AssetType) {
    let mut record = record;
    normalize_record(&mut record);

    let type_index = match roster
        .types
        .iter()
        .position(|asset_type| asset_type.kind() == kind)
    {
        Some(index) => index,
        None => {
            roster.types.push(AssetTypeRec {
                asset_type: kind.to_raw() as i32,
                nbr_assets: 0,
                first_asset: roster.records.len() as i32,
            });
            roster.types.len() - 1
        }
    };

    // Clamp defensively: a parsed map may declare a `firstAsset` past the record
    // array, which would otherwise make the slice below panic.
    let start = (roster.types[type_index].first_asset.max(0) as usize).min(roster.records.len());
    let count = roster.types[type_index].nbr_assets.max(0) as usize;
    let end = start.saturating_add(count).min(roster.records.len());

    let mut insert_at = end;
    let mut in_variant_run = false;
    for (offset, existing) in roster.records[start..end].iter().enumerate() {
        let id = existing.id();
        if id == record.id() {
            in_variant_run = true;
            insert_at = start + offset + 1;
        } else if in_variant_run {
            break;
        } else if id > record.id() {
            insert_at = start + offset;
            break;
        }
    }

    roster.records.insert(insert_at, record);
    roster.types[type_index].nbr_assets = roster.types[type_index].nbr_assets.saturating_add(1);
    finish(roster);
}

/// Remove and return the record with this exact identity, if present.
pub(super) fn remove_prop(roster: &mut Roster, key: PropKey) -> Option<PropRecord> {
    let index = roster
        .records
        .iter()
        .position(|record| record.key() == key)?;
    let removed = roster.records.remove(index);

    if let Some(asset_type) = roster.types.iter_mut().find(|asset_type| {
        let start = asset_type.first_asset.max(0) as usize;
        let end = start.saturating_add(asset_type.nbr_assets.max(0) as usize);
        index >= start && index < end
    }) {
        asset_type.nbr_assets = asset_type.nbr_assets.saturating_sub(1);
    }
    finish(roster);
    Some(removed)
}

/// Recompute the derived state after `records`/`types` changed: type ranges,
/// names blob, packed data region, CRCs and headers.
fn finish(roster: &mut Roster) {
    reindex_types(roster);
    rebuild_names(roster);
    repack_data(roster);
    recompute_crcs(roster);
    refresh_headers(roster);
}

fn normalize_record(record: &mut PropRecord) {
    record.rec.data_size = record.blob.len() as u32;
    let header = PropHeader::parse(&record.blob).ok();
    record.encoding = header.map(PropHeader::encoding);
    record.header = header;
    if record.blob.len() >= HEADER_LEN {
        record.rec.crc = asset_crc(&record.blob[HEADER_LEN..]);
    }
}

fn reindex_types(roster: &mut Roster) {
    let mut running = 0i32;
    for asset_type in &mut roster.types {
        asset_type.first_asset = running;
        running = running.saturating_add(asset_type.nbr_assets.max(0));
    }
}

fn rebuild_names(roster: &mut Roster) {
    let mut blob: Vec<u8> = Vec::new();
    let mut names: Vec<(u32, String)> = Vec::new();
    let mut seen: HashMap<String, u32> = HashMap::new();

    for record in &mut roster.records {
        match &record.name {
            Some(name) => {
                let bytes = latin1_bytes(name);
                let bytes = &bytes[..bytes.len().min(MAX_NAME_LEN)];
                let offset = match seen.get(name) {
                    Some(offset) => *offset,
                    None => {
                        let offset = blob.len() as u32;
                        blob.push(bytes.len() as u8);
                        blob.extend_from_slice(bytes);
                        seen.insert(name.clone(), offset);
                        names.push((offset, name.clone()));
                        offset
                    }
                };
                record.rec.name_offset = offset as i32;
            }
            None => record.rec.name_offset = -1,
        }
    }
    roster.names = names;
}

fn repack_data(roster: &mut Roster) {
    let mut data = Vec::new();
    for record in &mut roster.records {
        record.rec.data_offset = data.len() as u32;
        record.rec.data_size = record.blob.len() as u32;
        data.extend_from_slice(&record.blob);
    }
    roster.data = data;
}

fn recompute_crcs(roster: &mut Roster) {
    for record in &mut roster.records {
        let header = PropHeader::parse(&record.blob).ok();
        record.encoding = header.map(PropHeader::encoding);
        record.header = header;
        if record.blob.len() >= HEADER_LEN {
            record.rec.crc = asset_crc(&record.blob[HEADER_LEN..]);
        }
    }
}

fn refresh_headers(roster: &mut Roster) {
    let nbr_types = roster.types.len();
    let nbr_assets = roster.records.len();
    let len_names = roster
        .names
        .iter()
        .map(|(_, name)| 1 + latin1_bytes(name).len().min(MAX_NAME_LEN))
        .sum::<usize>();

    let types_offset = std::mem::size_of::<AssetMapHeader>();
    let recs_offset = types_offset + nbr_types * std::mem::size_of::<AssetTypeRec>();
    let names_offset = recs_offset + nbr_assets * std::mem::size_of::<AssetRec>();
    let map_size = names_offset + len_names;
    let data_size = roster.data.len();

    roster.map = AssetMapHeader {
        nbr_types: nbr_types as i32,
        nbr_assets: nbr_assets as i32,
        len_names: len_names as i32,
        types_offset: types_offset as u32,
        recs_offset: recs_offset as u32,
        names_offset: names_offset as u32,
    };
    roster.file = AssetFileHeader {
        data_offset: FILE_HEADER_LEN as u32,
        data_size: data_size as u32,
        asset_map_offset: (FILE_HEADER_LEN + data_size) as u32,
        asset_map_size: map_size as u32,
    };
}

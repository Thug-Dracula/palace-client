//! The tolerant offset walker.
//!
//! A `MSG_ROOMDESC` is a flat 40-byte header whose fields are byte offsets into
//! a trailing variable-length buffer (`varBuf`). Every sub-structure is reached
//! indirectly, so the whole job of this module is: validate an offset, extract
//! the bytes it names, and repeat.
//!
//! ## Tolerance rules
//!
//! * The fixed header and `varBuf` are mandatory: if either is truncated the
//!   decode returns [`WireError::UnexpectedEof`].
//! * Every derived offset is bounds-checked against `varBuf.len()` before it is
//!   used. Nothing indexes offset-derived data directly.
//! * A malformed offset degrades the affected sub-structure (usually to
//!   "absent") and records a [`RoomWarning`]; it never panics and never invents
//!   data. The raw buffer survives in [`RoomDesc::var_data`].
//! * Counts are treated as untrusted: arrays and linked lists stop at the end
//!   of `varBuf`, at a repeated link (cycle), or at the declared count,
//!   whichever comes first.

use palace_wire::byteorder::{latin1, ByteOrder, Reader};
use palace_wire::error::{Result, WireError, MAX_PAYLOAD_LEN};
use palace_wire::messages::{Point, RoomRec};

use crate::model::{
    draw_cmd, DrawCmd, DrawPayload, Hotspot, HotspotState, LooseProp, LoosePropSpec,
    PictureOverlay, RoomDesc, RoomWarning, DRAW_CMD_HEADER_LEN, HOTSPOT_LEN, HOTSPOT_STATE_LEN,
    LOOSE_PROP_LEN, PICTURE_OVERLAY_LEN, POINT_LEN, ROOM_REC_LEN,
};

/// How far past the end of one record [`decode_stream`] will look for the next
/// record header. The live server pads each record with 4 bytes; a small window
/// absorbs that (and a little more) without risking a false split.
pub const MAX_RECORD_GAP: usize = 64;

/// Length of a draw operand's fixed prefix before its points:
/// `penSize(2) + numPoints(2) + duplicated pen colour(6)`.
const DRAW_PAYLOAD_PREFIX: usize = 10;

/// Decode the first `MSG_ROOMDESC` record in `payload`.
///
/// Trailing bytes beyond the record (alignment padding, or a concatenated
/// second record) are not an error; they are reported through
/// [`RoomDesc::trailing_len`]. Use [`decode_stream`] to split a payload that
/// carries more than one record.
pub fn decode_payload(payload: &[u8], order: ByteOrder) -> Result<RoomDesc> {
    let mut r = Reader::new(payload, order);
    RoomDesc::decode(&mut r)
}

/// Decode every `MSG_ROOMDESC` record in `payload`.
///
/// A capture can contain more than one room frame back to back (the corpus used
/// to validate this crate has four such files). Records are split by parsing
/// one, then scanning a small window past its end for the next plausible
/// header. Decoding stops at the first hard error, which is returned as the
/// last element.
pub fn decode_stream(payload: &[u8], order: ByteOrder) -> Vec<Result<RoomDesc>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos.saturating_add(ROOM_REC_LEN) <= payload.len() {
        let Some(slice) = payload.get(pos..) else {
            break;
        };
        let mut r = Reader::new(slice, order);
        match RoomDesc::decode(&mut r) {
            Ok(desc) => {
                let record_len = ROOM_REC_LEN + desc.var_data.len();
                let end = pos.saturating_add(record_len);
                out.push(Ok(desc));
                match find_next_record(payload, end, order) {
                    Some(next) if next > pos => pos = next,
                    _ => break,
                }
            }
            Err(err) => {
                out.push(Err(err));
                break;
            }
        }
    }
    out
}

/// Scan `[from, from + MAX_RECORD_GAP)` for the first offset that looks like a
/// `RoomRec` header with internally consistent offsets.
fn find_next_record(payload: &[u8], from: usize, order: ByteOrder) -> Option<usize> {
    let end = from.checked_add(MAX_RECORD_GAP)?.min(payload.len());
    (from..end).find(|&at| is_plausible_header(payload, at, order))
}

/// Heuristic gate for record resynchronisation. Deliberately strict: a wrong
/// split is worse than a missed one.
fn is_plausible_header(payload: &[u8], at: usize, order: ByteOrder) -> bool {
    let Some(slice) = payload.get(at..) else {
        return false;
    };
    if slice.len() < ROOM_REC_LEN {
        return false;
    }
    // `reserved` must be a zero filler.
    if read_i16(slice, order, 36) != Some(0) {
        return false;
    }
    let Some(len_vars) = read_i16(slice, order, 38) else {
        return false;
    };
    if len_vars <= 0 {
        return false;
    }
    let len_vars = len_vars as usize;
    if ROOM_REC_LEN + len_vars > slice.len() {
        return false;
    }
    // Byte offsets must name a position inside varBuf (0 = absent is fine).
    for field in [10usize, 12, 14, 16, 20, 24, 28, 34] {
        match read_i16(slice, order, field) {
            Some(v) if v >= 0 && (v as usize) < len_vars => {}
            _ => return false,
        }
    }
    // Counts must be non-negative.
    for field in [18usize, 22, 26, 30, 32] {
        match read_i16(slice, order, field) {
            Some(v) if v >= 0 => {}
            _ => return false,
        }
    }
    // Declared arrays must fit.
    let fits = |count_ofst: usize, arr_ofst: usize, stride: usize| -> bool {
        let (Some(count), Some(base)) = (
            read_i16(slice, order, count_ofst),
            read_i16(slice, order, arr_ofst),
        ) else {
            return false;
        };
        if count <= 0 {
            return true;
        }
        let Some(bytes) = (count as usize).checked_mul(stride) else {
            return false;
        };
        (base as usize)
            .checked_add(bytes)
            .is_some_and(|end| end <= len_vars)
    };
    fits(18, 20, HOTSPOT_LEN) && fits(22, 24, PICTURE_OVERLAY_LEN)
}

impl RoomDesc {
    /// Decode a record, reading the 40-byte header and `lenVars` bytes of
    /// `varBuf` from `r`. The reader is left positioned just past `varBuf`.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let header = RoomRec::decode(r)?;
        if header.len_vars < 0 {
            return Err(WireError::ImplausibleLength {
                length: header.len_vars as u32,
                max: MAX_PAYLOAD_LEN,
            });
        }
        let var_data = r.read_bytes(header.len_vars as usize)?.to_vec();
        let trailing_len = r.remaining();
        let order = r.order();
        let var = Var {
            buf: &var_data,
            order,
        };

        let mut warnings = Vec::new();
        let name = var.optional_string(header.room_name_ofst, "roomNameOfst", &mut warnings);
        let picture = var.optional_string(header.pict_name_ofst, "pictNameOfst", &mut warnings);
        let artist = var.optional_string(header.artist_name_ofst, "artistNameOfst", &mut warnings);
        let password = var.optional_string(header.password_ofst, "passwordOfst", &mut warnings);

        if header.reserved != 0 {
            warnings.push(RoomWarning::ReservedNonZero {
                field: "RoomRec.reserved",
                value: header.reserved,
            });
        }

        let pictures = parse_pictures(&var, &header, &mut warnings);
        let hotspots = parse_hotspots(&var, &header, &mut warnings);
        let loose_props = parse_loose_props(&var, &header, &mut warnings);
        let draw_cmds = parse_draw_cmds(&var, &header, &mut warnings);

        Ok(RoomDesc {
            header,
            name,
            picture,
            artist,
            password,
            pictures,
            hotspots,
            loose_props,
            draw_cmds,
            var_data,
            trailing_len,
            warnings,
        })
    }
}

impl PictureOverlay {
    fn parse(var: &Var<'_>, at: usize, warn: &mut Vec<RoomWarning>) -> Option<Self> {
        // Caller verified `at + PICTURE_OVERLAY_LEN <= var.len()`.
        let ref_con = var.i32_at(at)?;
        let pic_id = var.i16_at(at + 4)?;
        let pic_name_ofst = var.i16_at(at + 6)?;
        let trans_color = var.i16_at(at + 8)?;
        let reserved = var.i16_at(at + 10)?;
        if reserved != 0 {
            warn.push(RoomWarning::ReservedNonZero {
                field: "PictureRec.reserved",
                value: reserved,
            });
        }
        let name = var.optional_string_opt(pic_name_ofst, "picNameOfst", warn);
        Some(PictureOverlay {
            ref_con,
            pic_id,
            pic_name_ofst,
            trans_color,
            reserved,
            name,
        })
    }
}

fn parse_pictures(
    var: &Var<'_>,
    header: &RoomRec,
    warn: &mut Vec<RoomWarning>,
) -> Vec<PictureOverlay> {
    let declared = header.nbr_pictures.max(0) as usize;
    if declared == 0 {
        return Vec::new();
    }
    let base = match array_base(header.picture_ofst, declared, "pictureOfst", warn) {
        Some(base) => base,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(declared.min(1024));
    for index in 0..declared {
        let Some(at) = index
            .checked_mul(PICTURE_OVERLAY_LEN)
            .and_then(|delta| base.checked_add(delta))
        else {
            break;
        };
        if var.bytes_at(at, PICTURE_OVERLAY_LEN).is_none() {
            warn.push(RoomWarning::ArrayTruncated {
                field: "pictureOfst",
                index,
                declared,
                available: var.len().saturating_sub(at),
            });
            break;
        }
        if let Some(overlay) = PictureOverlay::parse(var, at, warn) {
            out.push(overlay);
        }
    }
    out
}

impl HotspotState {
    fn parse(var: &Var<'_>, at: usize, warn: &mut Vec<RoomWarning>) -> Option<Self> {
        let pict_id = var.i16_at(at)?;
        let reserved = var.i16_at(at + 2)?;
        let pic_loc = Point::new(var.i16_at(at + 4)?, var.i16_at(at + 6)?);
        if reserved != 0 {
            warn.push(RoomWarning::ReservedNonZero {
                field: "StateRec.reserved",
                value: reserved,
            });
        }
        Some(HotspotState {
            pict_id,
            reserved,
            pic_loc,
        })
    }
}

impl Hotspot {
    fn parse(var: &Var<'_>, at: usize, warn: &mut Vec<RoomWarning>) -> Option<Self> {
        let script_event_mask = var.i32_at(at)?;
        let flags = var.i32_at(at + 4)?;
        let secure_info = var.i32_at(at + 8)?;
        let ref_con = var.i32_at(at + 12)?;
        let loc = Point::new(var.i16_at(at + 16)?, var.i16_at(at + 18)?);
        let id = var.i16_at(at + 20)?;
        let dest = var.i16_at(at + 22)?;
        let nbr_pts = var.i16_at(at + 24)?;
        let pts_ofst = var.i16_at(at + 26)?;
        let hotspot_type = var.i16_at(at + 28)?;
        let group_id = var.i16_at(at + 30)?;
        let nbr_scripts = var.i16_at(at + 32)?;
        let script_rec_ofst = var.i16_at(at + 34)?;
        let state = var.i16_at(at + 36)?;
        let nbr_states = var.i16_at(at + 38)?;
        let state_rec_ofst = var.i16_at(at + 40)?;
        let name_ofst = var.i16_at(at + 42)?;
        let script_text_ofst = var.i16_at(at + 44)?;
        let align_reserved = var.i16_at(at + 46)?;
        if align_reserved != 0 {
            warn.push(RoomWarning::ReservedNonZero {
                field: "Hotspot.alignReserved",
                value: align_reserved,
            });
        }

        let points = parse_points(var, nbr_pts, pts_ofst, warn);
        let states = parse_states(var, nbr_states, state_rec_ofst, warn);
        let name = var.optional_string_opt(name_ofst, "Hotspot.nameOfst", warn);
        let script = match script_text_ofst {
            0 => None,
            v if v < 0 => {
                warn.push(RoomWarning::NegativeOffset {
                    field: "Hotspot.scriptTextOfst",
                    value: v,
                });
                None
            }
            v => {
                let text = var.cstring(v as usize, "Hotspot.scriptTextOfst", warn);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
        };

        Some(Hotspot {
            script_event_mask,
            flags,
            secure_info,
            ref_con,
            loc,
            id,
            dest,
            nbr_pts,
            pts_ofst,
            hotspot_type,
            group_id,
            nbr_scripts,
            script_rec_ofst,
            state,
            nbr_states,
            state_rec_ofst,
            name_ofst,
            script_text_ofst,
            align_reserved,
            points,
            states,
            name,
            script,
        })
    }
}

fn parse_points(
    var: &Var<'_>,
    nbr_pts: i16,
    pts_ofst: i16,
    warn: &mut Vec<RoomWarning>,
) -> Vec<Point> {
    if nbr_pts <= 0 {
        return Vec::new();
    }
    let declared = nbr_pts as usize;
    let base = match array_base(pts_ofst, declared, "Hotspot.ptsOfst", warn) {
        Some(base) => base,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(declared.min(4096));
    for index in 0..declared {
        let Some(at) = index
            .checked_mul(POINT_LEN)
            .and_then(|delta| base.checked_add(delta))
        else {
            break;
        };
        if var.bytes_at(at, POINT_LEN).is_none() {
            warn.push(RoomWarning::ArrayTruncated {
                field: "Hotspot.ptsOfst",
                index,
                declared,
                available: var.len().saturating_sub(at),
            });
            break;
        }
        let y = var.i16_at(at).unwrap_or_default();
        let x = var.i16_at(at + 2).unwrap_or_default();
        out.push(Point::new(y, x));
    }
    out
}

fn parse_states(
    var: &Var<'_>,
    nbr_states: i16,
    state_rec_ofst: i16,
    warn: &mut Vec<RoomWarning>,
) -> Vec<HotspotState> {
    if nbr_states <= 0 {
        return Vec::new();
    }
    let declared = nbr_states as usize;
    let base = match array_base(state_rec_ofst, declared, "Hotspot.stateRecOfst", warn) {
        Some(base) => base,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(declared.min(1024));
    for index in 0..declared {
        let Some(at) = index
            .checked_mul(HOTSPOT_STATE_LEN)
            .and_then(|delta| base.checked_add(delta))
        else {
            break;
        };
        if var.bytes_at(at, HOTSPOT_STATE_LEN).is_none() {
            warn.push(RoomWarning::ArrayTruncated {
                field: "Hotspot.stateRecOfst",
                index,
                declared,
                available: var.len().saturating_sub(at),
            });
            break;
        }
        if let Some(state) = HotspotState::parse(var, at, warn) {
            out.push(state);
        }
    }
    out
}

fn parse_hotspots(var: &Var<'_>, header: &RoomRec, warn: &mut Vec<RoomWarning>) -> Vec<Hotspot> {
    let declared = header.nbr_hotspots.max(0) as usize;
    if declared == 0 {
        return Vec::new();
    }
    let base = match array_base(header.hotspot_ofst, declared, "hotspotOfst", warn) {
        Some(base) => base,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(declared.min(1024));
    for index in 0..declared {
        let Some(at) = index
            .checked_mul(HOTSPOT_LEN)
            .and_then(|delta| base.checked_add(delta))
        else {
            break;
        };
        if var.bytes_at(at, HOTSPOT_LEN).is_none() {
            warn.push(RoomWarning::ArrayTruncated {
                field: "hotspotOfst",
                index,
                declared,
                available: var.len().saturating_sub(at),
            });
            break;
        }
        if let Some(hotspot) = Hotspot::parse(var, at, warn) {
            out.push(hotspot);
        }
    }
    out
}

fn parse_loose_props(
    var: &Var<'_>,
    header: &RoomRec,
    warn: &mut Vec<RoomWarning>,
) -> Vec<LooseProp> {
    let declared = header.nbr_lprops.max(0) as usize;
    if declared == 0 {
        return Vec::new();
    }
    let Some(mut cursor) = linked_base(header.first_lprop, declared, "firstLProp", warn) else {
        return Vec::new();
    };

    let mut out: Vec<LooseProp> = Vec::with_capacity(declared.min(1024));
    let mut visited: Vec<usize> = Vec::with_capacity(declared.min(1024));
    let mut stopped_reason = false;
    let mut used_fallback = false;

    for index in 0..declared {
        // Guard: if the declared count cannot fit in varBuf at this stride, stop.
        if var.bytes_at(cursor, LOOSE_PROP_LEN).is_none() {
            warn.push(RoomWarning::OffsetOutOfRange {
                field: "firstLProp",
                offset: to_i16(cursor),
                var_len: var.len(),
            });
            stopped_reason = true;
            break;
        }
        if visited.contains(&cursor) {
            warn.push(RoomWarning::LinkedCycle {
                field: "firstLProp",
                offset: to_i16(cursor),
            });
            stopped_reason = true;
            break;
        }
        visited.push(cursor);

        // Bounds verified above.
        let next_ofst = var.i16_at(cursor).unwrap_or_default();
        let reserved = var.i16_at(cursor + 2).unwrap_or_default();
        let spec = LoosePropSpec {
            id: var.u32_at(cursor + 4).unwrap_or_default(),
            crc: var.u32_at(cursor + 8).unwrap_or_default(),
        };
        let flags = var.i32_at(cursor + 12).unwrap_or_default();
        let ref_con = var.i32_at(cursor + 16).unwrap_or_default();
        let loc = Point::new(
            var.i16_at(cursor + 20).unwrap_or_default(),
            var.i16_at(cursor + 22).unwrap_or_default(),
        );
        if reserved != 0 {
            warn.push(RoomWarning::ReservedNonZero {
                field: "LPropRec.link.reserved",
                value: reserved,
            });
        }
        out.push(LooseProp {
            next_ofst,
            reserved,
            spec,
            flags,
            ref_con,
            loc,
        });

        if next_ofst > 0 {
            cursor = next_ofst as usize;
        } else if index + 1 < declared {
            // A `0` link with records still declared: pserver packs records
            // contiguously and never fills the link, so fall back to the
            // struct stride rather than giving up.
            let packed = cursor.saturating_add(LOOSE_PROP_LEN);
            if var.bytes_at(packed, LOOSE_PROP_LEN).is_some() {
                if !used_fallback {
                    warn.push(RoomWarning::LinkedFallback {
                        field: "firstLProp",
                        offset: to_i16(cursor),
                        stride: LOOSE_PROP_LEN,
                    });
                    used_fallback = true;
                }
                cursor = packed;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if !stopped_reason && out.len() < declared {
        warn.push(RoomWarning::LinkedShort {
            field: "firstLProp",
            declared,
            parsed: out.len(),
        });
    }
    out
}

fn parse_draw_cmds(var: &Var<'_>, header: &RoomRec, warn: &mut Vec<RoomWarning>) -> Vec<DrawCmd> {
    let declared = header.nbr_draw_cmds.max(0) as usize;
    if declared == 0 {
        return Vec::new();
    }
    let Some(mut cursor) = linked_base(header.first_draw_cmd, declared, "firstDrawCmd", warn)
    else {
        return Vec::new();
    };

    let mut out: Vec<DrawCmd> = Vec::with_capacity(declared.min(1024));
    let mut visited: Vec<usize> = Vec::with_capacity(declared.min(1024));
    let mut stopped_reason = false;
    let mut used_fallback = false;

    for index in 0..declared {
        if var.bytes_at(cursor, DRAW_CMD_HEADER_LEN).is_none() {
            warn.push(RoomWarning::OffsetOutOfRange {
                field: "firstDrawCmd",
                offset: to_i16(cursor),
                var_len: var.len(),
            });
            stopped_reason = true;
            break;
        }
        if visited.contains(&cursor) {
            warn.push(RoomWarning::LinkedCycle {
                field: "firstDrawCmd",
                offset: to_i16(cursor),
            });
            stopped_reason = true;
            break;
        }
        visited.push(cursor);

        let next_ofst = var.i16_at(cursor).unwrap_or_default();
        let reserved = var.i16_at(cursor + 2).unwrap_or_default();
        let encoded = var.u16_at(cursor + 4).unwrap_or_default();
        let cmd_length = var.u16_at(cursor + 6).unwrap_or_default();
        let data_ofst = var.i16_at(cursor + 8).unwrap_or_default();
        if reserved != 0 {
            warn.push(RoomWarning::ReservedNonZero {
                field: "DrawRecord.link.reserved",
                value: reserved,
            });
        }
        let command = (encoded & 0xFF) as u8;
        let flags = (encoded >> 8) as u8;

        // The operand always follows the header, whatever `data_ofst` says.
        let data_start = cursor.saturating_add(DRAW_CMD_HEADER_LEN);
        let data = match var.bytes_at(data_start, cmd_length as usize) {
            Some(slice) => slice.to_vec(),
            None => {
                warn.push(RoomWarning::DrawDataOutOfRange {
                    index,
                    record_ofst: to_i16(cursor),
                    data_ofst,
                    cmd_length,
                    available: var.len().saturating_sub(data_start),
                });
                vec![]
            }
        };
        let payload = match command {
            draw_cmd::PATH | draw_cmd::SHAPE | draw_cmd::ELLIPSE => {
                decode_draw_payload(index, &data, var.order, warn)
            }
            _ => None,
        };
        out.push(DrawCmd {
            next_ofst,
            reserved,
            command,
            flags,
            cmd_length,
            data_ofst,
            data,
            payload,
        });

        if next_ofst > 0 {
            cursor = next_ofst as usize;
        } else if index + 1 < declared {
            let packed = cursor
                .saturating_add(DRAW_CMD_HEADER_LEN)
                .saturating_add(cmd_length as usize);
            if var.bytes_at(packed, DRAW_CMD_HEADER_LEN).is_some() {
                if !used_fallback {
                    warn.push(RoomWarning::LinkedFallback {
                        field: "firstDrawCmd",
                        offset: to_i16(cursor),
                        stride: DRAW_CMD_HEADER_LEN + cmd_length as usize,
                    });
                    used_fallback = true;
                }
                cursor = packed;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if !stopped_reason && out.len() < declared {
        warn.push(RoomWarning::LinkedShort {
            field: "firstDrawCmd",
            declared,
            parsed: out.len(),
        });
    }
    out
}

/// Decode the operand of a `PATH`/`SHAPE`/`ELLIPSE` command.
///
/// Returns `None` (and warns) when the operand is too short to hold the full
/// point array; the raw bytes are always kept in [`DrawCmd::data`].
fn decode_draw_payload(
    index: usize,
    data: &[u8],
    order: ByteOrder,
    warn: &mut Vec<RoomWarning>,
) -> Option<DrawPayload> {
    if data.len() < DRAW_PAYLOAD_PREFIX {
        warn.push(RoomWarning::DrawPayloadTruncated {
            index,
            available: data.len(),
        });
        return None;
    }
    let pen_size = read_i16(data, order, 0)?;
    let num_points = read_i16(data, order, 2)?;
    if num_points < 0 {
        warn.push(RoomWarning::DrawPayloadTruncated {
            index,
            available: data.len(),
        });
        return None;
    }
    let count = num_points as usize + 1;
    let points_bytes = count.checked_mul(POINT_LEN)?;
    let needed = DRAW_PAYLOAD_PREFIX.checked_add(points_bytes)?;
    if data.len() < needed {
        warn.push(RoomWarning::DrawPayloadTruncated {
            index,
            available: data.len(),
        });
        return None;
    }
    let pen_rgb = [*data.get(4)?, *data.get(6)?, *data.get(8)?];
    let mut points = Vec::with_capacity(count.min(4096));
    for k in 0..count {
        let at = DRAW_PAYLOAD_PREFIX + k * POINT_LEN;
        points.push(Point::new(
            read_i16(data, order, at)?,
            read_i16(data, order, at + 2)?,
        ));
    }

    // Optional PC5 tail: line RGBA then fill RGBA.
    let mut line_rgba = None;
    let mut fill_rgba = None;
    if data.len() >= needed + 8 {
        line_rgba = Some([
            *data.get(needed)?,
            *data.get(needed + 1)?,
            *data.get(needed + 2)?,
            *data.get(needed + 3)?,
        ]);
        fill_rgba = Some([
            *data.get(needed + 4)?,
            *data.get(needed + 5)?,
            *data.get(needed + 6)?,
            *data.get(needed + 7)?,
        ]);
    }

    Some(DrawPayload {
        pen_size,
        num_points,
        pen_rgb,
        points,
        line_rgba,
        fill_rgba,
    })
}

/// Resolve the base offset of a fixed-stride array, warning when the offset is
/// absent, negative or out of range.
fn array_base(
    ofst: i16,
    count: usize,
    field: &'static str,
    warn: &mut Vec<RoomWarning>,
) -> Option<usize> {
    match ofst {
        0 => {
            warn.push(RoomWarning::AbsentOffset { field, count });
            None
        }
        v if v < 0 => {
            warn.push(RoomWarning::NegativeOffset { field, value: v });
            None
        }
        v => Some(v as usize),
    }
}

/// Resolve the first link of a linked list. `0` means the list is empty even
/// when a count was declared (some servers write `firstLProp = 0`); that is
/// reported, not guessed at.
fn linked_base(
    ofst: i16,
    count: usize,
    field: &'static str,
    warn: &mut Vec<RoomWarning>,
) -> Option<usize> {
    match ofst {
        0 => {
            warn.push(RoomWarning::AbsentOffset { field, count });
            None
        }
        v if v < 0 => {
            warn.push(RoomWarning::NegativeOffset { field, value: v });
            None
        }
        v => Some(v as usize),
    }
}

fn to_i16(value: usize) -> i16 {
    i16::try_from(value).unwrap_or(i16::MAX)
}

/// A bounds-checked view over `varBuf`.
///
/// Every accessor returns `None` rather than panicking; callers either verify
/// the range first or treat `None` as "absent". `varBuf` is at most
/// `i16::MAX` bytes (its length is carried in an `i16`), so every valid offset
/// fits back into the warning's `i16` fields.
#[derive(Clone, Copy)]
struct Var<'a> {
    buf: &'a [u8],
    order: ByteOrder,
}

impl<'a> Var<'a> {
    fn len(&self) -> usize {
        self.buf.len()
    }

    fn bytes_at(&self, at: usize, n: usize) -> Option<&'a [u8]> {
        let end = at.checked_add(n)?;
        self.buf.get(at..end)
    }

    fn i16_at(&self, at: usize) -> Option<i16> {
        read_i16(self.buf, self.order, at)
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        read_u16(self.buf, self.order, at)
    }

    fn i32_at(&self, at: usize) -> Option<i32> {
        read_i32(self.buf, self.order, at)
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        read_u32(self.buf, self.order, at)
    }

    /// Resolve a `PString` at a known-good offset. The caller has already
    /// handled `0` (absent) and negatives.
    fn pstring(&self, at: usize, field: &'static str, warn: &mut Vec<RoomWarning>) -> String {
        let Some(&len_byte) = self.buf.get(at) else {
            warn.push(RoomWarning::OffsetOutOfRange {
                field,
                offset: to_i16(at),
                var_len: self.len(),
            });
            return String::new();
        };
        let len = len_byte as usize;
        let start = at.saturating_add(1);
        if let Some(bytes) = self.bytes_at(start, len) {
            latin1(bytes)
        } else {
            let available = self.len().saturating_sub(start);
            warn.push(RoomWarning::StringTruncated {
                field,
                offset: to_i16(at),
                declared: len,
                available,
            });
            latin1(self.buf.get(start.min(self.len())..).unwrap_or_default())
        }
    }

    /// Read a NUL-terminated string, tolerating a missing terminator.
    fn cstring(&self, at: usize, field: &'static str, warn: &mut Vec<RoomWarning>) -> String {
        match self.buf.get(at..) {
            None => {
                warn.push(RoomWarning::OffsetOutOfRange {
                    field,
                    offset: to_i16(at),
                    var_len: self.len(),
                });
                String::new()
            }
            Some(rest) => match rest.iter().position(|&b| b == 0) {
                Some(end) => latin1(rest.get(..end).unwrap_or_default()),
                None => {
                    warn.push(RoomWarning::ScriptUnterminated {
                        field,
                        offset: to_i16(at),
                    });
                    latin1(rest)
                }
            },
        }
    }

    /// Resolve a possibly-absent `PString` offset into a `String` (`0` and
    /// negative both yield `""`).
    fn optional_string(
        &self,
        ofst: i16,
        field: &'static str,
        warn: &mut Vec<RoomWarning>,
    ) -> String {
        self.optional_string_opt(ofst, field, warn)
            .unwrap_or_default()
    }

    /// Resolve a possibly-absent `PString` offset into an `Option<String>`.
    fn optional_string_opt(
        &self,
        ofst: i16,
        field: &'static str,
        warn: &mut Vec<RoomWarning>,
    ) -> Option<String> {
        match ofst {
            0 => None,
            v if v < 0 => {
                warn.push(RoomWarning::NegativeOffset { field, value: v });
                None
            }
            v => Some(self.pstring(v as usize, field, warn)),
        }
    }
}

fn read_i16(buf: &[u8], order: ByteOrder, at: usize) -> Option<i16> {
    let end = at.checked_add(2)?;
    let bytes: [u8; 2] = buf.get(at..end)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => i16::from_le_bytes(bytes),
        ByteOrder::Big => i16::from_be_bytes(bytes),
    })
}

fn read_u16(buf: &[u8], order: ByteOrder, at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    let bytes: [u8; 2] = buf.get(at..end)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u16::from_le_bytes(bytes),
        ByteOrder::Big => u16::from_be_bytes(bytes),
    })
}

fn read_i32(buf: &[u8], order: ByteOrder, at: usize) -> Option<i32> {
    let end = at.checked_add(4)?;
    let bytes: [u8; 4] = buf.get(at..end)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => i32::from_le_bytes(bytes),
        ByteOrder::Big => i32::from_be_bytes(bytes),
    })
}

fn read_u32(buf: &[u8], order: ByteOrder, at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let bytes: [u8; 4] = buf.get(at..end)?.try_into().ok()?;
    Some(match order {
        ByteOrder::Little => u32::from_le_bytes(bytes),
        ByteOrder::Big => u32::from_be_bytes(bytes),
    })
}

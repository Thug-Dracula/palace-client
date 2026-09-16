//! Data model for a decoded `MSG_ROOMDESC` (`room`) payload.
//!
//! Every structure here is a faithful, byte-for-byte model of the Palace room
//! description: the 40-byte [`RoomRec`](palace_wire::messages::RoomRec) header
//! plus the variable-length buffer (`varBuf`) its offset fields point into.
//! Field names match the 1999 Communities.com protocol reference closely so the
//! two can be read side by side; see the crate root and `README.md` for the
//! layout table and the record sizes.
//!
//! Sub-structure size constants are public because they are protocol facts, not
//! implementation details:
//!
//! | Structure | Bytes |
//! |---|---|
//! | [`RoomRec`](palace_wire::messages::RoomRec) header | 40 |
//! | [`PictureOverlay`] | 12 |
//! | [`Hotspot`] | 48 |
//! | [`HotspotState`] | 8 |
//! | point (`Point` / `sint16 y`, `sint16 x`) | 4 |
//! | [`LooseProp`] | 24 (traversal is via `next_ofst`, not this stride) |
//! | [`DrawCmd`] header | 10, then `cmd_length` bytes of operand data |

use palace_wire::messages::{Point, RoomRec};

/// Size of the fixed [`RoomRec`] header that precedes `varBuf`.
pub const ROOM_REC_LEN: usize = RoomRec::LEN;

/// Size of one picture-overlay record (`PictureRec`).
pub const PICTURE_OVERLAY_LEN: usize = 12;

/// Size of one hotspot record.
pub const HOTSPOT_LEN: usize = 48;

/// Size of one hotspot-state record (`StateRec`).
pub const HOTSPOT_STATE_LEN: usize = 8;

/// Size of one polygon point (`Point` = `sint16 y`, `sint16 x`).
pub const POINT_LEN: usize = 4;

/// Size of a loose-prop record. Traversal is through the `next_ofst` link, so
/// this is informational: the live corpus spaces records 48 bytes apart while
/// pserver packs them at 24.
pub const LOOSE_PROP_LEN: usize = 24;

/// Size of a draw-command header. The `cmd_length` bytes of operand data
/// immediately follow it.
pub const DRAW_CMD_HEADER_LEN: usize = 10;

/// The draw-command operand immediately follows the 10-byte header regardless
/// of what the vestigial `data_ofst` field says — confirmed against pserver's
/// `Draw::Serialise` ("data offset is always 10") and QPalace's streaming
/// reader, both of which ignore `data_ofst`.
pub const DRAW_CMD_DATA_OFFSET: usize = DRAW_CMD_HEADER_LEN;

/// A decoded `MSG_ROOMDESC` (`room`) payload.
///
/// `var_data` is kept whole and unmodified, exactly as it arrived, so any part
/// this decoder did not understand (or got wrong) can be revisited without
/// re-capturing traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDesc {
    /// The fixed 40-byte header.
    pub header: RoomRec,
    /// Room name (`roomNameOfst`).
    pub name: String,
    /// Background-picture file name (`pictNameOfst`).
    pub picture: String,
    /// Artist name (`artistNameOfst`), empty when absent.
    pub artist: String,
    /// Room password (`passwordOfst`), empty when open.
    pub password: String,
    /// Image overlays, in array order (`pictureOfst`, `nbrPictures` entries).
    pub pictures: Vec<PictureOverlay>,
    /// Hotspots, in array order (`hotspotOfst`, `nbrHotspots` entries).
    pub hotspots: Vec<Hotspot>,
    /// Loose props, following `first_lprop` through `next_ofst`.
    pub loose_props: Vec<LooseProp>,
    /// Draw commands, following `first_draw_cmd` through `next_ofst`.
    pub draw_cmds: Vec<DrawCmd>,
    /// The raw `varBuf`, kept verbatim.
    pub var_data: Vec<u8>,
    /// Bytes that followed `varBuf` in the payload.
    ///
    /// For a single-record payload this is the alignment padding the reference
    /// client computes as `size - lenVars - 40` (the live server emits 4). When
    /// a capture concatenated two room frames it also contains the next
    /// record(s); [`crate::decode_stream`] uses it to split them.
    pub trailing_len: usize,
    /// Recoverable problems found while parsing. An empty vector means the
    /// record parsed exactly as declared.
    pub warnings: Vec<RoomWarning>,
}

impl RoomDesc {
    /// True when the record parsed with no recoverable problems.
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

/// One image-overlay record (`PictureRec`): an image stamped over the
/// background, selected by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PictureOverlay {
    /// `refCon` — arbitrary, unused by the server.
    pub ref_con: i32,
    /// Picture id; IPTSCRAE and hotspot states select overlays by this.
    pub pic_id: i16,
    /// Offset into `varBuf` of the file-name `PString` (`0` = absent).
    pub pic_name_ofst: i16,
    /// Transparency index. `-1` = none, `0` = "use the bottom-left pixel",
    /// `> 0` = palette index (OpenPalace `PalaceImageOverlay`).
    pub trans_color: i16,
    /// Alignment filler; should be `0`.
    pub reserved: i16,
    /// Resolved file name, or `None` when `pic_name_ofst` is absent.
    pub name: Option<String>,
}

/// One hotspot record: a clickable, scripted region.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hotspot {
    /// Bitmask of `PE_*` events this hotspot handles. The live corpus leaves
    /// this `0` (handlers are derived from the script text by the client).
    pub script_event_mask: i32,
    /// `HS_*` display/behaviour bits.
    pub flags: i32,
    /// Purpose unclear; unused by the server.
    pub secure_info: i32,
    /// Arbitrary use variable; unused by the server.
    pub ref_con: i32,
    /// Nominal location of the hotspot, absolute signed pixels.
    pub loc: Point,
    /// Hotspot id.
    pub id: i16,
    /// Destination room id for a door, or door id for a bolt.
    pub dest: i16,
    /// Number of polygon points (`nbrPts`).
    pub nbr_pts: i16,
    /// Offset into `varBuf` of the point array.
    pub pts_ofst: i16,
    /// `HS_*` type: 0 normal, 1 door, 2 shuttable door, 3 lockable door,
    /// 4 bolt, 5 navigation area.
    pub hotspot_type: i16,
    /// Group id; purpose unclear, unused.
    pub group_id: i16,
    /// Number of scripts (`nbrScripts`); unused by the server.
    pub nbr_scripts: i16,
    /// Script-record offset; documented as unused, no record layout known.
    pub script_rec_ofst: i16,
    /// Current selected state.
    pub state: i16,
    /// Number of states (`nbrStates`).
    pub nbr_states: i16,
    /// Offset into `varBuf` of the state array.
    pub state_rec_ofst: i16,
    /// Offset into `varBuf` of the name `PString`.
    pub name_ofst: i16,
    /// Offset into `varBuf` of the NUL-terminated script (`CString`).
    pub script_text_ofst: i16,
    /// Alignment filler; should be `0`.
    pub align_reserved: i16,
    /// Resolved polygon points, in wire order.
    pub points: Vec<Point>,
    /// Resolved states.
    pub states: Vec<HotspotState>,
    /// Resolved hotspot name, or `None` when `name_ofst` is absent.
    pub name: Option<String>,
    /// Resolved script text, or `None` when `script_text_ofst` is absent.
    pub script: Option<String>,
}

/// One hotspot-state record (`StateRec`): a picture and its offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotspotState {
    /// Picture id selected when this state is active.
    pub pict_id: i16,
    /// Alignment filler; should be `0`.
    pub reserved: i16,
    /// Picture location, interpreted by the protocol as an offset from the
    /// hotspot's own [`Hotspot::loc`].
    pub pic_loc: Point,
}

/// The `AssetSpec` inside a loose prop.
///
/// The 1999 reference declares `sint32 id`, but prop ids routinely exceed
/// `i32::MAX` (the live corpus contains e.g. `0xA26F9DE3`) and OpenPalace reads
/// the field unsigned, so the bit pattern is exposed as `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoosePropSpec {
    /// Asset (prop) id — a 32-bit unsigned namespace in practice.
    pub id: u32,
    /// Asset CRC. `0` in the live corpus.
    pub crc: u32,
}

/// One loose-prop record: a prop placed on the floor of the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LooseProp {
    /// Offset of the next loose prop in `varBuf`, or `0` at the end of the
    /// chain. The live server spaces records 48 bytes apart, but the link — not
    /// a fixed stride — is what the protocol defines, so that is what is used.
    pub next_ofst: i16,
    /// Alignment filler; `0`.
    pub reserved: i16,
    /// The prop's asset identity.
    pub spec: LoosePropSpec,
    /// `LP_*` flags.
    pub flags: i32,
    /// Arbitrary use variable.
    pub ref_con: i32,
    /// Screen location of the prop, absolute signed pixels.
    pub loc: Point,
}

/// Draw-command opcodes (`DC_*`), the low byte of the encoded command word.
pub mod draw_cmd {
    /// `DC_Path` — a polyline.
    pub const PATH: u8 = 0;
    /// `DC_Shape` — a polygon.
    pub const SHAPE: u8 = 1;
    /// `DC_Text` — text; operand format undocumented (kept raw).
    pub const TEXT: u8 = 2;
    /// `DC_Detonate` — delete every draw command.
    pub const DETONATE: u8 = 3;
    /// `DC_Delete` — delete the most recent draw command.
    pub const DELETE: u8 = 4;
    /// `DC_Ellipse` — an ellipse.
    pub const ELLIPSE: u8 = 5;
}

/// Draw-command flag bits (`DF_*`), the high byte of the encoded command word.
pub mod draw_flags {
    /// Draw behind everything (cleared) or in front (set).
    pub const LAYER_FRONT: u8 = 0x80;
    /// Fill the shape.
    pub const USE_FILL: u8 = 0x01;
    /// Disambiguates a shape between polygon and ellipse.
    pub const IS_ELLIPSE: u8 = 0x40;
}

/// One draw command (`DrawRecord` + its operand data).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DrawCmd {
    /// Offset of the next draw command in `varBuf`, or `0` at the end.
    pub next_ofst: i16,
    /// Alignment filler; `0`.
    pub reserved: i16,
    /// `DC_*` opcode (low byte of the encoded command word).
    pub command: u8,
    /// `DF_*` flags (high byte of the encoded command word).
    pub flags: u8,
    /// Length of the operand data.
    pub cmd_length: u16,
    /// The vestigial `dataOfst` field, kept raw. The server sets it to `10`;
    /// the operand always follows the header, so this is not used to locate it.
    pub data_ofst: i16,
    /// The raw operand bytes (`cmd_length` of them).
    pub data: Vec<u8>,
    /// Decoded geometry for `PATH`/`SHAPE`/`ELLIPSE`, when the operand is long
    /// enough. `None` for `DETONATE`/`DELETE` and for undecodable operands.
    pub payload: Option<DrawPayload>,
}

impl DrawCmd {
    /// True when this command draws a filled shape.
    pub fn is_filled(&self) -> bool {
        self.flags & draw_flags::USE_FILL != 0
    }

    /// True when this command draws on the front layer.
    pub fn is_front_layer(&self) -> bool {
        self.flags & draw_flags::LAYER_FRONT != 0
    }
}

/// The decoded operand of a `PATH`, `SHAPE` or `ELLIPSE` draw command.
///
/// Layout ported from OpenPalace's `PalaceDrawRecord.readData` and QPalace's
/// `operator>>(QDataStream&, QPDraw*)`, which agree byte for byte:
///
/// ```text
/// sint16 penSize
/// sint16 numPoints        // number of segments; (numPoints + 1) points follow
/// uint8  r, r, g, g, b, b // pen colour, each channel duplicated
/// Point  points[numPoints + 1]
/// uint8  lineAlpha, r, g, b   // optional (PC5)
/// uint8  fillAlpha, r, g, b   // optional (PC5)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DrawPayload {
    /// Pen width.
    pub pen_size: i16,
    /// Number of segments; the point array holds `num_points + 1` points.
    pub num_points: i16,
    /// Pen colour, `[r, g, b]`.
    pub pen_rgb: [u8; 3],
    /// Polygon vertices.
    pub points: Vec<Point>,
    /// Line colour + alpha (`[a, r, g, b]`) from the extended (PC5) tail.
    pub line_rgba: Option<[u8; 4]>,
    /// Fill colour + alpha (`[a, r, g, b]`) from the extended (PC5) tail.
    pub fill_rgba: Option<[u8; 4]>,
}

/// A recoverable problem found while walking the offset graph.
///
/// These never abort a decode: the affected sub-structure is skipped (or
/// truncated to what is available) and the raw bytes stay in
/// [`RoomDesc::var_data`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomWarning {
    /// A header offset was negative where `0` (absent) or a positive index was
    /// expected.
    NegativeOffset { field: &'static str, value: i16 },
    /// An offset pointed at or past the end of `varBuf`.
    OffsetOutOfRange {
        field: &'static str,
        offset: i16,
        var_len: usize,
    },
    /// An array's declared count did not fit in `varBuf`; it was truncated.
    ArrayTruncated {
        field: &'static str,
        index: usize,
        declared: usize,
        available: usize,
    },
    /// A count was non-zero but its corresponding offset was `0` (absent).
    AbsentOffset { field: &'static str, count: usize },
    /// A linked list revisited an offset: corrupt link, traversal stopped.
    LinkedCycle { field: &'static str, offset: i16 },
    /// A linked list ended (or hit a bound) before the declared count.
    LinkedShort {
        field: &'static str,
        declared: usize,
        parsed: usize,
    },
    /// A linked list's `next` link was `0` before the declared count was
    /// reached, so traversal fell back to a packed stride (pserver writes
    /// `0` links and packs records contiguously).
    LinkedFallback {
        field: &'static str,
        offset: i16,
        stride: usize,
    },
    /// A `PString`'s length byte claimed more bytes than remain; the available
    /// bytes were decoded.
    StringTruncated {
        field: &'static str,
        offset: i16,
        declared: usize,
        available: usize,
    },
    /// A hotspot script `CString` had no NUL before the end of `varBuf`.
    ScriptUnterminated { field: &'static str, offset: i16 },
    /// A filler/reserved field was non-zero.
    ReservedNonZero { field: &'static str, value: i16 },
    /// A draw command's operand ran past the end of `varBuf`.
    DrawDataOutOfRange {
        index: usize,
        record_ofst: i16,
        data_ofst: i16,
        cmd_length: u16,
        available: usize,
    },
    /// A draw operand was too short to decode its geometry.
    DrawPayloadTruncated { index: usize, available: usize },
}

impl std::fmt::Display for RoomWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoomWarning::NegativeOffset { field, value } => {
                write!(f, "{field} is negative ({value})")
            }
            RoomWarning::OffsetOutOfRange {
                field,
                offset,
                var_len,
            } => write!(
                f,
                "{field} = {offset} is outside varBuf (len {var_len})"
            ),
            RoomWarning::ArrayTruncated {
                field,
                index,
                declared,
                available,
            } => write!(
                f,
                "{field} array truncated at index {index} of {declared} ({available} bytes left)"
            ),
            RoomWarning::AbsentOffset { field, count } => {
                write!(f, "{field} is 0 but {count} entries were declared")
            }
            RoomWarning::LinkedCycle { field, offset } => {
                write!(f, "{field} linked list revisits offset {offset} (cycle)")
            }
            RoomWarning::LinkedShort {
                field,
                declared,
                parsed,
            } => write!(
                f,
                "{field} linked list ended after {parsed} of {declared} records"
            ),
            RoomWarning::LinkedFallback {
                field,
                offset,
                stride,
            } => write!(
                f,
                "{field} link at {offset} was 0; fell back to packed stride {stride}"
            ),
            RoomWarning::StringTruncated {
                field,
                offset,
                declared,
                available,
            } => write!(
                f,
                "{field} at {offset} declares {declared} bytes but only {available} remain"
            ),
            RoomWarning::ScriptUnterminated { field, offset } => {
                write!(f, "{field} at {offset} has no NUL terminator")
            }
            RoomWarning::ReservedNonZero { field, value } => {
                write!(f, "{field} is non-zero ({value})")
            }
            RoomWarning::DrawDataOutOfRange {
                index,
                record_ofst,
                data_ofst,
                cmd_length,
                available,
            } => write!(
                f,
                "draw[{index}] at {record_ofst}: data {data_ofst}+{cmd_length} exceeds {available} bytes"
            ),
            RoomWarning::DrawPayloadTruncated { index, available } => write!(
                f,
                "draw[{index}] operand too short to decode ({available} bytes)"
            ),
        }
    }
}

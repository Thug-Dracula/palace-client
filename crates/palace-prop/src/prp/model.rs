//! The four fixed-layout structures that make up a `.prp` asset map.
//!
//! Field order and width come straight from `PRP-FORMAT.md` §2. Every struct is
//! `#[repr(C)]` so its in-memory size *is* its on-disk size: 16, 24, 12 and 32
//! bytes respectively. That is load-bearing — the reader slices the file by these
//! sizes and the writer must reproduce them byte for byte — so the sizes are
//! pinned by `tests/prp_model.rs`.
//!
//! All integers are little-endian. Names are snake_case versions of the SDK's
//! camelCase fields (`dataOffset` → `data_offset`); the byte layout is unchanged.

/// Bytes in the file header, and therefore the file offset at which the blob
/// region begins. Derived from the struct so the two can never drift.
pub const FILE_HEADER_LEN: usize = std::mem::size_of::<AssetFileHeader>();

/// The 16-byte file header at offset 0 (spec §2.1).
///
/// ```text
/// +0x00 data_offset       where the blob region starts (= 16)
/// +0x04 data_size         bytes of blob region
/// +0x08 asset_map_offset  data_offset + data_size
/// +0x0c asset_map_size    bytes of the map (header + types + records + names)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct AssetFileHeader {
    /// Offset of the blob region; the 16-byte header size in every file observed.
    pub data_offset: u32,
    /// Length in bytes of the blob region.
    pub data_size: u32,
    /// Offset of the asset map; `data_offset + data_size`.
    pub asset_map_offset: u32,
    /// Length in bytes of the asset map.
    pub asset_map_size: u32,
}

impl AssetFileHeader {
    /// The file-size invariant from spec §2: the map ends exactly at EOF.
    ///
    /// `filesize == asset_map_offset + asset_map_size` must hold for a file that
    /// has not been truncated or extended. If it does not, the map ran past the
    /// buffer (truncated) or the file has trailing bytes after the map (which the
    /// format allows to be garbage, so this is a *check*, not a repair).
    #[must_use]
    pub fn invariant_holds(&self, filesize: usize) -> bool {
        u64::from(self.asset_map_offset) + u64::from(self.asset_map_size) == filesize as u64
    }
}

/// The 24-byte map header, the first structure at `asset_map_offset` (spec §2.2).
///
/// Offsets in it are **map-relative**: measured from the start of the asset map,
/// not the file. `types_offset` is always 24, `recs_offset` follows the type
/// records and `names_offset` follows the 32-byte records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct AssetMapHeader {
    /// Number of [`AssetTypeRec`] entries.
    pub nbr_types: i32,
    /// Total number of [`AssetRec`] entries across every type.
    pub nbr_assets: i32,
    /// Length in bytes of the names blob.
    pub len_names: i32,
    /// Map-relative offset of the type records (= 24).
    pub types_offset: u32,
    /// Map-relative offset of the asset records.
    pub recs_offset: u32,
    /// Map-relative offset of the names blob.
    pub names_offset: u32,
}

/// A 12-byte asset-type record (spec §2.3).
///
/// [`Self::asset_type`] holds the raw 4CC *value* rather than an [`AssetType`]
/// because the enum is 8 bytes wide and keeping the raw 4-byte field is what makes
/// this struct exactly 12 bytes. [`Self::kind`] converts on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct AssetTypeRec {
    /// 4CC as stored: the big-endian reading of the four characters, stored
    /// little-endian (`"Prop"` → `0x5072_6F70`). Unknown values are legal.
    pub asset_type: i32,
    /// Number of records of this type.
    pub nbr_assets: i32,
    /// Index of this type's first record in the [`AssetRec`] array.
    pub first_asset: i32,
}

impl AssetTypeRec {
    /// The 4CC as a named [`AssetType`], or [`AssetType::Unknown`].
    #[must_use]
    pub fn kind(&self) -> AssetType {
        AssetType::from_raw(self.asset_type as u32)
    }
}

/// A 32-bit little-endian 4CC as used by [`AssetTypeRec::asset_type`].
///
/// The four characters are read as a big-endian integer and then stored
/// little-endian, so the numeric constants match [`Self::from_fourcc`] (`"Prop"`
/// is `0x5072_6F70`) even though the bytes on disk are `50 72 6F 70`.
///
/// The last variant carries the raw value so an unrecognised type can be
/// preserved and written back verbatim rather than losing its identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetType {
    /// `"Prop"` — the props actually served.
    Prop,
    /// `"Fave"` — a single sentinel record in real files (spec §7).
    Fave,
    /// `"User"` — user-defined asset type.
    User,
    /// `"IUsr"` — another user-defined asset type.
    IUsr,
    /// Any other 4CC, kept verbatim.
    Unknown(u32),
}

impl AssetType {
    /// `"Prop"` as stored.
    pub const PROP: u32 = 0x5072_6F70;
    /// `"Fave"` as stored.
    pub const FAVE: u32 = 0x4661_7665;
    /// `"User"` as stored.
    pub const USER: u32 = 0x5573_6572;
    /// `"IUsr"` as stored.
    pub const IUSR: u32 = 0x4955_7372;

    /// Classify a raw stored 4CC value. Never fails; anything unrecognised
    /// becomes [`AssetType::Unknown`] carrying the original value.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            Self::PROP => AssetType::Prop,
            Self::FAVE => AssetType::Fave,
            Self::USER => AssetType::User,
            Self::IUSR => AssetType::IUsr,
            other => AssetType::Unknown(other),
        }
    }

    /// Read a 4CC from its four characters, e.g. `*b"Prop"`.
    #[must_use]
    pub const fn from_fourcc(fourcc: [u8; 4]) -> Self {
        Self::from_raw(u32::from_be_bytes(fourcc))
    }

    /// The raw stored value, the exact inverse of [`Self::from_raw`].
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        match self {
            AssetType::Prop => Self::PROP,
            AssetType::Fave => Self::FAVE,
            AssetType::User => Self::USER,
            AssetType::IUsr => Self::IUSR,
            AssetType::Unknown(raw) => raw,
        }
    }

    /// A short human-readable label for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            AssetType::Prop => "Prop",
            AssetType::Fave => "Fave",
            AssetType::User => "User",
            AssetType::IUsr => "IUsr",
            AssetType::Unknown(_) => "Unknown",
        }
    }
}

/// A 32-byte asset record (spec §2.4). This is the record the server binary
/// searches and the writer must reproduce exactly.
///
/// Two of the fields are runtime state that is always zero on disk and must not
/// be interpreted:
///
/// * [`Self::r_handle`] — a loaded-asset handle, cleared to 0 on save;
/// * [`Self::flags`] — the server's runtime asset flags (`0x02` loaded,
///   `0x20` failed validation, …). **This is not the prop's flag word.** The
///   prop's own `HEAD`/`GHOST`/format bits live in the 12-byte blob header, which
///   the reader exposes via `PropRecord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct AssetRec {
    /// Prop id the scripts reference. Compared **signed**; duplicate ids are
    /// legal (see [`crate::prp::PropKey`]).
    pub id: i32,
    /// Runtime asset handle; always 0 on disk.
    pub r_handle: i32,
    /// Data-relative blob offset: the blob is at file offset
    /// `FILE_HEADER_LEN + data_offset`, **not** `data_offset`.
    pub data_offset: u32,
    /// Blob length in bytes, *including* the 12-byte blob header.
    pub data_size: u32,
    /// Timestamp-ish field, not validated; values in the wild use two different
    /// epochs (spec §7). Only preserved, never trusted.
    pub last_use_time: i32,
    /// Byte offset of the name entry inside the names blob, or `-1` when unnamed.
    pub name_offset: i32,
    /// Runtime asset flags (0 on disk); *not* the prop's flag word.
    pub flags: u32,
    /// CRC of the blob **after** its 12-byte header. The last field, not the
    /// one at `+0x10`.
    pub crc: u32,
}

impl AssetRec {
    /// Whether [`Self::name_offset`] marks the record unnamed (`-1`).
    #[must_use]
    pub const fn is_unnamed(&self) -> bool {
        self.name_offset < 0
    }

    /// File offset of this record's blob: `FILE_HEADER_LEN + data_offset`.
    #[must_use]
    pub const fn blob_file_offset(&self) -> u64 {
        FILE_HEADER_LEN as u64 + self.data_offset as u64
    }
}

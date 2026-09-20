//! Pure invariants of the `.prp` data model.
//!
//! These tests never touch a file: `Roster::parse`/`write` are stubs until tasks
//! T2/T8, so what can be pinned now is the contract the reader and writer build
//! on — the exact struct sizes, the file-size equation, signed-id ordering, the
//! encoding selector and graceful handling of an unknown 4CC. A change to any of
//! these breaks byte-exact reading and writing, so they fail loudly here first.

use std::mem::size_of;

use palace_prop::prp::{
    AssetFileHeader, AssetMapHeader, AssetRec, AssetType, AssetTypeRec, PropEncoding, PropHeader,
    PropKey,
};

#[test]
fn the_on_disk_structs_have_their_exact_sizes() {
    // Each size is pinned on its own rather than as a sum, so a failure names the
    // struct whose layout drifted.
    assert_eq!(size_of::<AssetFileHeader>(), 16, "AssetFileHeader");
    assert_eq!(size_of::<AssetMapHeader>(), 24, "AssetMapHeader");
    assert_eq!(size_of::<AssetTypeRec>(), 12, "AssetTypeRec");
    assert_eq!(size_of::<AssetRec>(), 32, "AssetRec");
    assert_eq!(size_of::<PropHeader>(), 12, "PropHeader");
}

#[test]
fn the_filesize_invariant_matches_the_worked_example() {
    // PRP-FORMAT.md §9: a 317,301-byte file with
    // assetMapOffset = 302,428 and assetMapSize = 14,873.
    let file = AssetFileHeader {
        data_offset: 16,
        data_size: 302_412,
        asset_map_offset: 302_428,
        asset_map_size: 14_873,
    };
    assert_eq!(
        u64::from(file.asset_map_offset) + u64::from(file.asset_map_size),
        317_301
    );
    assert!(file.invariant_holds(317_301));
    // One byte short (truncated) or one byte long (trailing garbage) both fail.
    assert!(!file.invariant_holds(317_300));
    assert!(!file.invariant_holds(317_302));

    // The invariant also holds for an empty tail: map ending exactly at EOF.
    let empty = AssetFileHeader {
        data_offset: 16,
        data_size: 0,
        asset_map_offset: 16,
        asset_map_size: 24,
    };
    assert!(empty.invariant_holds(40));
}

#[test]
fn the_id_comparator_is_signed_not_unsigned() {
    // Roster ids are compared as signed 32-bit values because the server
    // binary-searches with a signed compare (PRP-FORMAT.md §5). The failure mode
    // is treating them as unsigned, which only shows up once an id has the high
    // bit set: 0xFFFF_FFFE is -2 signed but ~4.29 billion unsigned.
    //
    // 0x3A3AD1F7 is the real 44x44 avatar id and is +976,933,367 — positive under
    // either reading — so it correctly sorts *after* a small positive id. It is
    // included because it is the id the format's worked example pins.
    let high_bit = PropKey::new(0xFFFF_FFFEu32 as i32, 0);
    let small = PropKey::new(1, 0);
    let avatar = PropKey::new(0x3A3A_D1F7u32 as i32, 0);
    assert_eq!(high_bit.id, -2);
    assert!(avatar.id > 0);

    let mut ordered = vec![avatar, small, high_bit];
    ordered.sort();
    assert_eq!(ordered, vec![high_bit, small, avatar]);
    assert!(high_bit < small, "a high-bit id must sort first, signed");
    assert!(small < avatar, "0x3A3AD1F7 is positive and sorts after 1");

    // The unsigned interpretation would have produced [small, high_bit, avatar],
    // i.e. it would place the high-bit id after every positive one. Asserting the
    // two orders differ is what pins "signed, not unsigned".
    let mut unsigned = vec![small, high_bit, avatar];
    unsigned.sort_by_key(|key| key.id as u32);
    assert_eq!(unsigned, vec![small, avatar, high_bit]);
    assert_ne!(ordered, unsigned);
}

#[test]
fn crc_breaks_ties_between_variants_of_one_id() {
    // Duplicate ids are legal, so within one id the crc must give a stable order
    // and keep the variant chain contiguous.
    let first = PropKey::new(7, 0x0000_0001);
    let second = PropKey::new(7, 0x0000_0002);
    assert!(first < second);
    assert_eq!(
        first.cmp(&PropKey::new(7, 0x0000_0001)),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn an_unknown_fourcc_is_unknown_and_never_panics() {
    for raw in [0x4445_4144u32, 0x0000_0000, 0x0000_0001, 0xFFFF_FFFF] {
        assert_eq!(AssetType::from_raw(raw), AssetType::Unknown(raw));
        assert_eq!(AssetType::from_raw(raw).to_raw(), raw);
    }

    // The four known 4CCs still resolve, and round-trip to their stored value.
    assert_eq!(AssetType::from_fourcc(*b"Prop"), AssetType::Prop);
    assert_eq!(AssetType::from_fourcc(*b"Fave"), AssetType::Fave);
    assert_eq!(AssetType::from_fourcc(*b"User"), AssetType::User);
    assert_eq!(AssetType::from_fourcc(*b"IUsr"), AssetType::IUsr);
    assert_eq!(AssetType::Prop.to_raw(), 0x5072_6F70);
    assert_eq!(AssetType::Fave.to_raw(), 0x4661_7665);

    // A type record with an unknown 4CC classifies without panicking.
    let rec = AssetTypeRec {
        asset_type: 0x4445_4144,
        nbr_assets: 1,
        first_asset: 0,
    };
    assert_eq!(rec.kind(), AssetType::Unknown(0x4445_4144));
}

#[test]
fn the_encoding_is_selected_by_the_blob_flag_bits() {
    assert_eq!(PropEncoding::from_flags(0x0000), PropEncoding::EightBit);
    assert_eq!(PropEncoding::from_flags(0x0200), PropEncoding::S20Bit);
    assert_eq!(PropEncoding::from_flags(0x0100), PropEncoding::ThirtyTwoBit);
    assert_eq!(PropEncoding::from_flags(0x0040), PropEncoding::TwentyBit);

    // All three format bits set is the 16-bit special case, not S20: the 16-bit
    // test comes first and must win.
    assert_eq!(PropEncoding::from_flags(0xFF80), PropEncoding::SixteenBit);
    assert_eq!(PropEncoding::from_flags(0xFFA0), PropEncoding::SixteenBit);
    assert_eq!(PropEncoding::from_flags(0x0340), PropEncoding::S20Bit);
}

#[test]
fn the_raw_blob_header_is_twelve_little_endian_bytes() {
    // A real 44x44 8-bit head prop header (fixtures/8bit_head_rare.bin):
    // width 44, height 44, h/v offset 7, script 0, flags 0x000a (HEAD|RARE).
    // This is self-consistent, unlike the §9 example line, which prints the
    // avatar's last word as `02 00` yet labels it 0x0200 (little-endian `02 00`
    // is 0x0002; 0x0200 would be stored `00 02`).
    let raw = [
        0x2c, 0x00, 0x2c, 0x00, 0x07, 0x00, 0x07, 0x00, 0x00, 0x00, 0x0a, 0x00,
    ];
    let header = PropHeader::parse(&raw).expect("12 bytes is a header");
    assert_eq!((header.width, header.height), (44, 44));
    assert_eq!((header.h_offset, header.v_offset), (7, 7));
    assert_eq!(header.flags, 0x000a);
    assert_eq!(header.encoding(), PropEncoding::EightBit);
    assert!(header.is_head());
    assert!(!header.is_ghost());
    assert_eq!(header.encode(), raw);

    // An S20 header round-trips too: 0x0200 is stored little-endian as `00 02`.
    let s20 = PropHeader {
        width: 44,
        height: 44,
        flags: 0x0200,
        ..PropHeader::default()
    };
    let bytes = s20.encode();
    assert_eq!(&bytes[10..], &[0x00, 0x02]);
    assert_eq!(PropHeader::parse(&bytes), Ok(s20));
    assert_eq!(s20.encoding(), PropEncoding::S20Bit);

    // One byte short is an error, not a panic.
    assert!(PropHeader::parse(&raw[..11]).is_err());
}

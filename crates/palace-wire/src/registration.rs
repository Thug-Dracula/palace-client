//! The client-generated registration-code / PUID pair.
//!
//! Ported from OpenPalace's `RegistrationCode.as`, cross-checked against Taj's
//! `PalaceRegistration.cs` (an independent C# implementation of the same
//! algorithm). [`RegistrationCode::generate`] is what the reference client uses
//! to mint the PUID it persists per install; see [`crate::messages::Puid`] for
//! how the pair maps onto the logon record.
//!
//! The two constants and the 256-entry mask are protocol data, not tunables.

/// CRC seed constant.
pub const CRC_MAGIC: u32 = 0xa95a_de76;
/// Counter mix constant.
pub const MAGIC_LONG: u32 = 0x9602_c9bf;

#[rustfmt::skip]
const CRCMASK: [u32; 256] = [
    0xEBE1_9B94, 0x7604_DE74, 0xE3F9_D651, 0x604F_D612,
    0xE889_7C2C, 0xADC4_0920, 0x37EC_DFB7, 0x3349_89ED,
    0x2834_C33B, 0x8BD2_FE15, 0xCBF0_01A7, 0xBD96_B9D6,
    0x315E_2CE0, 0x4F16_7884, 0xA489_B1B6, 0xA51C_7A62,
    0x5462_2636, 0x0BC0_16FC, 0x68DE_2D22, 0x3C9D_304C,
    0x44FD_06FB, 0xBBB3_F772, 0xD637_E099, 0x849A_A9F9,
    0x5F24_0988, 0xF837_3BB7, 0x3037_9087, 0xC772_2864,
    0xB0A2_A643, 0xE331_6071, 0x956F_ED7C, 0x966F_937D,
    0x9945_AE16, 0xF0B2_37CE, 0x2234_79A0, 0xD835_9782,
    0x05AE_1B89, 0xE365_3292, 0xC34E_EA0D, 0x2691_DFC2,
    0xE914_5F51, 0xD9AA_7F35, 0xC7C4_344E, 0x4370_EBA1,
    0x1E43_833E, 0x634B_CF18, 0x0C50_E26B, 0x0649_2118,
    0xF78B_8BFE, 0x5F2B_B95C, 0xA3EB_54A6, 0x1E15_A2F0,
    0x6CC0_1887, 0xDE4E_7405, 0x1C1D_7374, 0x8575_7FEB,
    0xE372_517E, 0x9B99_79C7, 0xF378_07E8, 0x18F9_7235,
    0x645A_149B, 0x9556_C6CF, 0xF389_119E, 0x1D6C_BF85,
    0xA976_0CE5, 0xA985_C5FF, 0x5F4D_B574, 0x1317_6CAC,
    0x2F14_AA85, 0xF520_832C, 0xD21E_E917, 0x6F30_7A5B,
    0xC1FB_01C6, 0x1941_5378, 0x797F_A2C3, 0x24F4_2481,
    0x4F65_2C30, 0x39BC_02ED, 0x11ED_A1D7, 0x8C79_A136,
    0x6BD3_7A86, 0x80B3_54EE, 0xC424_E066, 0xAAE1_6427,
    0x6BD3_BE12, 0x868D_8E37, 0xD1D4_3C54, 0x4D62_081F,
    0x4330_56D7, 0xF2E4_CB02, 0x043F_C5A2, 0x9DA5_8CA4,
    0x1ED6_3321, 0x2067_9F26, 0xB38A_4758, 0x8464_19F7,
    0x6BDC_6352, 0xABF2_C24D, 0x40AC_386C, 0x2758_8588,
    0x5E1A_B2E5, 0x76BD_EAD4, 0x7144_4D32, 0x02FC_6084,
    0x92DB_41FB, 0xEF86_BAEB, 0xF7D8_572A, 0xB75A_EABF,
    0x84DC_5C93, 0xCBC1_3881, 0x641D_6E73, 0x0CB2_7A99,
    0xDED3_69A6, 0x617E_5DFA, 0x248B_D13E, 0xB859_6D66,
    0x9B36_A9FA, 0x52ED_AF1C, 0x3C65_9784, 0x146D_F599,
    0x109F_CAE8, 0xC9ED_4841, 0xBF59_3F49, 0xC94A_6E73,
    0x5AFA_0D2F, 0xB203_5002, 0xCAB3_1104, 0x7C4F_5A82,
    0xEAC9_3638, 0x63FC_5385, 0xDF0C_AE06, 0x26E5_5BE3,
    0x2921_B9B8, 0xB80B_3408, 0x917E_137D, 0x127A_48BC,
    0xE031_858A, 0x7222_13D7, 0x2DBC_96FA, 0x5359_F112,
    0xAB25_6019, 0x6E2A_756E, 0x4DC6_2F76, 0x2688_32DE,
    0x5980_E578, 0xD338_B668, 0xEEE2_E4D7, 0x1FFF_8FC6,
    0x9B17_ED10, 0xF3E6_BE0F, 0xC1BA_9D78, 0xBB86_93C5,
    0x24D5_7EC0, 0x5D64_0AED, 0xEE87_979B, 0x9632_3E11,
    0xCCBC_1601, 0x0E83_F43B, 0x2C2F_7495, 0x5F15_0B2A,
    0x710A_77E2, 0x281B_51DC, 0x2385_D03C, 0x6723_9BFF,
    0xA719_E8F9, 0x21C3_B9DE, 0x2648_9C22, 0x0DE6_8989,
    0xCA75_8F0D, 0x417E_8CD2, 0x67ED_61F8, 0xD15F_C001,
    0x3BA2_F272, 0x57E2_F7A9, 0xE723_B883, 0x914E_43E1,
    0x71AA_5B97, 0xFCEB_1BE1, 0x7FFA_4FD9, 0x67A0_B494,
    0x5E1C_741E, 0xC8C2_A5E6, 0xE13B_A068, 0x2452_5548,
    0x397A_9CF6, 0x3DDD_D4D6, 0xB626_234C, 0x39E7_B04D,
    0x36CA_279F, 0x89AE_A387, 0xCFE9_3789, 0x04E1_761B,
    0x9D62_0EDC, 0x6E9D_F1E7, 0x4A15_DFA6, 0xD446_41AC,
    0x3979_6769, 0x6D06_2637, 0xF967_AF35, 0xDDB4_A233,
    0x4840_7280, 0xA9F2_2E7E, 0xD987_8F67, 0xA05B_3BC1,
    0xE8C9_237A, 0x81CE_C53E, 0x4BE5_3E70, 0x6030_8E5E,
    0xF03D_E922, 0xA712_AF7B, 0xBB61_68B4, 0xCC6C_15B5,
    0x2F20_2775, 0x3045_27E3, 0xD32B_C1E6, 0xBA95_8058,
    0xA01F_7214, 0xC6E8_D190, 0xAB96_F14B, 0x1866_9984,
    0x4F93_A385, 0x403B_5B40, 0x5807_55F1, 0x59DE_50E8,
    0xF746_729F, 0xFF6F_7D47, 0x8022_EA34, 0xB24B_0BCD,
    0xF687_A7CC, 0x7E95_BAB3, 0x8DC1_583D, 0x0B44_3FE9,
    0xE6E4_5618, 0x224D_746F, 0xF306_24BB, 0xB742_7258,
    0xC78E_19BF, 0xD1EE_98A6, 0x66BE_7D3A, 0x791E_342F,
    0x68CB_AAB0, 0xBBB5_355D, 0x8DDA_9081, 0xDC27_36DC,
    0x5733_55AD, 0xC3FF_EC65, 0xE97F_0270, 0xC6A2_65E8,
    0xD9D4_9152, 0x4BB3_5BDB, 0xA1C7_BBE6, 0x15A3_699A,
    0xE69E_1EB5, 0x7CDD_A410, 0x4886_09DF, 0xD196_78D3,
];

/// A generated registration-code pair: `crc` validates `counter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationCode {
    /// The CRC.
    pub crc: u32,
    /// The counter the CRC validates.
    pub counter: u32,
}

impl RegistrationCode {
    /// Generate the pair from `seed`, the current time in milliseconds truncated
    /// to 32 bits (matching Flash's `uint` coercion of `Date.valueOf()`).
    #[must_use]
    pub fn generate(seed: u32) -> Self {
        let crc = compute_license_crc(seed);
        RegistrationCode {
            crc,
            counter: compute_license_counter(seed, crc),
        }
    }
}

/// The CRC half of [`RegistrationCode::generate`].
///
/// The seed is byte-reversed first (a big-endian write read back little-endian),
/// then folded through [`CRCMASK`] four bytes at a time.
#[must_use]
pub fn compute_license_crc(seed: u32) -> u32 {
    let mut seed = seed.swap_bytes();
    let mut crc = CRC_MAGIC;
    for _ in 0..4 {
        let current_byte = (seed & 0xff) as usize;
        crc = crc.rotate_left(1) ^ CRCMASK[current_byte];
        seed >>= 8;
    }
    crc
}

/// The counter half of [`RegistrationCode::generate`].
///
/// The reference calls this with the *original* seed, not the byte-reversed one
/// `compute_license_crc` works on.
#[must_use]
pub fn compute_license_counter(seed: u32, crc: u32) -> u32 {
    (seed ^ MAGIC_LONG) ^ crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validation provenance for the expected values below.
    ///
    /// The algorithm was re-implemented independently in a scratch Python script
    /// that parses `CRCMASK` straight out of the reference `.as` file, and the
    /// results were checked three ways: (1) `generate(0)` equals the
    /// `OPENPALACE_GUEST`/`OPENPALACE_GUEST_PUID` constants in OpenPalace's
    /// `PalaceClient.as` and Taj's `MH_Logon.cs`; (2) the same script reproduces
    /// Taj's C# `ComputeLicenseCRC`/`ComputeLicenseCounter` logic exactly; and
    /// (3) `generate(0x0ee1f3d9)` reproduces the `crc`/`counter` pair this
    /// repository's own `palace_walker.py` capture carries.
    #[test]
    fn generate_reproduces_the_reference_guest_identity() {
        // OpenPalace's guest registration and guest PUID, and Taj's
        // OPENPALACE_GUEST_PUID, are all generate(0).
        let guest = RegistrationCode::generate(0);
        assert_eq!(guest.crc, 0x5905_f923);
        assert_eq!(guest.counter, 0xcf07_309c);
    }

    #[test]
    fn generate_reproduces_the_captured_walker_registration_pair() {
        // The `crc`/`counter` pair in `WALKER_RICO_LOGON_HEX` is itself a
        // generate() output for this seed.
        let captured = RegistrationCode::generate(0x0ee1_f3d9);
        assert_eq!(captured.crc, 0x32fb_23e9);
        assert_eq!(captured.counter, 0xaa18_198f);
    }

    #[test]
    fn compute_license_crc_matches_known_answers() {
        assert_eq!(compute_license_crc(0), 0x5905_f923);
        assert_eq!(compute_license_crc(1), 0xc4e0_bcc3);
        assert_eq!(compute_license_crc(0x0102_0304), 0x827a_9d86);
        assert_eq!(compute_license_crc(0xdead_beef), 0xecd0_cd79);
        assert_eq!(compute_license_crc(0x1234_5678), 0xd36b_5e25);
        assert_eq!(compute_license_crc(u32::MAX), 0x2dfd_4bcf);
    }

    #[test]
    fn generate_matches_known_answers() {
        for (seed, crc, counter) in [
            (0x0000_0001, 0xc4e0_bcc3, 0x52e2_757d),
            (0x0102_0304, 0x827a_9d86, 0x157a_573d),
            (0xdead_beef, 0xecd0_cd79, 0xa47f_ba29),
            (0x1234_5678, 0xd36b_5e25, 0x575d_c1e2),
            (0xffff_ffff, 0x2dfd_4bcf, 0x4400_7d8f),
        ] {
            assert_eq!(
                RegistrationCode::generate(seed),
                RegistrationCode { crc, counter }
            );
        }
    }

    #[test]
    fn generate_is_deterministic_for_a_seed() {
        let seed = 0x00c0_ffee;
        assert_eq!(
            RegistrationCode::generate(seed),
            RegistrationCode::generate(seed)
        );
    }
}

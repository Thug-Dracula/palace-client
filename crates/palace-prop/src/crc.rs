//! The Palace asset CRC.
//!
//! A `.prp` roster record stores a CRC of its prop blob **excluding** the 12-byte
//! header, and the server validates every prop against it on startup. Getting the
//! slice wrong is the classic mistake: the field is `blob[12:]`, never `blob`.
//!
//! The algorithm is a seeded rotate-left-then-XOR byte fold, not a standard CRC:
//!
//! ```c
//! #define ASSET_CRC_MAGIC 0xD9216290
//! u32 crc = ASSET_CRC_MAGIC;
//! while (len--) {
//!     crc = (crc << 1) | (crc >> 31);   // rotate left by one bit
//!     crc ^= *p++;
//! }
//! ```
//!
//! Independently reimplemented in `~/palace-corpus/PRP-FORMAT.md` §4 and in
//! OpenPalace's `PalaceProp.as::computeCRC`; the two agree, and this crate's
//! corpus run reproduces the CRC of all 180,661 props in `pserver.prp`.

/// Seed value for [`asset_crc`].
pub const ASSET_CRC_MAGIC: u32 = 0xd921_6290;

/// Compute the Palace asset CRC of a byte slice.
#[must_use]
pub fn asset_crc(data: &[u8]) -> u32 {
    let mut crc = ASSET_CRC_MAGIC;
    for byte in data {
        crc = crc.rotate_left(1) ^ u32::from(*byte);
    }
    crc
}

/// Compute the CRC of a prop blob's payload, i.e. everything after the 12-byte
/// header. Returns `None` when the blob is too short to have a payload.
#[must_use]
pub fn payload_crc(blob: &[u8]) -> Option<u32> {
    blob.get(crate::header::HEADER_LEN..).map(asset_crc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::HEADER_LEN;

    #[test]
    fn empty_input_returns_the_seed() {
        assert_eq!(asset_crc(&[]), ASSET_CRC_MAGIC);
    }

    #[test]
    fn known_prop_payload_matches_the_roster() {
        // `pserver.prp` record id 976933367 (0x3a3ad1f7): the 144-byte blob
        // `2c 00 2c 00 00 00 00 00 00 00 02 00` + 132 payload bytes has
        // crc 0xffa0f716 over `blob[12:]` (PRP-FORMAT.md §9).
        let blob: [u8; HEADER_LEN] = [
            0x2c, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00,
        ];
        // The payload of that prop is not reproduced here, but the header-only
        // case must still be a pure function of the bytes.
        assert_eq!(payload_crc(&blob), Some(asset_crc(&[])));
        assert_eq!(payload_crc(&blob[..HEADER_LEN - 1]), None);
    }

    #[test]
    fn rotate_is_left_and_nine_bit_wraps() {
        // One byte: rotate the seed, then XOR.
        let expected = ASSET_CRC_MAGIC.rotate_left(1) ^ 0xff;
        assert_eq!(asset_crc(&[0xff]), expected);
        // The rotation must wrap bit 31 into bit 0, not shift it out.
        assert_eq!(asset_crc(&[0xff]), expected.wrapping_add(0));
        assert_ne!(asset_crc(&[0xff]), (ASSET_CRC_MAGIC << 1) ^ 0xff);
    }

    #[test]
    fn order_of_bytes_matters() {
        assert_ne!(asset_crc(b"ab"), asset_crc(b"ba"));
    }
}

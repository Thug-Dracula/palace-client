//! The Palace string cipher used by `xtlk` / `xwis`.
//!
//! `palace-wire` decodes plaintext `talk`/`whis` but implements no cipher for
//! the encrypted variants, so the runtime supplies it here rather than letting
//! encrypted chat degrade to an unknown opcode.
//!
//! The keystream is a fixed 512-byte table built once from a Park–Miller
//! (Lehmer) generator seeded with `0xa2c2a`, using Schrage's method to stay in
//! 32-bit arithmetic. Plaintext and ciphertext are consumed from the end
//! backwards, two table bytes per character, with a one-byte feedback chain.
//! Ported from the verified reference implementations (`QPalace`'s `QPCodec`
//! and `OpenPalace`'s `PalaceEncryption`); tests lock the round-trip.

use std::sync::OnceLock;

use palace_wire::byteorder::{latin1, ByteOrder};

const LUT_LEN: usize = 512;
const SEED: i64 = 0xa2c2a;
const Q: i64 = 0x1f31d;
const A: i64 = 16807;
const M: i64 = 2836;
const MODULUS: i64 = 0x7fff_ffff;

fn build_lut() -> [u8; LUT_LEN] {
    let mut lut = [0u8; LUT_LEN];
    let mut key = SEED;
    for slot in &mut lut {
        let quotient = key / Q;
        let remainder = key % Q;
        let next = A * remainder - M * quotient;
        key = if next > 0 { next } else { next + MODULUS };
        *slot = ((key as f64 / MODULUS as f64) * 256.0) as u8;
    }
    lut
}

fn lut() -> &'static [u8; LUT_LEN] {
    static LUT: OnceLock<[u8; LUT_LEN]> = OnceLock::new();
    LUT.get_or_init(build_lut)
}

/// Encrypt `data` in place-compatible form, returning the ciphertext.
#[must_use]
pub fn encrypt(data: &[u8]) -> Vec<u8> {
    let table = lut();
    let mut out = data.to_vec();
    let mut last: u8 = 0;
    let mut rc = 0usize;
    let mut i = out.len();
    while i > 0 {
        i -= 1;
        let b = out[i];
        let encoded = b ^ table[rc % LUT_LEN] ^ last;
        out[i] = encoded;
        last = encoded ^ table[(rc + 1) % LUT_LEN];
        rc += 2;
    }
    out
}

/// Decrypt `data`, returning the plaintext. Exact inverse of [`encrypt`].
#[must_use]
pub fn decrypt(data: &[u8]) -> Vec<u8> {
    let table = lut();
    let mut out = data.to_vec();
    let mut last: u8 = 0;
    let mut rc = 0usize;
    let mut i = out.len();
    while i > 0 {
        i -= 1;
        let b = out[i];
        out[i] = b ^ table[rc % LUT_LEN] ^ last;
        last = b ^ table[(rc + 1) % LUT_LEN];
        rc += 2;
    }
    out
}

/// Decrypt an `xtlk`/`xwis` payload body.
///
/// The body is `[i16 length][ciphertext][one trailing byte]`; the declared
/// length is unreliable (the reference clients call it a lie), so it is honoured
/// only when self-consistent and the trailing byte is always dropped.
#[must_use]
pub fn decode_payload(payload: &[u8], order: ByteOrder) -> Option<String> {
    if payload.len() < 4 {
        return None;
    }
    let head = [payload[0], payload[1]];
    let declared = match order {
        ByteOrder::Little => i16::from_le_bytes(head),
        ByteOrder::Big => i16::from_be_bytes(head),
    } as i64;
    let len = payload.len();
    let end = if declared >= 3 && declared <= len as i64 {
        declared as usize - 1
    } else {
        len - 1
    };
    if end <= 2 {
        return None;
    }
    Some(latin1(&decrypt(&payload[2..end])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_starts_with_the_reference_prefix() {
        let table = lut();
        assert_eq!(table.len(), LUT_LEN);
        assert_eq!(&table[0..6], &[55, 197, 96, 114, 205, 165]);
    }

    #[test]
    fn round_trip_is_identity() {
        for text in [
            &b""[..],
            b"Hello, Palace!",
            b"a",
            b"the quick brown fox jumps over the lazy dog 0123456789",
            &[0x00u8, 0xff, 0x80, 0x7f, 0x01],
        ] {
            let cipher = encrypt(text);
            assert_eq!(decrypt(&cipher), text, "round trip failed for {text:?}");
        }
    }

    #[test]
    fn ciphertext_differs_from_plaintext() {
        let text = b"Balamb Garden";
        assert_ne!(encrypt(text), text);
    }

    #[test]
    fn decode_payload_reads_length_prefixed_body() {
        let text = b"hi there";
        let mut cipher = encrypt(text);
        let mut body = Vec::new();
        let declared = (cipher.len() + 3) as i16;
        body.extend_from_slice(&declared.to_le_bytes());
        body.append(&mut cipher);
        body.push(0);
        assert_eq!(
            decode_payload(&body, ByteOrder::Little).as_deref(),
            Some("hi there")
        );
    }

    #[test]
    fn decode_payload_tolerates_a_lying_length() {
        let text = b"trust the bytes";
        let mut body = Vec::new();
        body.extend_from_slice(&999i16.to_le_bytes());
        body.extend_from_slice(&encrypt(text));
        body.push(0);
        assert_eq!(
            decode_payload(&body, ByteOrder::Little).as_deref(),
            Some("trust the bytes")
        );
    }

    #[test]
    fn decode_payload_rejects_short_bodies() {
        assert!(decode_payload(&[], ByteOrder::Little).is_none());
        assert!(decode_payload(&[0, 0, 1], ByteOrder::Little).is_none());
    }
}

//! Chat message bodies: `MSG_TALK` (`talk`) and `MSG_WHISPER` (`whis`).
//!
//! Both carry NUL-terminated text. The decoder reads to the terminator when
//! present and otherwise takes the remaining bytes, so a client that forgets
//! the terminator cannot desynchronise the session.

use crate::byteorder::{latin1, Reader, Writer};
use crate::error::Result;

/// `MSG_TALK`: `refNum` is the speaker's user id, the body is the chat text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Talk {
    /// Id of the user who spoke.
    pub user_id: i32,
    /// Chat text, decoded as Latin-1.
    pub text: String,
}

impl Talk {
    /// Decode a `talk` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(Talk {
            user_id: ref_num,
            text: read_text(r)?,
        })
    }

    /// Encode a `talk` body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_cstring(&self.text);
    }
}

/// `MSG_WHISPER`: `refNum` is the sender, the body is a `sint32` target id
/// followed by the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Whisper {
    /// Id of the user who whispered.
    pub user_id: i32,
    /// Id of the recipient.
    pub target_id: i32,
    /// Chat text.
    pub text: String,
}

impl Whisper {
    /// Decode a `whis` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let target_id = r.read_i32()?;
        Ok(Whisper {
            user_id: ref_num,
            target_id,
            text: read_text(r)?,
        })
    }

    /// Encode a `whis` body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_i32(self.target_id);
        w.write_cstring(&self.text);
    }
}

fn read_text(r: &mut Reader<'_>) -> Result<String> {
    let bytes = r.read_bytes(r.remaining())?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(latin1(&bytes[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::ByteOrder;

    #[test]
    fn talk_reads_nul_terminated_text() {
        let mut r = Reader::new(b"hello world\0", ByteOrder::Little);
        assert_eq!(Talk::decode(7, &mut r).unwrap().text, "hello world");
    }

    #[test]
    fn talk_tolerates_a_missing_terminator() {
        let mut r = Reader::new(b"no terminator", ByteOrder::Little);
        assert_eq!(Talk::decode(7, &mut r).unwrap().text, "no terminator");
    }

    #[test]
    fn whisper_reads_target_then_text() {
        let mut body = 42i32.to_le_bytes().to_vec();
        body.extend_from_slice(b"psst\0");
        let mut r = Reader::new(&body, ByteOrder::Little);
        let w = Whisper::decode(1, &mut r).unwrap();
        assert_eq!(w.target_id, 42);
        assert_eq!(w.text, "psst");
    }

    #[test]
    fn whisper_text_is_latin1() {
        let mut body = 1i32.to_le_bytes().to_vec();
        body.extend_from_slice(&[0xe9, 0x00]); // 'é'
        let mut r = Reader::new(&body, ByteOrder::Little);
        assert_eq!(Whisper::decode(1, &mut r).unwrap().text, "\u{e9}");
    }

    #[test]
    fn chat_round_trips_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let t = Whisper {
                user_id: 1,
                target_id: -2,
                text: "hi".into(),
            };
            let mut w = Writer::new(order);
            t.encode(&mut w);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            assert_eq!(Whisper::decode(1, &mut r).unwrap(), t);
        }
    }
}

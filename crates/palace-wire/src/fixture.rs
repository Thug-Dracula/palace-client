//! On-disk fixture corpus format.
//!
//! A fixture directory holds a `manifest.json` plus one `.bin` per frame,
//! written in wire order (12-byte header included). The manifest records the
//! session byte order, the decoded form of every frame, and enough metadata to
//! replay the whole session **with no server** — which is what later milestones
//! test against.
//!
//! ```text
//! fixtures/logon-run1/
//!   manifest.json
//!   frames/
//!     0000-server-tiyr.bin
//!     0001-client-regi.bin
//!     ...
//! ```
//!
//! Raw bytes are never transformed: a frame file is exactly what the socket
//! carried. The decoded form is derived, never authoritative.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::byteorder::ByteOrder;
use crate::error::{Result, WireError};
use crate::frame::Frame;
use crate::messages::Message;

/// Format identifier written into every manifest.
pub const FIXTURE_FORMAT: &str = "palace-fixture/1";

/// Which side of the connection a frame travelled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Client → server.
    Client,
    /// Server → client.
    Server,
}

impl Direction {
    /// Stable lowercase name used in manifests and file names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Direction::Client => "client",
            Direction::Server => "server",
        }
    }

    /// Parse the manifest spelling.
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "client" => Some(Direction::Client),
            "server" => Some(Direction::Server),
            _ => None,
        }
    }
}

/// One captured frame.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Monotonic position in the session.
    pub seq: usize,
    /// Who sent it.
    pub direction: Direction,
    /// The decoded frame.
    pub frame: Frame,
    /// Human-readable decode of the body.
    pub decoded: String,
}

impl CapturedFrame {
    /// Decode `frame`'s body and pair it with the frame.
    pub fn new(seq: usize, direction: Direction, frame: Frame, order: ByteOrder) -> Self {
        let decoded = match Message::decode(frame.opcode, frame.ref_num, &frame.payload, order) {
            Ok(msg) => msg.describe(),
            Err(err) => format!("decode error: {err}"),
        };
        CapturedFrame {
            seq,
            direction,
            frame,
            decoded,
        }
    }
}

/// A whole captured session.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// Format identifier; [`FIXTURE_FORMAT`] for files we write.
    pub format: String,
    /// Producer string.
    pub generated_by: String,
    /// `host:port` the session was captured against.
    pub server: String,
    /// Session byte order.
    pub byte_order: ByteOrder,
    /// Every frame, in order.
    pub frames: Vec<CapturedFrame>,
}

impl Fixture {
    /// Create an empty fixture.
    pub fn new(server: impl Into<String>, byte_order: ByteOrder) -> Self {
        Fixture {
            format: FIXTURE_FORMAT.to_string(),
            generated_by: format!("palace-wire {}", env!("CARGO_PKG_VERSION")),
            server: server.into(),
            byte_order,
            frames: Vec::new(),
        }
    }

    /// Append a frame, decoding its body for the manifest.
    pub fn push(&mut self, direction: Direction, frame: Frame) {
        let seq = self.frames.len();
        self.frames
            .push(CapturedFrame::new(seq, direction, frame, self.byte_order));
    }

    /// Number of frames captured.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True when no frames have been captured.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Frames sent by the server.
    pub fn server_frames(&self) -> impl Iterator<Item = &CapturedFrame> {
        self.frames
            .iter()
            .filter(|f| f.direction == Direction::Server)
    }

    /// Frames sent by the client.
    pub fn client_frames(&self) -> impl Iterator<Item = &CapturedFrame> {
        self.frames
            .iter()
            .filter(|f| f.direction == Direction::Client)
    }

    /// Write the fixture to `dir`, creating it if needed.
    pub fn write_to_dir(&self, dir: &Path) -> Result<()> {
        let frames_dir = dir.join("frames");
        fs::create_dir_all(&frames_dir)?;

        let mut entries = Vec::with_capacity(self.frames.len());
        for cf in &self.frames {
            let file = frame_file_name(cf);
            let bytes = cf.frame.encode(self.byte_order)?;
            fs::write(frames_dir.join(&file), &bytes)?;
            entries.push(json!({
                "seq": cf.seq,
                "direction": cf.direction.as_str(),
                "opcode": cf.frame.opcode.mnemonic(),
                "opcode_value": cf.frame.opcode.value(),
                "ref_num": cf.frame.ref_num,
                "length": cf.frame.payload.len(),
                "file": format!("frames/{file}"),
                "decoded": cf.decoded,
            }));
        }

        let manifest = json!({
            "format": self.format,
            "generated_by": self.generated_by,
            "server": self.server,
            "byte_order": self.byte_order.label(),
            "frame_count": self.frames.len(),
            "frames": entries,
        });
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )
        .map_err(WireError::from)?;
        Ok(())
    }

    /// Load a fixture from `dir`.
    ///
    /// Every frame file is re-decoded and re-encoded; a mismatch means the file
    /// is corrupt, so loading fails loudly rather than yielding bad data.
    pub fn load(dir: &Path) -> Result<Self> {
        let manifest_bytes = fs::read(dir.join("manifest.json"))?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
        let obj = manifest
            .as_object()
            .ok_or_else(|| manifest_shape_error("object"))?;

        let format = string_field(obj, "format")?;
        let generated_by = string_field(obj, "generated_by").unwrap_or_default();
        let server = string_field(obj, "server").unwrap_or_default();
        let byte_order_label = string_field(obj, "byte_order")?;
        let byte_order = ByteOrder::from_label(&byte_order_label)
            .ok_or_else(|| manifest_shape_error("byte_order"))?;

        let frames_json = obj
            .get("frames")
            .and_then(Value::as_array)
            .ok_or_else(|| manifest_shape_error("frames"))?;

        let mut frames = Vec::with_capacity(frames_json.len());
        for (index, entry) in frames_json.iter().enumerate() {
            let e = entry
                .as_object()
                .ok_or_else(|| manifest_shape_error("frames[i]"))?;
            let seq = e.get("seq").and_then(Value::as_u64).unwrap_or(index as u64) as usize;
            let direction = Direction::parse(&string_field(e, "direction")?)
                .ok_or_else(|| manifest_shape_error("direction"))?;
            let file = string_field(e, "file")?;
            let expected_len = e.get("length").and_then(Value::as_u64).unwrap_or(u64::MAX);
            let expected_opcode = e.get("opcode_value").and_then(Value::as_u64);
            let expected_ref = e.get("ref_num").and_then(Value::as_i64);
            let declared_decoded = string_field(e, "decoded").unwrap_or_default();

            let bytes = fs::read(dir.join(&file))?;
            let frame = Frame::decode_from(&bytes, byte_order)?;

            if frame.payload.len() as u64 != expected_len {
                return Err(manifest_shape_error("length mismatch"));
            }
            if let Some(op) = expected_opcode {
                if frame.opcode.value() as u64 != op {
                    return Err(manifest_shape_error("opcode mismatch"));
                }
            }
            if let Some(ref_num) = expected_ref {
                if frame.ref_num as i64 != ref_num {
                    return Err(manifest_shape_error("ref_num mismatch"));
                }
            }
            let reencoded = frame.encode(byte_order)?;
            if reencoded != bytes {
                return Err(manifest_shape_error("round-trip mismatch"));
            }

            let decoded =
                match Message::decode(frame.opcode, frame.ref_num, &frame.payload, byte_order) {
                    Ok(msg) => msg.describe(),
                    Err(err) => format!("decode error: {err}"),
                };
            if !declared_decoded.is_empty() && declared_decoded != decoded {
                return Err(manifest_shape_error("decoded text mismatch"));
            }

            frames.push(CapturedFrame {
                seq,
                direction,
                frame,
                decoded,
            });
        }

        Ok(Fixture {
            format,
            generated_by,
            server,
            byte_order,
            frames,
        })
    }

    /// Check structural invariants that must hold for any captured session.
    ///
    /// This is the offline assertion the test suite runs: it needs no network
    /// and no manifest beyond the fixture itself.
    pub fn verify(&self) -> std::result::Result<(), String> {
        if self.frames.is_empty() {
            return Err("fixture contains no frames".to_string());
        }
        if let Some(first) = self.frames.first() {
            if first.frame.opcode != crate::opcode::TIYID {
                return Err(format!(
                    "first frame is {}, expected tiyr(TIYID)",
                    first.frame.opcode.describe()
                ));
            }
            if first.direction != Direction::Server {
                return Err("first frame must be server → client".to_string());
            }
        }
        for cf in &self.frames {
            if cf.frame.payload.len() > crate::error::MAX_PAYLOAD_LEN as usize {
                return Err(format!("frame {} is implausibly large", cf.seq));
            }
            match Message::decode(
                cf.frame.opcode,
                cf.frame.ref_num,
                &cf.frame.payload,
                self.byte_order,
            ) {
                Ok(Message::RoomList(list)) => {
                    if list.rooms.len() != cf.frame.ref_num.max(0) as usize {
                        return Err(format!(
                            "frame {} rLst count {} != refNum {}",
                            cf.seq,
                            list.rooms.len(),
                            cf.frame.ref_num
                        ));
                    }
                }
                Ok(Message::UserList(list)) | Ok(Message::RoomUsers(list)) => {
                    if list.users.len() != cf.frame.ref_num.max(0) as usize {
                        return Err(format!(
                            "frame {} user count {} != refNum {}",
                            cf.seq,
                            list.users.len(),
                            cf.frame.ref_num
                        ));
                    }
                }
                Ok(_) => {}
                Err(err) => return Err(format!("frame {} failed to decode: {err}", cf.seq)),
            }
        }
        Ok(())
    }
}

fn frame_file_name(cf: &CapturedFrame) -> String {
    format!(
        "{:04}-{}-{}.bin",
        cf.seq,
        cf.direction.as_str(),
        sanitize_mnemonic(&cf.frame.opcode.mnemonic())
    )
}

fn sanitize_mnemonic(mnemonic: &str) -> String {
    let mut out = String::with_capacity(4);
    for ch in mnemonic.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("unkn");
    }
    out
}

fn string_field(obj: &Map<String, Value>, key: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| manifest_shape_error(key))
}

fn manifest_shape_error(what: &str) -> WireError {
    WireError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("malformed fixture manifest: {what}"),
    ))
}

/// Convenience: the fixture directory under a crate root.
pub fn default_fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::{LISTOFALLROOMS, LOGON, TIYID};

    fn tiny_fixture(order: ByteOrder) -> Fixture {
        let mut fx = Fixture::new("example.invalid:9998", order);
        fx.push(Direction::Server, Frame::empty(TIYID, 3));
        fx.push(Direction::Client, Frame::new(LOGON, 0, vec![0u8; 128]));
        let mut payload = Vec::new();
        let mut w = crate::byteorder::Writer::new(order);
        w.write_i32(186);
        w.write_u16(0x0200);
        w.write_u16(2);
        w.write_pstring_aligned("Entrance");
        payload.extend_from_slice(w.as_slice());
        fx.push(Direction::Server, Frame::new(LISTOFALLROOMS, 1, payload));
        fx
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("palace-fixture-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_through_disk_in_both_orders() {
        for (tag, order) in [("le", ByteOrder::Little), ("be", ByteOrder::Big)] {
            let dir = temp_dir(tag);
            let fx = tiny_fixture(order);
            fx.write_to_dir(&dir).unwrap();
            let loaded = Fixture::load(&dir).unwrap();
            assert_eq!(loaded.byte_order, order);
            assert_eq!(loaded.frames.len(), fx.frames.len());
            for (a, b) in loaded.frames.iter().zip(fx.frames.iter()) {
                assert_eq!(a.frame, b.frame);
                assert_eq!(a.decoded, b.decoded);
            }
            loaded.verify().unwrap();
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn verify_rejects_a_corrupt_frame_file() {
        let dir = temp_dir("corrupt");
        let fx = tiny_fixture(ByteOrder::Little);
        fx.write_to_dir(&dir).unwrap();
        let victim = dir.join("frames/0002-server-rLst.bin");
        let mut bytes = fs::read(&victim).unwrap();
        bytes[4] ^= 0xff; // corrupt the length field
        fs::write(&victim, bytes).unwrap();
        assert!(Fixture::load(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manifest_records_decode_text() {
        let dir = temp_dir("decoded");
        let fx = tiny_fixture(ByteOrder::Little);
        fx.write_to_dir(&dir).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let frames = manifest["frames"].as_array().unwrap();
        assert_eq!(frames[2]["opcode"], "rLst");
        assert!(frames[2]["decoded"].as_str().unwrap().contains("1 rooms"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frame_file_names_sanitize_non_alphanumeric_mnemonics() {
        assert_eq!(sanitize_mnemonic("bye "), "bye_");
        assert_eq!(sanitize_mnemonic("log "), "log_");
        assert_eq!(sanitize_mnemonic("...."), "____");
    }
}

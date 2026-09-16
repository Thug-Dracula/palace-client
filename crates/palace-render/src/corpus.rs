//! Loading room descriptions from local payload files.
//!
//! Rooms arrive as raw `MSG_ROOMDESC` payloads (`~/palace-corpus/payloads_all/*.bin`,
//! no frame header) or inside a captured frame (`fixtures/logon-run1/frames/
//! 0007-server-room.bin`, with the 12-byte `[type][len][ref]` header). This
//! module reads both and finds a room by id.

use std::path::{Path, PathBuf};

use palace_room::{decode_payload, decode_stream, RoomDesc};
use palace_wire::byteorder::ByteOrder;
use palace_wire::Frame;

use crate::error::RenderError;

/// A directory of room payloads, searched by room id.
#[derive(Debug, Clone)]
pub struct RoomSource {
    dir: PathBuf,
    order: ByteOrder,
}

impl RoomSource {
    /// A source over a directory of `MSG_ROOMDESC` payloads in the given byte
    /// order (the live corpus is little-endian).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, order: ByteOrder) -> Self {
        RoomSource {
            dir: dir.into(),
            order,
        }
    }

    /// The backing directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Decode a raw payload (no frame header).
    pub fn decode(bytes: &[u8]) -> Result<RoomDesc, RenderError> {
        decode_payload(bytes, ByteOrder::Little).map_err(|e| RenderError::NoPayload(e.to_string()))
    }

    /// Decode a captured frame (12-byte header then the payload).
    pub fn decode_frame(bytes: &[u8]) -> Result<RoomDesc, RenderError> {
        let frame = Frame::decode_from(bytes, ByteOrder::Little)
            .map_err(|e| RenderError::NoPayload(e.to_string()))?;
        decode_payload(&frame.payload, ByteOrder::Little)
            .map_err(|e| RenderError::NoPayload(e.to_string()))
    }

    /// Find a room by id.
    ///
    /// Tries `<dir>/<id>.bin` first, then scans every payload — some captures
    /// concatenate several records, so a file name is a hint and not proof.
    #[must_use]
    pub fn find(&self, room_id: i32) -> Option<RoomDesc> {
        if let Ok(bytes) = std::fs::read(self.dir.join(format!("{room_id}.bin"))) {
            if let Some(found) = self.match_in(&bytes, room_id) {
                return Some(found);
            }
        }
        self.all()
            .into_iter()
            .find(|(_, desc)| i32::from(desc.header.room_id) == room_id)
            .map(|(_, desc)| desc)
    }

    /// Decode every payload in the directory, oldest file name first.
    #[must_use]
    pub fn all(&self) -> Vec<(PathBuf, RoomDesc)> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("bin"))
            .collect();
        files.sort();
        for path in files {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            for desc in decode_stream(&bytes, self.order).into_iter().flatten() {
                out.push((path.clone(), desc));
            }
        }
        out
    }

    fn match_in(&self, bytes: &[u8], room_id: i32) -> Option<RoomDesc> {
        decode_stream(bytes, self.order)
            .into_iter()
            .flatten()
            .find(|desc| i32::from(desc.header.room_id) == room_id)
    }

    /// The number of `*.bin` files in the directory.
    #[must_use]
    pub fn file_count(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("bin"))
                    .count()
            })
            .unwrap_or(0)
    }
}

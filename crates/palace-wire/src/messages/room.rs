//! `MSG_ROOMDESC` (`room`): the fixed `RoomRec` header plus the variable-length
//! buffer its offset fields point into.
//!
//! This milestone decodes the header and the four named strings. Hotspots,
//! pictures, draw commands and loose props live in the same buffer and are a
//! later milestone; [`RoomDescription::var_data`] keeps the buffer intact so
//! nothing is lost.

use crate::byteorder::{ByteOrder, Reader, Writer};
use crate::error::{Result, WireError};

/// The fixed 40-byte room header. Protocol reference §3.30.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoomRec {
    /// `RF_*` room flags.
    pub room_flags: u32,
    /// Face-set id.
    pub faces_id: i32,
    /// Room id.
    pub room_id: i16,
    /// Offset of the room name `PString` inside the variable buffer.
    pub room_name_ofst: i16,
    /// Offset of the background-picture file name.
    pub pict_name_ofst: i16,
    /// Offset of the artist name.
    pub artist_name_ofst: i16,
    /// Offset of the room password.
    pub password_ofst: i16,
    /// Number of hotspots.
    pub nbr_hotspots: i16,
    /// Offset of the hotspot array.
    pub hotspot_ofst: i16,
    /// Number of pictures.
    pub nbr_pictures: i16,
    /// Offset of the picture array.
    pub picture_ofst: i16,
    /// Number of draw commands.
    pub nbr_draw_cmds: i16,
    /// Offset of the first draw command.
    pub first_draw_cmd: i16,
    /// Number of users in the room.
    pub nbr_people: i16,
    /// Number of loose props.
    pub nbr_lprops: i16,
    /// Offset of the first loose prop.
    pub first_lprop: i16,
    /// Alignment filler; should be 0.
    pub reserved: i16,
    /// Length of the variable buffer that follows the header.
    pub len_vars: i16,
}

impl RoomRec {
    /// Encoded size, in bytes.
    pub const LEN: usize = 40;

    /// Decode the fixed header.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let rec = RoomRec {
            room_flags: r.read_u32()?,
            faces_id: r.read_i32()?,
            room_id: r.read_i16()?,
            room_name_ofst: r.read_i16()?,
            pict_name_ofst: r.read_i16()?,
            artist_name_ofst: r.read_i16()?,
            password_ofst: r.read_i16()?,
            nbr_hotspots: r.read_i16()?,
            hotspot_ofst: r.read_i16()?,
            nbr_pictures: r.read_i16()?,
            picture_ofst: r.read_i16()?,
            nbr_draw_cmds: r.read_i16()?,
            first_draw_cmd: r.read_i16()?,
            nbr_people: r.read_i16()?,
            nbr_lprops: r.read_i16()?,
            first_lprop: r.read_i16()?,
            reserved: r.read_i16()?,
            len_vars: r.read_i16()?,
        };
        Ok(rec)
    }

    /// Encode the fixed header.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.room_flags);
        w.write_i32(self.faces_id);
        w.write_i16(self.room_id);
        w.write_i16(self.room_name_ofst);
        w.write_i16(self.pict_name_ofst);
        w.write_i16(self.artist_name_ofst);
        w.write_i16(self.password_ofst);
        w.write_i16(self.nbr_hotspots);
        w.write_i16(self.hotspot_ofst);
        w.write_i16(self.nbr_pictures);
        w.write_i16(self.picture_ofst);
        w.write_i16(self.nbr_draw_cmds);
        w.write_i16(self.first_draw_cmd);
        w.write_i16(self.nbr_people);
        w.write_i16(self.nbr_lprops);
        w.write_i16(self.first_lprop);
        w.write_i16(self.reserved);
        w.write_i16(self.len_vars);
    }

    /// The `room_flags` bits, named. Unknown bits are reported as their hex
    /// value so a future flag does not go unnoticed.
    pub fn flag_names(&self) -> Vec<String> {
        const BITS: &[(u32, &str)] = &[
            (0x0001, "AuthorLocked"),
            (0x0002, "Private"),
            (0x0004, "NoPainting"),
            (0x0008, "Closed"),
            (0x0010, "CyborgFreeZone"),
            (0x0020, "Hidden"),
            (0x0040, "NoGuests"),
            (0x0080, "WizardsOnly"),
            (0x0100, "DropZone"),
            (0x0200, "NoLooseProps"),
        ];
        let mut out = Vec::new();
        for (bit, name) in BITS {
            if self.room_flags & bit != 0 {
                out.push((*name).to_string());
            }
        }
        let known: u32 = BITS.iter().map(|(b, _)| *b).sum();
        let unknown = self.room_flags & !known;
        if unknown != 0 {
            out.push(format!("0x{unknown:08x}"));
        }
        if out.is_empty() {
            out.push("none".to_string());
        }
        out
    }
}

/// A decoded `MSG_ROOMDESC` (`room`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDescription {
    /// The fixed header.
    pub header: RoomRec,
    /// Room name.
    pub name: String,
    /// Background picture file name.
    pub picture: String,
    /// Artist name.
    pub artist: String,
    /// Room password (`""` when open).
    pub password: String,
    /// The variable buffer, kept whole for later milestones.
    pub var_data: Vec<u8>,
}

impl RoomDescription {
    /// Decode a `room` body: fixed header, then `len_vars` bytes of variable
    /// data, then the four `PString`s selected by their offsets.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let header = RoomRec::decode(r)?;
        if header.len_vars < 0 {
            return Err(WireError::ImplausibleLength {
                length: header.len_vars as u32,
                max: crate::error::MAX_PAYLOAD_LEN,
            });
        }
        let var_data = r.read_bytes(header.len_vars as usize)?.to_vec();
        let order = r.order();
        let name = string_at(&var_data, header.room_name_ofst, order);
        let picture = string_at(&var_data, header.pict_name_ofst, order);
        let artist = string_at(&var_data, header.artist_name_ofst, order);
        let password = string_at(&var_data, header.password_ofst, order);
        Ok(RoomDescription {
            header,
            name,
            picture,
            artist,
            password,
            var_data,
        })
    }
}

fn string_at(var_data: &[u8], offset: i16, order: ByteOrder) -> String {
    if offset < 0 {
        return String::new();
    }
    let offset = offset as usize;
    if offset >= var_data.len() {
        return String::new();
    }
    let mut r = Reader::new(&var_data[offset..], order);
    r.read_pstring().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::ByteOrder;

    #[test]
    fn room_rec_is_40_bytes() {
        let rec = RoomRec::default();
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            rec.encode(&mut w);
            assert_eq!(w.len(), RoomRec::LEN);
        }
    }

    #[test]
    fn decodes_the_live_balamb_garden_header() {
        // First 40 bytes of the live `room` payload.
        let header = [
            0x14, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x85, 0x03, 0x02, 0x00, 0x10, 0x00,
            0x1c, 0x00, 0x00, 0x00, 0x06, 0x00, 0x80, 0x03, 0x01, 0x00, 0x74, 0x03, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa0, 0x04,
        ];
        let mut r = Reader::new(&header, ByteOrder::Little);
        let rec = RoomRec::decode(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(rec.room_flags, 0x0000_0114);
        assert_eq!(rec.room_id, 901);
        assert_eq!(rec.room_name_ofst, 2);
        assert_eq!(rec.pict_name_ofst, 16);
        assert_eq!(rec.artist_name_ofst, 28);
        assert_eq!(rec.nbr_hotspots, 6);
        assert_eq!(rec.hotspot_ofst, 896);
        assert_eq!(rec.nbr_pictures, 1);
        assert_eq!(rec.picture_ofst, 884);
        assert_eq!(rec.len_vars, 1184);
    }

    #[test]
    fn decodes_offsets_into_the_variable_buffer() {
        let mut var = Vec::new();
        var.extend_from_slice(&[0, 0]);
        var.push(13);
        var.extend_from_slice(b"Balamb Garden");
        assert_eq!(string_at(&var, 2, ByteOrder::Little), "Balamb Garden");
        assert_eq!(string_at(&var, 999, ByteOrder::Little), "");
        assert_eq!(string_at(&var, -1, ByteOrder::Little), "");
    }

    #[test]
    fn truncation_is_an_error() {
        let mut r = Reader::new(&[0u8; 10], ByteOrder::Little);
        assert!(RoomDescription::decode(&mut r).is_err());
    }
}

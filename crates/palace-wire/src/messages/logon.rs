//! `MSG_LOGON` (`regi`), the `AuxRegistrationRec` structure it carries, and the
//! `MSG_AUTHENTICATE` request that can follow it.

use crate::byteorder::{ByteOrder, Reader, Writer};
use crate::error::{Result, WireError};
use crate::frame::Frame;
use crate::opcode;

/// Encoded size of an [`AuxRegistrationRec`], in bytes.
pub const AUX_REGISTRATION_REC_LEN: usize = 128;

/// Length of the `reserved` field, in bytes.
pub const RESERVED_LEN: usize = 6;

/// `MSG_LOGON` body: everything the server learns about us at logon.
///
/// Field order and widths are from the protocol reference §3.20, confirmed by
/// the live capture (`logon_run1.pcap`) and byte-for-byte against
/// `palace_walker.py`'s `LOGON_PACKET`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxRegistrationRec {
    /// Registration-code CRC (`crc.validate(counter)` on the server).
    pub crc: u32,
    /// Registration-code number.
    pub counter: u32,
    /// User name, `Str31` (max 31 bytes).
    pub user_name: String,
    /// Wizard password, `Str31`.
    pub wiz_password: String,
    /// OS/authenticate flags — see [`AuxFlags`].
    pub aux_flags: u32,
    /// PUID counter (client-generated pseudo-random id number).
    pub puid_ctr: u32,
    /// PUID CRC validating [`AuxRegistrationRec::puid_ctr`].
    pub puid_crc: u32,
    /// Historical, unused.
    pub demo_elapsed: u32,
    /// Historical, unused.
    pub total_elapsed: u32,
    /// Historical, unused.
    pub demo_limit: u32,
    /// Room to enter on arrival; 0 means "any".
    pub desired_room: i16,
    /// Six bytes the spec calls reserved. The reference client writes a client
    /// tag here because the server logs it.
    pub reserved: [u8; RESERVED_LEN],
    /// Requested protocol version (the server ignores it).
    pub requested_protocol_version: u32,
    /// Upload capability bitmask.
    pub upload_caps: u32,
    /// Download capability bitmask. The Unix server only examines
    /// `DLCAPS_FILES_HTTPSRVR` (0x0100).
    pub download_caps: u32,
    /// 2-D engine capabilities (unused by the server).
    pub engine_2d_caps: u32,
    /// 2-D graphics capabilities (unused by the server).
    pub graphics_2d_caps: u32,
    /// 3-D engine capabilities (unused by the server).
    pub engine_3d_caps: u32,
}

/// `auxFlags` bit values (protocol reference §3.20).
pub mod aux_flags {
    /// Unknown machine.
    pub const UNKNOWN: u32 = 0;
    /// 68k Macintosh.
    pub const MAC68K: u32 = 1;
    /// PowerPC Macintosh.
    pub const MACPPC: u32 = 2;
    /// 16-bit Windows.
    pub const WIN16: u32 = 3;
    /// 32-bit Windows.
    pub const WIN32: u32 = 4;
    /// Java.
    pub const JAVA: u32 = 5;
    /// Mask isolating the OS field.
    pub const OSMASK: u32 = 0x0000_000F;
    /// The server should ask the client to authenticate.
    pub const AUTHENTICATE: u32 = 0x8000_0000;
}

impl AuxRegistrationRec {
    /// Decode exactly 128 bytes.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let rec = AuxRegistrationRec {
            crc: r.read_u32()?,
            counter: r.read_u32()?,
            user_name: r.read_str31()?,
            wiz_password: r.read_str31()?,
            aux_flags: r.read_u32()?,
            puid_ctr: r.read_u32()?,
            puid_crc: r.read_u32()?,
            demo_elapsed: r.read_u32()?,
            total_elapsed: r.read_u32()?,
            demo_limit: r.read_u32()?,
            desired_room: r.read_i16()?,
            reserved: read_reserved(r)?,
            requested_protocol_version: r.read_u32()?,
            upload_caps: r.read_u32()?,
            download_caps: r.read_u32()?,
            engine_2d_caps: r.read_u32()?,
            graphics_2d_caps: r.read_u32()?,
            engine_3d_caps: r.read_u32()?,
        };
        Ok(rec)
    }

    /// Encode into a writer using the session byte order.
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.crc);
        w.write_u32(self.counter);
        w.write_str31(&self.user_name);
        w.write_str31(&self.wiz_password);
        w.write_u32(self.aux_flags);
        w.write_u32(self.puid_ctr);
        w.write_u32(self.puid_crc);
        w.write_u32(self.demo_elapsed);
        w.write_u32(self.total_elapsed);
        w.write_u32(self.demo_limit);
        w.write_i16(self.desired_room);
        w.write_bytes(&self.reserved);
        w.write_u32(self.requested_protocol_version);
        w.write_u32(self.upload_caps);
        w.write_u32(self.download_caps);
        w.write_u32(self.engine_2d_caps);
        w.write_u32(self.graphics_2d_caps);
        w.write_u32(self.engine_3d_caps);
    }

    /// The encoded body as a standalone buffer.
    pub fn encode_to_vec(&self, order: ByteOrder) -> Vec<u8> {
        let mut w = Writer::with_capacity(order, AUX_REGISTRATION_REC_LEN);
        self.encode(&mut w);
        w.into_vec()
    }

    /// Build a complete `MSG_LOGON` frame for this record.
    pub fn logon_frame(&self, order: ByteOrder) -> Frame {
        Frame::new(opcode::LOGON, 0, self.encode_to_vec(order))
    }

    /// True when the record encodes to the expected size.
    pub fn verify_size(&self, order: ByteOrder) -> Result<()> {
        let len = self.encode_to_vec(order).len();
        if len != AUX_REGISTRATION_REC_LEN {
            return Err(WireError::TrailingBytes { remaining: len });
        }
        Ok(())
    }
}

fn read_reserved(r: &mut Reader<'_>) -> Result<[u8; RESERVED_LEN]> {
    let bytes = r.read_bytes(RESERVED_LEN)?;
    let mut out = [0u8; RESERVED_LEN];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// The registration values six independent reference clients agree on
/// (OpenPalace's `OPENPALACE_GUEST`, Taj's logged-in defaults, pserver's
/// tolerated range) **as actually written by `palace_walker.py`**, which is
/// verified working against the target server.
///
/// The `reserved` field and the capability masks are deliberately *not* the
/// honest values: PalaceChat itself misstates its caps, and matching the
/// reference client avoids being classified as a hacked client. See the README
/// for the provenance table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceProfile {
    /// Registration CRC.
    pub crc: u32,
    /// Registration counter.
    pub counter: u32,
    /// `auxFlags`.
    pub aux_flags: u32,
    /// PUID counter.
    pub puid_ctr: u32,
    /// PUID CRC.
    pub puid_crc: u32,
    /// Historical demo timer 1.
    pub demo_elapsed: u32,
    /// Historical demo timer 2.
    pub total_elapsed: u32,
    /// Historical demo timer 3.
    pub demo_limit: u32,
    /// Client tag written into `reserved`.
    pub reserved: [u8; RESERVED_LEN],
    /// Protocol version value.
    pub requested_protocol_version: u32,
    /// Upload caps.
    pub upload_caps: u32,
    /// Download caps.
    pub download_caps: u32,
    /// 2-D engine caps.
    pub engine_2d_caps: u32,
    /// 2-D graphics caps.
    pub graphics_2d_caps: u32,
    /// 3-D engine caps.
    pub engine_3d_caps: u32,
}

impl Default for ReferenceProfile {
    fn default() -> Self {
        ReferenceProfile {
            crc: 0x32fb_23e9,
            counter: 0xaa18_198f,
            aux_flags: 0x8000_0008,
            puid_ctr: 0xe054_6e4f,
            puid_crc: 0x92c9_6d66,
            demo_elapsed: 72_000,
            total_elapsed: 72_000,
            demo_limit: 72_000,
            reserved: *b"SCOUT1",
            requested_protocol_version: 0x0007_abcc,
            upload_caps: 0x41,
            download_caps: 0x151,
            engine_2d_caps: 1,
            graphics_2d_caps: 1,
            engine_3d_caps: 0,
        }
    }
}

impl ReferenceProfile {
    /// Build an [`AuxRegistrationRec`] for `user_name` entering `desired_room`.
    pub fn to_record(self, user_name: &str, desired_room: i16) -> AuxRegistrationRec {
        AuxRegistrationRec {
            crc: self.crc,
            counter: self.counter,
            user_name: user_name.to_string(),
            wiz_password: String::new(),
            aux_flags: self.aux_flags,
            puid_ctr: self.puid_ctr,
            puid_crc: self.puid_crc,
            demo_elapsed: self.demo_elapsed,
            total_elapsed: self.total_elapsed,
            demo_limit: self.demo_limit,
            desired_room,
            reserved: self.reserved,
            requested_protocol_version: self.requested_protocol_version,
            upload_caps: self.upload_caps,
            download_caps: self.download_caps,
            engine_2d_caps: self.engine_2d_caps,
            graphics_2d_caps: self.graphics_2d_caps,
            engine_3d_caps: self.engine_3d_caps,
        }
    }
}

/// Build the default reference logon record for `user_name`.
pub fn reference_logon_record(user_name: &str, desired_room: i16) -> AuxRegistrationRec {
    ReferenceProfile::default().to_record(user_name, desired_room)
}

/// The logon profile this client actually advertises.
///
/// It is the [`ReferenceProfile`] with [`aux_flags::AUTHENTICATE`] cleared. That
/// bit promises the server an authentication challenge will be answered, but
/// `AUTHRESPONSE` is not implemented: a server that believes the advertisement
/// sends `AUTHENTICATE` and then waits in silence. The application sends this
/// profile so it never claims a capability it cannot honour. The reference
/// profile keeps the bit because it mirrors the captured client byte for byte;
/// see its documentation for why that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientProfile(ReferenceProfile);

impl Default for ClientProfile {
    fn default() -> Self {
        // Start from every reference value and remove only the one bit this
        // client cannot back up, so the two profiles cannot drift apart.
        ClientProfile(ReferenceProfile {
            aux_flags: ReferenceProfile::default().aux_flags & !aux_flags::AUTHENTICATE,
            ..ReferenceProfile::default()
        })
    }
}

impl ClientProfile {
    /// Build an [`AuxRegistrationRec`] for `user_name` entering `desired_room`.
    pub fn to_record(self, user_name: &str, desired_room: i16) -> AuxRegistrationRec {
        self.0.to_record(user_name, desired_room)
    }
}

/// Build the logon record this client sends for `user_name`.
///
/// Unlike [`reference_logon_record`] it does not advertise
/// [`aux_flags::AUTHENTICATE`], because the reply is not implemented.
pub fn client_logon_record(user_name: &str, desired_room: i16) -> AuxRegistrationRec {
    ClientProfile::default().to_record(user_name, desired_room)
}

/// `MSG_AUTHENTICATE` (`auth`): the server asking the client to authenticate.
///
/// The body is empty — "there are no parameters in this message, so the length
/// field should be 0 and the msg field should be empty" (:739-742). The reply is
/// `MSG_AUTHRESPONSE` (:744), a PString of `user:password`, which this client does
/// not send; knowing the request arrived is what lets it say so instead of stalling
/// in silence. The frame `refNum` is unused (:737).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Authenticate;

impl Authenticate {
    /// Decode an `auth` body, which must be empty.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        if !r.is_empty() {
            return Err(WireError::TrailingBytes {
                remaining: r.remaining(),
            });
        }
        Ok(Authenticate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode;

    /// `palace_walker.py`'s `make_logon(b"Rico")`, byte for byte. This pins our
    /// encoder to the known-good oracle.
    const WALKER_RICO_LOGON_HEX: &str = concat!(
        "696765728000000000000000e923fb328f1918aa04526963",
        "6f0000000000000000000000000000000000000000000000",
        "000000000000000000000000000000000000000000000000",
        "000000000000000000000000080000804f6e54e0666dc992",
        "401901004019010040190100000053434f555431ccab0700",
        "4100000051010000010000000100000000000000",
    );
    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_palace_walker_logon_packet_byte_for_byte() {
        let expected = hex_to_bytes(WALKER_RICO_LOGON_HEX);
        assert_eq!(expected.len(), 140, "fixture sanity");
        let record = reference_logon_record("Rico", 0);
        let encoded = record.encode_to_vec(ByteOrder::Little);
        assert_eq!(encoded.len(), AUX_REGISTRATION_REC_LEN);
        assert_eq!(encoded[..], expected[12..], "AuxRegistrationRec mismatch");
        let frame = record.logon_frame(ByteOrder::Little);
        assert_eq!(frame.opcode, opcode::LOGON);
        assert_eq!(frame.ref_num, 0);
        assert_eq!(frame.encode(ByteOrder::Little).unwrap(), expected);
    }

    #[test]
    fn record_round_trips_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let record = reference_logon_record("Interop", 901);
            let body = record.encode_to_vec(order);
            assert_eq!(body.len(), AUX_REGISTRATION_REC_LEN);
            let mut r = Reader::new(&body, order);
            let back = AuxRegistrationRec::decode(&mut r).unwrap();
            assert!(r.is_empty(), "body must be consumed exactly");
            assert_eq!(back, record);
        }
    }

    #[test]
    fn big_endian_encoding_is_byte_reversed_only_in_multi_byte_fields() {
        let record = reference_logon_record("Rico", 0);
        let le = record.encode_to_vec(ByteOrder::Little);
        let be = record.encode_to_vec(ByteOrder::Big);
        assert_eq!(le.len(), be.len());
        assert_eq!(&le[0..4], &0x32fb_23e9u32.to_le_bytes());
        assert_eq!(&be[0..4], &0x32fb_23e9u32.to_be_bytes());
        // The ASCII name bytes are order-independent.
        assert_eq!(&le[8..12], &be[8..12]);
    }

    #[test]
    fn reserved_field_and_caps_match_the_reference_client() {
        let record = reference_logon_record("x", 0);
        assert_eq!(&record.reserved, b"SCOUT1");
        assert_eq!(record.upload_caps, 0x41);
        assert_eq!(record.download_caps, 0x151);
        assert_eq!(record.aux_flags, 0x8000_0008);
        assert_eq!(
            record.aux_flags & aux_flags::AUTHENTICATE,
            aux_flags::AUTHENTICATE
        );
    }

    #[test]
    fn client_profile_clears_only_the_authenticate_bit() {
        // The unnamed OS tag the reference client writes in the low nibble.
        const OS_TAG_8: u32 = 0x0000_0008;

        let reference = reference_logon_record("Rico", 0);
        let client = client_logon_record("Rico", 0);

        // The oracle advertises the capability; what we send does not.
        assert_eq!(
            reference.aux_flags & aux_flags::AUTHENTICATE,
            aux_flags::AUTHENTICATE
        );
        assert_eq!(client.aux_flags & aux_flags::AUTHENTICATE, 0);
        assert_eq!(
            reference.aux_flags ^ client.aux_flags,
            aux_flags::AUTHENTICATE,
            "the profiles must agree on every other auxFlags bit"
        );

        // And the encoding differs only within the `auxFlags` word at offset 72.
        let ref_bytes = reference.encode_to_vec(ByteOrder::Little);
        let client_bytes = client.encode_to_vec(ByteOrder::Little);
        assert_eq!(ref_bytes.len(), client_bytes.len());
        let differing: Vec<usize> = (0..ref_bytes.len())
            .filter(|&i| ref_bytes[i] != client_bytes[i])
            .collect();
        assert!(!differing.is_empty(), "the profiles must differ");
        assert!(
            differing.iter().all(|&i| (72..76).contains(&i)),
            "only the auxFlags word may differ, got {differing:?}"
        );
        assert_eq!(
            &ref_bytes[72..76],
            &(aux_flags::AUTHENTICATE | OS_TAG_8).to_le_bytes()
        );
        assert_eq!(&client_bytes[72..76], &OS_TAG_8.to_le_bytes());
    }

    #[test]
    fn long_username_is_truncated_to_31_bytes() {
        let name = "a".repeat(40);
        let record = reference_logon_record(&name, 0);
        let body = record.encode_to_vec(ByteOrder::Little);
        assert_eq!(body.len(), AUX_REGISTRATION_REC_LEN);
        assert_eq!(body[8], 31);
    }
}

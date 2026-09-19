//! `MSG_LOGON` (`regi`), the `AuxRegistrationRec` structure it carries, and the
//! `MSG_AUTHENTICATE` request that can follow it.

use crate::byteorder::{ByteOrder, Reader, Writer};
use crate::error::{Result, WireError};
use crate::frame::Frame;
use crate::opcode;
use crate::registration::RegistrationCode;

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

/// A client-generated PUID: the per-install identity a Palace server keys a
/// session on.
///
/// The reference client mints one with [`RegistrationCode::generate`] the first
/// time it runs and persists it, then sends that same pair on every logon. A
/// server that sees two connections with one PUID treats them as the same user
/// and disconnects one, which is why this must not be copied from another
/// client's capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Puid {
    /// The wire `puidCtr` (`RegistrationCode::counter`).
    pub ctr: u32,
    /// The wire `puidCRC` (`RegistrationCode::crc`).
    pub crc: u32,
}

impl From<RegistrationCode> for Puid {
    fn from(code: RegistrationCode) -> Self {
        // The reference maps the registration code's `counter` onto `puidCtr`
        // and its `crc` onto `puidCRC` (`PalaceClient.setPuid`).
        Puid {
            ctr: code.counter,
            crc: code.crc,
        }
    }
}

impl Puid {
    /// Mint a PUID from `seed`: the current time in milliseconds truncated to
    /// 32 bits, as the reference client seeds its persisted identity.
    #[must_use]
    pub fn generate(seed: u32) -> Self {
        RegistrationCode::generate(seed).into()
    }
}

impl Default for Puid {
    fn default() -> Self {
        // OpenPalace's `OPENPALACE_GUEST_PUID` and Taj's guest PUID are both
        // `generate(0)`; using it keeps a config-less client on a legitimate
        // guest identity instead of an identity copied from a capture.
        RegistrationCode::generate(0).into()
    }
}

/// The seed offset that keeps the PUID draw distinct from the registration
/// draw when one persisted seed mints both pairs.
const PUID_SEED_OFFSET: u32 = 0x9e37_79b9;

/// The per-install identity a logon advertises: the registration pair the
/// server validates at `crc`/`counter` and the PUID pair at
/// `puidCtr`/`puidCRC`.
///
/// The reference client mints both with [`RegistrationCode::generate`] the
/// first time it runs and persists them; a server treats two sessions with the
/// same identity as one user and drops one of them, so neither pair may be
/// copied from another client's capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIdentity {
    /// The registration pair written to `crc`/`counter`.
    pub registration: RegistrationCode,
    /// The PUID pair written to `puidCtr`/`puidCRC`.
    pub puid: Puid,
}

impl ClientIdentity {
    /// Mint both pairs from `seed`: the registration pair is `generate(seed)`
    /// and the PUID is an independent draw for an offset seed.
    #[must_use]
    pub fn generate(seed: u32) -> Self {
        ClientIdentity {
            registration: RegistrationCode::generate(seed),
            puid: Puid::generate(seed.wrapping_add(PUID_SEED_OFFSET)),
        }
    }
}

impl Default for ClientIdentity {
    fn default() -> Self {
        // OpenPalace's `OPENPALACE_GUEST` and `OPENPALACE_GUEST_PUID` are both
        // `generate(0)`; the guest identity keeps a config-less client
        // legitimate instead of putting captured constants on the wire.
        let registration = RegistrationCode::generate(0);
        ClientIdentity {
            registration,
            puid: registration.into(),
        }
    }
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
    /// Build an [`AuxRegistrationRec`] for `user_name` entering `desired_room`,
    /// carrying this profile's own captured identity.
    pub fn to_record(self, user_name: &str, desired_room: i16) -> AuxRegistrationRec {
        self.to_record_with_identity(
            user_name,
            desired_room,
            ClientIdentity {
                registration: RegistrationCode {
                    crc: self.crc,
                    counter: self.counter,
                },
                puid: Puid {
                    ctr: self.puid_ctr,
                    crc: self.puid_crc,
                },
            },
        )
    }

    /// Build an [`AuxRegistrationRec`] for `user_name` entering `desired_room`
    /// with the supplied `identity` instead of this profile's captured one.
    ///
    /// Both the registration pair (`crc`/`counter`) and the PUID are replaced:
    /// the server keys a session on either pair, so leaving the captured
    /// registration behind still lets it mistake this install for the capture.
    pub fn to_record_with_identity(
        self,
        user_name: &str,
        desired_room: i16,
        identity: ClientIdentity,
    ) -> AuxRegistrationRec {
        AuxRegistrationRec {
            crc: identity.registration.crc,
            counter: identity.registration.counter,
            user_name: user_name.to_string(),
            wiz_password: String::new(),
            aux_flags: self.aux_flags,
            puid_ctr: identity.puid.ctr,
            puid_crc: identity.puid.crc,
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

/// The logon profile this client advertises when no credential is configured.
///
/// It is the [`ReferenceProfile`] with [`aux_flags::AUTHENTICATE`] cleared: with
/// no credential there is no reply to give, and a server that believed the
/// advertisement would send `AUTHENTICATE` and then wait in silence. With a
/// credential, [`authenticating_logon_record`] keeps the bit so the challenge
/// arrives. The reference profile keeps the bit because it mirrors the captured
/// client byte for byte; see its documentation for why that matters.
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

    /// Build an [`AuxRegistrationRec`] with the supplied `identity` instead of
    /// the profile's captured one.
    pub fn to_record_with_identity(
        self,
        user_name: &str,
        desired_room: i16,
        identity: ClientIdentity,
    ) -> AuxRegistrationRec {
        self.0
            .to_record_with_identity(user_name, desired_room, identity)
    }
}

/// Build the logon record this client sends for `user_name` without a
/// credential.
///
/// Unlike [`reference_logon_record`] and [`authenticating_logon_record`] it does
/// not advertise [`aux_flags::AUTHENTICATE`], because without a credential the
/// reply cannot be sent.
pub fn client_logon_record(user_name: &str, desired_room: i16) -> AuxRegistrationRec {
    ClientProfile::default().to_record(user_name, desired_room)
}

/// Build the logon record this client sends for `user_name` without a
/// credential, carrying the supplied `identity`.
pub fn client_logon_record_with_identity(
    user_name: &str,
    desired_room: i16,
    identity: ClientIdentity,
) -> AuxRegistrationRec {
    ClientProfile::default().to_record_with_identity(user_name, desired_room, identity)
}

/// Build the logon record this client sends for `user_name` when it can answer
/// an authentication challenge.
///
/// It is the [`ReferenceProfile`], so it advertises
/// [`aux_flags::AUTHENTICATE`] and is byte-identical to the captured client;
/// [`authresponse_frame`] supplies the reply that bit promises.
pub fn authenticating_logon_record(user_name: &str, desired_room: i16) -> AuxRegistrationRec {
    ReferenceProfile::default().to_record(user_name, desired_room)
}

/// Like [`authenticating_logon_record`] but carrying the supplied `identity`.
pub fn authenticating_logon_record_with_identity(
    user_name: &str,
    desired_room: i16,
    identity: ClientIdentity,
) -> AuxRegistrationRec {
    ReferenceProfile::default().to_record_with_identity(user_name, desired_room, identity)
}

/// Build the `MSG_AUTHRESPONSE` (`autr`) frame that answers a server's
/// `MSG_AUTHENTICATE` (`auth`) challenge for `user_name`.
///
/// The body is the `PString` `user:password` (:744). The reference client writes
/// exactly that string and the server splits it on the first `:`; the frame
/// `refNum` is unused (:737).
pub fn authresponse_frame(user_name: &str, password: &str, order: ByteOrder) -> Frame {
    let mut credential = String::with_capacity(user_name.len() + password.len() + 1);
    credential.push_str(user_name);
    credential.push(':');
    credential.push_str(password);
    let mut w = Writer::with_capacity(order, credential.len() + 1);
    w.write_pstring(&credential);
    Frame::new(opcode::AUTHRESPONSE, 0, w.into_vec())
}

/// `MSG_AUTHENTICATE` (`auth`): the server asking the client to authenticate.
///
/// The body is empty — "there are no parameters in this message, so the length
/// field should be 0 and the msg field should be empty" (:739-742). The reply is
/// [`authresponse_frame`], a PString of `user:password` (:744); when the client
/// has a credential it sends that, and when it does not, knowing the request
/// arrived is what lets it say so instead of stalling in silence. The frame
/// `refNum` is unused (:737).
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
    fn authresponse_body_is_the_pstring_user_colon_password() {
        let frame = authresponse_frame("Rico", "hunter2", ByteOrder::Little);
        assert_eq!(frame.opcode, opcode::AUTHRESPONSE);
        assert_eq!(frame.ref_num, 0);
        assert_eq!(frame.payload, b"\x0cRico:hunter2");
        #[rustfmt::skip]
        let expected_le: Vec<u8> = vec![
            0x72, 0x74, 0x75, 0x61, // "rtua" = autr, little-endian
            0x0d, 0x00, 0x00, 0x00, // length = 13
            0x00, 0x00, 0x00, 0x00, // refNum = 0
            0x0c, b'R', b'i', b'c', b'o', b':', b'h', b'u', b'n', b't', b'e', b'r', b'2',
        ];
        assert_eq!(frame.encode(ByteOrder::Little).unwrap(), expected_le);

        let be = authresponse_frame("Rico", "hunter2", ByteOrder::Big)
            .encode(ByteOrder::Big)
            .unwrap();
        assert_eq!(&be[..4], b"autr");
        assert_eq!(&be[4..8], &13u32.to_be_bytes());
        assert_eq!(&be[8..12], &0i32.to_be_bytes());
        assert_eq!(&be[12..], b"\x0cRico:hunter2");
    }

    #[test]
    fn an_authenticating_logon_keeps_the_authenticate_bit_a_plain_one_clears() {
        let authenticating = authenticating_logon_record("Rico", 0);
        let plain = client_logon_record("Rico", 0);
        assert_eq!(
            authenticating.aux_flags & aux_flags::AUTHENTICATE,
            aux_flags::AUTHENTICATE
        );
        assert_eq!(plain.aux_flags & aux_flags::AUTHENTICATE, 0);
        assert_eq!(
            authenticating,
            reference_logon_record("Rico", 0),
            "the authenticating record is the reference profile, byte for byte"
        );
    }

    #[test]
    fn long_username_is_truncated_to_31_bytes() {
        let name = "a".repeat(40);
        let record = reference_logon_record(&name, 0);
        let body = record.encode_to_vec(ByteOrder::Little);
        assert_eq!(body.len(), AUX_REGISTRATION_REC_LEN);
        assert_eq!(body[8], 31);
    }

    #[test]
    fn a_supplied_identity_replaces_only_the_identity_fields() {
        let identity = ClientIdentity {
            registration: RegistrationCode {
                crc: 0x1357_2468,
                counter: 0x90ab_cdef,
            },
            puid: Puid {
                ctr: 0x1122_3344,
                crc: 0x5566_7788,
            },
        };
        let record = client_logon_record_with_identity("Rico", 0, identity);

        assert_eq!(record.crc, identity.registration.crc);
        assert_eq!(record.counter, identity.registration.counter);
        assert_eq!(record.puid_ctr, identity.puid.ctr);
        assert_eq!(record.puid_crc, identity.puid.crc);
        assert_eq!(
            record.aux_flags,
            client_logon_record("Rico", 0).aux_flags,
            "supplying an identity must not disturb the rest of the client profile"
        );
    }

    #[test]
    fn a_generated_identity_replaces_both_captured_pairs() {
        let captured = ReferenceProfile::default();
        let record =
            client_logon_record_with_identity("Rico", 0, ClientIdentity::generate(0x0bad_f00d));

        assert_ne!(record.crc, captured.crc);
        assert_ne!(record.counter, captured.counter);
        assert_ne!(record.puid_ctr, captured.puid_ctr);
        assert_ne!(record.puid_crc, captured.puid_crc);
    }

    #[test]
    fn the_default_identity_is_the_generated_guest_not_the_capture() {
        let guest = RegistrationCode::generate(0);
        let identity = ClientIdentity::default();
        assert_eq!(identity.registration, guest);
        assert_eq!(identity.puid, Puid::from(guest));
        let record = client_logon_record_with_identity("Rico", 0, identity);
        assert_eq!(record.crc, 0x5905_f923);
        assert_eq!(record.counter, 0xcf07_309c);
        assert_ne!(record.crc, ReferenceProfile::default().crc);
        assert_ne!(record.puid_ctr, ReferenceProfile::default().puid_ctr);
    }

    #[test]
    fn a_generated_identity_draws_two_distinct_pairs() {
        let identity = ClientIdentity::generate(0x0ee1_f3d9);
        assert_eq!(
            identity.registration,
            RegistrationCode::generate(0x0ee1_f3d9)
        );
        assert_ne!(identity.puid.ctr, identity.registration.counter);
        assert_ne!(identity.puid.crc, identity.registration.crc);
    }

    #[test]
    fn an_authenticating_logon_with_an_identity_keeps_the_authenticate_bit() {
        let identity = ClientIdentity {
            registration: RegistrationCode {
                crc: 0x0102_0304,
                counter: 0x0506_0708,
            },
            puid: Puid {
                ctr: 0x1122_3344,
                crc: 0x5566_7788,
            },
        };
        let record = authenticating_logon_record_with_identity("Rico", 0, identity);
        assert_eq!(record.crc, identity.registration.crc);
        assert_eq!(record.counter, identity.registration.counter);
        assert_eq!(record.puid_ctr, identity.puid.ctr);
        assert_eq!(record.puid_crc, identity.puid.crc);
        assert_eq!(
            record.aux_flags & aux_flags::AUTHENTICATE,
            aux_flags::AUTHENTICATE
        );
    }

    #[test]
    fn a_registration_code_maps_to_a_puid_the_way_the_reference_writes_it() {
        let code = RegistrationCode {
            crc: 0xaaaa_bbbb,
            counter: 0xcccc_dddd,
        };
        let puid = Puid::from(code);
        assert_eq!(puid.crc, code.crc);
        assert_eq!(puid.ctr, code.counter);
    }
}

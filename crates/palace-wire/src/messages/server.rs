//! The logon-burst messages that describe the server itself.

use crate::byteorder::{Reader, Writer};
use crate::error::Result;

/// `MSG_ALTLOGONREPLY` (`rep2`) carries an [`AuxRegistrationRec`] echo of the
/// client's own logon record. Sent when the server runs in
/// "guests-are-members" mode, and used to hand the client a corrected PUID.
pub type AltLogonReply = crate::messages::AuxRegistrationRec;

/// `MSG_VERSION` (`vers`): the version lives in the frame `refNum`, with the
/// major number in the high 16 bits and the minor in the low 16 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerVersion {
    /// Major version.
    pub major: u16,
    /// Minor version.
    pub minor: u16,
}

impl ServerVersion {
    /// Split a frame `refNum` into major/minor halves.
    pub fn from_ref_num(ref_num: i32) -> Self {
        ServerVersion {
            major: ((ref_num as u32) >> 16) as u16,
            minor: (ref_num as u32 & 0xffff) as u16,
        }
    }

    /// Recombine into the wire `refNum` value.
    pub fn to_ref_num(self) -> i32 {
        (((self.major as u32) << 16) | self.minor as u32) as i32
    }
}

impl std::fmt::Display for ServerVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// `MSG_SERVERINFO` (`sinf`): permissions word followed by the server name.
///
/// The protocol reference also lists `serverOptions` and two capability words,
/// but the live server omits them (Taj and OpenPalace both note the same
/// shortfall), so anything after the name is kept verbatim in
/// [`ServerInfo::trailing`] instead of being misread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// `PM_*` server permission bits.
    pub permissions: u32,
    /// Server name (`Str63`).
    pub name: String,
    /// Any bytes the server sent after the name.
    pub trailing: Vec<u8>,
}

impl ServerInfo {
    /// Decode a `sinf` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        let permissions = r.read_u32()?;
        let name = r.read_pstring()?;
        let trailing = r.read_bytes(r.remaining())?.to_vec();
        Ok(ServerInfo {
            permissions,
            name,
            trailing,
        })
    }

    /// Encode a `sinf` body (without `trailing`).
    pub fn encode(&self, w: &mut Writer) {
        w.write_u32(self.permissions);
        w.write_pstring(&self.name);
        w.write_bytes(&self.trailing);
    }
}

/// `MSG_HTTPSERVER` (`HTTP`): the base URL of the media server, as a C string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpServer {
    /// Base URL, e.g. `https://colosseum.thugdracule.com/palace/media`.
    pub url: String,
}

impl HttpServer {
    /// Decode an `HTTP` body.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(HttpServer {
            url: r.read_cstring()?,
        })
    }

    /// Encode an `HTTP` body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_cstring(&self.url);
    }
}

/// `MSG_USERLOG` (`log `): `refNum` is the user who logged on, the body is the
/// revised server-wide user count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserLog {
    /// Id of the user who logged on.
    pub user_id: i32,
    /// Total users on the server after the logon.
    pub user_count: i32,
}

impl UserLog {
    /// Decode a `log ` body.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        Ok(UserLog {
            user_id: ref_num,
            user_count: r.read_i32()?,
        })
    }

    /// Encode a `log ` body.
    pub fn encode(&self, w: &mut Writer) {
        w.write_i32(self.user_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::byteorder::ByteOrder;

    #[test]
    fn version_splits_ref_num() {
        let v = ServerVersion::from_ref_num(0x0001_0016);
        assert_eq!(
            v,
            ServerVersion {
                major: 1,
                minor: 22
            }
        );
        assert_eq!(v.to_ref_num(), 0x0001_0016);
        assert_eq!(v.to_string(), "1.22");
    }

    #[test]
    fn server_info_matches_live_capture() {
        // Live `sinf`: permissions 0x00002e7f, then `0d "Balamb Garden"`.
        let mut body = 0x0000_2e7fu32.to_le_bytes().to_vec();
        body.push(13);
        body.extend_from_slice(b"Balamb Garden");
        let mut r = Reader::new(&body, ByteOrder::Little);
        let info = ServerInfo::decode(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(info.permissions, 0x0000_2e7f);
        assert_eq!(info.name, "Balamb Garden");
        assert!(info.trailing.is_empty());
    }

    #[test]
    fn server_info_keeps_unexpected_tail() {
        let mut body = 1u32.to_le_bytes().to_vec();
        body.push(2);
        body.extend_from_slice(b"hi");
        body.extend_from_slice(&[9, 9, 9]);
        let mut r = Reader::new(&body, ByteOrder::Little);
        let info = ServerInfo::decode(&mut r).unwrap();
        assert_eq!(info.name, "hi");
        assert_eq!(info.trailing, vec![9, 9, 9]);
    }

    #[test]
    fn http_server_decodes_live_url() {
        let mut body = b"https://example.invalid/palace/media".to_vec();
        body.push(0);
        let mut r = Reader::new(&body, ByteOrder::Little);
        let h = HttpServer::decode(&mut r).unwrap();
        assert_eq!(h.url, "https://example.invalid/palace/media");
    }

    #[test]
    fn http_server_round_trips_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let h = HttpServer {
                url: "http://x/y".into(),
            };
            let mut w = Writer::new(order);
            h.encode(&mut w);
            let bytes = w.into_vec();
            let mut r = Reader::new(&bytes, order);
            assert_eq!(HttpServer::decode(&mut r).unwrap(), h);
        }
    }
}

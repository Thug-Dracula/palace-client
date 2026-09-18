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
    /// Base URL, e.g. `https://media.palace.example.info/palace/media`.
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

/// The reason a server gave for dropping the connection.
///
/// The code travels in the `SERVERDOWN` frame's `refNum` field, not in the body.
/// The reference documents 0-16; any other value is kept verbatim as
/// [`ServerDownReason::Undocumented`] across the full `u32` range rather than
/// lost, coerced or panicked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerDownReason {
    /// `K_Unknown` (0).
    Unknown,
    /// `K_LoggedOff` (1).
    LoggedOff,
    /// `K_CommError` (2).
    CommError,
    /// `K_Flood` (3).
    Flood,
    /// `K_KilledByPlayer` (4).
    KilledByPlayer,
    /// `K_ServerDown` (5).
    ServerDown,
    /// `K_Unresponsive` (6).
    Unresponsive,
    /// `K_KilledBySysop` (7).
    KilledBySysop,
    /// `K_ServerFull` (8).
    ServerFull,
    /// `K_InvalidSerialNumber` (9).
    InvalidSerialNumber,
    /// `K_DuplicateUser` (10).
    DuplicateUser,
    /// `K_DeathPenaltyActive` (11).
    DeathPenaltyActive,
    /// `K_Banished` (12).
    Banished,
    /// `K_BanishKill` (13).
    BanishKill,
    /// `K_NoGuests` (14).
    NoGuests,
    /// `K_DemoExpired` (15).
    DemoExpired,
    /// `K_Verbose` (16): the body carries the server's own explanation.
    Verbose,
    /// A code outside 0-16, kept as the wire sent it.
    Undocumented(u32),
}

impl ServerDownReason {
    /// Classify a frame `refNum`.
    #[must_use]
    pub const fn from_ref_num(ref_num: i32) -> Self {
        match ref_num {
            0 => ServerDownReason::Unknown,
            1 => ServerDownReason::LoggedOff,
            2 => ServerDownReason::CommError,
            3 => ServerDownReason::Flood,
            4 => ServerDownReason::KilledByPlayer,
            5 => ServerDownReason::ServerDown,
            6 => ServerDownReason::Unresponsive,
            7 => ServerDownReason::KilledBySysop,
            8 => ServerDownReason::ServerFull,
            9 => ServerDownReason::InvalidSerialNumber,
            10 => ServerDownReason::DuplicateUser,
            11 => ServerDownReason::DeathPenaltyActive,
            12 => ServerDownReason::Banished,
            13 => ServerDownReason::BanishKill,
            14 => ServerDownReason::NoGuests,
            15 => ServerDownReason::DemoExpired,
            16 => ServerDownReason::Verbose,
            other => ServerDownReason::Undocumented(other as u32),
        }
    }

    /// The wire code, exact for every input.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            ServerDownReason::Unknown => 0,
            ServerDownReason::LoggedOff => 1,
            ServerDownReason::CommError => 2,
            ServerDownReason::Flood => 3,
            ServerDownReason::KilledByPlayer => 4,
            ServerDownReason::ServerDown => 5,
            ServerDownReason::Unresponsive => 6,
            ServerDownReason::KilledBySysop => 7,
            ServerDownReason::ServerFull => 8,
            ServerDownReason::InvalidSerialNumber => 9,
            ServerDownReason::DuplicateUser => 10,
            ServerDownReason::DeathPenaltyActive => 11,
            ServerDownReason::Banished => 12,
            ServerDownReason::BanishKill => 13,
            ServerDownReason::NoGuests => 14,
            ServerDownReason::DemoExpired => 15,
            ServerDownReason::Verbose => 16,
            ServerDownReason::Undocumented(code) => code,
        }
    }

    /// The reference name for a documented code, `"undocumented"` otherwise.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ServerDownReason::Unknown => "K_Unknown",
            ServerDownReason::LoggedOff => "K_LoggedOff",
            ServerDownReason::CommError => "K_CommError",
            ServerDownReason::Flood => "K_Flood",
            ServerDownReason::KilledByPlayer => "K_KilledByPlayer",
            ServerDownReason::ServerDown => "K_ServerDown",
            ServerDownReason::Unresponsive => "K_Unresponsive",
            ServerDownReason::KilledBySysop => "K_KilledBySysop",
            ServerDownReason::ServerFull => "K_ServerFull",
            ServerDownReason::InvalidSerialNumber => "K_InvalidSerialNumber",
            ServerDownReason::DuplicateUser => "K_DuplicateUser",
            ServerDownReason::DeathPenaltyActive => "K_DeathPenaltyActive",
            ServerDownReason::Banished => "K_Banished",
            ServerDownReason::BanishKill => "K_BanishKill",
            ServerDownReason::NoGuests => "K_NoGuests",
            ServerDownReason::DemoExpired => "K_DemoExpired",
            ServerDownReason::Verbose => "K_Verbose",
            ServerDownReason::Undocumented(_) => "undocumented",
        }
    }

    /// The user-visible explanation for a documented code. An undocumented code
    /// is reported by its number, never given a borrowed name.
    #[must_use]
    pub fn text(self) -> String {
        match self {
            ServerDownReason::Unknown => "the server ended the session".to_string(),
            ServerDownReason::LoggedOff => "you were logged off".to_string(),
            ServerDownReason::CommError => "a communications error ended the session".to_string(),
            ServerDownReason::Flood => "disconnected for flooding".to_string(),
            ServerDownReason::KilledByPlayer => "another user disconnected you".to_string(),
            ServerDownReason::ServerDown => "the server is shutting down".to_string(),
            ServerDownReason::Unresponsive => "disconnected for not responding".to_string(),
            ServerDownReason::KilledBySysop => "a sysop disconnected you".to_string(),
            ServerDownReason::ServerFull => "the server is full".to_string(),
            ServerDownReason::InvalidSerialNumber => {
                "the server rejected your serial number".to_string()
            }
            ServerDownReason::DuplicateUser => {
                "another user is already using this account".to_string()
            }
            ServerDownReason::DeathPenaltyActive => {
                "your death penalty is still active".to_string()
            }
            ServerDownReason::Banished => "you were banished from this server".to_string(),
            ServerDownReason::BanishKill => {
                "you were banished and disconnected from this server".to_string()
            }
            ServerDownReason::NoGuests => "guests are not allowed on this server".to_string(),
            ServerDownReason::DemoExpired => "your free demo has expired".to_string(),
            ServerDownReason::Verbose => "the server ended the session".to_string(),
            ServerDownReason::Undocumented(code) => {
                format!("undocumented server reason code {code}")
            }
        }
    }
}

/// `MSG_SERVERDOWN` (`down`): the server is dropping the connection.
///
/// The reason is the frame `refNum`, not a body field, so decoding needs the
/// operand passed in. The body is empty unless the reason is
/// [`ServerDownReason::Verbose`], when it is a `CString` with the server's own
/// message (protocol reference :1800-1828).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDown {
    /// Why the server is dropping the connection.
    pub reason: ServerDownReason,
    /// The server's own explanation, present only for `K_Verbose`.
    pub message: Option<String>,
}

impl ServerDown {
    /// Classify a bare `refNum` without reading a body.
    #[must_use]
    pub const fn from_ref_num(ref_num: i32) -> Self {
        ServerDown {
            reason: ServerDownReason::from_ref_num(ref_num),
            message: None,
        }
    }

    /// Decode a `down` body. `ref_num` is the frame operand carrying the reason;
    /// the body is read only for `K_Verbose`.
    pub fn decode(ref_num: i32, r: &mut Reader<'_>) -> Result<Self> {
        let reason = ServerDownReason::from_ref_num(ref_num);
        let message = if reason == ServerDownReason::Verbose && !r.is_empty() {
            Some(r.read_cstring()?)
        } else {
            None
        };
        Ok(ServerDown { reason, message })
    }

    /// Encode a `down` body. The reason is the frame's `refNum`, so only the
    /// optional `CString` is written.
    pub fn encode(&self, w: &mut Writer) {
        if let Some(message) = &self.message {
            w.write_cstring(message);
        }
    }

    /// The user-visible explanation: the server's own words for a verbose
    /// reason, otherwise the text for the code.
    #[must_use]
    pub fn reason_text(&self) -> String {
        if self.reason == ServerDownReason::Verbose {
            if let Some(message) = self.message.as_deref() {
                if !message.is_empty() {
                    return message.to_string();
                }
            }
        }
        self.reason.text()
    }

    /// One-line summary for logs and the probe.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "server down: {} ({}): {}",
            self.reason.label(),
            self.reason.code(),
            self.reason_text()
        )
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

    /// Every documented reason, with a phrase its text must contain. Codes 0-16
    /// (17 total) cover the whole documented enum.
    const DOCUMENTED_REASONS: [(i32, ServerDownReason, &str); 17] = [
        (0, ServerDownReason::Unknown, "ended the session"),
        (1, ServerDownReason::LoggedOff, "logged off"),
        (2, ServerDownReason::CommError, "communications error"),
        (3, ServerDownReason::Flood, "flooding"),
        (4, ServerDownReason::KilledByPlayer, "another user"),
        (5, ServerDownReason::ServerDown, "shutting down"),
        (6, ServerDownReason::Unresponsive, "not responding"),
        (7, ServerDownReason::KilledBySysop, "sysop"),
        (8, ServerDownReason::ServerFull, "full"),
        (9, ServerDownReason::InvalidSerialNumber, "serial number"),
        (10, ServerDownReason::DuplicateUser, "already using"),
        (11, ServerDownReason::DeathPenaltyActive, "death penalty"),
        (12, ServerDownReason::Banished, "banished"),
        (13, ServerDownReason::BanishKill, "banished"),
        (14, ServerDownReason::NoGuests, "guests"),
        (15, ServerDownReason::DemoExpired, "demo"),
        (16, ServerDownReason::Verbose, "ended the session"),
    ];

    #[test]
    fn server_down_names_every_documented_reason_in_both_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            for (code, expected, phrase) in DOCUMENTED_REASONS {
                let empty: [u8; 0] = [];
                let mut r = Reader::new(&empty, order);
                let down = ServerDown::decode(code, &mut r).expect("an empty down decodes");
                assert_eq!(down.reason, expected, "code {code} under {order:?}");
                assert!(down.message.is_none());
                assert!(r.is_empty());
                assert_eq!(down.reason.code(), code as u32);
                assert!(
                    down.reason_text().contains(phrase),
                    "code {code} should read as {phrase:?}, got {:?}",
                    down.reason_text()
                );
                assert!(down.describe().contains(&code.to_string()));
            }
        }
    }

    #[test]
    fn server_down_documented_reason_codes_match_their_labels() {
        for (code, reason, _) in DOCUMENTED_REASONS {
            assert_eq!(reason.code(), code as u32);
            assert_eq!(
                ServerDownReason::from_ref_num(code),
                reason,
                "code {code} must round-trip through from_ref_num"
            );
            if !matches!(reason, ServerDownReason::Undocumented(_)) {
                assert!(
                    reason.label().starts_with("K_"),
                    "a documented code must be named, got {:?}",
                    reason.label()
                );
            }
        }
    }

    #[test]
    fn a_verbose_server_down_carries_the_servers_own_message() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut w = Writer::new(order);
            w.write_cstring("scheduled maintenance");
            let body = w.into_vec();

            let mut r = Reader::new(&body, order);
            let down = ServerDown::decode(16, &mut r).expect("a verbose down decodes");
            assert_eq!(down.reason, ServerDownReason::Verbose);
            assert_eq!(down.message.as_deref(), Some("scheduled maintenance"));
            assert!(r.is_empty());
            assert_eq!(down.reason_text(), "scheduled maintenance");
            assert!(down.describe().contains("scheduled maintenance"));

            let mut w = Writer::new(order);
            down.encode(&mut w);
            assert_eq!(w.into_vec(), body);

            // An empty verbose body falls back to the code's text rather than
            // failing or inventing a message.
            let empty: [u8; 0] = [];
            let mut r = Reader::new(&empty, order);
            let bare = ServerDown::decode(16, &mut r).expect("an empty verbose down decodes");
            assert!(bare.message.is_none());
            assert_eq!(bare.reason_text(), "the server ended the session");
        }

        // A non-verbose code's body is empty and round-trips with no message.
        let mut w = Writer::new(ByteOrder::Little);
        ServerDown {
            reason: ServerDownReason::Banished,
            message: None,
        }
        .encode(&mut w);
        assert!(w.is_empty(), "a non-verbose down has no body");
    }

    #[test]
    fn server_down_preserves_an_out_of_range_ref_num() {
        for code in [-1i32, 17, 999, i32::MIN, i32::MAX] {
            let down = ServerDown::from_ref_num(code);
            assert_eq!(
                down.reason,
                ServerDownReason::Undocumented(code as u32),
                "an undocumented code must be kept verbatim"
            );
            assert_eq!(down.reason.code(), code as u32);
            assert_eq!(down.reason.label(), "undocumented");
            let text = down.reason_text();
            assert!(
                text.contains(&(code as u32).to_string()),
                "the numeric code must be reported: {text}"
            );
            for (_, _, phrase) in DOCUMENTED_REASONS {
                assert!(
                    !text.contains(phrase),
                    "an undocumented code must not borrow {phrase:?}: {text}"
                );
            }
        }
    }

    #[test]
    fn a_wrong_looking_verbose_body_is_an_error_not_a_panic() {
        // A K_Verbose body with no NUL terminator cannot be a CString.
        let body = b"unterminated".to_vec();
        let mut r = Reader::new(&body, ByteOrder::Little);
        assert!(ServerDown::decode(16, &mut r).is_err());
    }
}

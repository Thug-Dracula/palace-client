//! The Palace opcode table.
//!
//! An opcode is a 32-bit value whose four bytes spell a four-character ASCII
//! mnemonic when laid out big-endian. For example `0x7265_6769` is `r`,`e`,`g`,`i`
//! (`MSG_LOGON`). On the wire the value is written in the *session* byte order,
//! so on a little-endian server the bytes read `iger`. Decoding with the session
//! [`ByteOrder`](crate::ByteOrder) recovers the canonical value, which is why
//! [`Opcode`] is a plain `u32` wrapper with no byte juggling of its own.
//!
//! Unknown opcodes are **not** errors. Palace deployments grow extra messages
//! (plugins, forks, `MSG_BLOWTHRU` payloads), and the project requirement is
//! tolerant parsing: an unknown opcode must be logged and skipped, never abort
//! the session. [`Opcode::name`] therefore returns `None` for them and
//! [`Opcode::mnemonic`] still renders the four bytes when printable.

use std::fmt;

/// A 32-bit Palace message type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Opcode(pub u32);

/// `0x72656769`
pub const LOGON: Opcode = Opcode(0x7265_6769);
/// `0x72657032`
pub const ALTLOGONREPLY: Opcode = Opcode(0x7265_7032);
/// `0x74697972` — "This Is Your ID", always the server's first packet.
pub const TIYID: Opcode = Opcode(0x7469_7972);
/// `0x70736572` — HTTP-tunnel mode marker (unsupported).
pub const TROPSER: Opcode = Opcode(0x7073_6572);
/// `0x724c7374` — room list request/response.
pub const LISTOFALLROOMS: Opcode = Opcode(0x724c_7374);
/// `0x754c7374` — user list request/response.
pub const LISTOFALLUSERS: Opcode = Opcode(0x754c_7374);
/// `0x726f6f6d` — room description.
pub const ROOMDESC: Opcode = Opcode(0x726f_6f6d);
/// `0x656e6472` — room description end marker.
pub const ROOMDESCEND: Opcode = Opcode(0x656e_6472);
/// `0x6e705273`-style: `0x6e707273` — a user entered the room.
pub const USERNEW: Opcode = Opcode(0x6e70_7273);
/// `0x72707273` — users in the current room.
pub const USERLIST: Opcode = Opcode(0x7270_7273);
/// `0x65707273` — a user left the room.
pub const USEREXIT: Opcode = Opcode(0x6570_7273);
/// `0x6c6f6720` — `log ` — a user logged onto the server.
pub const USERLOG: Opcode = Opcode(0x6c6f_6720);
/// `0x73696e66` — server info.
pub const SERVERINFO: Opcode = Opcode(0x7369_6e66);
/// `0x76657273` — server version.
pub const VERSION: Opcode = Opcode(0x7665_7273);
/// `0x48545450` — media/HTTP server URL.
pub const HTTPSERVER: Opcode = Opcode(0x4854_5450);
/// `0x70696e67` — keepalive request.
pub const PING: Opcode = Opcode(0x7069_6e67);
/// `0x706f6e67` — keepalive response.
pub const PONG: Opcode = Opcode(0x706f_6e67);
/// `0x62796520` — `bye ` — logoff.
pub const LOGOFF: Opcode = Opcode(0x6279_6520);
/// `0x6e617652` — request to move to a room.
pub const ROOMGOTO: Opcode = Opcode(0x6e61_7652);
/// `0x754c6f63` — a user moved.
pub const USERMOVE: Opcode = Opcode(0x754c_6f63);
/// `0x75537461` — own user status/flags.
pub const USERSTATUS: Opcode = Opcode(0x7553_7461);
/// `0x74616c6b` — public chat.
pub const TALK: Opcode = Opcode(0x7461_6c6b);
/// `0x77686973` — private chat.
pub const WHISPER: Opcode = Opcode(0x7768_6973);
/// `0x78746c6b` — encrypted public chat.
pub const XTALK: Opcode = Opcode(0x7874_6c6b);
/// `0x78776973` — encrypted private chat.
pub const XWHISPER: Opcode = Opcode(0x7877_6973);
/// `0x71417374` — asset query.
pub const ASSETQUERY: Opcode = Opcode(0x7141_7374);
/// `0x72417374` — asset registration (client → server).
pub const ASSETREGI: Opcode = Opcode(0x7241_7374);
/// `0x73417374` — asset send.
pub const ASSETSEND: Opcode = Opcode(0x7341_7374);
/// `0x61417374` — asset new.
pub const ASSETNEW: Opcode = Opcode(0x6141_7374);
/// `0x75737244` — user description.
pub const USERDESC: Opcode = Opcode(0x7573_7244);
/// `0x75737246` — user face.
pub const USERFACE: Opcode = Opcode(0x7573_7246);
/// `0x75737243` — user colour.
pub const USERCOLOR: Opcode = Opcode(0x7573_7243);
/// `0x75737250` — user props.
pub const USERPROP: Opcode = Opcode(0x7573_7250);
/// `0x7573724e` — user name change.
pub const USERNAME: Opcode = Opcode(0x7573_724e);
/// `0x6e707273` alias kept for symmetry with the other `*prs` names.
pub const USERENTER: Opcode = Opcode(0x7770_7273);
pub const DRAW: Opcode = Opcode(0x6472_6177);
pub const DOORLOCK: Opcode = Opcode(0x6c6f_636b);
pub const DOORUNLOCK: Opcode = Opcode(0x756e_6c6f);
pub const PROPNEW: Opcode = Opcode(0x6e50_7270);
pub const PROPDEL: Opcode = Opcode(0x6450_7270);
pub const PROPMOVE: Opcode = Opcode(0x6d50_7270);
pub const SPOTSTATE: Opcode = Opcode(0x7353_7461);
/// `0x6f70536e` — a hotspot was created; empty body.
pub const SPOTNEW: Opcode = Opcode(0x6f70_536e);
/// `0x6f705364` — a hotspot was deleted; one `HotspotID`.
pub const SPOTDEL: Opcode = Opcode(0x6f70_5364);
/// `0x636f4c73` — a hotspot moved; a `RoomID`, `HotspotID` and `Point`.
pub const SPOTMOVE: Opcode = Opcode(0x636f_4c73);
/// `0x704c6f63` — a picture moved; a `RoomID`, `HotspotID` and `Point`.
pub const PICTMOVE: Opcode = Opcode(0x704c_6f63);
/// `0x61757468` — the server asking the client to authenticate; empty body.
pub const AUTHENTICATE: Opcode = Opcode(0x6175_7468);
/// `0x7345_7272` — the server refused a room change; the failure code is the
/// frame's `refNum` and the body is empty (protocol reference :1371).
pub const NAVERROR: Opcode = Opcode(0x7345_7272);
/// `0x646f_776e` — the server is dropping the connection. The reason code is
/// the frame's `refNum`; the body is empty unless the reason is
/// `K_Verbose` (16), when it is a `CString` explanation (protocol reference
/// :1800-1828).
pub const SERVERDOWN: Opcode = Opcode(0x646f_776e);

/// Full opcode table: every message type we have a name for, taken from the
/// official 1999 protocol reference, Taj's `MessageTypes.cs`, and QPalace's
/// `message.hpp`.
///
/// This is the authoritative list the probe reports and the README documents.
pub const TABLE: &[(u32, &str, &str)] = &[
    (0x7265_7032, "rep2", "ALTLOGONREPLY"),
    (0x6141_7374, "aAst", "ASSETNEW"),
    (0x7141_7374, "qAst", "ASSETQUERY"),
    (0x7241_7374, "rAst", "ASSETREGI"),
    (0x7341_7374, "sAst", "ASSETSEND"),
    (0x6175_7468, "auth", "AUTHENTICATE"),
    (0x6175_7472, "autr", "AUTHRESPONSE"),
    (0x626c_6f77, "blow", "BLOWTHRU"),
    (0x6475_726c, "durl", "DISPLAYURL"),
    (0x7279_6974, "ryit", "DIYIT"),
    (0x6c6f_636b, "lock", "DOORLOCK"),
    (0x756e_6c6f, "unlo", "DOORUNLOCK"),
    (0x6472_6177, "draw", "DRAW"),
    (0x7349_6e66, "sInf", "EXTENDEDINFO"),
    (0x666e_6665, "fnfe", "FILENOTFND"),
    (0x7146_696c, "qFil", "FILEQUERY"),
    (0x7346_696c, "sFil", "FILESEND"),
    (0x676d_7367, "gmsg", "GMSG"),
    (0x4854_5450, "HTTP", "HTTPSERVER"),
    (0x634c_6f67, "cLog", "INITCONNECTION"),
    (0x6b69_6c6c, "kill", "KILLUSER"),
    (0x724c_7374, "rLst", "LISTOFALLROOMS"),
    (0x754c_7374, "uLst", "LISTOFALLUSERS"),
    (0x6279_6520, "bye ", "LOGOFF"),
    (0x7265_6769, "regi", "LOGON"),
    (0x7345_7272, "sErr", "NAVERROR"),
    (0x4e4f_4f50, "NOOP", "NOOP"),
    (0x4650_5371, "FPSq", "PICTDEL"),
    (0x704c_6f63, "pLoc", "PICTMOVE"),
    (0x6e50_6374, "nPct", "PICTNEW"),
    (0x7350_6374, "sPct", "PICTSETDESC"),
    (0x7069_6e67, "ping", "PING"),
    (0x706f_6e67, "pong", "PONG"),
    (0x6450_7270, "dPrp", "PROPDEL"),
    (0x6d50_7270, "mPrp", "PROPMOVE"),
    (0x6e50_7270, "nPrp", "PROPNEW"),
    (0x7350_7270, "sPrp", "PROPSETDESC"),
    (0x7265_7370, "resp", "RESPORT"),
    (0x726d_7367, "rmsg", "RMSG"),
    (0x726f_6f6d, "room", "ROOMDESC"),
    (0x656e_6472, "endr", "ROOMDESCEND"),
    (0x6e61_7652, "navR", "ROOMGOTO"),
    (0x6e52_6f6d, "nRom", "ROOMNEW"),
    (0x7352_6f6d, "sRom", "ROOMSETDESC"),
    (0x646f_776e, "down", "SERVERDOWN"),
    (0x7369_6e66, "sinf", "SERVERINFO"),
    (0x696e_6974, "init", "SERVERUP"),
    (0x736d_7367, "smsg", "SMSG"),
    (0x6f70_5364, "opSd", "SPOTDEL"),
    (0x636f_4c73, "coLs", "SPOTMOVE"),
    (0x6f70_536e, "opSn", "SPOTNEW"),
    (0x6f70_5373, "opSs", "SPOTSETDESC"),
    (0x7353_7461, "sSta", "SPOTSTATE"),
    (0x7375_7372, "susr", "SUPERUSER"),
    (0x7461_6c6b, "talk", "TALK"),
    (0x7469_6d79, "timy", "TIMYID"),
    (0x7469_7972, "tiyr", "TIYID"),
    (0x7073_6572, "pser", "TROPSER"),
    (0x7573_7243, "usrC", "USERCOLOR"),
    (0x7573_7244, "usrD", "USERDESC"),
    (0x7770_7273, "wprs", "USERENTER"),
    (0x6570_7273, "eprs", "USEREXIT"),
    (0x7573_7246, "usrF", "USERFACE"),
    (0x7270_7273, "rprs", "USERLIST"),
    (0x6c6f_6720, "log ", "USERLOG"),
    (0x754c_6f63, "uLoc", "USERMOVE"),
    (0x7573_724e, "usrN", "USERNAME"),
    (0x6e70_7273, "nprs", "USERNEW"),
    (0x7573_7250, "usrP", "USERPROP"),
    (0x7553_7461, "uSta", "USERSTATUS"),
    (0x7665_7273, "vers", "VERSION"),
    (0x7768_6973, "whis", "WHISPER"),
    (0x776d_7367, "wmsg", "WMSG"),
    (0x7874_6c6b, "xtlk", "XTALK"),
    (0x7877_6973, "xwis", "XWHISPER"),
];

impl Opcode {
    /// Wrap a raw 32-bit value.
    pub const fn new(value: u32) -> Self {
        Opcode(value)
    }

    /// Pack a four-byte ASCII mnemonic into an opcode, big-endian.
    ///
    /// `Opcode::from_mnemonic(b"regi") == LOGON`.
    pub const fn from_mnemonic(mnemonic: &[u8; 4]) -> Self {
        Opcode(u32::from_be_bytes(*mnemonic))
    }

    /// The raw value.
    pub const fn value(self) -> u32 {
        self.0
    }

    /// The four ASCII bytes that spell this opcode, big-endian.
    pub const fn mnemonic_bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// The mnemonic, with non-printable bytes replaced by `.`.
    pub fn mnemonic(self) -> String {
        self.mnemonic_bytes()
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect()
    }

    /// The protocol name (e.g. `"LOGON"`), or `None` when unknown.
    pub fn name(self) -> Option<&'static str> {
        TABLE
            .iter()
            .find(|(v, _, _)| *v == self.0)
            .map(|(_, _, name)| *name)
    }

    /// True when this opcode appears in [`TABLE`].
    pub fn is_known(self) -> bool {
        self.name().is_some()
    }

    /// A compact human-readable form: `regi(LOGON)` for known opcodes and
    /// `0xdeadbeef(unknown)` otherwise. Safe on any 32-bit input.
    pub fn describe(self) -> String {
        match self.name() {
            Some(name) => format!("{}({})", self.mnemonic(), name),
            None => format!("0x{:08x}(unknown)", self.0),
        }
    }
}

impl From<u32> for Opcode {
    fn from(v: u32) -> Self {
        Opcode(v)
    }
}

impl From<Opcode> for u32 {
    fn from(o: Opcode) -> Self {
        o.0
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.mnemonic())
    }
}

impl fmt::Debug for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opcode({:#010x} {})", self.0, self.mnemonic())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_pack_big_endian() {
        assert_eq!(Opcode::from_mnemonic(b"regi"), LOGON);
        assert_eq!(Opcode::from_mnemonic(b"tiyr"), TIYID);
        assert_eq!(Opcode::from_mnemonic(b"rLst"), LISTOFALLROOMS);
        assert_eq!(LOGON.value(), 0x7265_6769);
        assert_eq!(LOGON.mnemonic(), "regi");
        assert_eq!(LOGON.name(), Some("LOGON"));
    }

    #[test]
    fn every_table_entry_is_self_consistent() {
        for (value, mnemonic, name) in TABLE {
            let op = Opcode::new(*value);
            assert_eq!(&op.mnemonic(), mnemonic, "mnemonic mismatch for {name}");
            assert_eq!(op.name(), Some(*name), "name mismatch for {mnemonic}");
        }
    }

    #[test]
    fn table_has_no_duplicate_values() {
        let mut seen = std::collections::BTreeSet::new();
        for (value, _, name) in TABLE {
            assert!(seen.insert(*value), "duplicate opcode value for {name}");
        }
    }

    #[test]
    fn doc_comment_constants_are_in_the_table() {
        for op in [
            LOGON,
            ALTLOGONREPLY,
            TIYID,
            TROPSER,
            LISTOFALLROOMS,
            LISTOFALLUSERS,
            ROOMDESC,
            ROOMDESCEND,
            USERNEW,
            USERLIST,
            USEREXIT,
            USERLOG,
            SERVERINFO,
            VERSION,
            HTTPSERVER,
            PING,
            PONG,
            LOGOFF,
        ] {
            assert!(op.is_known(), "{} missing from TABLE", op.describe());
        }
    }

    #[test]
    fn unknown_opcodes_are_tolerated() {
        let unknown = Opcode::new(0xdead_beef);
        assert!(!unknown.is_known());
        assert_eq!(unknown.name(), None);
        assert_eq!(unknown.describe(), "0xdeadbeef(unknown)");
        // Non-printable bytes render as dots rather than panicking.
        assert_eq!(Opcode::new(0x0001_0203).mnemonic(), "....");
    }
}

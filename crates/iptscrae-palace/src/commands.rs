//! The Palace command surface: names, documented stack effects, and the
//! adapter that serves them from a [`PalaceHost`].
//!
//! [`PALACE_COMMANDS`] is the list every Palace dialect ships, with the operand
//! counts the language guide documents. It exists for two reasons:
//!
//! 1. **Lexing.** A symbol is a command only if it is registered. Registering
//!    the Palace names means `SAY` lexes as a command and `MYVAR` does not.
//! 2. **Stack discipline.** A host that cannot implement a command still needs
//!    to consume the right operands, or every later instruction sees a
//!    corrupted stack. The counts come from the guide's quick-reference table.
//!
//! [`PalaceCommands`] is the intended way to *implement* commands: it turns
//! [`PalaceHost`] methods into registered host commands, and is the pattern the
//! full Palace command set will follow. It implements a handful of commands
//! here as proof that the trait and the VM fit together.

use iptscrae::error::{IptError, Result};
use iptscrae::registry::CommandSet;
use iptscrae::value::Value;
use iptscrae::Host;

use crate::traits::PalaceHost;

/// What a Palace command leaves on the stack when the host stubs it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// Nothing.
    None,
    /// A number, defaulting to 0.
    Int,
    /// A string, defaulting to empty.
    Str,
}

/// A Palace command's name and documented stack effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// Upper-cased command name.
    pub name: &'static str,
    /// Operands consumed.
    pub pops: u8,
    /// Values produced.
    pub pushes: u8,
    /// Kind of each produced value.
    pub push: Push,
}

const fn spec(name: &'static str, pops: u8, pushes: u8, push: Push) -> CommandSpec {
    CommandSpec {
        name,
        pops,
        pushes,
        push,
    }
}

const NONE: Push = Push::None;
const INT: Push = Push::Int;
const STR: Push = Push::Str;

/// Every Palace command whose stack effect the reference implementations
/// document, grouped the way the language guide groups them.
///
/// Core bindings are preserved except for `IPTVERSION`, deliberately overridden
/// to report the Palace version (2) rather than the generic engine version (1).
///
/// Extended signatures below cite `reference/repos/sparky/index.js` under the
/// colosseum reference root: the `GS` command table and handler classes on line 7.
/// The OpenPalace AS3Iptscrae and PalaceClient command directories have no classes
/// for these additions. Legacy words with no known stack effect are registered
/// at the end of the table with zero operands, so a call is an explicit, traced
/// refusal rather than a silent auto-vivified variable; guessing an operand
/// count would corrupt the stack. `TEXT` is not `DRAWTEXT`.
pub const PALACE_COMMANDS: &[CommandSpec] = &[
    // ---- messaging --------------------------------------------------------
    spec("SAY", 1, 0, NONE),
    spec("CHAT", 1, 0, NONE),
    spec("SAYAT", 3, 0, NONE),
    spec("PRIVATEMSG", 2, 0, NONE),
    spec("GLOBALMSG", 1, 0, NONE),
    spec("ROOMMSG", 1, 0, NONE),
    spec("LOCALMSG", 1, 0, NONE),
    spec("SUSRMSG", 1, 0, NONE),
    spec("STATUSMSG", 1, 0, NONE),
    spec("LOGMSG", 1, 0, NONE),
    // ---- identity and state ----------------------------------------------
    spec("ME", 0, 1, INT),
    // `ID` is the hotspot id, the same value as `ME`
    // (`PalaceIptscraeCommands.as:29` maps it to `MECommand`); it is not the
    // user id, which is `USERID`/`WHOME`.
    spec("ID", 0, 1, INT),
    spec("USERID", 0, 1, INT),
    spec("WHOME", 0, 1, INT),
    spec("USERNAME", 0, 1, STR),
    spec("SERVERNAME", 0, 1, STR),
    spec("ROOMID", 0, 1, INT),
    spec("ROOMNAME", 0, 1, STR),
    spec("ROOMWIDTH", 0, 1, INT),
    spec("ROOMHEIGHT", 0, 1, INT),
    spec("POSX", 0, 1, INT),
    spec("POSY", 0, 1, INT),
    spec("MOUSEPOS", 0, 2, INT),
    // `MOUSEX`/`MOUSEY` have host arms (`ScriptHost`) but no reference command
    // table entry (`PalaceIptscraeCommands.as`, `sparky/index.js`). Registering
    // them would shadow the arena's `mousex`/`mousey` variables — the corpus
    // declares them with `mousex GLOBAL` — because PalaceChat resolves symbols
    // to commands case-insensitively, so they stay unregistered variables.
    spec("CLIENTTYPE", 0, 1, STR),
    spec("OPENPALACE", 0, 1, INT),
    spec("IPTVERSION", 0, 1, INT),
    spec("ISGOD", 0, 1, INT),
    spec("ISGUEST", 0, 1, INT),
    spec("ISWIZARD", 0, 1, INT),
    // Sparky GS `ISRIGHTCLICK:RS` pushes one int.
    spec("ISRIGHTCLICK", 0, 1, INT),
    spec("WHOCHAT", 0, 1, INT),
    spec("WHOTARGET", 0, 1, INT),
    spec("DEST", 0, 1, INT),
    spec("NBRDOORS", 0, 1, INT),
    spec("NBRLOOSEPROPS", 0, 1, INT),
    spec("NBRROOMUSERS", 0, 1, INT),
    spec("NBRSPOTS", 0, 1, INT),
    spec("NBRUSERPROPS", 0, 1, INT),
    spec("TOPPROP", 0, 1, INT),
    // ---- lookups ----------------------------------------------------------
    spec("GETSPOTLOC", 1, 2, INT),
    spec("GETSPOTSTATE", 1, 1, INT),
    spec("GETPICLOC", 2, 2, INT),
    spec("GETPICDIMENSIONS", 2, 2, INT),
    spec("SPOTDEST", 1, 1, INT),
    spec("SPOTIDX", 1, 1, INT),
    spec("SPOTNAME", 1, 1, STR),
    spec("ISLOCKED", 1, 1, INT),
    spec("INSPOT", 1, 1, INT),
    spec("ROOMUSER", 1, 1, INT),
    spec("WHONAME", 1, 1, STR),
    spec("WHOPOS", 1, 2, INT),
    spec("USERPROP", 1, 1, INT),
    spec("LOOSEPROP", 1, 1, INT),
    spec("LOOSEPROPIDX", 1, 1, INT),
    spec("LOOSEPROPPOS", 1, 2, INT),
    spec("PROPDIMENSIONS", 1, 2, INT),
    spec("PROPOFFSETS", 1, 2, INT),
    spec("DOORIDX", 1, 0, NONE),
    // ---- spot mutation ----------------------------------------------------
    spec("SETSPOTSTATE", 2, 0, NONE),
    spec("SETSPOTSTATELOCAL", 2, 0, NONE),
    spec("SETSPOTNAMELOCAL", 2, 0, NONE),
    spec("SETLOC", 3, 0, NONE),
    spec("SETLOCLOCAL", 3, 0, NONE),
    spec("SETPICLOC", 3, 0, NONE),
    spec("SETPICLOCLOCAL", 4, 0, NONE),
    spec("SETPICOPACITY", 3, 0, NONE),
    spec("SETPICBRIGHTNESS", 3, 0, NONE),
    spec("SETPICSATURATION", 3, 0, NONE),
    spec("LOCK", 1, 0, NONE),
    spec("UNLOCK", 1, 0, NONE),
    spec("SELECT", 1, 0, NONE),
    spec("KILLUSER", 1, 0, NONE),
    spec("DIMROOM", 1, 0, NONE),
    spec("MACRO", 1, 0, NONE),
    spec("SETCOLOR", 1, 0, NONE),
    spec("SETFACE", 1, 0, NONE),
    spec("SETUSERNAME", 1, 0, NONE),
    spec("SETALARM", 2, 0, NONE),
    spec("ALARMEXEC", 2, 0, NONE),
    spec("ADDLOOSEPROP", 3, 0, NONE),
    spec("REMOVELOOSEPROP", 1, 0, NONE),
    spec("MOVELOOSEPROP", 3, 0, NONE),
    spec("DROPPROP", 2, 0, NONE),
    spec("CLEARLOOSEPROPS", 0, 0, NONE),
    spec("SHOWLOOSEPROPS", 0, 0, NONE),
    spec("HIDEAVATARS", 0, 0, NONE),
    spec("SHOWAVATARS", 0, 0, NONE),
    // ---- props ------------------------------------------------------------
    spec("DONPROP", 1, 0, NONE),
    spec("REMOVEPROP", 1, 0, NONE),
    spec("DOFFPROP", 0, 0, NONE),
    spec("NAKED", 0, 0, NONE),
    spec("CLEARPROPS", 0, 0, NONE),
    spec("HASPROP", 1, 1, INT),
    spec("SETPROPS", 1, 0, NONE),
    spec("LOADPROPS", 1, 0, NONE),
    // ---- movement and paint ----------------------------------------------
    spec("SETPOS", 2, 0, NONE),
    spec("MOVE", 2, 0, NONE),
    spec("GOTOROOM", 1, 0, NONE),
    spec("ROOMGOTO", 1, 0, NONE),
    spec("NETGOTO", 1, 0, NONE),
    spec("GOTOURL", 1, 0, NONE),
    spec("GOTOURLFRAME", 2, 0, NONE),
    spec("LAUNCHAPP", 1, 0, NONE),
    spec("LAUNCHEVENT", 1, 0, NONE),
    spec("LAUNCHPPA", 1, 0, NONE),
    spec("LOADJAVA", 1, 0, NONE),
    // Shell command string; Sparky GS SHELLCMD:gb -> sn consumes one string (stub).
    // Already dispatched through the host's default unsupported-command arm.
    spec("SHELLCMD", 1, 0, NONE),
    spec("TALKPPA", 1, 0, NONE),
    spec("MIDIPLAY", 1, 0, NONE),
    spec("MIDILOOP", 2, 0, NONE),
    spec("MIDISTOP", 0, 0, NONE),
    spec("SOUND", 1, 0, NONE),
    spec("LINE", 4, 0, NONE),
    spec("LINETO", 2, 0, NONE),
    spec("PENPOS", 2, 0, NONE),
    spec("PENTO", 2, 0, NONE),
    spec("PENSIZE", 1, 0, NONE),
    spec("PENCOLOR", 3, 0, NONE),
    spec("PENBACK", 0, 0, NONE),
    spec("PENFRONT", 0, 0, NONE),
    spec("PAINTCLEAR", 0, 0, NONE),
    spec("PAINTUNDO", 0, 0, NONE),
    // ---- web and misc extensions -----------------------------------------
    // Automatic user-layer flag; Sparky GS AUTOUSERLAYER:kS consumes one int (stub).
    spec("AUTOUSERLAYER", 1, 0, NONE),
    // Set tooltip text; Sparky GS SETTOOLTIP:Rc pops one string.
    spec("SETTOOLTIP", 1, 0, NONE),
    // Clear the tooltip; Sparky GS CLEARTOOLTIP:Oc reads/writes no stack values.
    spec("CLEARTOOLTIP", 0, 0, NONE),
    // Set spot type, top-layer flag and flags; Sparky GS SETSPOTOPTIONS:vb pops four ints.
    spec("SETSPOTOPTIONS", 4, 0, NONE),
    // Append a picture to a spot; Sparky GS ADDPIC:C1 pops spot id and filename.
    spec("ADDPIC", 2, 0, NONE),
    // Remove a spot picture by index; Sparky GS REMOVEPIC:_1 pops two ints.
    spec("REMOVEPIC", 2, 0, NONE),
    // Add a polygon spot and return its id; Sparky GS ADDSPOT:k1 pops two ints and an array.
    spec("ADDSPOT", 3, 1, INT),
    // Set a spot event handler; Sparky GS SETSPOTSCRIPT:x1 pops id, event string and script.
    spec("SETSPOTSCRIPT", 3, 0, NONE),
    // Load a script source; Sparky GS LOADSCRIPT:HS -> sn consumes one string (stub).
    spec("LOADSCRIPT", 1, 0, NONE),
    // Request a URL for the current spot; Sparky GS HTTPGET:a1 pops one string, no result.
    spec("HTTPGET", 1, 0, NONE),
    // Send a ban request for a user; Sparky GS BAN:Dw pops one string.
    spec("BAN", 1, 0, NONE),
    // Send a kick request for a user; Sparky GS KICK:Uw pops one string.
    spec("KICK", 1, 0, NONE),
    // ---- PalaceChat version gates -----------------------------------------
    // The client build scripts gate features on. Sparky's PALACECHAT pushes the
    // constant 50_000, which clears every threshold the harvested corpus tests
    // (40913, 41155, 41171, 41182, 42365), so a gated branch takes the modern
    // path instead of offering a download.
    spec("PALACECHAT", 0, 1, INT),
    // Percent-encode a string; Sparky's ENCODEURL wraps `encodeURIComponent`.
    spec("ENCODEURL", 1, 1, STR),
    // `STR` is deliberately *not* registered. It is not a reference command
    // (`IptDefaultCommands.as`, `sparky/index.js`); the converter is `ITOA`.
    // The harvested corpus uses the lower-case spelling as a variable in
    // `5011_hs0` and `5013_hs1` — `str GLOBAL`, `$1 str =`, `str 2 ==` — and in
    // a dynamically compiled template (`"str GLOBAL $1 str ="` through
    // `STRTOATOM`). Registering it turns every one of those into a command call
    // that never leaves the variable slot `GLOBAL`/`=` need. The host dispatch
    // table still answers `STR` for a caller that asks directly.
    // A completed in-script HTTP fetch. Sparky dispatches HTTPRECEIVED as an
    // event name; called as a word the host answers `0`.
    spec("HTTPRECEIVED", 0, 1, INT),
    // Ask the user to confirm; Sparky's CONFIRMBOX pops a string and pushes a
    // 0/1 answer. The corpus calls it as `"message" CONFIRMBOX IF`.
    spec("CONFIRMBOX", 1, 1, INT),
    // Suppress smiley rendering. Called bare inside a block by the corpus, so it
    // consumes nothing; Sparky has no entry for it.
    spec("HIDESMILEYS", 0, 0, NONE),
    // Stop other users changing our props. Bare, as above.
    spec("LOCKUSERPROPS", 0, 0, NONE),
    // ---- Sparky GS: pen and drawing -------------------------------------
    // Operand counts come from the handler classes in the Sparky `GS` table
    // (`reference/repos/sparky/index.js` line 7); the symbol and byte offset of
    // each table entry are cited per group below.
    spec("DRAWTEXT", 3, 0, NONE),       // e1 @56542
    spec("OVAL", 4, 0, NONE),           // Qw @56534
    spec("POLYGON", 1, 0, NONE),        // qw @56496
    spec("PENFONT", 1, 0, NONE),        // t1 @56554
    spec("PENBOLD", 1, 0, NONE),        // n1 @56565
    spec("PENITALIC", 1, 0, NONE),      // s1 @56592
    spec("PENUNDERLINE", 1, 0, NONE),   // o1 @56576
    spec("PENSHADOW", 1, 0, NONE),      // r1 @56605
    spec("PENOPACITY", 1, 0, NONE),     // jw @56448
    spec("PENFILLCOLOR", 3, 0, NONE),   // Xw @56462
    spec("PENFILLOPACITY", 1, 0, NONE), // Kw @56478
    // ---- Sparky GS: spot geometry and queries ---------------------------
    spec("SETSPOTLOC", 3, 0, NONE),          // T1 @56814
    spec("SETSPOTDEST", 2, 0, NONE),         // E1 @56828
    spec("SETSPOTPOINTS", 4, 0, NONE),       // Eb @48267
    spec("SETSPOTPICMODE", 2, 0, NONE),      // Ub @57862
    spec("SETSPOTCLIP", 2, 0, NONE),         // Lb @57831
    spec("SETSPOTCURVE", 2, 0, NONE),        // Mb @57846
    spec("SETSPOTGRADIENT", 4, 0, NONE),     // _b @57758
    spec("SETSPOTPATHGRADIENT", 4, 0, NONE), // Rb @57777
    spec("SETSPOTSTYLE", 4, 0, NONE),        // Ob @57800
    spec("SETSPOTFONT", 10, 0, NONE),        // Ab @57816
    spec("ADDPICNAME", 3, 0, NONE),          // I1 @56870
    spec("INSERTPIC", 3, 0, NONE),           // P1 @56884
    spec("REMOVESPOT", 1, 0, NONE),          // v1 @56800
    spec("GETSPOTOPTIONS", 1, 3, INT),       // kb @57632
    spec("GETSPOTPOINTS", 1, 1, INT),        // Tb @57668
    spec("GETSPOTTYPE", 1, 1, INT),          // S1 @56774
    spec("GETROOMOPTIONS", 0, 1, INT),       // Sb @57614
    spec("LOCINSPOT", 2, 1, INT),            // z1 @57089
    spec("GETSPOTTEXTSIZE", 3, 4, INT),      // BS @58781
    // ---- Sparky GS: pictures --------------------------------------------
    spec("GETPICNAME", 2, 1, STR),       // $b @57911
    spec("GETPICBRIGHTNESS", 2, 1, INT), // Hb @57960
    spec("GETPICSATURATION", 2, 1, INT), // Wb @57980
    spec("GETPICOPACITY", 2, 1, INT),    // zb @58000
    spec("GETPICANGLE", 2, 1, INT),      // Gb @58017
    spec("GETPICPIXEL", 4, 1, INT),      // Fb @57945
    spec("SETPICCONTRAST", 3, 0, NONE),  // Vb @58052
    spec("SETPICHUE", 3, 0, NONE),       // Xb @58090
    spec("SETPICANGLE", 3, 0, NONE),     // qb @58120
    spec("SETPICBLUR", 3, 0, NONE),      // Zb @58135
    spec("SETPICFRAME", 3, 0, NONE),     // Qb @58165
    spec("NBRPICFRAMES", 2, 1, INT),     // Jb @58149
    spec("PAUSEPIC", 2, 0, NONE),        // eS @58180
    spec("RESUMEPIC", 2, 0, NONE),       // tS @58192
    // ---- Sparky GS: sound -----------------------------------------------
    spec("SOUNDPLAY", 1, 0, NONE),       // Sr @57382
    spec("SOUNDPLAYFROM", 1, 0, NONE),   // Sr @57395
    spec("SOUNDOPEN", 1, 0, NONE),       // Sr @57412
    spec("SOUNDLOOP", 1, 0, NONE),       // Pc @57425
    spec("SOUNDSTOP", 1, 0, NONE),       // kr @57450
    spec("SOUNDPAUSE", 1, 0, NONE),      // kr @57463
    spec("SOUNDSEEK", 2, 0, NONE),       // NS @58753
    spec("SOUNDGETPOSITION", 1, 1, INT), // US @58718
    spec("SOUNDLENGTH", 1, 1, INT),      // DS @58738
    spec("ISSOUNDPLAYING", 1, 1, INT),   // _c @57489
    spec("SOUNDISPLAYING", 1, 1, INT),   // _c @57507
    // ---- Sparky GS: web -------------------------------------------------
    spec("WEBEMBED", 2, 0, NONE),     // uS @58333
    spec("WEBLOCATION", 1, 1, STR),   // pS @58345
    spec("WEBSCRIPT", 2, 0, NONE),    // fS @58372
    spec("WEBTITLE", 1, 1, STR),      // hS @58360
    spec("WEBCLICKTHRU", 2, 0, NONE), // mS @58385
    spec("LOADWEBSITE", 1, 0, NONE),  // dS @58318
    // ---- Sparky GS: HTTP headers and POST -------------------------------
    spec("ADDHEADER", 2, 0, NONE),    // u1 @56641
    spec("REMOVEHEADER", 1, 0, NONE), // p1 @56654
    spec("RESETHEADERS", 0, 0, NONE), // h1 @56670
    spec("HTTPPOST", 2, 0, NONE),     // d1 @39854
    spec("HTTPCANCEL", 0, 0, NONE),   // f1 @56686
    // ---- Sparky GS: files -----------------------------------------------
    spec("FILEEXISTS", 1, 1, INT),  // vS @58546
    spec("FILEDATE", 1, 1, STR),    // TS @58560
    spec("FILEDELETE", 1, 0, NONE), // ES @58572
    spec("SELECTFILE", 1, 1, INT),  // xS @58586
    // ---- Sparky GS: alerts, prompts, cursor and help --------------------
    spec("ALERTBOX", 1, 0, NONE),     // iS @58282
    spec("PROMPT", 2, 1, STR),        // lS @58308
    spec("SETCURSOR", 1, 0, NONE),    // gS @58401
    spec("SETCURSORPIC", 3, 0, NONE), // yS @58414
    spec("SETHELPTAG", 1, 0, NONE),   // Rc @58460
    spec("CLEARHELPTAG", 0, 0, NONE), // Oc @58474
    // ---- Sparky GS: alarms, identity and time ---------------------------
    spec("STOPALARM", 1, 0, NONE),  // N1 @57019
    spec("STOPALARMS", 0, 0, NONE), // $1 @57032
    spec("TIMEREXEC", 2, 0, NONE),  // D1 @57006
    spec("CLIENTID", 0, 1, INT),    // Ic @57251
    spec("GETTIMEZONE", 0, 1, INT), // CS @58600
    // ---- Sparky GS: other queries and no-ops ----------------------------
    spec("MEDIAADDRESS", 0, 1, STR),   // IS @58615
    spec("NBRSERVERUSERS", 0, 1, INT), // tb @57233
    spec("NBRROOMPICS", 0, 1, INT),    // y1 @56729
    spec("ROOMPICNAME", 0, 1, STR),    // Q1 @57204
    spec("ISKEYDOWN", 1, 1, INT),      // _S @58643
    spec("ISFUNCTION", 1, 1, INT),     // g1 @56715
    spec("WHEREPROP", 0, 2, INT),      // yw @56117
    spec("WHOCOLOR", 1, 1, INT),       // rb @57294
    spec("WHOFACE", 1, 1, INT),        // sb @57283
    spec("MUTE", 1, 0, NONE),          // Nw @56347
    spec("UNMUTE", 1, 0, NONE),        // $w @56355
    spec("CACHESCRIPT", 2, 0, NONE),   // m1 @56700
    spec("IMAGETOPROP", 1, 0, NONE),   // $S @58766
    spec("TEXTSPEECH", 1, 0, NONE),    // wb (extends sn) @57586
    spec("UPDATELATER", 0, 0, NONE),   // bS @58501
    spec("UPDATENOW", 0, 0, NONE),     // SS @58516
    // ---- legacy Palace words ------------------------------------------------
    // No corpus reference defines these (OpenPalace registry, Sparky `GS`,
    // `docs/iptscrae.txt`). Zero operands matches how the recovered scripts
    // call them, and keeps a call an explicit refusal rather than a silent
    // variable; an invented operand count would corrupt the stack.
    spec("ADDPROP", 0, 0, NONE),
    spec("AWAY", 0, 0, NONE),
    spec("CIRCLE", 0, 0, NONE),
    spec("CLRPROPS", 0, 0, NONE),
    spec("DELPIC", 0, 0, NONE),
    spec("FILL", 0, 0, NONE),
    spec("FLUSH", 0, 0, NONE),
    spec("GETWHOTALKING", 0, 0, NONE),
    spec("HIDEPROPS", 0, 0, NONE),
    spec("MSGTO", 0, 0, NONE),
    spec("NBRUSERS", 0, 0, NONE),
    spec("OFFLINE", 0, 0, NONE),
    spec("ONLINE", 0, 0, NONE),
    spec("PAINT", 0, 0, NONE),
    spec("PING", 0, 0, NONE),
    spec("PURGE", 0, 0, NONE),
    spec("ROOMDESC", 0, 0, NONE),
    spec("ROOMUNZOOM", 0, 0, NONE),
    spec("ROOMZOOM", 0, 0, NONE),
    spec("SETDESC", 0, 0, NONE),
    spec("SETPICDIM", 0, 0, NONE),
    spec("SETPROPSLOCAL", 0, 0, NONE),
    spec("SETSPOTSTATEALL", 0, 0, NONE),
    spec("SHOWALLPROPS", 0, 0, NONE),
    spec("SHOWPROPS", 0, 0, NONE),
    spec("TEXT", 0, 0, NONE),
    spec("TOGGLECTRL", 0, 0, NONE),
];

/// Register every Palace command name as a host command.
///
/// Core names keep their core binding, except `IPTVERSION`, which is overridden
/// deliberately — the note in the body says why.
///
/// Returns how many names were newly registered.
pub fn register_palace_commands(set: &mut CommandSet) -> usize {
    let mut added = 0;
    if !set.contains("SGLOBAL") {
        set.register_builtin("SGLOBAL", iptscrae::Builtin::Global);
        added += 1;
    }
    // Deliberate override of a core name: the generic engine answers 1, a Palace
    // client answers 2, and scripts branch on it. The reference client removes
    // and re-adds this command for the same reason. Left core-bound, every
    // `IPTVERSION 1 ==` legacy branch fired on a modern client.
    set.register_host("IPTVERSION");
    for cmd in PALACE_COMMANDS {
        if set.register_host_if_absent(cmd.name) {
            added += 1;
        }
    }
    added
}

/// Look up a Palace command's specification by name, case-insensitively.
pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    PALACE_COMMANDS
        .iter()
        .find(|cmd| cmd.name.eq_ignore_ascii_case(name))
}

/// Commands this crate actually implements against [`PalaceHost`].
///
/// Everything else in [`PALACE_COMMANDS`] is recognised and stack-balanced by
/// the harness, but not yet wired: those belong to the next milestone.
pub const IMPLEMENTED: &[&str] = &[
    "SAY",
    "CHAT",
    "SAYAT",
    "PRIVATEMSG",
    "USERNAME",
    "USERID",
    "WHOME",
    "SETPOS",
    "GOTOROOM",
];

/// Adapts a [`PalaceHost`] into an [`iptscrae::Host`].
///
/// This is the shape the full command set will take: one match arm per command,
/// each reading its operands from `args` (push order, already dereferenced) and
/// calling the corresponding trait method.
#[derive(Debug, Clone, Copy, Default)]
pub struct PalaceCommands<H> {
    /// The Palace implementation commands run against.
    pub inner: H,
}

impl<H: PalaceHost> PalaceCommands<H> {
    /// Wrap a Palace host.
    pub fn new(inner: H) -> Self {
        Self { inner }
    }

    /// Unwrap the Palace host.
    pub fn into_inner(self) -> H {
        self.inner
    }
}

impl<H: PalaceHost> Host for PalaceCommands<H> {
    fn random(&mut self, bound: i64) -> i64 {
        self.inner.random(bound)
    }

    fn ticks(&self) -> i64 {
        self.inner.ticks()
    }

    fn datetime(&self) -> i64 {
        self.inner.datetime()
    }

    fn trace(&mut self, message: &str) {
        self.inner.trace(message);
    }

    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        self.inner.grep_match(pattern, text)
    }

    fn schedule_alarm(&mut self, ticks: i64, body: iptscrae::Chunk, spot: i64) -> Result<()> {
        self.inner.schedule_alarm(ticks, body, spot)
    }

    fn command_pops(&self, name: &str) -> usize {
        match name {
            "SAY" | "CHAT" | "SAYAT" | "PRIVATEMSG" | "USERNAME" | "USERID" | "WHOME"
            | "SETPOS" | "GOTOROOM" => command_spec(name).map(|s| usize::from(s.pops)).unwrap_or(0),
            _ => 0,
        }
    }

    fn command(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        match name {
            "SAY" | "CHAT" => {
                let text = text_arg(args, 0)?;
                self.inner.chat(&text)?;
                Ok(Vec::new())
            }
            "SAYAT" => {
                let message = text_arg(args, 0)?;
                let x = int_arg(args, 1)?;
                let y = int_arg(args, 2)?;
                self.inner.chat(&format!("@{x},{y} {message}"))?;
                Ok(Vec::new())
            }
            "PRIVATEMSG" => {
                let message = text_arg(args, 0)?;
                let user = int_arg(args, 1)?;
                self.inner.send_private_message(user, &message)?;
                Ok(Vec::new())
            }
            "SETPOS" => {
                let x = int_arg(args, 0)?;
                let y = int_arg(args, 1)?;
                self.inner.move_user_abs(x, y)?;
                Ok(Vec::new())
            }
            "GOTOROOM" => {
                let room = int_arg(args, 0)?;
                self.inner.goto_room(room)?;
                Ok(Vec::new())
            }
            "USERNAME" => Ok(vec![Value::str(self.inner.get_self_user_name())]),
            "USERID" | "WHOME" => Ok(vec![Value::Int(self.inner.get_self_user_id())]),
            other => Err(IptError::CommandUnavailable {
                command: other.to_owned(),
            }),
        }
    }
}

fn int_arg(args: &[Value], index: usize) -> Result<i64> {
    match args.get(index) {
        Some(Value::Int(n)) => Ok(*n),
        Some(other) => Err(IptError::TypeMismatch {
            expected: "number operand",
            found: other.type_name(),
        }),
        None => Err(IptError::StackUnderflow {
            needed: index + 1,
            available: args.len(),
        }),
    }
}

fn text_arg(args: &[Value], index: usize) -> Result<String> {
    match args.get(index) {
        Some(Value::Str(s)) => Ok(s.to_string()),
        Some(other) => Err(IptError::TypeMismatch {
            expected: "string operand",
            found: other.type_name(),
        }),
        None => Err(IptError::StackUnderflow {
            needed: index + 1,
            available: args.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_revived_host_names_are_registered() {
        for name in ["ISRIGHTCLICK", "HTTPRECEIVED"] {
            assert!(command_spec(name).is_some(), "{name} must be a command");
        }
        for name in ["MOUSEX", "MOUSEY", "STR"] {
            assert!(
                command_spec(name).is_none(),
                "{name} must stay a variable: the corpus uses it as one"
            );
        }
    }

    #[test]
    fn lastname_is_not_a_command() {
        assert!(command_spec("LASTNAME").is_none());
    }

    #[test]
    fn str_is_a_variable_because_the_corpus_assigns_to_it() {
        assert!(command_spec("STR").is_none());
        let mut set = CommandSet::core();
        register_palace_commands(&mut set);
        assert_eq!(set.get("STR"), None);
        let limits = iptscrae::Limits::default();
        let body = iptscrae::lexer::parse_body("str GLOBAL 0 str =", &set, &limits).unwrap();
        let names: Vec<&str> = body
            .ops()
            .iter()
            .filter_map(|op| match op {
                iptscrae::value::Op::Var(name) => Some(&**name),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["STR", "STR"], "str must lex as a variable");
    }

    #[test]
    fn the_legacy_words_are_registered_as_bare_refusals() {
        for name in [
            "ADDPROP",
            "AWAY",
            "CIRCLE",
            "CLRPROPS",
            "DELPIC",
            "FILL",
            "FLUSH",
            "GETWHOTALKING",
            "HIDEPROPS",
            "MSGTO",
            "NBRUSERS",
            "OFFLINE",
            "ONLINE",
            "PAINT",
            "PING",
            "PURGE",
            "ROOMDESC",
            "ROOMUNZOOM",
            "ROOMZOOM",
            "SETDESC",
            "SETPICDIM",
            "SETPROPSLOCAL",
            "SETSPOTSTATEALL",
            "SHOWALLPROPS",
            "SHOWPROPS",
            "TEXT",
            "TOGGLECTRL",
        ] {
            let spec = command_spec(name).unwrap_or_else(|| panic!("{name} must be registered"));
            assert_eq!(spec.pops, 0, "{name} must not consume operands");
            assert_eq!(spec.pushes, 0, "{name} must not push a value");
        }
    }

    #[test]
    fn every_group1_host_word_resolves_to_a_command_not_a_variable() {
        for name in [
            "ISRIGHTCLICK",
            "HTTPRECEIVED",
            "ADDPROP",
            "AWAY",
            "CIRCLE",
            "CLRPROPS",
            "DELPIC",
            "FILL",
            "FLUSH",
            "GETWHOTALKING",
            "HIDEPROPS",
            "MSGTO",
            "NBRUSERS",
            "OFFLINE",
            "ONLINE",
            "PAINT",
            "PING",
            "PURGE",
            "ROOMDESC",
            "ROOMUNZOOM",
            "ROOMZOOM",
            "SETDESC",
            "SETPICDIM",
            "SETPROPSLOCAL",
            "SETSPOTSTATEALL",
            "SHOWALLPROPS",
            "SHOWPROPS",
            "TEXT",
            "TOGGLECTRL",
        ] {
            assert!(
                command_spec(name).is_some(),
                "{name} must resolve to a command, not a variable"
            );
        }
        assert!(
            command_spec("LASTNAME").is_none(),
            "LASTNAME was dropped: no reference table defines it"
        );
    }
}

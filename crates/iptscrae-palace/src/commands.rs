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
/// for these additions. Names absent from both references remain unregistered:
/// guessing their operand counts would corrupt the stack. In particular, `TEXT`
/// is not `DRAWTEXT`, and the wire-protocol `PING` constant is not a command.
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
    spec("CLIENTTYPE", 0, 1, STR),
    spec("OPENPALACE", 0, 1, INT),
    spec("IPTVERSION", 0, 1, INT),
    spec("ISGOD", 0, 1, INT),
    spec("ISGUEST", 0, 1, INT),
    spec("ISWIZARD", 0, 1, INT),
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
            "USERID" | "WHOME" => Ok(vec![Value::Int(self.inner.get_self_user_id() as i32)]),
            other => Err(IptError::CommandUnavailable {
                command: other.to_owned(),
            }),
        }
    }
}

fn int_arg(args: &[Value], index: usize) -> Result<i64> {
    match args.get(index) {
        Some(Value::Int(n)) => Ok(i64::from(*n)),
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

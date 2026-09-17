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
/// The three names the core engine also provides (`GREPSTR`, `ALARMEXEC`,
/// `IPTVERSION`) are listed for completeness but lose to the core bindings when
/// both are registered.
///
/// The extended PalaceChat command set (175 names: `WEBEMBED`, `SETSPOTSCRIPT`,
/// `DRAWTEXT`, …) is deliberately **not** listed. This crate has no source for
/// their stack effects, and registering a guessed operand count would corrupt
/// the stack. Left unregistered they lex as variables — which is exactly what
/// the reference engine does, since it does not know them either. A later
/// milestone adds them with signatures in hand.
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
];

/// Register every Palace command name as a host command.
///
/// Names already bound to a core builtin are left alone: `ALARMEXEC`,
/// `GREPSTR` and `IPTVERSION` belong to the generic engine, and a Palace client
/// only overrides their *behaviour*, not their identity.
///
/// Returns how many names were newly registered.
pub fn register_palace_commands(set: &mut CommandSet) -> usize {
    let mut added = 0;
    if !set.contains("SGLOBAL") {
        set.register_builtin("SGLOBAL", iptscrae::Builtin::Global);
        added += 1;
    }
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

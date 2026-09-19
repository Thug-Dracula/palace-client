//! The capability implementation: IPTSCRAE commands over [`HostView`].
//!
//! Every Palace command the corpus uses is mapped to a [`PalaceHost`] call. A
//! call either records an [`Effect`] (the runtime performs it) or answers from
//! the snapshot. Commands with no implementation yet are still *consumed* with
//! the documented operand count and answered with a neutral value, and their
//! names are tallied in [`ScriptHost::unsupported`], so a script keeps running
//! and the run report can name every gap instead of hiding it.

use std::collections::BTreeMap;

use iptscrae::error::{IptError, Result};
use iptscrae::value::{Chunk, Value};
use iptscrae::{Host, Limits};
use iptscrae_palace::commands::command_spec;
use iptscrae_palace::commands::Push;
use iptscrae_palace::PalaceHost;

use crate::effect::Effect;
use crate::view::HostView;

/// A delay a script asked for (`ALARMEXEC` / `SETALARM`).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAlarm {
    /// Delay in ticks (1/60 s), relative to the host clock at request time.
    pub ticks: i64,
    /// What to run when it expires.
    pub kind: AlarmKind,
    /// Hotspot the alarm belongs to; `0` for a body alarm with no hotspot.
    pub spot: i32,
}

/// The two kinds of alarm the language has.
#[derive(Debug, Clone, PartialEq)]
pub enum AlarmKind {
    /// `{ body } ticks ALARMEXEC` — run an atomlist.
    Body(Chunk),
    /// `spot ticks SETALARM` — fire the spot's `ON ALARM`.
    Spot,
}

/// Pen state a script can move and colour.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PenState {
    /// Pen position `(x, y)`.
    pub pos: (i32, i32),
    /// Pen colour.
    pub rgb: (i32, i32, i32),
    /// Pen width.
    pub size: i32,
    /// Whether new strokes land in front of avatars.
    pub front: bool,
}

/// The host scripts run against.
#[derive(Debug, Clone)]
pub struct ScriptHost {
    /// The snapshot every getter answers from.
    pub view: HostView,
    /// Everything the script asked the client to do.
    pub effects: Vec<Effect>,
    /// Commands invoked that this host does not implement, with call counts.
    pub unsupported: BTreeMap<String, u64>,
    /// Alarm requests, turned into absolute deadlines by the engine.
    pub alarms: Vec<PendingAlarm>,
    /// Current host clock in ticks.
    pub tick: i64,
    /// Diagnostic lines the script produced.
    pub trace: Vec<String>,
    /// Pen state.
    pub pen: PenState,
    /// The hotspot whose handler is running; `0` for cyborg or room-wide work.
    /// `DEST` reads this, exactly as the reference's execution context does.
    pub current_spot: i32,
    /// The limiter, kept so a host can report what bound a runaway script.
    pub limits: Limits,
    /// Next id [`Effect::AddSpot`] will hand out; `None` until the first
    /// `ADDSPOT`, when it is seeded from the view's highest spot id.
    next_spot_id: Option<i32>,
    rng: u64,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new(HostView::default())
    }
}

impl ScriptHost {
    /// A host over a snapshot.
    #[must_use]
    pub fn new(view: HostView) -> Self {
        ScriptHost {
            view,
            effects: Vec::new(),
            unsupported: BTreeMap::new(),
            alarms: Vec::new(),
            tick: 0,
            trace: Vec::new(),
            pen: PenState::default(),
            current_spot: 0,
            limits: Limits::default(),
            next_spot_id: None,
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    /// Take the effects recorded so far, leaving the host ready for the next
    /// handler while keeping `unsupported` and `trace` cumulative.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Take the alarm requests recorded so far.
    pub fn take_alarms(&mut self) -> Vec<PendingAlarm> {
        std::mem::take(&mut self.alarms)
    }

    /// Record an unimplemented command and answer with neutral values.
    fn unimplemented(&mut self, name: &str, pushes: usize, push: Push) -> Result<Vec<Value>> {
        *self.unsupported.entry(name.to_owned()).or_insert(0) += 1;
        self.effects.push(Effect::Unsupported {
            command: name.to_owned(),
        });
        Ok(stub_values(pushes, push))
    }

    /// Record an explicit, traced refusal and answer with neutral values.
    ///
    /// Unlike [`ScriptHost::unimplemented`] this is not a gap: the command is
    /// recognised, its operands are consumed and the reason is reported, so the
    /// corpus "reached but not implemented" tally stays at zero.
    fn refuse(&mut self, name: &str, reason: &str) -> Result<Vec<Value>> {
        let spec = command_spec(name);
        let pushes = spec.map_or(0, |s| usize::from(s.pushes));
        let push = spec.map_or(Push::None, |s| s.push);
        self.effects.push(Effect::Refused {
            command: name.to_owned(),
            reason: reason.to_owned(),
        });
        Ok(stub_values(pushes, push))
    }

    fn next_u64(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Allocate the next `ADDSPOT` id. The first call seeds from the highest
    /// id the snapshot holds (1 when there are none); each call then consumes
    /// the counter, so two `ADDSPOT`s in one handler get distinct ids even
    /// though the snapshot does not change under them.
    fn alloc_spot_id(&mut self) -> i32 {
        let seed = self
            .view
            .spots
            .iter()
            .map(|spot| spot.id)
            .max()
            .unwrap_or(0)
            .max(0)
            .saturating_add(1);
        let id = self.next_spot_id.map_or(seed, |next| next.max(seed));
        self.next_spot_id = Some(id.saturating_add(1));
        id
    }

    /// Apply a worn-list change to the snapshot now.
    ///
    /// `PalaceUser.setProps` (`OpenPalace/PalaceClient/src/net/codecomposer/palace/model/PalaceUser.as:149-164`)
    /// mutates `currentUser.props` and `propCount` and only then calls
    /// `updatePropsOnServer`, so a command later in the same handler reads the
    /// new list. The snapshot is updated the same way; the recorded [`Effect`]
    /// still carries the raw request so the runtime sends it to the server.
    fn set_props_now(&mut self, requested: &[i64]) {
        let mut worn: Vec<i64> = Vec::new();
        for id in requested {
            if *id == 0 || u32::try_from(*id).is_err() || worn.contains(id) {
                continue;
            }
            if worn.len() >= MAX_WORN_PROPS {
                break;
            }
            worn.push(*id);
        }
        self.view.self_props = worn;
    }

    /// `PalaceUser.wearProp` (`PalaceUser.as:139-147`): add one prop unless it is
    /// the empty slot, already worn, or the list is full.
    fn wear_prop_now(&mut self, prop: i64) {
        if u32::try_from(prop).is_err() || self.view.self_props.contains(&prop) {
            return;
        }
        if self.view.self_props.len() < MAX_WORN_PROPS {
            self.view.self_props.push(prop);
        }
    }

    /// `PalaceUser.removeProp` (`PalaceUser.as:166-174`): drop the first match.
    fn remove_prop_now(&mut self, prop: i64) {
        if let Some(index) = self.view.self_props.iter().position(|worn| *worn == prop) {
            self.view.self_props.remove(index);
        }
    }
}

fn stub_values(pushes: usize, push: Push) -> Vec<Value> {
    (0..pushes)
        .map(|_| match push {
            Push::Str => Value::str(""),
            Push::None | Push::Int => Value::Int(0),
        })
        .collect()
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

/// The PalaceChat build number scripts gate features on.
///
/// Sparky's `PALACECHAT` pushes the constant 50_000, which clears every
/// threshold the harvested corpus tests, so gated branches take the modern path.
pub const PALACECHAT_VERSION: i32 = 50_000;

/// How many props a user may wear at once (`PalaceUser.wearProp`).
const MAX_WORN_PROPS: usize = 9;

/// Percent-encode a string the way the reference's `ENCODEURL` does.
///
/// Sparky's implementation is `encodeURIComponent`, so the unreserved set is
/// `A-Za-z0-9-_.!~*'()` and every other byte becomes `%XX` over its UTF-8 bytes.
#[must_use]
pub fn encode_url(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => {
                out.push('%');
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0F)] as char);
            }
        }
    }
    out
}

/// Read a `{ ... }` code-block operand.
///
/// The block must carry its recorded inner source; a chunk built without one (a
/// bare body, or one made programmatically) cannot name a handler to attach.
fn chunk_arg(args: &[Value], index: usize) -> Result<&Chunk> {
    match args.get(index) {
        Some(Value::Chunk(chunk)) => Ok(chunk),
        Some(other) => Err(IptError::TypeMismatch {
            expected: "code block operand",
            found: other.type_name(),
        }),
        None => Err(IptError::StackUnderflow {
            needed: index + 1,
            available: args.len(),
        }),
    }
}

/// Read an operand that may be either a number or a string (prop ids, colours).
fn loose_int(args: &[Value], index: usize) -> Result<i64> {
    match args.get(index) {
        Some(Value::Int(n)) => Ok(i64::from(*n)),
        Some(Value::Str(s)) => Ok(s.trim().parse::<i64>().unwrap_or(0)),
        _ => Ok(0),
    }
}

fn props_arg(args: &[Value], index: usize) -> Vec<i64> {
    int_list(args, index, false)
}

/// The `ADDSPOT` polygon operand: like [`props_arg`], but a quoted numeric
/// string counts as its number, which the extended dialect relies on.
fn point_list_arg(args: &[Value], index: usize) -> Vec<i64> {
    int_list(args, index, true)
}

fn int_list(args: &[Value], index: usize, quoted_numbers: bool) -> Vec<i64> {
    match args.get(index) {
        Some(Value::Array(items)) => match items.try_borrow() {
            Ok(items) => items
                .iter()
                .filter_map(|value| as_int(value, quoted_numbers))
                .collect(),
            Err(_) => Vec::new(),
        },
        Some(other) => as_int(other, quoted_numbers).into_iter().collect(),
        None => Vec::new(),
    }
}

fn as_int(value: &Value, quoted_numbers: bool) -> Option<i64> {
    match value {
        Value::Int(n) => Some(i64::from(*n)),
        Value::Str(s) if quoted_numbers => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// A flat `[x y x y …]` operand as `(x, y)` pairs.
fn flat_points(args: &[Value], index: usize) -> Vec<(i32, i32)> {
    point_list_arg(args, index)
        .chunks_exact(2)
        .map(|pair| (pair[0] as i32, pair[1] as i32))
        .collect()
}

/// The local UTC offset in whole hours for `GETTIMEZONE`.
///
/// The Sparky reference pushes the IANA zone name, but the corpus template
/// (`113_hs0.txt`) feeds the result straight into `tz dst + 3600 *`, so the
/// host answers the numeric offset those scripts consume. The offset comes
/// from the TZif database at `/etc/localtime`; a `TZ` fixed offset is the
/// fallback, and `0` (UTC) is the last resort.
fn local_utc_offset_hours() -> i32 {
    if let Some(seconds) = std::fs::read("/etc/localtime")
        .ok()
        .and_then(|data| tzif_offset_seconds(&data))
    {
        return (seconds / 3600) as i32;
    }
    if let Ok(tz) = std::env::var("TZ") {
        if let Some(hours) = fixed_tz_offset_hours(&tz) {
            return hours;
        }
    }
    0
}

/// The current UTC offset in seconds from a TZif (`/etc/localtime`) blob.
fn tzif_offset_seconds(data: &[u8]) -> Option<i64> {
    if data.len() < 44 || &data[..4] != b"TZif" {
        return None;
    }
    let version = data[4];
    let counts = |at: usize| -> Option<[usize; 6]> {
        let mut out = [0usize; 6];
        for (index, slot) in out.iter_mut().enumerate() {
            let start = at + index * 4;
            let bytes: [u8; 4] = data.get(start..start + 4)?.try_into().ok()?;
            *slot = i32::from_be_bytes(bytes) as usize;
        }
        Some(out)
    };
    let first = counts(20)?;
    let first_block =
        first[3] * 4 + first[3] + first[4] * 6 + first[5] + first[2] * 8 + first[1] + first[0];
    if matches!(version, b'2' | b'3' | b'4') {
        let second = 44 + first_block;
        let wide = counts(second + 20)?;
        parse_tzif_block(data, second + 44, &wide, true)
    } else {
        parse_tzif_block(data, 44, &first, false)
    }
}

/// The offset of the transition active now in one TZif data block.
fn parse_tzif_block(data: &[u8], pos: usize, counts: &[usize; 6], wide: bool) -> Option<i64> {
    let (timecnt, typecnt, charcnt, leapcnt, isstdcnt, isutcnt) = (
        counts[3], counts[4], counts[5], counts[2], counts[1], counts[0],
    );
    let time_size = if wide { 8 } else { 4 };
    let transitions = pos;
    let indices = transitions + timecnt * time_size;
    let types = indices + timecnt;
    let end =
        types + typecnt * 6 + charcnt + leapcnt * (if wide { 12 } else { 8 }) + isstdcnt + isutcnt;
    if end > data.len() {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let mut chosen = 0usize;
    let mut found = false;
    for index in 0..timecnt {
        let at = transitions + index * time_size;
        let time = if wide {
            i64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?)
        } else {
            i64::from(i32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
        };
        if time <= now {
            chosen = usize::from(*data.get(indices + index)?);
            found = true;
        }
    }
    if !found || chosen >= typecnt {
        chosen = (0..typecnt)
            .find(|&index| data[types + index * 6 + 4] == 0)
            .unwrap_or(0);
    }
    let offset: [u8; 4] = data
        .get(types + chosen * 6..types + chosen * 6 + 4)?
        .try_into()
        .ok()?;
    Some(i64::from(i32::from_be_bytes(offset)))
}

/// A POSIX-style fixed-offset `TZ` value, in whole hours.
fn fixed_tz_offset_hours(tz: &str) -> Option<i32> {
    let tz = tz.trim();
    for prefix in ["UTC", "GMT"] {
        if let Some(rest) = tz.strip_prefix(prefix) {
            if rest.is_empty() {
                return Some(0);
            }
            let sign = match rest.as_bytes().first()? {
                b'+' => -1,
                b'-' => 1,
                _ => continue,
            };
            let digits: String = rest[1..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ':')
                .collect();
            let hours = digits.split(':').next()?.parse::<i32>().ok()?;
            return Some(sign * hours);
        }
    }
    None
}

impl Host for ScriptHost {
    fn random(&mut self, bound: i64) -> i64 {
        if bound <= 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as i64
        }
    }

    fn ticks(&self) -> i64 {
        self.tick
    }

    fn datetime(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn trace(&mut self, message: &str) {
        self.trace.push(message.to_owned());
    }

    fn request_beep(&mut self) {
        let _ = PalaceHost::beep(self);
    }

    fn grep_match(&mut self, pattern: &str, text: &str) -> Result<Option<Vec<String>>> {
        Ok(iptscrae::regex::find(pattern, text))
    }

    fn schedule_alarm(&mut self, ticks: i64, body: Chunk, spot: i64) -> Result<()> {
        let spot = if spot == 0 {
            self.current_spot
        } else {
            spot as i32
        };
        self.alarms.push(PendingAlarm {
            ticks: ticks.max(0),
            kind: AlarmKind::Body(body),
            spot,
        });
        Ok(())
    }

    fn initial_variables(&self) -> Vec<(String, Value)> {
        // `ERRORMSG` is a host-bound string variable, not a command: the GS
        // variable table binds it to the HTTP error text (`sparky/index.js`),
        // and corpus `ON HTTPERROR { ERRORMSG LOGMSG }` handlers read it.
        vec![
            (
                "CHATSTR".to_owned(),
                Value::str(self.view.chat_string.as_str()),
            ),
            ("ERRORMSG".to_owned(), Value::str("")),
        ]
    }

    fn command_pops(&self, name: &str) -> usize {
        command_spec(name).map_or(0, |s| usize::from(s.pops))
    }

    fn command(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let spec = command_spec(name);
        let pushes = spec.map_or(0, |s| usize::from(s.pushes));
        let push = spec.map_or(Push::None, |s| s.push);

        match name {
            // ------------------------------------------------------ messaging
            "SAY" | "CHAT" => self.chat(&text_arg(args, 0)?).map(|_| Vec::new()),
            "SAYAT" => self
                .say_at(&text_arg(args, 0)?, int_arg(args, 1)?, int_arg(args, 2)?)
                .map(|_| Vec::new()),
            "PRIVATEMSG" => self
                .send_private_message(int_arg(args, 1)?, &text_arg(args, 0)?)
                .map(|_| Vec::new()),
            "GLOBALMSG" => self
                .send_global_message(&text_arg(args, 0)?)
                .map(|_| Vec::new()),
            "ROOMMSG" => self
                .send_room_message(&text_arg(args, 0)?)
                .map(|_| Vec::new()),
            "LOCALMSG" => self.send_local_msg(&text_arg(args, 0)?).map(|_| Vec::new()),
            "SUSRMSG" => self
                .send_susr_message(&text_arg(args, 0)?)
                .map(|_| Vec::new()),
            "STATUSMSG" => self.status_message(&text_arg(args, 0)?).map(|_| Vec::new()),
            "LOGMSG" => self.log_message(&text_arg(args, 0)?).map(|_| Vec::new()),

            // ------------------------------------------------------ identity
            // `ME` is the hotspot the running script belongs to, not the user
            // (`MECommand.as` pushes `PalaceIptExecutionContext.hotspotId`). The
            // arena rooms navigate with `ME DEST GOTOROOM`, so collapsing it into
            // `USERID` sent them to the destination of a spot id that does not
            // exist, and left the user id on the stack.
            //
            // `ID` is the same hotspot id: `PalaceIptscraeCommands.as:29` maps
            // `ID` to `MECommand`, and the guide documents it as "spotID/doorID
            // executing script or 0 for Cyborg" — so it must not answer with the
            // user id the way `USERID`/`WHOME` do.
            "ME" | "ID" => Ok(vec![Value::Int(self.current_spot)]),
            "USERID" | "WHOME" => Ok(vec![Value::Int(self.get_self_user_id() as i32)]),
            "USERNAME" => Ok(vec![Value::str(self.get_self_user_name())]),
            "SERVERNAME" => Ok(vec![Value::str(self.get_server_name())]),
            "ROOMID" => Ok(vec![Value::Int(self.get_room_id() as i32)]),
            "ROOMNAME" => Ok(vec![Value::str(self.get_room_name())]),
            "ROOMWIDTH" => Ok(vec![Value::Int(self.get_room_width() as i32)]),
            "ROOMHEIGHT" => Ok(vec![Value::Int(self.get_room_height() as i32)]),
            "POSX" => Ok(vec![Value::Int(self.get_self_pos_x() as i32)]),
            "POSY" => Ok(vec![Value::Int(self.get_self_pos_y() as i32)]),
            "MOUSEPOS" => Ok(vec![
                Value::Int(self.get_mouse_x() as i32),
                Value::Int(self.get_mouse_y() as i32),
            ]),
            "CLIENTTYPE" => Ok(vec![Value::str(self.client_type())]),
            "OPENPALACE" => Ok(vec![Value::Int(1)]),
            "PALACECHAT" => Ok(vec![Value::Int(PALACECHAT_VERSION)]),
            "IPTVERSION" => Ok(vec![Value::Int(2)]),
            "ISGOD" => Ok(vec![Value::Int(i32::from(self.is_god()))]),
            "ISGUEST" => Ok(vec![Value::Int(i32::from(self.is_guest()))]),
            "ISWIZARD" => Ok(vec![Value::Int(i32::from(self.is_wizard()))]),
            "ISRIGHTCLICK" => Ok(vec![Value::Int(i32::from(self.view.right_click))]),
            "WHOCHAT" => Ok(vec![Value::Int(self.get_who_chat() as i32)]),
            "WHOTARGET" => Ok(vec![Value::Int(self.get_who_target() as i32)]),
            "DEST" => Ok(vec![Value::Int(
                self.get_spot_dest(i64::from(self.current_spot)) as i32,
            )]),
            "NBRDOORS" => Ok(vec![Value::Int(
                self.view.spots.iter().filter(|s| s.kind == 1).count() as i32,
            )]),
            "NBRSPOTS" => Ok(vec![Value::Int(self.view.spots.len() as i32)]),
            "NBRROOMUSERS" => Ok(vec![Value::Int(self.get_num_room_users() as i32)]),
            "NBRUSERPROPS" => Ok(vec![Value::Int(self.get_num_user_props() as i32)]),
            "TOPPROP" => Ok(vec![Value::Int(self.get_top_prop() as i32)]),

            // ------------------------------------------------------ lookups
            "GETSPOTLOC" => {
                let (x, y) = self.get_spot_location(int_arg(args, 0)?);
                Ok(vec![Value::Int(x as i32), Value::Int(y as i32)])
            }
            "GETSPOTSTATE" => Ok(vec![Value::Int(
                self.get_spot_state(int_arg(args, 0)?) as i32
            )]),
            "GETPICLOC" => {
                let (x, y) = self.get_pic_offset(int_arg(args, 0)?, int_arg(args, 1)?);
                Ok(vec![Value::Int(x as i32), Value::Int(y as i32)])
            }
            "GETPICDIMENSIONS" => {
                let (w, h) = self.get_pic_dimensions(int_arg(args, 0)?, int_arg(args, 1)?);
                Ok(vec![Value::Int(w as i32), Value::Int(h as i32)])
            }
            "SPOTDEST" => Ok(vec![Value::Int(
                self.get_spot_dest(int_arg(args, 0)?) as i32
            )]),
            "SPOTIDX" => Ok(vec![Value::Int(
                self.get_spot_id_by_index(int_arg(args, 0)?) as i32,
            )]),
            "SPOTNAME" => Ok(vec![Value::str(self.get_spot_name(int_arg(args, 0)?))]),
            "ISLOCKED" => Ok(vec![Value::Int(i32::from(
                self.is_locked(int_arg(args, 0)?),
            ))]),
            "INSPOT" => Ok(vec![Value::Int(i32::from(self.in_spot(int_arg(args, 0)?)))]),
            "ROOMUSER" => Ok(vec![Value::Int(
                self.get_room_user_id_by_index(int_arg(args, 0)?) as i32,
            )]),
            "WHONAME" => Ok(vec![Value::str(self.get_user_name(int_arg(args, 0)?))]),
            "WHOPOS" => {
                let user = int_arg(args, 0)?;
                Ok(vec![
                    Value::Int(self.get_pos_x(user) as i32),
                    Value::Int(self.get_pos_y(user) as i32),
                ])
            }
            "USERPROP" => Ok(vec![Value::Int(
                self.get_user_prop(int_arg(args, 0)?) as i32
            )]),
            // `HASPROP` accepts a prop id or a prop name
            // (`HASPROPCommand.as:20-22`); a string routes to the by-name
            // resolver, a number to the id check.
            "HASPROP" => match args.first() {
                Some(Value::Str(name)) => {
                    Ok(vec![Value::Int(i32::from(self.has_prop_by_name(name)))])
                }
                _ => Ok(vec![Value::Int(i32::from(
                    self.has_prop_by_id(int_arg(args, 0)?),
                ))]),
            },
            "LOOSEPROP" => Ok(vec![Value::Int(
                self.get_loose_prop_id_by_index(int_arg(args, 0)?) as i32,
            )]),
            "LOOSEPROPIDX" => Ok(vec![Value::Int(
                self.get_loose_prop_index_by_id(int_arg(args, 0)?) as i32,
            )]),
            "LOOSEPROPPOS" => {
                let (x, y) = self.get_loose_prop_pos(int_arg(args, 0)?);
                Ok(vec![Value::Int(x as i32), Value::Int(y as i32)])
            }
            "NBRLOOSEPROPS" => Ok(vec![Value::Int(self.get_num_loose_props() as i32)]),
            "MOUSEX" => Ok(vec![Value::Int(self.get_mouse_x() as i32)]),
            "MOUSEY" => Ok(vec![Value::Int(self.get_mouse_y() as i32)]),
            "PROPDIMENSIONS" => {
                let (w, h) = self.get_prop_dimensions(int_arg(args, 0)?);
                Ok(vec![Value::Int(w as i32), Value::Int(h as i32)])
            }
            "PROPOFFSETS" => {
                let (x, y) = self.get_prop_offsets(int_arg(args, 0)?);
                Ok(vec![Value::Int(x as i32), Value::Int(y as i32)])
            }
            "HTTPRECEIVED" => Ok(vec![Value::Int(0)]),
            "DOORIDX" => Ok(vec![Value::Int(
                self.get_door_id_by_index(int_arg(args, 0)?) as i32,
            )]),

            // ------------------------------------------------------ spot state
            "SETSPOTSTATE" | "SETSPOTSTATELOCAL" => {
                let state = int_arg(args, 0)?;
                let spot = int_arg(args, 1)?;
                let effect = if name == "SETSPOTSTATE" {
                    Effect::SetSpotState {
                        spot: spot as i32,
                        state: state as i32,
                    }
                } else {
                    Effect::SetSpotStateLocal {
                        spot: spot as i32,
                        state: state as i32,
                    }
                };
                self.effects.push(effect);
                Ok(Vec::new())
            }
            "SETSPOTNAMELOCAL" => self
                .set_spot_name_local(int_arg(args, 1)?, &text_arg(args, 0)?)
                .map(|_| Vec::new()),
            "SETLOC" | "SETLOCLOCAL" => {
                let x = int_arg(args, 0)?;
                let y = int_arg(args, 1)?;
                let spot = int_arg(args, 2)?;
                self.effects.push(if name == "SETLOC" {
                    Effect::MoveSpot {
                        spot: spot as i32,
                        dx: x as i32,
                        dy: y as i32,
                    }
                } else {
                    Effect::MoveSpotLocal {
                        spot: spot as i32,
                        dx: x as i32,
                        dy: y as i32,
                    }
                });
                Ok(Vec::new())
            }
            "SETPICLOC" | "SETPICLOCLOCAL" => {
                let x = int_arg(args, 0)?;
                let y = int_arg(args, 1)?;
                if name == "SETPICLOC" {
                    self.effects.push(Effect::SetPicOffset {
                        spot: int_arg(args, 2)? as i32,
                        dx: x as i32,
                        dy: y as i32,
                    });
                } else {
                    self.effects.push(Effect::SetPicOffsetLocal {
                        dx: x as i32,
                        dy: y as i32,
                        state: int_arg(args, 2)? as i32,
                        spot: int_arg(args, 3)? as i32,
                    });
                }
                Ok(Vec::new())
            }
            "SETPICOPACITY" => {
                self.effects.push(Effect::SetPicOpacity {
                    spot: int_arg(args, 2)? as i32,
                    state: int_arg(args, 1)? as i32,
                    opacity: int_arg(args, 0)? as f64 / 100.0,
                });
                Ok(Vec::new())
            }
            // Operand order matches `SETPICOPACITY`: value, then state, then
            // spot id. OpenPalace's `SETPICBRIGHTNESSCommand.as:9-13` pops all
            // three and discards them; Sparky's `setPicFilter` is the reference
            // mutation this records.
            "SETPICBRIGHTNESS" => {
                self.effects.push(Effect::SetPicBrightness {
                    spot: int_arg(args, 2)? as i32,
                    state: int_arg(args, 1)? as i32,
                    value: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "SETPICSATURATION" => {
                self.effects.push(Effect::SetPicSaturation {
                    spot: int_arg(args, 2)? as i32,
                    state: int_arg(args, 1)? as i32,
                    value: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "ADDSPOT" => {
                let flat = point_list_arg(args, 0);
                let x = int_arg(args, 1)?;
                let y = int_arg(args, 2)?;
                let points: Vec<(i32, i32)> = flat
                    .chunks_exact(2)
                    .map(|pair| (pair[0] as i32, pair[1] as i32))
                    .collect();
                let id = self.alloc_spot_id();
                self.effects.push(Effect::AddSpot {
                    id,
                    points,
                    x: x as i32,
                    y: y as i32,
                });
                Ok(vec![Value::Int(id)])
            }
            "ADDPIC" => {
                let name = text_arg(args, 0)?;
                let spot = int_arg(args, 1)?;
                self.effects.push(Effect::AddPic {
                    spot: spot as i32,
                    name,
                });
                Ok(Vec::new())
            }
            "SETSPOTOPTIONS" => {
                let flags = int_arg(args, 0)?;
                let top_layer = int_arg(args, 1)? > 0;
                let hotspot_type = int_arg(args, 2)?;
                let spot = int_arg(args, 3)?;
                self.effects.push(Effect::SetSpotOptions {
                    spot: spot as i32,
                    hotspot_type: hotspot_type as i32,
                    flags: flags as i32,
                    top_layer,
                });
                Ok(Vec::new())
            }
            "SETSPOTSCRIPT" => {
                let block = chunk_arg(args, 0)?;
                let source = block.source().ok_or(IptError::TypeMismatch {
                    expected: "code block with source text",
                    found: "atomlist",
                })?;
                let event = text_arg(args, 1)?;
                let bytes = event.len();
                if !(2..=30).contains(&bytes) {
                    return Err(IptError::BadArgument(
                        "SETSPOTSCRIPT event name must be 2..=30 bytes",
                    ));
                }
                self.effects.push(Effect::SetSpotScript {
                    spot: int_arg(args, 2)? as i32,
                    event: event.to_ascii_uppercase(),
                    script: source.to_owned(),
                });
                Ok(Vec::new())
            }
            "SETTOOLTIP" => {
                let text = text_arg(args, 0)?;
                self.set_tooltip(&text).map(|()| Vec::new())
            }
            "CLEARTOOLTIP" => self.clear_tooltip().map(|()| Vec::new()),
            "LOCK" => {
                self.effects.push(Effect::Lock {
                    spot: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "UNLOCK" => {
                self.effects.push(Effect::Unlock {
                    spot: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "SELECT" => {
                self.effects.push(Effect::SelectSpot {
                    spot: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "SETALARM" => {
                let ticks = int_arg(args, 0)?.max(0);
                let requested = int_arg(args, 1)? as i32;
                let spot = if requested == 0 {
                    self.current_spot
                } else {
                    requested
                };
                self.alarms.push(PendingAlarm {
                    ticks,
                    kind: AlarmKind::Spot,
                    spot,
                });
                Ok(Vec::new())
            }

            // ------------------------------------------------------ props
            "DONPROP" => {
                match args.first() {
                    Some(Value::Str(name)) => self.don_prop_by_name(name)?,
                    _ => self.don_prop_by_id(loose_int(args, 0)?)?,
                }
                Ok(Vec::new())
            }
            "REMOVEPROP" => {
                match args.first() {
                    Some(Value::Str(name)) => self.doff_prop_by_name(name)?,
                    _ => self.doff_prop_by_id(loose_int(args, 0)?)?,
                }
                Ok(Vec::new())
            }
            "DOFFPROP" => self.doff_prop().map(|_| Vec::new()),
            "NAKED" | "CLEARPROPS" | "CLRPROPS" => self.naked().map(|_| Vec::new()),
            "SETPROPS" => self.set_props(&props_arg(args, 0)).map(|_| Vec::new()),
            "LOADPROPS" => {
                let Some(Value::Array(items)) = args.first() else {
                    return Err(IptError::TypeMismatch {
                        expected: "an array of prop IDs",
                        found: args.first().map_or("nothing", Value::type_name),
                    });
                };
                let Ok(items) = items.try_borrow() else {
                    return Ok(Vec::new());
                };
                if items.len() > 500 {
                    return Err(IptError::BadArgument(
                        "You may only load up to 500 props at a time.",
                    ));
                }
                if items.iter().any(|item| !matches!(item, Value::Int(_))) {
                    return Err(IptError::BadArgument(
                        "Only Prop IDs are allowed to be specified for LOADPROPS.",
                    ));
                }
                let props: Vec<i64> = items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Int(n) => Some(i64::from(*n)),
                        _ => None,
                    })
                    .collect();
                drop(items);
                self.load_props(&props).map(|_| Vec::new())
            }

            // ------------------------------------------------------ movement
            "SETPOS" => {
                self.effects.push(Effect::MoveUserAbs {
                    x: int_arg(args, 0)? as i32,
                    y: int_arg(args, 1)? as i32,
                });
                Ok(Vec::new())
            }
            "MOVE" => {
                self.effects.push(Effect::MoveUserRel {
                    dx: int_arg(args, 0)? as i32,
                    dy: int_arg(args, 1)? as i32,
                });
                Ok(Vec::new())
            }
            "GOTOROOM" | "ROOMGOTO" => {
                self.effects.push(Effect::GotoRoom {
                    room: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "NETGOTO" | "GOTOURL" => {
                self.effects.push(Effect::GotoUrl {
                    url: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "LOADSCRIPT" | "HTTPGET" => {
                let url = text_arg(args, 0)?;
                self.effects.push(Effect::FetchScript {
                    url,
                    spot: self.current_spot,
                });
                Ok(Vec::new())
            }
            "SETUSERNAME" => {
                self.effects.push(Effect::SetUserName {
                    name: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "SETCOLOR" => {
                self.effects.push(Effect::SetColor {
                    color: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "SETFACE" => {
                self.effects.push(Effect::SetFace {
                    face: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "MACRO" => {
                self.effects.push(Effect::Macro {
                    index: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            // `PalaceController.as:632-635` whispers the literal `` `kill `` to
            // the target user id.
            "KILLUSER" => {
                let user = int_arg(args, 0)?;
                self.send_private_message(user, "`kill").map(|_| Vec::new())
            }
            "HIDEAVATARS" => {
                self.effects.push(Effect::HideAvatars);
                Ok(Vec::new())
            }
            "SHOWAVATARS" => {
                self.effects.push(Effect::ShowAvatars);
                Ok(Vec::new())
            }
            "DIMROOM" => {
                self.effects.push(Effect::DimRoom {
                    percent: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "LAUNCHAPP" => {
                self.effects.push(Effect::LaunchApp {
                    app: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "GOTOURLFRAME" => {
                let url = text_arg(args, 0)?;
                let frame = text_arg(args, 1)?;
                self.effects.push(Effect::GotoUrlFrame { url, frame });
                Ok(Vec::new())
            }
            // OpenPalace maps these to `UnsupportedCommand` (pop one value, trace
            // `"Unsupported Iptscrae Command"`); the Sparky host pops one string
            // and does nothing. Recording a named effect keeps the call out of
            // the silent catch-all and lets the runtime report it.
            "LAUNCHEVENT" => {
                self.effects.push(Effect::LaunchEvent {
                    event: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "LAUNCHPPA" => {
                self.effects.push(Effect::LaunchPpa {
                    ppa: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "LOADJAVA" => {
                self.effects.push(Effect::LoadJava {
                    url: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "TALKPPA" => {
                self.effects.push(Effect::TalkPpa {
                    text: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }

            // ------------------------------------------------------ sound
            "SOUND" => {
                self.effects.push(Effect::PlaySound {
                    name: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "MIDIPLAY" => {
                self.effects.push(Effect::MidiPlay {
                    name: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "MIDILOOP" => {
                self.effects.push(Effect::MidiLoop {
                    name: text_arg(args, 1)?,
                    loops: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "MIDISTOP" => {
                self.effects.push(Effect::MidiStop);
                Ok(Vec::new())
            }
            "BEEP" => {
                self.effects.push(Effect::Beep);
                Ok(Vec::new())
            }

            // ------------------------------------------------------ loose props
            "ADDLOOSEPROP" => {
                self.effects.push(Effect::AddLooseProp {
                    prop: loose_int(args, 0)?,
                    x: int_arg(args, 1)? as i32,
                    y: int_arg(args, 2)? as i32,
                });
                Ok(Vec::new())
            }
            "REMOVELOOSEPROP" => {
                self.effects.push(Effect::RemoveLooseProp {
                    index: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "MOVELOOSEPROP" => {
                self.effects.push(Effect::MoveLooseProp {
                    x: int_arg(args, 0)? as i32,
                    y: int_arg(args, 1)? as i32,
                    index: int_arg(args, 2)? as i32,
                });
                Ok(Vec::new())
            }
            "CLEARLOOSEPROPS" => {
                self.effects.push(Effect::ClearLooseProps);
                Ok(Vec::new())
            }
            "DROPPROP" => {
                self.effects.push(Effect::DropProp {
                    x: int_arg(args, 0)? as i32,
                    y: int_arg(args, 1)? as i32,
                });
                Ok(Vec::new())
            }
            "SHOWLOOSEPROPS" => self.show_loose_props().map(|_| Vec::new()),

            // ------------------------------------------------------ paint
            "LINE" => {
                self.effects.push(Effect::DrawLine {
                    x1: int_arg(args, 0)? as i32,
                    y1: int_arg(args, 1)? as i32,
                    x2: int_arg(args, 2)? as i32,
                    y2: int_arg(args, 3)? as i32,
                });
                Ok(Vec::new())
            }
            "LINETO" => {
                let (x1, y1) = self.pen.pos;
                let (x2, y2) = (x1 + int_arg(args, 0)? as i32, y1 + int_arg(args, 1)? as i32);
                self.pen.pos = (x2, y2);
                self.effects.push(Effect::DrawLineRel { x1, y1, x2, y2 });
                Ok(Vec::new())
            }
            "PENPOS" => self
                .move_pen_abs(int_arg(args, 0)?, int_arg(args, 1)?)
                .map(|_| Vec::new()),
            "PENTO" => self
                .move_pen_rel(int_arg(args, 0)?, int_arg(args, 1)?)
                .map(|_| Vec::new()),
            "PENSIZE" => self.set_pen_size(int_arg(args, 0)?).map(|_| Vec::new()),
            "PENCOLOR" => self
                .set_pen_color(int_arg(args, 0)?, int_arg(args, 1)?, int_arg(args, 2)?)
                .map(|_| Vec::new()),
            "PENBACK" => self.paint_back_layer().map(|_| Vec::new()),
            "PENFRONT" => self.paint_front_layer().map(|_| Vec::new()),
            "PAINTCLEAR" => self.paint_clear().map(|_| Vec::new()),
            "PAINTUNDO" => self.paint_undo().map(|_| Vec::new()),

            // ------------------------------------------------------ misc
            "STR" => Ok(vec![Value::str(match args.first() {
                Some(Value::Int(n)) => n.to_string(),
                Some(Value::Str(s)) => s.to_string(),
                _ => String::new(),
            })]),
            "ENCODEURL" => Ok(vec![Value::str(encode_url(&text_arg(args, 0)?))]),
            "SHELLCMD" => {
                self.effects.push(Effect::ShellCommand {
                    command: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "HIDESMILEYS" => {
                self.effects.push(Effect::HideSmileys);
                Ok(Vec::new())
            }
            "LOCKUSERPROPS" => {
                self.effects.push(Effect::LockUserProps);
                Ok(Vec::new())
            }
            "AUTOUSERLAYER" => {
                self.effects.push(Effect::AutoUserLayer {
                    on: int_arg(args, 0)? > 0,
                });
                Ok(Vec::new())
            }
            "REMOVEPIC" => {
                self.effects.push(Effect::RemovePic {
                    spot: int_arg(args, 1)? as i32,
                    picture: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            // OpenPalace has no BAN/KICK; Sparky gates both on operator status
            // and whispers `` `ban ``/`` `kick `` to user 0 (the server). The
            // server enforces the privilege, so `is_wizard` is the local gate.
            "BAN" => {
                let name = text_arg(args, 0)?;
                if self.is_wizard() {
                    self.send_private_message(0, &format!("`ban {name}"))?;
                }
                Ok(Vec::new())
            }
            "KICK" => {
                let name = text_arg(args, 0)?;
                if self.is_wizard() {
                    self.send_private_message(0, &format!("`kick {name}"))?;
                }
                Ok(Vec::new())
            }
            "CONFIRMBOX" => {
                self.effects.push(Effect::Confirm {
                    text: text_arg(args, 0)?,
                });
                Ok(vec![Value::Int(0)])
            }

            // -------------------------------------------- Sparky GS: pen/draw
            "DRAWTEXT" => {
                let text = text_arg(args, 0)?;
                let x = int_arg(args, 1)? as i32;
                let y = int_arg(args, 2)? as i32;
                self.effects.push(Effect::DrawText { text, x, y });
                Ok(Vec::new())
            }
            "OVAL" => {
                let h = int_arg(args, 0)? as i32;
                let w = int_arg(args, 1)? as i32;
                let y = int_arg(args, 2)? as i32;
                let x = int_arg(args, 3)? as i32;
                self.effects.push(Effect::DrawOval { x, y, w, h });
                Ok(Vec::new())
            }
            "POLYGON" => {
                let points = flat_points(args, 0);
                self.effects.push(Effect::DrawPolygon { points });
                Ok(Vec::new())
            }
            "PENFONT" => {
                self.effects.push(Effect::SetPenFont {
                    name: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "PENBOLD" => {
                self.effects.push(Effect::SetPenBold {
                    on: int_arg(args, 0)? != 0,
                });
                Ok(Vec::new())
            }
            "PENITALIC" => {
                self.effects.push(Effect::SetPenItalic {
                    on: int_arg(args, 0)? != 0,
                });
                Ok(Vec::new())
            }
            "PENUNDERLINE" => {
                self.effects.push(Effect::SetPenUnderline {
                    on: int_arg(args, 0)? != 0,
                });
                Ok(Vec::new())
            }
            "PENSHADOW" => {
                self.effects.push(Effect::SetPenShadow {
                    on: loose_int(args, 0)? != 0,
                });
                Ok(Vec::new())
            }
            "PENOPACITY" => {
                self.effects.push(Effect::SetPenOpacity {
                    value: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "PENFILLCOLOR" => {
                self.effects.push(Effect::SetPenFillColor {
                    r: int_arg(args, 0)? as i32,
                    g: int_arg(args, 1)? as i32,
                    b: int_arg(args, 2)? as i32,
                });
                Ok(Vec::new())
            }
            "PENFILLOPACITY" => {
                self.effects.push(Effect::SetPenFillOpacity {
                    value: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }

            // ----------------------------------------- Sparky GS: spot geometry
            "SETSPOTLOC" => {
                let x = int_arg(args, 0)? as i32;
                let y = int_arg(args, 1)? as i32;
                let spot = int_arg(args, 2)? as i32;
                self.effects.push(Effect::SetSpotLoc { spot, x, y });
                Ok(Vec::new())
            }
            "SETSPOTDEST" => {
                let spot = int_arg(args, 0)? as i32;
                let dest = int_arg(args, 1)? as i32;
                self.effects.push(Effect::SetSpotDest { spot, dest });
                Ok(Vec::new())
            }
            "SETSPOTPOINTS" => {
                let points = flat_points(args, 0);
                let x = int_arg(args, 1)? as i32;
                let y = int_arg(args, 2)? as i32;
                let spot = int_arg(args, 3)? as i32;
                self.effects
                    .push(Effect::SetSpotPoints { spot, x, y, points });
                Ok(Vec::new())
            }
            "SETSPOTPICMODE" => {
                let mode = int_arg(args, 0)? as i32;
                let spot = int_arg(args, 1)? as i32;
                self.effects.push(Effect::SetSpotPicMode { spot, mode });
                Ok(Vec::new())
            }
            "SETSPOTSTYLE" => {
                let color = match args.first() {
                    Some(Value::Str(s)) => s.to_string(),
                    Some(Value::Int(n)) => n.to_string(),
                    _ => String::new(),
                };
                let border = int_arg(args, 1)? as i32;
                let size = int_arg(args, 2)? as i32;
                let spot = int_arg(args, 3)? as i32;
                self.effects.push(Effect::SetSpotStyle {
                    spot,
                    color,
                    border,
                    size,
                });
                Ok(Vec::new())
            }
            "REMOVESPOT" => {
                self.effects.push(Effect::RemoveSpot {
                    spot: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "ADDPICNAME" => {
                let name = text_arg(args, 0)?;
                let spot = int_arg(args, 2)? as i32;
                self.effects.push(Effect::AddPic { spot, name });
                Ok(Vec::new())
            }
            "GETSPOTTYPE" => {
                let kind = self
                    .view
                    .spot(int_arg(args, 0)? as i32)
                    .map_or(0, |s| s.kind);
                Ok(vec![Value::Int(kind)])
            }
            "GETSPOTOPTIONS" => {
                let spot = self.view.spot(int_arg(args, 0)? as i32);
                let kind = spot.map_or(0, |s| s.kind);
                let flags = spot.map_or(0, |s| s.flags);
                Ok(vec![Value::Int(kind), Value::Int(0), Value::Int(flags)])
            }
            "GETSPOTPOINTS" => {
                let flat = self.view.spot(int_arg(args, 0)? as i32).map(|s| {
                    s.points
                        .iter()
                        .flat_map(|(x, y)| [Value::Int(*x), Value::Int(*y)])
                        .collect::<Vec<Value>>()
                });
                Ok(vec![Value::array(flat.unwrap_or_default())])
            }
            "GETROOMOPTIONS" => Ok(vec![Value::Int(0)]),
            "LOCINSPOT" => {
                let x = int_arg(args, 0)? as i32;
                let y = int_arg(args, 1)? as i32;
                let spot = self.view.spot_at(x, y).map_or(0, |s| s.id);
                Ok(vec![Value::Int(spot)])
            }
            // Sparky's `BS` handler pops three operands and answers four zeros
            // (the browser build has no text metrics).
            "GETSPOTTEXTSIZE" => Ok(vec![
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
            ]),

            // ----------------------------------------------- Sparky GS: pictures
            "GETPICNAME" => {
                let _ = int_arg(args, 0)?;
                let _ = int_arg(args, 1)?;
                Ok(vec![Value::str("")])
            }
            "GETPICBRIGHTNESS" | "GETPICSATURATION" | "GETPICOPACITY" | "GETPICANGLE" => {
                let _ = int_arg(args, 0)?;
                let _ = int_arg(args, 1)?;
                Ok(vec![Value::Int(0)])
            }
            "GETPICPIXEL" => {
                for index in 0..4 {
                    let _ = int_arg(args, index)?;
                }
                Ok(vec![Value::Int(0)])
            }
            "NBRPICFRAMES" => {
                let _ = int_arg(args, 0)?;
                let _ = int_arg(args, 1)?;
                Ok(vec![Value::Int(0)])
            }

            // -------------------------------------------------- Sparky GS: sound
            "SOUNDPLAY" | "SOUNDPLAYFROM" | "SOUNDOPEN" => {
                self.effects.push(Effect::PlaySound {
                    name: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "SOUNDGETPOSITION" | "SOUNDLENGTH" => {
                let _ = args.first();
                Ok(vec![Value::Int(0)])
            }
            "ISSOUNDPLAYING" | "SOUNDISPLAYING" => {
                let _ = text_arg(args, 0)?;
                Ok(vec![Value::Int(0)])
            }

            // ---------------------------------------------------- Sparky GS: web
            "WEBEMBED" => {
                let url = text_arg(args, 0)?;
                let spot = int_arg(args, 1)? as i32;
                self.effects.push(Effect::WebEmbed { spot, url });
                Ok(Vec::new())
            }
            "WEBSCRIPT" => {
                let script = text_arg(args, 0)?;
                let spot = int_arg(args, 1)? as i32;
                self.effects.push(Effect::WebScript { spot, script });
                Ok(Vec::new())
            }
            "WEBLOCATION" | "WEBTITLE" => {
                let _ = int_arg(args, 0)?;
                Ok(vec![Value::str("")])
            }
            "LOADWEBSITE" => {
                self.effects.push(Effect::GotoUrl {
                    url: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }

            // --------------------------------------------------- Sparky GS: HTTP
            "HTTPCANCEL" => {
                self.effects.push(Effect::HttpCancel);
                Ok(Vec::new())
            }

            // --------------------------------------------------- Sparky GS: files
            // Sparky resolves a file name against the room's picture list; this
            // host keeps picture ids but not names, so the honest answer is
            // "not found".
            "FILEEXISTS" => {
                let _ = text_arg(args, 0)?;
                Ok(vec![Value::Int(0)])
            }
            "FILEDATE" => {
                let _ = text_arg(args, 0)?;
                Ok(vec![Value::str("")])
            }
            "SELECTFILE" => {
                let _ = text_arg(args, 0)?;
                Ok(vec![Value::Int(0)])
            }

            // ------------------------------------------ Sparky GS: alerts/prompt
            "ALERTBOX" => {
                self.effects.push(Effect::Alert {
                    text: text_arg(args, 0)?,
                });
                Ok(Vec::new())
            }
            "PROMPT" => {
                let label = text_arg(args, 0)?;
                let default = text_arg(args, 1)?;
                self.effects.push(Effect::Prompt {
                    label,
                    default: default.clone(),
                });
                Ok(vec![Value::str(default)])
            }
            "SETCURSOR" => {
                self.effects.push(Effect::SetCursor {
                    index: int_arg(args, 0)? as i32,
                });
                Ok(Vec::new())
            }
            "SETCURSORPIC" => {
                let name = text_arg(args, 2)?;
                let x = int_arg(args, 0)? as i32;
                let y = int_arg(args, 1)? as i32;
                self.effects.push(Effect::SetCursorPic { name, x, y });
                Ok(Vec::new())
            }
            "SETHELPTAG" => self.set_tooltip(&text_arg(args, 0)?).map(|()| Vec::new()),
            "CLEARHELPTAG" => self.clear_tooltip().map(|()| Vec::new()),

            // ---------------------------------------- Sparky GS: alarms/identity
            "TIMEREXEC" => {
                let body = chunk_arg(args, 0)?.clone();
                let ticks = int_arg(args, 1)?.max(0);
                self.alarms.push(PendingAlarm {
                    ticks,
                    kind: AlarmKind::Body(body),
                    spot: self.current_spot,
                });
                Ok(Vec::new())
            }
            "STOPALARM" => {
                let spot = int_arg(args, 0)? as i32;
                self.alarms.retain(|alarm| alarm.spot != spot);
                Ok(Vec::new())
            }
            "STOPALARMS" => self.clear_alarms().map(|()| Vec::new()),
            "CLIENTID" => Ok(vec![Value::Int(self.get_self_user_id() as i32)]),
            "GETTIMEZONE" => Ok(vec![Value::Int(local_utc_offset_hours())]),

            // --------------------------------------------- Sparky GS: other reads
            "MEDIAADDRESS" => Ok(vec![Value::str("")]),
            "NBRSERVERUSERS" => Ok(vec![Value::Int(self.view.users.len() as i32)]),
            "NBRROOMPICS" => Ok(vec![Value::Int(0)]),
            "ROOMPICNAME" => Ok(vec![Value::str("")]),
            "ISKEYDOWN" => {
                let _ = int_arg(args, 0)?;
                Ok(vec![Value::Int(0)])
            }
            "WHEREPROP" => Ok(vec![Value::Int(0), Value::Int(0)]),
            "WHOCOLOR" => {
                let user = int_arg(args, 0)?;
                let color = if user == i64::from(self.view.self_id) {
                    self.view.color
                } else {
                    0
                };
                Ok(vec![Value::Int(color)])
            }
            "WHOFACE" => {
                let user = int_arg(args, 0)?;
                let face = if user == i64::from(self.view.self_id) {
                    self.view.face
                } else {
                    0
                };
                Ok(vec![Value::Int(face)])
            }
            "MUTE" | "UNMUTE" => {
                let target = text_arg(args, 0)?;
                if self.is_wizard() {
                    let verb = if name == "MUTE" { "mute" } else { "unmute" };
                    self.send_private_message(0, &format!("`{verb} {target}"))?;
                }
                Ok(Vec::new())
            }

            // ------------------------------- Sparky GS: explicit, traced refusals
            "SETSPOTCLIP"
            | "SETSPOTCURVE"
            | "SETSPOTGRADIENT"
            | "SETSPOTPATHGRADIENT"
            | "SETSPOTFONT" => self.refuse(
                name,
                "hotspot gradient, curve, clip and font rendering is not wired into this client",
            ),
            "INSERTPIC" => self.refuse(
                name,
                "inserting a picture at an index is not wired into this client",
            ),
            "SETPICCONTRAST" | "SETPICHUE" | "SETPICANGLE" | "SETPICBLUR" | "SETPICFRAME"
            | "PAUSEPIC" | "RESUMEPIC" => self.refuse(
                name,
                "picture filter and frame animation is not wired into this client",
            ),
            "SOUNDLOOP" | "SOUNDSTOP" | "SOUNDPAUSE" | "SOUNDSEEK" => self.refuse(
                name,
                "sound looping, stopping and seeking is not tracked by this client",
            ),
            "WEBCLICKTHRU" => self.refuse(
                name,
                "click-through control for embedded web views is not available",
            ),
            "ADDHEADER" | "REMOVEHEADER" | "RESETHEADERS" | "HTTPPOST" => self.refuse(
                name,
                "HTTP headers and POST are not exposed to scripts by this client",
            ),
            "FILEDELETE" => self.refuse(name, "scripts may not delete files on this client"),
            "ISFUNCTION" => self.refuse(
                name,
                "the command dictionary is not exposed to scripts at runtime",
            ),
            "CACHESCRIPT" => self.refuse(
                name,
                "script caching is internal to the engine and not script-driven",
            ),
            "IMAGETOPROP" => self.refuse(name, "converting an image to a prop is not available"),
            "TEXTSPEECH" => self.refuse(name, "text-to-speech output is not available"),
            "UPDATELATER" | "UPDATENOW" => self.refuse(
                name,
                "the reference handler is a no-op; this client reports it",
            ),

            // The legacy Palace commands below have no implementation in any
            // reference in the corpus (OpenPalace, Sparky's GS table, or the
            // guide), so they stay explicit, traced refusals via the catch-all.
            _ => self.unimplemented(name, pushes, push),
        }
    }
}

impl PalaceHost for ScriptHost {
    fn get_self_user_id(&self) -> i64 {
        i64::from(self.view.self_id)
    }

    fn get_self_user_name(&self) -> String {
        self.view.self_name.clone()
    }

    fn get_user_name(&self, user: i64) -> String {
        self.view
            .user(user as i32)
            .map(|u| u.name.clone())
            .unwrap_or_default()
    }

    fn get_user_by_name(&self, name: &str) -> i64 {
        i64::from(self.view.user_by_name(name))
    }

    fn is_guest(&self) -> bool {
        self.view.is_guest
    }

    /// `PalaceController.as:210-213`: `isWizard()` returns
    /// `currentUser.isWizard || currentUser.isGod`, so a server owner answers
    /// `ISWIZARD` true here too.
    fn is_wizard(&self) -> bool {
        self.view.is_wizard || self.view.is_god
    }

    fn is_god(&self) -> bool {
        self.view.is_god
    }

    /// `CLIENTTYPECommand.as:10` pushes the literal `"WINDOWS32"`.
    fn client_type(&self) -> String {
        "WINDOWS32".to_owned()
    }

    fn get_room_id(&self) -> i64 {
        i64::from(self.view.room_id)
    }

    fn get_room_name(&self) -> String {
        self.view.room_name.clone()
    }

    fn get_server_name(&self) -> String {
        self.view.server_name.clone()
    }

    fn get_room_width(&self) -> i64 {
        i64::from(self.view.room_width)
    }

    fn get_room_height(&self) -> i64 {
        i64::from(self.view.room_height)
    }

    fn get_num_room_users(&self) -> i64 {
        self.view.users.len() as i64
    }

    fn get_room_user_id_by_index(&self, index: i64) -> i64 {
        self.view
            .users
            .get(index.max(0) as usize)
            .map_or(0, |u| i64::from(u.id))
    }

    fn get_num_spots(&self) -> i64 {
        self.view.spots.len() as i64
    }

    fn get_spot_id_by_index(&self, index: i64) -> i64 {
        self.view
            .spots
            .get(index.max(0) as usize)
            .map_or(0, |s| i64::from(s.id))
    }

    fn get_door_id_by_index(&self, index: i64) -> i64 {
        self.view
            .spots
            .iter()
            .filter(|s| s.kind == 1)
            .nth(index.max(0) as usize)
            .map_or(0, |s| i64::from(s.id))
    }

    fn goto_room(&mut self, room: i64) -> Result<()> {
        self.effects.push(Effect::GotoRoom { room: room as i32 });
        Ok(())
    }

    fn goto_url(&mut self, url: &str) -> Result<()> {
        self.effects.push(Effect::GotoUrl {
            url: url.to_owned(),
        });
        Ok(())
    }

    fn launch_app(&mut self, app: &str) -> Result<()> {
        self.effects.push(Effect::LaunchApp {
            app: app.to_owned(),
        });
        Ok(())
    }

    fn dim_room(&mut self, percent: i64) -> Result<()> {
        self.effects.push(Effect::DimRoom {
            percent: percent as i32,
        });
        Ok(())
    }

    fn get_self_pos_x(&self) -> i64 {
        i64::from(self.view.self_x)
    }

    fn get_self_pos_y(&self) -> i64 {
        i64::from(self.view.self_y)
    }

    fn get_pos_x(&self, user: i64) -> i64 {
        self.view.user(user as i32).map_or(0, |u| i64::from(u.x))
    }

    fn get_pos_y(&self, user: i64) -> i64 {
        self.view.user(user as i32).map_or(0, |u| i64::from(u.y))
    }

    fn get_mouse_x(&self) -> i64 {
        i64::from(self.view.mouse.0)
    }

    fn get_mouse_y(&self) -> i64 {
        i64::from(self.view.mouse.1)
    }

    fn move_user_abs(&mut self, x: i64, y: i64) -> Result<()> {
        self.effects.push(Effect::MoveUserAbs {
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }

    fn move_user_rel(&mut self, dx: i64, dy: i64) -> Result<()> {
        self.effects.push(Effect::MoveUserRel {
            dx: dx as i32,
            dy: dy as i32,
        });
        Ok(())
    }

    fn get_spot_state(&self, spot: i64) -> i64 {
        self.view
            .spot(spot as i32)
            .map_or(0, |s| i64::from(s.state))
    }

    fn get_spot_dest(&self, spot: i64) -> i64 {
        self.view.spot(spot as i32).map_or(0, |s| i64::from(s.dest))
    }

    fn get_spot_name(&self, spot: i64) -> String {
        self.view
            .spot(spot as i32)
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    fn get_spot_location(&self, spot: i64) -> (i64, i64) {
        self.view
            .spot(spot as i32)
            .map_or((0, 0), |s| (i64::from(s.loc.0), i64::from(s.loc.1)))
    }

    fn get_pic_offset(&self, spot: i64, state: i64) -> (i64, i64) {
        let (x, y) = self.view.pic_offset(spot as i32, state as i32);
        (i64::from(x), i64::from(y))
    }

    fn get_pic_dimensions(&self, spot: i64, state: i64) -> (i64, i64) {
        let (w, h) = self.view.pic_dimensions(spot as i32, state as i32);
        (i64::from(w), i64::from(h))
    }

    fn get_prop_dimensions(&self, prop: i64) -> (i64, i64) {
        let (w, h) = self.view.prop_dimensions(prop);
        (i64::from(w), i64::from(h))
    }

    fn get_prop_offsets(&self, prop: i64) -> (i64, i64) {
        let (x, y) = self.view.prop_offsets(prop);
        (i64::from(x), i64::from(y))
    }

    fn set_spot_state(&mut self, spot: i64, state: i64) -> Result<()> {
        self.effects.push(Effect::SetSpotState {
            spot: spot as i32,
            state: state as i32,
        });
        Ok(())
    }

    fn set_spot_state_local(&mut self, spot: i64, state: i64) -> Result<()> {
        self.effects.push(Effect::SetSpotStateLocal {
            spot: spot as i32,
            state: state as i32,
        });
        Ok(())
    }

    fn set_spot_name_local(&mut self, spot: i64, name: &str) -> Result<()> {
        self.effects.push(Effect::SetSpotNameLocal {
            spot: spot as i32,
            name: name.to_owned(),
        });
        Ok(())
    }

    fn move_spot(&mut self, spot: i64, dx: i64, dy: i64) -> Result<()> {
        self.effects.push(Effect::MoveSpot {
            spot: spot as i32,
            dx: dx as i32,
            dy: dy as i32,
        });
        Ok(())
    }

    fn move_spot_local(&mut self, spot: i64, dx: i64, dy: i64) -> Result<()> {
        self.effects.push(Effect::MoveSpotLocal {
            spot: spot as i32,
            dx: dx as i32,
            dy: dy as i32,
        });
        Ok(())
    }

    fn set_pic_offset(&mut self, spot: i64, dx: i64, dy: i64) -> Result<()> {
        self.effects.push(Effect::SetPicOffset {
            spot: spot as i32,
            dx: dx as i32,
            dy: dy as i32,
        });
        Ok(())
    }

    fn set_pic_offset_local(&mut self, spot: i64, state: i64, dx: i64, dy: i64) -> Result<()> {
        self.effects.push(Effect::SetPicOffsetLocal {
            spot: spot as i32,
            state: state as i32,
            dx: dx as i32,
            dy: dy as i32,
        });
        Ok(())
    }

    fn set_pic_opacity(&mut self, spot: i64, state: i64, opacity: f64) -> Result<()> {
        self.effects.push(Effect::SetPicOpacity {
            spot: spot as i32,
            state: state as i32,
            opacity,
        });
        Ok(())
    }

    fn set_tooltip(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::SetTooltip {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn clear_tooltip(&mut self) -> Result<()> {
        self.effects.push(Effect::ClearTooltip);
        Ok(())
    }

    /// Locked means a shuttable (2) or lockable (3) door whose state is 1.
    /// Every other kind answers `false` however its state reads, as does an id
    /// that is not in the room.
    fn is_locked(&self, door: i64) -> bool {
        self.view
            .spot(door as i32)
            .is_some_and(|spot| matches!(spot.kind, 2 | 3) && spot.state == 1)
    }

    fn in_spot(&self, spot: i64) -> bool {
        self.view.self_in_spot(spot as i32)
    }

    fn lock(&mut self, door: i64) -> Result<()> {
        self.effects.push(Effect::Lock { spot: door as i32 });
        Ok(())
    }

    fn unlock(&mut self, door: i64) -> Result<()> {
        self.effects.push(Effect::Unlock { spot: door as i32 });
        Ok(())
    }

    fn select_hot_spot(&mut self, spot: i64) -> Result<()> {
        self.effects.push(Effect::SelectSpot { spot: spot as i32 });
        Ok(())
    }

    fn set_spot_alarm(&mut self, spot: i64, future_ticks: i64) -> Result<()> {
        let spot = if spot == 0 {
            self.current_spot
        } else {
            spot as i32
        };
        self.alarms.push(PendingAlarm {
            ticks: future_ticks.max(0),
            kind: AlarmKind::Spot,
            spot,
        });
        Ok(())
    }

    fn get_top_prop(&self) -> i64 {
        self.view.self_props.last().copied().unwrap_or(0)
    }

    fn get_user_prop(&self, index: i64) -> i64 {
        self.view
            .self_props
            .get(index.max(0) as usize)
            .copied()
            .unwrap_or(0)
    }

    fn get_num_user_props(&self) -> i64 {
        self.view.self_props.len() as i64
    }

    /// A numeric string names a prop id directly; the Palace prop format stores
    /// no name, so any other spelling resolves to `0` (not worn).
    fn get_prop_id_by_name(&self, name: &str) -> i64 {
        name.trim().parse::<i64>().unwrap_or(0)
    }

    fn has_prop_by_id(&self, prop: i64) -> bool {
        self.view.self_props.contains(&prop)
    }

    fn has_prop_by_name(&self, name: &str) -> bool {
        let prop = self.get_prop_id_by_name(name);
        prop != 0 && self.has_prop_by_id(prop)
    }

    fn don_prop_by_id(&mut self, prop: i64) -> Result<()> {
        self.wear_prop_now(prop);
        self.effects.push(Effect::DonProp { prop });
        Ok(())
    }

    fn don_prop_by_name(&mut self, name: &str) -> Result<()> {
        let prop = self.get_prop_id_by_name(name);
        if prop == 0 {
            return Ok(());
        }
        self.don_prop_by_id(prop)
    }

    fn set_props(&mut self, props: &[i64]) -> Result<()> {
        self.set_props_now(props);
        self.effects.push(Effect::SetProps {
            props: props.to_vec(),
        });
        Ok(())
    }

    fn doff_prop(&mut self) -> Result<()> {
        self.view.self_props.pop();
        self.effects.push(Effect::DoffProp);
        Ok(())
    }

    fn doff_prop_by_id(&mut self, prop: i64) -> Result<()> {
        self.remove_prop_now(prop);
        self.effects.push(Effect::RemoveProp { prop });
        Ok(())
    }

    fn doff_prop_by_name(&mut self, name: &str) -> Result<()> {
        let prop = self.get_prop_id_by_name(name);
        if prop == 0 {
            return Ok(());
        }
        self.doff_prop_by_id(prop)
    }

    fn naked(&mut self) -> Result<()> {
        self.view.self_props.clear();
        self.effects.push(Effect::Naked);
        Ok(())
    }

    fn load_props(&mut self, props: &[i64]) -> Result<()> {
        self.effects.push(Effect::LoadProps {
            props: props.to_vec(),
        });
        Ok(())
    }

    fn get_num_loose_props(&self) -> i64 {
        self.view.loose_props.len() as i64
    }

    fn get_loose_prop_id_by_index(&self, index: i64) -> i64 {
        self.view
            .loose_props
            .get(index.max(0) as usize)
            .map_or(0, |p| p.id)
    }

    fn get_loose_prop_index_by_id(&self, prop: i64) -> i64 {
        self.view.loose_prop_index(prop)
    }

    fn get_loose_prop_pos(&self, index: i64) -> (i64, i64) {
        self.view
            .loose_props
            .get(index.max(0) as usize)
            .map_or((0, 0), |p| (i64::from(p.x), i64::from(p.y)))
    }

    fn add_loose_prop(&mut self, prop: i64, x: i64, y: i64) -> Result<()> {
        self.effects.push(Effect::AddLooseProp {
            prop,
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }

    fn remove_loose_prop(&mut self, index: i64) -> Result<()> {
        self.effects.push(Effect::RemoveLooseProp {
            index: index as i32,
        });
        Ok(())
    }

    fn move_loose_prop(&mut self, index: i64, x: i64, y: i64) -> Result<()> {
        self.effects.push(Effect::MoveLooseProp {
            index: index as i32,
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }

    fn drop_prop(&mut self, x: i64, y: i64) -> Result<()> {
        self.effects.push(Effect::DropProp {
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }

    fn clear_loose_props(&mut self) -> Result<()> {
        self.effects.push(Effect::ClearLooseProps);
        Ok(())
    }

    /// `PalaceController.as:697-706` appends `id x y ADDLOOSEPROP` per loose
    /// prop and logs the buffer once, only when it is non-empty.
    fn show_loose_props(&mut self) -> Result<()> {
        let mut text = String::new();
        for prop in &self.view.loose_props {
            text.push_str(&format!("{} {} {} ADDLOOSEPROP\n", prop.id, prop.x, prop.y));
        }
        if !text.is_empty() {
            self.effects.push(Effect::LogMessage { text });
        }
        Ok(())
    }

    fn chat(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::Say {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn send_global_message(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::GlobalMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn send_room_message(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::RoomMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn send_local_msg(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::LocalMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn send_susr_message(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::SuperUserMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn send_private_message(&mut self, user: i64, text: &str) -> Result<()> {
        self.effects.push(Effect::PrivateMessage {
            user: user as i32,
            text: text.to_owned(),
        });
        Ok(())
    }

    fn status_message(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::StatusMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn log_message(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::LogMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn log_error(&mut self, text: &str) -> Result<()> {
        self.effects.push(Effect::ErrorMessage {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn get_who_chat(&self) -> i64 {
        i64::from(self.view.who_chat)
    }

    fn get_who_target(&self) -> i64 {
        i64::from(self.view.who_target)
    }

    fn change_color(&mut self, colour: i64) -> Result<()> {
        self.effects.push(Effect::SetColor {
            color: colour as i32,
        });
        Ok(())
    }

    fn set_face(&mut self, face: i64) -> Result<()> {
        self.effects.push(Effect::SetFace { face: face as i32 });
        Ok(())
    }

    fn do_macro(&mut self, _macro: i64) -> Result<()> {
        Ok(())
    }

    fn kill_user(&mut self, _user: i64) -> Result<()> {
        Err(IptError::CommandUnavailable {
            command: "KILLUSER".to_owned(),
        })
    }

    fn hide_avatars(&mut self) -> Result<()> {
        self.effects.push(Effect::HideAvatars);
        Ok(())
    }

    fn show_avatars(&mut self) -> Result<()> {
        self.effects.push(Effect::ShowAvatars);
        Ok(())
    }

    fn play_sound(&mut self, name: &str) -> Result<()> {
        self.effects.push(Effect::PlaySound {
            name: name.to_owned(),
        });
        Ok(())
    }

    fn midi_play(&mut self, name: &str) -> Result<()> {
        self.effects.push(Effect::MidiPlay {
            name: name.to_owned(),
        });
        Ok(())
    }

    fn midi_loop(&mut self, name: &str, loops: i64) -> Result<()> {
        self.effects.push(Effect::MidiLoop {
            name: name.to_owned(),
            loops: loops as i32,
        });
        Ok(())
    }

    fn midi_stop(&mut self) -> Result<()> {
        self.effects.push(Effect::MidiStop);
        Ok(())
    }

    fn beep(&mut self) -> Result<()> {
        self.effects.push(Effect::Beep);
        Ok(())
    }

    fn draw_line_abs(&mut self, x1: i64, y1: i64, x2: i64, y2: i64) -> Result<()> {
        self.effects.push(Effect::DrawLine {
            x1: x1 as i32,
            y1: y1 as i32,
            x2: x2 as i32,
            y2: y2 as i32,
        });
        self.pen.pos = (x2 as i32, y2 as i32);
        Ok(())
    }

    fn draw_line_rel(&mut self, dx: i64, dy: i64) -> Result<()> {
        let (x1, y1) = self.pen.pos;
        let (x2, y2) = (x1 + dx as i32, y1 + dy as i32);
        self.pen.pos = (x2, y2);
        self.effects.push(Effect::DrawLineRel { x1, y1, x2, y2 });
        Ok(())
    }

    fn move_pen_abs(&mut self, x: i64, y: i64) -> Result<()> {
        self.pen.pos = (x as i32, y as i32);
        self.effects.push(Effect::MovePen {
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }

    fn move_pen_rel(&mut self, dx: i64, dy: i64) -> Result<()> {
        let (x, y) = (self.pen.pos.0 + dx as i32, self.pen.pos.1 + dy as i32);
        self.pen.pos = (x, y);
        self.effects.push(Effect::MovePen { x, y });
        Ok(())
    }

    fn set_pen_color(&mut self, r: i64, g: i64, b: i64) -> Result<()> {
        self.pen.rgb = (r as i32, g as i32, b as i32);
        self.effects.push(Effect::SetPenColor {
            r: r as i32,
            g: g as i32,
            b: b as i32,
        });
        Ok(())
    }

    fn set_pen_size(&mut self, size: i64) -> Result<()> {
        self.pen.size = size as i32;
        self.effects.push(Effect::SetPenSize { size: size as i32 });
        Ok(())
    }

    fn paint_back_layer(&mut self) -> Result<()> {
        self.pen.front = false;
        self.effects.push(Effect::PaintLayer { front: false });
        Ok(())
    }

    fn paint_front_layer(&mut self) -> Result<()> {
        self.pen.front = true;
        self.effects.push(Effect::PaintLayer { front: true });
        Ok(())
    }

    fn paint_clear(&mut self) -> Result<()> {
        self.effects.push(Effect::PaintClear);
        Ok(())
    }

    fn paint_undo(&mut self) -> Result<()> {
        self.effects.push(Effect::PaintUndo);
        Ok(())
    }

    fn clear_alarms(&mut self) -> Result<()> {
        self.alarms.clear();
        Ok(())
    }

    fn delay_ticks(&mut self, _ticks: i64) -> Result<()> {
        Ok(())
    }

    fn set_chat_string(&mut self, text: &str) -> Result<()> {
        self.view.chat_string = text.to_owned();
        self.effects.push(Effect::SetChatString {
            text: text.to_owned(),
        });
        Ok(())
    }

    fn get_chat_string(&self) -> String {
        self.view.chat_string.clone()
    }
}

impl ScriptHost {
    fn say_at(&mut self, text: &str, x: i64, y: i64) -> Result<()> {
        self.effects.push(Effect::SayAt {
            text: text.to_owned(),
            x: x as i32,
            y: y as i32,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::LoosePropView;

    fn view() -> HostView {
        HostView {
            self_id: 42,
            self_name: "Self".to_owned(),
            self_props: vec![7, 9],
            loose_props: vec![LoosePropView {
                id: 100,
                x: 5,
                y: 6,
            }],
            ..HostView::default()
        }
    }

    fn run(host: &mut ScriptHost, name: &str, args: &[Value]) -> Vec<Value> {
        Host::command(host, name, args).expect("command runs")
    }

    fn one_effect(host: &ScriptHost) -> &Effect {
        assert_eq!(host.effects.len(), 1, "effects: {:?}", host.effects);
        &host.effects[0]
    }

    #[test]
    fn id_and_me_answer_the_hotspot_id() {
        let mut host = ScriptHost::new(view());
        host.current_spot = 7;
        assert_eq!(run(&mut host, "ME", &[]), vec![Value::Int(7)]);
        assert_eq!(run(&mut host, "ID", &[]), vec![Value::Int(7)]);
        assert_eq!(run(&mut host, "USERID", &[]), vec![Value::Int(42)]);
        assert_eq!(run(&mut host, "WHOME", &[]), vec![Value::Int(42)]);
    }

    #[test]
    fn clienttype_answers_windows32() {
        let mut host = ScriptHost::new(view());
        assert_eq!(
            run(&mut host, "CLIENTTYPE", &[]),
            vec![Value::str("WINDOWS32")]
        );
    }

    #[test]
    fn iswizard_is_true_for_a_god() {
        let mut god = ScriptHost::new(HostView {
            is_god: true,
            ..view()
        });
        assert_eq!(run(&mut god, "ISWIZARD", &[]), vec![Value::Int(1)]);
        let mut wizard = ScriptHost::new(HostView {
            is_wizard: true,
            ..view()
        });
        assert_eq!(run(&mut wizard, "ISWIZARD", &[]), vec![Value::Int(1)]);
        let mut plain = ScriptHost::new(view());
        assert_eq!(run(&mut plain, "ISWIZARD", &[]), vec![Value::Int(0)]);
    }

    #[test]
    fn the_revived_names_are_reachable() {
        let mut host = ScriptHost::new(HostView {
            mouse: (11, 22),
            right_click: true,
            ..view()
        });
        assert_eq!(run(&mut host, "MOUSEX", &[]), vec![Value::Int(11)]);
        assert_eq!(run(&mut host, "MOUSEY", &[]), vec![Value::Int(22)]);
        assert_eq!(run(&mut host, "ISRIGHTCLICK", &[]), vec![Value::Int(1)]);
        assert_eq!(run(&mut host, "HTTPRECEIVED", &[]), vec![Value::Int(0)]);
        assert_eq!(
            run(&mut host, "STR", &[Value::Int(5)]),
            vec![Value::str("5")]
        );
        assert!(command_spec("STR").is_none(), "STR is a corpus variable");
        assert!(command_spec("LASTNAME").is_none());
    }

    #[test]
    fn props_resolve_by_numeric_name() {
        let mut host = ScriptHost::new(view());
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::str("9")]),
            vec![Value::Int(1)]
        );
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::str("404")]),
            vec![Value::Int(0)]
        );
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::str("nope")]),
            vec![Value::Int(0)]
        );
        host.take_effects();
        run(&mut host, "DONPROP", &[Value::str("9")]);
        assert_eq!(one_effect(&host), &Effect::DonProp { prop: 9 });
        host.take_effects();
        run(&mut host, "REMOVEPROP", &[Value::str("7")]);
        assert_eq!(one_effect(&host), &Effect::RemoveProp { prop: 7 });
    }

    #[test]
    fn a_worn_prop_change_is_read_back_in_the_same_handler() {
        // `PalaceUser.setProps`/`wearProp`/`removeProp`/`naked`
        // (`OpenPalace/PalaceClient/src/net/codecomposer/palace/model/PalaceUser.as:139-185`)
        // mutate `currentUser.props` and `propCount` before calling
        // `updatePropsOnServer`. `NBRUSERPROPS` reads `propCount` and `HASPROP`/
        // `USERPROP`/`TOPPROP` read `currentUser.props`
        // (`PalaceClient-iptscrae/PalaceController.as:222-272,474-481,522-525`),
        // so each command below must be visible to the read that follows it,
        // while the `Effect` for the server is still recorded.
        let mut host = ScriptHost::new(HostView::default());
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(0)]);

        run(
            &mut host,
            "SETPROPS",
            &[Value::array(vec![Value::Int(5), Value::Int(6)])],
        );
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(2)]);
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::Int(5)]),
            vec![Value::Int(1)]
        );
        assert_eq!(run(&mut host, "TOPPROP", &[]), vec![Value::Int(6)]);
        assert_eq!(
            run(&mut host, "USERPROP", &[Value::Int(1)]),
            vec![Value::Int(6)]
        );
        assert_eq!(one_effect(&host), &Effect::SetProps { props: vec![5, 6] });
        host.take_effects();

        run(&mut host, "DONPROP", &[Value::Int(7)]);
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(3)]);
        assert_eq!(one_effect(&host), &Effect::DonProp { prop: 7 });
        host.take_effects();

        run(&mut host, "REMOVEPROP", &[Value::Int(5)]);
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(2)]);
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::Int(5)]),
            vec![Value::Int(0)]
        );
        assert_eq!(one_effect(&host), &Effect::RemoveProp { prop: 5 });
        host.take_effects();

        run(&mut host, "DOFFPROP", &[]);
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(1)]);
        assert_eq!(one_effect(&host), &Effect::DoffProp);
        host.take_effects();

        run(&mut host, "NAKED", &[]);
        assert_eq!(run(&mut host, "NBRUSERPROPS", &[]), vec![Value::Int(0)]);
        assert_eq!(one_effect(&host), &Effect::Naked);
    }

    #[test]
    fn setprops_skips_the_empty_slot_duplicates_and_the_cap_like_the_model() {
        let mut host = ScriptHost::new(HostView::default());
        run(
            &mut host,
            "SETPROPS",
            &[Value::array((0..12).map(Value::Int).collect::<Vec<_>>())],
        );
        assert_eq!(
            run(&mut host, "NBRUSERPROPS", &[]),
            vec![Value::Int(9)],
            "the empty slot is skipped and the list stops at nine"
        );
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::Int(9)]),
            vec![Value::Int(1)]
        );
        assert_eq!(
            run(&mut host, "HASPROP", &[Value::Int(10)]),
            vec![Value::Int(0)]
        );

        run(&mut host, "DONPROP", &[Value::Int(99)]);
        assert_eq!(
            run(&mut host, "NBRUSERPROPS", &[]),
            vec![Value::Int(9)],
            "a tenth DONPROP is refused, not appended"
        );
    }

    #[test]
    fn setspotnamelocal_renames_locally() {
        let mut host = ScriptHost::new(view());
        run(
            &mut host,
            "SETSPOTNAMELOCAL",
            &[Value::str("Gate"), Value::Int(5)],
        );
        assert_eq!(
            one_effect(&host),
            &Effect::SetSpotNameLocal {
                spot: 5,
                name: "Gate".to_owned()
            }
        );
    }

    #[test]
    fn loadprops_queues_a_preload() {
        let mut host = ScriptHost::new(view());
        run(
            &mut host,
            "LOADPROPS",
            &[Value::array(vec![Value::Int(7), Value::Int(9)])],
        );
        assert_eq!(one_effect(&host), &Effect::LoadProps { props: vec![7, 9] });
    }

    #[test]
    fn killuser_whispers_the_kill_command() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "KILLUSER", &[Value::Int(3)]);
        assert_eq!(
            one_effect(&host),
            &Effect::PrivateMessage {
                user: 3,
                text: "`kill".to_owned()
            }
        );
    }

    #[test]
    fn showlooseprops_logs_one_addlooseprop_line_each() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "SHOWLOOSEPROPS", &[]);
        match one_effect(&host) {
            Effect::LogMessage { text } => assert_eq!(text, "100 5 6 ADDLOOSEPROP\n"),
            other => panic!("expected a log message, got {other:?}"),
        }
    }

    #[test]
    fn pic_brightness_and_saturation_record_their_filters() {
        let mut host = ScriptHost::new(view());
        run(
            &mut host,
            "SETPICBRIGHTNESS",
            &[Value::Int(50), Value::Int(1), Value::Int(2)],
        );
        assert_eq!(
            one_effect(&host),
            &Effect::SetPicBrightness {
                spot: 2,
                state: 1,
                value: 50
            }
        );
        host.take_effects();
        run(
            &mut host,
            "SETPICSATURATION",
            &[Value::Int(-20), Value::Int(3), Value::Int(4)],
        );
        assert_eq!(
            one_effect(&host),
            &Effect::SetPicSaturation {
                spot: 4,
                state: 3,
                value: -20
            }
        );
    }

    #[test]
    fn removepic_takes_the_index_pushed_first() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "REMOVEPIC", &[Value::Int(1), Value::Int(2)]);
        assert_eq!(
            one_effect(&host),
            &Effect::RemovePic {
                spot: 2,
                picture: 1
            }
        );
    }

    #[test]
    fn gotourlframe_keeps_the_url_and_frame() {
        let mut host = ScriptHost::new(view());
        run(
            &mut host,
            "GOTOURLFRAME",
            &[Value::str("http://x"), Value::str("body")],
        );
        assert_eq!(
            one_effect(&host),
            &Effect::GotoUrlFrame {
                url: "http://x".to_owned(),
                frame: "body".to_owned()
            }
        );
    }

    #[test]
    fn the_launch_family_records_named_effects() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "LAUNCHEVENT", &[Value::str("Go")]);
        assert_eq!(
            one_effect(&host),
            &Effect::LaunchEvent {
                event: "Go".to_owned()
            }
        );
        host.take_effects();
        run(&mut host, "LAUNCHPPA", &[Value::str("plugin")]);
        assert_eq!(
            one_effect(&host),
            &Effect::LaunchPpa {
                ppa: "plugin".to_owned()
            }
        );
        host.take_effects();
        run(&mut host, "LOADJAVA", &[Value::str("mod")]);
        assert_eq!(
            one_effect(&host),
            &Effect::LoadJava {
                url: "mod".to_owned()
            }
        );
        host.take_effects();
        run(&mut host, "TALKPPA", &[Value::str("hi")]);
        assert_eq!(
            one_effect(&host),
            &Effect::TalkPpa {
                text: "hi".to_owned()
            }
        );
        host.take_effects();
        run(&mut host, "SHELLCMD", &[Value::str("rm -rf /")]);
        assert_eq!(
            one_effect(&host),
            &Effect::ShellCommand {
                command: "rm -rf /".to_owned()
            }
        );
    }

    #[test]
    fn the_local_flags_record_their_effects() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "HIDESMILEYS", &[]);
        assert_eq!(one_effect(&host), &Effect::HideSmileys);
        host.take_effects();
        run(&mut host, "LOCKUSERPROPS", &[]);
        assert_eq!(one_effect(&host), &Effect::LockUserProps);
        host.take_effects();
        run(&mut host, "AUTOUSERLAYER", &[Value::Int(1)]);
        assert_eq!(one_effect(&host), &Effect::AutoUserLayer { on: true });
        host.take_effects();
        run(&mut host, "AUTOUSERLAYER", &[Value::Int(0)]);
        assert_eq!(one_effect(&host), &Effect::AutoUserLayer { on: false });
    }

    #[test]
    fn confirmbox_asks_and_answers_declined() {
        let mut host = ScriptHost::new(view());
        let pushed = run(&mut host, "CONFIRMBOX", &[Value::str("sure?")]);
        assert_eq!(pushed, vec![Value::Int(0)]);
        assert_eq!(
            one_effect(&host),
            &Effect::Confirm {
                text: "sure?".to_owned()
            }
        );
    }

    #[test]
    fn clrprops_clears_props_like_clearprops() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "CLRPROPS", &[]);
        assert_eq!(one_effect(&host), &Effect::Naked);
    }

    #[test]
    fn ban_and_kick_are_gated_on_operator_status() {
        let mut wizard = ScriptHost::new(HostView {
            is_god: true,
            ..view()
        });
        run(&mut wizard, "BAN", &[Value::str("bob")]);
        assert_eq!(
            one_effect(&wizard),
            &Effect::PrivateMessage {
                user: 0,
                text: "`ban bob".to_owned()
            }
        );
        wizard.take_effects();
        run(&mut wizard, "KICK", &[Value::str("bob")]);
        assert_eq!(
            one_effect(&wizard),
            &Effect::PrivateMessage {
                user: 0,
                text: "`kick bob".to_owned()
            }
        );

        let mut plain = ScriptHost::new(view());
        run(&mut plain, "BAN", &[Value::str("bob")]);
        assert!(plain.effects.is_empty(), "a guest cannot ban");
    }

    #[test]
    fn the_legacy_words_are_explicit_refusals() {
        let mut host = ScriptHost::new(view());
        run(&mut host, "ADDPROP", &[]);
        assert_eq!(
            one_effect(&host),
            &Effect::Unsupported {
                command: "ADDPROP".to_owned()
            }
        );
        assert_eq!(host.unsupported.get("ADDPROP"), Some(&1));
    }
}

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
    match args.get(index) {
        Some(Value::Array(items)) => match items.try_borrow() {
            Ok(items) => items.iter().filter_map(as_int).collect(),
            Err(_) => Vec::new(),
        },
        Some(other) => as_int(other).into_iter().collect(),
        None => Vec::new(),
    }
}

fn as_int(value: &Value) -> Option<i64> {
    match value {
        Value::Int(n) => Some(i64::from(*n)),
        _ => None,
    }
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
        vec![(
            "CHATSTR".to_owned(),
            Value::str(self.view.chat_string.as_str()),
        )]
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
            "ME" | "ID" | "USERID" | "WHOME" => {
                Ok(vec![Value::Int(self.get_self_user_id() as i32)])
            }
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
            "PALACECHAT" => Ok(vec![Value::Int(1)]),
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
            "LASTNAME" => Ok(vec![Value::str(String::new())]),

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
            "HASPROP" => Ok(vec![Value::Int(i32::from(
                self.has_prop_by_id(int_arg(args, 0)?),
            ))]),
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
            "PROPDIMENSIONS" | "PROPOFFSETS" => Ok(stub_values(pushes.max(2), Push::Int)),
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
            "SETSPOTNAMELOCAL" => {
                let text = text_arg(args, 0)?;
                let spot = int_arg(args, 1)?;
                self.effects.push(Effect::Unsupported {
                    command: format!("SETSPOTNAMELOCAL({spot},{text})"),
                });
                Ok(Vec::new())
            }
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
            "SETPICBRIGHTNESS" | "SETPICSATURATION" | "SETPICDIM" => {
                self.unimplemented(name, pushes, push)
            }
            "ADDSPOT" => {
                let flat = props_arg(args, 0);
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
                self.effects.push(Effect::DonProp {
                    prop: loose_int(args, 0)?,
                });
                Ok(Vec::new())
            }
            "REMOVEPROP" => {
                self.effects.push(Effect::RemoveProp {
                    prop: loose_int(args, 0)?,
                });
                Ok(Vec::new())
            }
            "DOFFPROP" => {
                self.effects.push(Effect::DoffProp);
                Ok(Vec::new())
            }
            "NAKED" | "CLEARPROPS" => {
                self.effects.push(Effect::Naked);
                Ok(Vec::new())
            }
            "SETPROPS" => {
                self.effects.push(Effect::SetProps {
                    props: props_arg(args, 0),
                });
                Ok(Vec::new())
            }
            "LOADPROPS" => Ok(Vec::new()),

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
            "KILLUSER" => self.unimplemented(name, pushes, push),
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
            "GOTOURLFRAME" | "LAUNCHEVENT" | "LAUNCHPPA" | "LOADJAVA" | "TALKPPA" => {
                self.unimplemented(name, pushes, push)
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
            "SHOWLOOSEPROPS" => self.unimplemented(name, pushes, push),

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
            "HIDESMILEYS" | "LOCKUSERPROPS" | "AUTOUSERLAYER" | "REMOVEPIC" | "DELPIC"
            | "ROOMZOOM" | "ROOMUNZOOM" | "CIRCLE" | "FILL" | "PAINT" | "TEXT" | "PING"
            | "CLRPROPS" | "SHOWALLPROPS" | "HIDEPROPS" | "SHOWPROPS" | "SETPROPSLOCAL"
            | "ADDPROP" | "PURGE" | "ROOMDESC" | "OFFLINE" | "ONLINE" | "NBRUSERS"
            | "GETWHOTALKING" | "MSGTO" | "FLUSH" | "SETSPOTSTATEALL" | "AWAY" | "TOGGLECTRL"
            | "SETDESC" | "BAN" | "KICK" => self.unimplemented(name, pushes, push),

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

    fn is_wizard(&self) -> bool {
        self.view.is_wizard
    }

    fn is_god(&self) -> bool {
        self.view.is_god
    }

    fn client_type(&self) -> String {
        "OPENPALACE".to_owned()
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
        self.view
            .spot(spot as i32)
            .and_then(|s| s.state_pics.get(state.max(0) as usize))
            .map_or((0, 0), |(_, dx, dy)| (i64::from(*dx), i64::from(*dy)))
    }

    fn get_pic_dimensions(&self, _spot: i64, _state: i64) -> (i64, i64) {
        (0, 0)
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
        self.effects.push(Effect::Unsupported {
            command: format!("SETSPOTNAMELOCAL({spot},{name})"),
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

    fn get_prop_id_by_name(&self, _name: &str) -> i64 {
        0
    }

    fn has_prop_by_id(&self, prop: i64) -> bool {
        self.view.self_props.contains(&prop)
    }

    fn has_prop_by_name(&self, _name: &str) -> bool {
        false
    }

    fn don_prop_by_id(&mut self, prop: i64) -> Result<()> {
        self.effects.push(Effect::DonProp { prop });
        Ok(())
    }

    fn don_prop_by_name(&mut self, name: &str) -> Result<()> {
        self.effects.push(Effect::Unsupported {
            command: format!("DONPROP({name:?})"),
        });
        Ok(())
    }

    fn set_props(&mut self, props: &[i64]) -> Result<()> {
        self.effects.push(Effect::SetProps {
            props: props.to_vec(),
        });
        Ok(())
    }

    fn doff_prop(&mut self) -> Result<()> {
        self.effects.push(Effect::DoffProp);
        Ok(())
    }

    fn doff_prop_by_id(&mut self, prop: i64) -> Result<()> {
        self.effects.push(Effect::RemoveProp { prop });
        Ok(())
    }

    fn doff_prop_by_name(&mut self, name: &str) -> Result<()> {
        self.effects.push(Effect::Unsupported {
            command: format!("REMOVEPROP({name:?})"),
        });
        Ok(())
    }

    fn naked(&mut self) -> Result<()> {
        self.effects.push(Effect::Naked);
        Ok(())
    }

    fn load_props(&mut self, _props: &[i64]) -> Result<()> {
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

    fn show_loose_props(&mut self) -> Result<()> {
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

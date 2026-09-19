//! The virtual machine.
//!
//! ## Execution model
//!
//! The reference VM has no jumps. A chunk is a linear list of operations; when
//! the interpreter meets an atomlist it pushes it as *data*, and `IF`, `IFELSE`,
//! `WHILE`, `FOREACH` and `EXEC` schedule it for execution. This crate mirrors
//! that with an explicit frame stack instead of native recursion, so a script
//! cannot overflow the Rust stack no matter how deeply it nests.
//!
//! Three frame kinds exist:
//!
//! * `Chunk` — an instruction pointer over an atomlist.
//! * `While` — alternates between evaluating the condition chunk and the body
//!   chunk; the state lives in the frame, so re-entrant `WHILE`s cannot clobber
//!   each other (the reference stores it on the command object and *can*).
//! * `ForEach` — walks an array, pushing one element per iteration.
//!
//! ## Budgets
//!
//! Every loop iteration, every frame push and every allocation is bounded; see
//! [`Limits`]. A script that would loop forever is stopped by the `WHILE`
//! iteration cap, the frame-depth cap, or the instruction budget, and returns an
//! [`IptError`] rather than hanging.
//!
//! ## Arithmetic
//!
//! Integers are 32-bit and wrap, matching the reference clients' `int`. The
//! only float in the language is the intermediate inside `SINE`/`COSINE`/
//! `TANGENT`, and its conversion back to an integer follows the ECMAScript
//! `ToInt32` rule (truncate toward zero, wrap modulo 2³², `NaN`/`±Infinity` → 0).
//! That is what makes division by zero yield `0` rather than an error.

use std::collections::HashMap;
use std::rc::Rc;

use crate::budget::Limits;
use crate::error::{IptError, Result};
use crate::host::Host;
use crate::lexer::parse_body;
use crate::registry::{Builtin, CommandSet};
use crate::script::Script;
use crate::stack::Stack;
use crate::value::{ArrayRef, Chunk, Op, Value};

/// What one activation of one handler did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    /// Instructions retired by this activation.
    pub steps: u64,
}

impl Execution {
    /// Whether the activation retired no instruction at all.
    pub fn is_empty(&self) -> bool {
        self.steps == 0
    }
}

/// What one activation did, including variables read back afterwards.
///
/// See [`Engine::run_handler_capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// Instructions retired by this activation.
    pub steps: u64,
    /// One entry per name requested, in the order asked. `None` when the
    /// activation never wrote the name.
    pub captured: Vec<Option<Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Cond,
    Check,
}

#[derive(Debug, Clone)]
enum Frame {
    Chunk {
        chunk: Chunk,
        ip: usize,
    },
    While {
        body: Chunk,
        cond: Chunk,
        phase: Phase,
        iterations: u64,
    },
    ForEach {
        body: Chunk,
        array: ArrayRef,
        index: usize,
    },
}

#[derive(Debug, Clone, Default)]
struct LocalVar {
    value: Option<Value>,
    global: bool,
}

type VarStore = HashMap<Rc<str>, Value>;

/// The interpreter for one activation of one handler.
///
/// Borrows the host, the shared global store and the command set from an
/// [`Engine`]. A fresh `Vm` per activation is what makes globals the only state
/// that survives across events, exactly as in the reference clients.
pub struct Vm<'a, H: Host + ?Sized> {
    host: &'a mut H,
    globals: &'a mut VarStore,
    commands: &'a CommandSet,
    limits: Limits,
    stack: Stack,
    frames: Vec<Frame>,
    locals: HashMap<Rc<str>, LocalVar>,
    return_requested: bool,
    break_requested: bool,
    exit_requested: bool,
    grep_captures: Option<Vec<String>>,
    spot: i64,
    steps: u64,
}

impl<'a, H: Host + ?Sized> Vm<'a, H> {
    fn new(
        host: &'a mut H,
        globals: &'a mut VarStore,
        commands: &'a CommandSet,
        limits: Limits,
    ) -> Self {
        let mut locals = HashMap::new();
        for (name, value) in host.initial_variables() {
            locals.insert(
                Rc::from(name.to_ascii_uppercase().as_str()),
                LocalVar {
                    value: Some(value),
                    global: false,
                },
            );
        }
        Self {
            host,
            globals,
            commands,
            limits,
            stack: Stack::with_limit(limits.stack_depth),
            frames: Vec::new(),
            locals,
            return_requested: false,
            break_requested: false,
            exit_requested: false,
            grep_captures: None,
            spot: 0,
            steps: 0,
        }
    }

    /// Set the hotspot this activation belongs to (used by `ALARMEXEC`).
    pub fn set_spot(&mut self, spot: i64) {
        self.spot = spot;
    }

    /// The current data stack.
    pub fn stack(&self) -> &Stack {
        &self.stack
    }

    /// Consume the VM and return the stack, bottom-first.
    pub fn into_stack(self) -> Vec<Value> {
        self.stack.into_items()
    }

    /// The current value of a variable in this activation's local scope.
    ///
    /// Names are matched case-insensitively. `None` means the activation never
    /// touched the name (or read it while it was still unset). Used to read back
    /// a variable a host seeded through [`Host::initial_variables`] and the
    /// script then rewrote — `CHATSTR` is the canonical case.
    ///
    /// [`Host::initial_variables`]: crate::host::Host::initial_variables
    #[must_use]
    pub fn local_value(&self, name: &str) -> Option<Value> {
        let upper = name.to_ascii_uppercase();
        self.locals
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(&upper))
            .and_then(|(_, slot)| slot.value.clone())
    }

    /// Replace every variable reference on the stack with its current value.
    ///
    /// A script that ends with a bare variable name leaves a *reference* on the
    /// stack, exactly as the reference VM does. Most callers do not care about
    /// that distinction, so this is the convenience for observing a result.
    pub fn resolve_stack(&mut self) -> Result<()> {
        let top_first = self.stack.drain_top_first();
        let mut resolved = Vec::with_capacity(top_first.len());
        for value in top_first {
            resolved.push(self.deref(&value)?);
        }
        for value in resolved.into_iter().rev() {
            self.stack.push(value)?;
        }
        Ok(())
    }

    /// Run a chunk to completion.
    pub fn run_chunk(&mut self, chunk: &Chunk) -> Result<Execution> {
        self.frames.clear();
        self.push_frame(Frame::Chunk {
            chunk: chunk.clone(),
            ip: 0,
        })?;
        self.run()?;
        Ok(Execution { steps: self.steps })
    }

    fn run(&mut self) -> Result<()> {
        while !self.frames.is_empty() {
            if self.steps >= self.limits.steps {
                return Err(IptError::StepBudgetExhausted {
                    limit: self.limits.steps,
                });
            }
            self.steps += 1;
            self.step()?;
        }
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame) -> Result<()> {
        if self.frames.len() >= self.limits.nesting {
            return Err(IptError::NestingTooDeep {
                limit: self.limits.nesting,
            });
        }
        self.frames.push(frame);
        Ok(())
    }

    fn step(&mut self) -> Result<()> {
        let top = self.frames.len() - 1;
        match self.frames.get(top) {
            Some(Frame::Chunk { .. }) => self.step_chunk(top),
            Some(Frame::While { .. }) => self.step_while(top),
            Some(Frame::ForEach { .. }) => self.step_foreach(top),
            None => Ok(()),
        }
    }

    fn step_chunk(&mut self, top: usize) -> Result<()> {
        if self.return_requested {
            self.return_requested = false;
            self.frames.truncate(top);
            return Ok(());
        }
        if self.exit_requested || self.break_requested {
            self.frames.truncate(top);
            return Ok(());
        }
        let next = match self.frames.get_mut(top) {
            Some(Frame::Chunk { chunk, ip }) => match chunk.ops().get(*ip) {
                Some(op) => {
                    let op = op.clone();
                    *ip += 1;
                    Some(op)
                }
                None => None,
            },
            _ => None,
        };
        match next {
            Some(op) => self.exec_op(op),
            None => {
                self.frames.truncate(top);
                Ok(())
            }
        }
    }

    fn step_while(&mut self, top: usize) -> Result<()> {
        let (phase, iterations, cond, body) = match self.frames.get(top) {
            Some(Frame::While {
                phase,
                iterations,
                cond,
                body,
            }) => (*phase, *iterations, cond.clone(), body.clone()),
            _ => return Ok(()),
        };
        if self.return_requested || self.exit_requested {
            self.frames.truncate(top);
            return Ok(());
        }
        if self.break_requested {
            self.break_requested = false;
            self.frames.truncate(top);
            return Ok(());
        }
        match phase {
            Phase::Cond => {
                if let Some(Frame::While { phase, .. }) = self.frames.get_mut(top) {
                    *phase = Phase::Check;
                }
                self.push_frame(Frame::Chunk { chunk: cond, ip: 0 })
            }
            Phase::Check => {
                let value = self.stack.pop()?;
                let resolved = self.deref(&value)?;
                if !resolved.is_truthy() || self.break_requested {
                    self.break_requested = false;
                    self.frames.truncate(top);
                    return Ok(());
                }
                let next = iterations + 1;
                if next > self.limits.while_iterations {
                    return Err(IptError::WhileLimitExceeded {
                        limit: self.limits.while_iterations,
                    });
                }
                if let Some(Frame::While {
                    phase, iterations, ..
                }) = self.frames.get_mut(top)
                {
                    *phase = Phase::Cond;
                    *iterations = next;
                }
                self.push_frame(Frame::Chunk { chunk: body, ip: 0 })
            }
        }
    }

    fn step_foreach(&mut self, top: usize) -> Result<()> {
        let (index, array, body) = match self.frames.get(top) {
            Some(Frame::ForEach { index, array, body }) => (*index, array.clone(), body.clone()),
            _ => return Ok(()),
        };
        if self.return_requested || self.exit_requested || self.break_requested {
            self.break_requested = false;
            self.frames.truncate(top);
            return Ok(());
        }
        let len = array.try_borrow().map(|v| v.len()).unwrap_or(0);
        if index >= len {
            self.frames.truncate(top);
            return Ok(());
        }
        let element = match array.try_borrow() {
            Ok(items) => items.get(index).cloned().unwrap_or(Value::Int(0)),
            Err(_) => Value::Int(0),
        };
        if let Some(Frame::ForEach { index, .. }) = self.frames.get_mut(top) {
            *index += 1;
        }
        self.stack.push(element)?;
        self.push_frame(Frame::Chunk { chunk: body, ip: 0 })
    }

    fn exec_op(&mut self, op: Op) -> Result<()> {
        match op {
            Op::Int(n) => self.stack.push(Value::Int(n)),
            Op::Str(s) => self.stack.push(Value::Str(s)),
            Op::Chunk(c) => self.stack.push(Value::Chunk(c)),
            Op::Mark => self.stack.push(Value::Mark),
            Op::ArrayClose => self.array_close(),
            Op::Var(name) => {
                self.var_get(&name)?;
                self.stack.push(Value::Var(name))
            }
            Op::Builtin(builtin) => self
                .exec_builtin(builtin)
                .map_err(|e| e.in_command(&format!("{builtin:?}"))),
            Op::Host(name) => {
                let name = name.clone();
                let pops = self.host.command_pops(&name);
                let mut args = Vec::with_capacity(pops);
                for _ in 0..pops {
                    let value = self.pop_deref().map_err(|e| e.in_command(&name))?;
                    args.push(value);
                }
                args.reverse();
                let results = self.host.command(&name, &args);
                let results = results.map_err(|e| e.in_command(&name))?;
                for value in results {
                    self.stack.push(value)?;
                }
                Ok(())
            }
        }
    }

    /// `]` — collect everything above the nearest mark into an array.
    ///
    /// Faithful to the reference's `ArrayParseToken`, which pops until it finds
    /// the mark *or the stack runs out*, so a `[` whose mark was consumed (by
    /// `POP`, say) turns the whole remaining stack into an array rather than
    /// faulting. A `]` with no `[` at all never reaches here — the lexer rejects
    /// it, as the reference tokenizer does.
    fn array_close(&mut self) -> Result<()> {
        let mut items = Vec::new();
        while let Some(value) = self.stack.pop_opt() {
            if matches!(value, Value::Mark) {
                break;
            }
            if items.len() >= self.limits.array_elements {
                return Err(IptError::ArrayTooLarge {
                    requested: items.len() + 1,
                    limit: self.limits.array_elements,
                });
            }
            items.push(value);
        }
        items.reverse();
        self.stack.push(Value::array(items))
    }

    fn exec_builtin(&mut self, builtin: Builtin) -> Result<()> {
        match builtin {
            Builtin::Dup => {
                let v = self.stack.peek(0)?.clone();
                self.stack.push(v)
            }
            Builtin::Over => {
                let v = self.stack.peek(1)?.clone();
                self.stack.push(v)
            }
            Builtin::Pick => {
                let n = self.pop_int()?;
                if n < 0 {
                    return Err(IptError::IndexOutOfRange {
                        index: n,
                        len: self.stack.depth(),
                    });
                }
                let v = self.stack.peek(n as usize)?.clone();
                self.stack.push(v)
            }
            Builtin::Pop => {
                self.stack.pop()?;
                Ok(())
            }
            Builtin::Swap => {
                let a = self.stack.pop()?;
                let b = self.stack.pop()?;
                self.stack.push(a)?;
                self.stack.push(b)
            }
            Builtin::StackDepth => {
                let n = self.stack.depth() as i64;
                self.stack.push(Value::Int(n))
            }
            Builtin::TopType => {
                if self.stack.is_empty() {
                    return self.stack.push(Value::Int(0));
                }
                let code = self.stack.peek(0)?.type_code();
                self.stack.push(Value::Int(i64::from(code)))
            }
            Builtin::VarType => {
                if self.stack.is_empty() {
                    return self.stack.push(Value::Int(0));
                }
                let top = self.stack.peek(0)?.clone();
                let resolved = self.deref(&top)?;
                let code = resolved.type_code();
                self.stack.push(Value::Int(i64::from(code)))
            }
            Builtin::Add => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                match (a, b) {
                    (Value::Int(x), Value::Int(y)) => {
                        self.stack.push(Value::Int(x.wrapping_add(y)))
                    }
                    (Value::Str(x), Value::Str(y)) => {
                        let mut s = String::with_capacity(x.len() + y.len());
                        s.push_str(&x);
                        s.push_str(&y);
                        let v = self.make_string(s)?;
                        self.stack.push(v)
                    }
                    (_, b) => Err(IptError::TypeMismatch {
                        expected: "two numbers or two strings",
                        found: b.type_name(),
                    }),
                }
            }
            Builtin::Sub => {
                let b = self.pop_arith_int()?;
                let a = self.pop_arith_int()?;
                self.stack.push(Value::Int(a.wrapping_sub(b)))
            }
            Builtin::Mul => {
                let b = self.pop_arith_int()?;
                let a = self.pop_arith_int()?;
                self.stack.push(Value::Int(a.wrapping_mul(b)))
            }
            Builtin::Div => {
                let b = self.pop_arith_int()?;
                let a = self.pop_arith_int()?;
                let r = if b == 0 { 0 } else { a.wrapping_div(b) };
                self.stack.push(Value::Int(r))
            }
            Builtin::Mod => {
                let b = self.pop_arith_int()?;
                let a = self.pop_arith_int()?;
                let r = if b == 0 { 0 } else { a.wrapping_rem(b) };
                self.stack.push(Value::Int(r))
            }
            Builtin::Inc => {
                let name = self.pop_var_name()?;
                let current = self.var_get(&name)?;
                match current {
                    Value::Int(n) => self.var_set(&name, Value::Int(n.wrapping_add(1))),
                    other => Err(IptError::TypeMismatch {
                        expected: "a number",
                        found: other.type_name(),
                    }),
                }
            }
            Builtin::Dec => {
                let name = self.pop_var_name()?;
                let current = self.var_get(&name)?;
                match current {
                    Value::Int(n) => self.var_set(&name, Value::Int(n.wrapping_sub(1))),
                    other => Err(IptError::TypeMismatch {
                        expected: "a number",
                        found: other.type_name(),
                    }),
                }
            }
            Builtin::AddAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_deref()?;
                let current = self.var_get(&name)?;
                match (current, argument) {
                    (Value::Int(x), Value::Int(y)) => {
                        self.var_set(&name, Value::Int(x.wrapping_add(y)))
                    }
                    (Value::Str(x), Value::Str(y)) => {
                        let mut s = String::with_capacity(x.len() + y.len());
                        s.push_str(&x);
                        s.push_str(&y);
                        let v = self.make_string(s)?;
                        self.var_set(&name, v)
                    }
                    (_, b) => Err(IptError::TypeMismatch {
                        expected: "a matching number or string",
                        found: b.type_name(),
                    }),
                }
            }
            Builtin::SubAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_arith_int()?;
                let current = self.pop_var_arith(&name)?;
                self.var_set(&name, Value::Int(current.wrapping_sub(argument)))
            }
            Builtin::MulAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_arith_int()?;
                let current = self.pop_var_arith(&name)?;
                self.var_set(&name, Value::Int(current.wrapping_mul(argument)))
            }
            Builtin::DivAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_arith_int()?;
                let current = self.pop_var_arith(&name)?;
                let r = if argument == 0 {
                    0
                } else {
                    current.wrapping_div(argument)
                };
                self.var_set(&name, Value::Int(r))
            }
            Builtin::ModAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_arith_int()?;
                let current = self.pop_var_arith(&name)?;
                let r = if argument == 0 {
                    0
                } else {
                    current.wrapping_rem(argument)
                };
                self.var_set(&name, Value::Int(r))
            }
            Builtin::Random => {
                let n = self.pop_int()?;
                let value = if n <= 0 {
                    0
                } else {
                    let r = self.host.random(n);
                    r.clamp(0, n - 1)
                };
                self.stack.push(Value::Int(value))
            }
            Builtin::Sine => self.trig(Trig::Sine),
            Builtin::Cosine => self.trig(Trig::Cosine),
            Builtin::Tangent => self.trig(Trig::Tangent),
            Builtin::SquareRoot => {
                // PalaceChat `iptService.js:478` is `Math.floor(Math.sqrt(n))`;
                // a negative `n` is NaN there, which `to_int64` maps to 0.
                let n = self.pop_int()?;
                self.stack
                    .push(Value::Int(to_int64((n as f64).sqrt().floor())))
            }
            Builtin::Atoi => {
                let s = self.pop_str()?;
                self.stack.push(Value::Int(parse_int_js(&s)))
            }
            Builtin::Itoa => {
                let n = self.pop_int()?;
                self.stack.push(Value::str(n.to_string()))
            }
            Builtin::DateTime => {
                let v = self.host.datetime();
                self.stack.push(Value::Int(v))
            }
            Builtin::Ticks => {
                let v = self.host.ticks();
                self.stack.push(Value::Int(v))
            }
            Builtin::IptVersion => self.stack.push(Value::Int(1)),
            Builtin::Concat => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                // `ConcatOperator` stringifies every operand (`toStr`): two
                // numbers become their decimal text, `5 6 &` -> "56".
                let a = concat_operand(&a)?;
                let b = concat_operand(&b)?;
                let mut s = String::with_capacity(a.len() + b.len());
                s.push_str(&a);
                s.push_str(&b);
                let v = self.make_string(s)?;
                self.stack.push(v)
            }
            Builtin::ConcatAssign => {
                let name = self.pop_var_name()?;
                let argument = self.pop_str()?;
                let current = self.var_get(&name)?;
                match current {
                    Value::Str(a) => {
                        let mut s = String::with_capacity(a.len() + argument.len());
                        s.push_str(&a);
                        s.push_str(&argument);
                        let v = self.make_string(s)?;
                        self.var_set(&name, v)
                    }
                    other => Err(IptError::TypeMismatch {
                        expected: "a string",
                        found: other.type_name(),
                    }),
                }
            }
            Builtin::Substr => {
                let fragment = self.pop_str()?;
                let whole = self.pop_str()?;
                let found = whole
                    .to_lowercase()
                    .contains(fragment.to_lowercase().as_str());
                self.stack.push(Value::Int(i64::from(found)))
            }
            Builtin::Substring => {
                let length = self.pop_int()?;
                let offset = self.pop_int()?;
                let s = self.pop_str()?;
                if offset < 0 {
                    return Err(IptError::BadArgument("offset cannot be negative"));
                }
                let text: String = if length <= 0 {
                    String::new()
                } else {
                    s.chars()
                        .skip(offset as usize)
                        .take(length as usize)
                        .collect()
                };
                let v = self.make_string(text)?;
                self.stack.push(v)
            }
            Builtin::StrIndex => {
                let needle = self.pop_str()?;
                let haystack = self.pop_str()?;
                let index = match haystack.find(needle.as_ref()) {
                    Some(byte) => haystack[..byte].encode_utf16().count() as i64,
                    None => -1,
                };
                self.stack.push(Value::Int(index))
            }
            Builtin::StrLen => {
                let s = self.pop_str()?;
                let n = s.encode_utf16().count() as i64;
                self.stack.push(Value::Int(n))
            }
            Builtin::Lowercase => {
                let s = self.pop_str()?;
                let v = self.make_string(s.to_lowercase())?;
                self.stack.push(v)
            }
            Builtin::Uppercase => {
                let s = self.pop_str()?;
                let v = self.make_string(s.to_uppercase())?;
                self.stack.push(v)
            }
            Builtin::StrToAtom => {
                let s = self.pop_str()?;
                let chunk = parse_body(&s, self.commands, &self.limits)?;
                self.stack.push(Value::Chunk(chunk))
            }
            Builtin::GrepStr => {
                let pattern = self.pop_str()?;
                let text = self.pop_str()?;
                let captures = self.host.grep_match(&pattern, &text)?;
                let matched = captures.is_some();
                self.grep_captures = captures;
                self.stack.push(Value::Int(i64::from(matched)))
            }
            Builtin::GrepSub => {
                let source = self.pop_str()?;
                let mut result = String::from(&*source);
                if let Some(captures) = self.grep_captures.clone() {
                    for (index, capture) in captures.iter().enumerate() {
                        result = result.replace(&format!("${index}"), capture);
                    }
                }
                let v = self.make_string(result)?;
                self.stack.push(v)
            }
            Builtin::And => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                let r = a.is_truthy() && b.is_truthy();
                self.stack.push(Value::Int(i64::from(r)))
            }
            Builtin::Or => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                let r = a.is_truthy() || b.is_truthy();
                self.stack.push(Value::Int(i64::from(r)))
            }
            Builtin::Not => {
                let a = self.pop_deref()?;
                let r = !a.is_truthy();
                self.stack.push(Value::Int(i64::from(r)))
            }
            Builtin::Eq => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                let r = values_equal(&a, &b);
                self.stack.push(Value::Int(i64::from(r)))
            }
            Builtin::Ne => {
                let b = self.pop_deref()?;
                let a = self.pop_deref()?;
                let r = !values_equal(&a, &b);
                self.stack.push(Value::Int(i64::from(r)))
            }
            Builtin::Lt => self.compare(Ordering::Lt),
            Builtin::Le => self.compare(Ordering::Le),
            Builtin::Gt => self.compare(Ordering::Gt),
            Builtin::Ge => self.compare(Ordering::Ge),
            Builtin::If => {
                let condition = self.pop_deref()?;
                let body = self.pop_chunk()?;
                if condition.is_truthy() {
                    self.push_frame(Frame::Chunk { chunk: body, ip: 0 })?;
                }
                Ok(())
            }
            Builtin::IfElse => {
                let condition = self.pop_deref()?;
                let false_body = self.pop_chunk()?;
                let true_body = self.pop_chunk()?;
                let chosen = if condition.is_truthy() {
                    true_body
                } else {
                    false_body
                };
                self.push_frame(Frame::Chunk {
                    chunk: chosen,
                    ip: 0,
                })
            }
            Builtin::While => {
                let cond = self.pop_chunk()?;
                let body = self.pop_chunk()?;
                self.push_frame(Frame::While {
                    body,
                    cond,
                    phase: Phase::Cond,
                    iterations: 0,
                })
            }
            Builtin::ForEach => {
                let array = self.pop_array()?;
                let body = self.pop_chunk()?;
                self.push_frame(Frame::ForEach {
                    body,
                    array,
                    index: 0,
                })
            }
            Builtin::Exec => {
                let value = self.pop_deref()?;
                match value {
                    Value::Int(0) => Ok(()),
                    Value::Chunk(chunk) => self.push_frame(Frame::Chunk { chunk, ip: 0 }),
                    other => Err(IptError::TypeMismatch {
                        expected: "atomlist",
                        found: other.type_name(),
                    }),
                }
            }
            Builtin::Return => {
                self.return_requested = true;
                Ok(())
            }
            Builtin::Break => {
                self.break_requested = true;
                Ok(())
            }
            Builtin::Exit => {
                self.exit_requested = true;
                Ok(())
            }
            Builtin::AlarmExec => {
                let ticks = self.pop_int()?;
                let body = self.pop_chunk()?;
                let spot = self.spot;
                self.host.schedule_alarm(ticks, body, spot)
            }
            Builtin::Assign => {
                let name = self.pop_var_name()?;
                let value = self.pop_deref()?;
                self.var_set(&name, value)
            }
            Builtin::Global => {
                let name = self.pop_var_name()?;
                self.globalize(&name)
            }
            Builtin::Array => {
                let n = self.pop_int()?;
                if n < 0 {
                    return self.stack.push(Value::Int(0));
                }
                let count = n as usize;
                if count > self.limits.array_elements {
                    return Err(IptError::ArrayTooLarge {
                        requested: count,
                        limit: self.limits.array_elements,
                    });
                }
                self.stack.push(Value::array(vec![Value::Int(0); count]))
            }
            Builtin::Get => {
                let index = self.pop_int()?;
                let array = self.pop_array()?;
                let cell = array
                    .try_borrow()
                    .map_err(|_| IptError::BadArgument("array is already borrowed"))?;
                if index < 0 || index as usize >= cell.len() {
                    return Err(IptError::IndexOutOfRange {
                        index,
                        len: cell.len(),
                    });
                }
                let value = cell[index as usize].clone();
                drop(cell);
                self.stack.push(value)
            }
            Builtin::Put => {
                let index = self.pop_int()?;
                let array = self.pop_array()?;
                let data = self.pop_deref()?;
                let mut cell = array
                    .try_borrow_mut()
                    .map_err(|_| IptError::BadArgument("array is already borrowed"))?;
                if index < 0 || index as usize >= cell.len() {
                    return Err(IptError::IndexOutOfRange {
                        index,
                        len: cell.len(),
                    });
                }
                cell[index as usize] = data;
                Ok(())
            }
            Builtin::Length => {
                let array = self.pop_array()?;
                let len = array.try_borrow().map(|v| v.len()).unwrap_or(0);
                self.stack.push(Value::Int(len as i64))
            }
            Builtin::Trace => {
                let s = self.pop_str()?;
                self.host.trace(&s);
                Ok(())
            }
            Builtin::TraceStack => {
                while let Some(value) = self.stack.pop_opt() {
                    let line = format!("{value:?}");
                    self.host.trace(&line);
                }
                Ok(())
            }
            Builtin::Breakpoint => Ok(()),
            Builtin::Beep => {
                self.host.request_beep();
                Ok(())
            }
            Builtin::Delay => {
                self.pop_int()?;
                Ok(())
            }
        }
    }

    fn trig(&mut self, which: Trig) -> Result<()> {
        let degrees = self.pop_int()?;
        let radians = degrees as f64 * std::f64::consts::PI / 180.0;
        let scaled = match which {
            Trig::Sine => radians.sin(),
            Trig::Cosine => radians.cos(),
            Trig::Tangent => radians.tan(),
        } * 1000.0;
        self.stack.push(Value::Int(to_int64(scaled.round())))
    }

    fn compare(&mut self, ordering: Ordering) -> Result<()> {
        let b = self.pop_deref()?;
        let a = self.pop_deref()?;
        let result = match (&a, &b) {
            (Value::Int(x), Value::Int(y)) => match ordering {
                Ordering::Lt => x < y,
                Ordering::Le => x <= y,
                Ordering::Gt => x > y,
                Ordering::Ge => x >= y,
            },
            (Value::Str(x), Value::Str(y)) => {
                let x = x.to_uppercase();
                let y = y.to_uppercase();
                match ordering {
                    Ordering::Lt => x < y,
                    Ordering::Le => x <= y,
                    Ordering::Gt => x > y,
                    Ordering::Ge => x >= y,
                }
            }
            // Unlike kinds are never ordered. PalaceChat answers 0 for
            // `1 "abc" <` rather than raising (reference line
            // `IPT|<|mixed 1 abc|0`).
            _ => false,
        };
        self.stack.push(Value::Int(i64::from(result)))
    }

    fn make_string(&self, s: String) -> Result<Value> {
        if s.len() > self.limits.string_bytes {
            return Err(IptError::StringTooLong {
                len: s.len(),
                limit: self.limits.string_bytes,
            });
        }
        Ok(Value::Str(Rc::from(s.as_str())))
    }

    fn deref(&mut self, value: &Value) -> Result<Value> {
        let mut current = value.clone();
        for _ in 0..64 {
            match current {
                Value::Var(name) => current = self.var_get(&name)?,
                other => return Ok(other),
            }
        }
        Ok(Value::Int(0))
    }

    /// Read a name from this activation's local store.
    ///
    /// A non-globalized name never reaches the global store: the reference only
    /// links a name to it through `GLOBAL` (`IptVariable.as` getter/setter,
    /// `GLOBALCommand.as`). Adding a fallback here would be a superset.
    fn var_get(&mut self, name: &Rc<str>) -> Result<Value> {
        self.ensure_local(name)?;
        if self.locals.get(name).map(|v| v.global).unwrap_or(false) {
            Ok(self.globals.get(name).cloned().unwrap_or(Value::Int(0)))
        } else {
            Ok(self
                .locals
                .get(name)
                .and_then(|v| v.value.clone())
                .unwrap_or(Value::Int(0)))
        }
    }

    fn var_set(&mut self, name: &Rc<str>, value: Value) -> Result<()> {
        self.ensure_local(name)?;
        let global = self.locals.get(name).map(|v| v.global).unwrap_or(false);
        if global {
            self.globals.insert(name.clone(), value);
        } else if let Some(slot) = self.locals.get_mut(name) {
            slot.value = Some(value);
        }
        Ok(())
    }

    fn globalize(&mut self, name: &Rc<str>) -> Result<()> {
        let local = self.locals.get(name).and_then(|v| v.value.clone());
        let slot = self.globals.entry(name.clone()).or_insert(Value::Int(0));
        if let Some(value) = local {
            *slot = value;
        }
        self.ensure_local(name)?;
        if let Some(local) = self.locals.get_mut(name) {
            local.global = true;
        }
        Ok(())
    }

    fn ensure_local(&mut self, name: &Rc<str>) -> Result<()> {
        if !self.locals.contains_key(name) {
            if self.locals.len() >= self.limits.variables {
                return Err(IptError::TooManyVariables {
                    limit: self.limits.variables,
                });
            }
            self.locals.insert(name.clone(), LocalVar::default());
        }
        Ok(())
    }

    fn pop_deref(&mut self) -> Result<Value> {
        let value = self.stack.pop()?;
        self.deref(&value)
    }

    fn pop_int(&mut self) -> Result<i64> {
        match self.pop_deref()? {
            Value::Int(n) => Ok(n),
            other => Err(IptError::TypeMismatch {
                expected: "number",
                found: other.type_name(),
            }),
        }
    }

    /// A numeric operand as the arithmetic operators coerce it: an integer, or a
    /// string parsed the way `ATOI` parses one (`toInteger` in the reference).
    fn pop_arith_int(&mut self) -> Result<i64> {
        match self.pop_deref()? {
            Value::Int(n) => Ok(n),
            Value::Str(s) => Ok(parse_int_js(&s)),
            other => Err(IptError::TypeMismatch {
                expected: "number or numeric string",
                found: other.type_name(),
            }),
        }
    }

    fn pop_str(&mut self) -> Result<Rc<str>> {
        match self.pop_deref()? {
            Value::Str(s) => Ok(s),
            other => Err(IptError::TypeMismatch {
                expected: "string",
                found: other.type_name(),
            }),
        }
    }

    fn pop_chunk(&mut self) -> Result<Chunk> {
        match self.pop_deref()? {
            Value::Chunk(c) => Ok(c),
            other => Err(IptError::TypeMismatch {
                expected: "atomlist",
                found: other.type_name(),
            }),
        }
    }

    fn pop_array(&mut self) -> Result<ArrayRef> {
        match self.pop_deref()? {
            Value::Array(a) => Ok(a),
            other => Err(IptError::TypeMismatch {
                expected: "array",
                found: other.type_name(),
            }),
        }
    }

    fn pop_var_name(&mut self) -> Result<Rc<str>> {
        match self.stack.pop()? {
            Value::Var(name) => Ok(name),
            other => Err(IptError::TypeMismatch {
                expected: "variable",
                found: other.type_name(),
            }),
        }
    }

    fn pop_var_arith(&mut self, name: &Rc<str>) -> Result<i64> {
        match self.var_get(name)? {
            Value::Int(n) => Ok(n),
            Value::Str(s) => Ok(parse_int_js(&s)),
            other => Err(IptError::TypeMismatch {
                expected: "a number stored in the variable",
                found: other.type_name(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Trig {
    Sine,
    Cosine,
    Tangent,
}

#[derive(Debug, Clone, Copy)]
enum Ordering {
    Lt,
    Le,
    Gt,
    Ge,
}

/// `==` equality, shared by `==` and its exact negation `!=`/`<>`.
///
/// Strings compare case-insensitively (the guide: "case-insensitive when
/// comparing strings"), and unlike kinds are never equal. `!=` must be the
/// precise negation of this for every pair, or `a != b` and `not (a == b)`
/// disagree.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x.to_uppercase() == y.to_uppercase(),
        _ => false,
    }
}

/// The text form `&` gives an operand: strings pass through, numbers become
/// decimal text (`iptService.js:2716`, JS `+`); any other kind is a type error.
fn concat_operand(value: &Value) -> Result<Rc<str>> {
    match value {
        Value::Str(s) => Ok(s.clone()),
        Value::Int(n) => Ok(Rc::from(n.to_string().as_str())),
        other => Err(IptError::TypeMismatch {
            expected: "string or number",
            found: other.type_name(),
        }),
    }
}

/// The reference clients' `int(...)` coercion for a float result: truncate
/// toward zero and map non-finite to 0.
///
/// PalaceChat stores integers in an 8-byte `IntegerToken` (`IntegerToken` is
/// constructed from and compared through `%i8`), so a negative result such as
/// `180 COSINE` stays negative instead of wrapping at 32 bits like a JS `|0`.
pub fn to_int64(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    if truncated >= -(2f64.powi(63)) && truncated < 2f64.powi(63) {
        truncated as i64
    } else {
        truncated.rem_euclid(18446744073709551616.0) as u64 as i64
    }
}

/// `parseInt(text)` as the reference clients implement it.
///
/// Skips leading whitespace, takes an optional sign, auto-detects `0x`/`0X` as
/// hexadecimal, consumes the longest valid prefix and ignores the rest. An
/// empty or invalid prefix is 0, matching `int(NaN)`.
pub fn parse_int_js(text: &str) -> i64 {
    let trimmed = text.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let (negative, rest) = if let Some(rest) = trimmed.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (false, rest)
    } else {
        (false, trimmed)
    };
    let hexadecimal = rest.len() >= 2 && (rest.starts_with("0x") || rest.starts_with("0X"));
    let radix = if hexadecimal { 16 } else { 10 };
    let digits = if hexadecimal { &rest[2..] } else { rest };
    let mut accumulator = 0i64;
    let mut any = false;
    for c in digits.chars() {
        match c.to_digit(radix) {
            Some(digit) => {
                accumulator = accumulator
                    .wrapping_mul(i64::from(radix))
                    .wrapping_add(i64::from(digit));
                any = true;
            }
            None => break,
        }
    }
    if !any {
        return 0;
    }
    if negative {
        accumulator.wrapping_neg()
    } else {
        accumulator
    }
}

/// Owns the host, the global variables and the command set.
///
/// One `Engine` corresponds to one Palace client session: globals live here and
/// therefore survive between handlers, while each handler activation gets a
/// fresh local scope.
pub struct Engine<H: Host> {
    /// The capability implementation scripts talk to.
    pub host: H,
    /// Resource budgets.
    pub limits: Limits,
    /// The command dictionary consulted while lexing.
    pub commands: CommandSet,
    globals: VarStore,
}

impl<H: Host> Engine<H> {
    /// An engine with the core command set and default budgets.
    pub fn new(host: H) -> Self {
        Self {
            host,
            limits: Limits::default(),
            commands: CommandSet::core(),
            globals: VarStore::new(),
        }
    }

    /// Replace the budgets.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Replace the command dictionary.
    pub fn with_commands(mut self, commands: CommandSet) -> Self {
        self.commands = commands;
        self
    }

    /// Run one handler, returning how much work it did.
    pub fn run_handler(&mut self, chunk: &Chunk) -> Result<Execution> {
        self.run_handler_spot(chunk, 0)
    }

    /// Run one handler with a hotspot identity, for `ALARMEXEC`.
    pub fn run_handler_spot(&mut self, chunk: &Chunk, spot: i64) -> Result<Execution> {
        let Engine {
            host,
            globals,
            commands,
            limits,
        } = self;
        let mut vm = Vm::new(host, globals, commands, *limits);
        vm.set_spot(spot);
        vm.run_chunk(chunk)
    }

    /// Run one handler and return the stack it left behind.
    pub fn run_handler_collect(&mut self, chunk: &Chunk) -> Result<Vec<Value>> {
        let Engine {
            host,
            globals,
            commands,
            limits,
        } = self;
        let mut vm = Vm::new(host, globals, commands, *limits);
        vm.run_chunk(chunk)?;
        Ok(vm.into_stack())
    }

    /// Parse and run a bare instruction sequence (no `ON` handler).
    pub fn run_source(&mut self, source: &str) -> Result<Execution> {
        let chunk = parse_body(source, &self.commands, &self.limits)?;
        self.run_handler(&chunk)
    }

    /// Parse and run a bare instruction sequence, returning the final stack.
    pub fn run_source_collect(&mut self, source: &str) -> Result<Vec<Value>> {
        let chunk = parse_body(source, &self.commands, &self.limits)?;
        self.run_handler_collect(&chunk)
    }

    /// Parse and run a bare instruction sequence, resolving the final stack.
    pub fn run_source_resolved(&mut self, source: &str) -> Result<Vec<Value>> {
        let chunk = parse_body(source, &self.commands, &self.limits)?;
        self.run_handler_resolved(&chunk)
    }

    /// Run one handler and read back the named local variables afterwards.
    ///
    /// This is how a host observes a variable a script rewrote during the
    /// activation — `ON OUTCHAT { "" CHATSTR = }` clears the outgoing chat text
    /// by assigning to `CHATSTR`, which the host seeded through
    /// [`Host::initial_variables`].
    ///
    /// [`Host::initial_variables`]: crate::host::Host::initial_variables
    pub fn run_handler_capture(
        &mut self,
        chunk: &Chunk,
        spot: i64,
        capture: &[&str],
    ) -> Result<Captured> {
        let Engine {
            host,
            globals,
            commands,
            limits,
        } = self;
        let mut vm = Vm::new(host, globals, commands, *limits);
        vm.set_spot(spot);
        let steps = {
            let execution = vm.run_chunk(chunk)?;
            execution.steps
        };
        let captured = capture.iter().map(|name| vm.local_value(name)).collect();
        Ok(Captured { steps, captured })
    }

    /// Run one handler and return its final stack with variables resolved.
    pub fn run_handler_resolved(&mut self, chunk: &Chunk) -> Result<Vec<Value>> {
        let Engine {
            host,
            globals,
            commands,
            limits,
        } = self;
        let mut vm = Vm::new(host, globals, commands, *limits);
        vm.run_chunk(chunk)?;
        vm.resolve_stack()?;
        Ok(vm.into_stack())
    }

    /// Run a named handler of a parsed script.
    pub fn run_script_handler(&mut self, script: &Script, name: &str) -> Result<Execution> {
        let chunk = script
            .handler(name)
            .ok_or_else(|| IptError::Host(format!("no handler named {name}")))?;
        self.run_handler(chunk)
    }

    /// Forget every global. Call between sessions.
    pub fn reset_globals(&mut self) {
        self.globals.clear();
    }

    /// How many globals are currently defined.
    pub fn globals_len(&self) -> usize {
        self.globals.len()
    }

    /// A snapshot of the shared global store, sorted by name.
    ///
    /// Diagnostic only: the values are clones, so the caller cannot change the
    /// session, and sorting keeps the output stable from run to run.
    #[must_use]
    pub fn globals_snapshot(&self) -> Vec<(String, Value)> {
        let mut out: Vec<(String, Value)> = self
            .globals
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        out.sort_by(|left, right| left.0.cmp(&right.0));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::NullHost;

    fn run(source: &str) -> Result<Vec<Value>> {
        Engine::new(NullHost).run_source_resolved(source)
    }

    fn int(source: &str) -> i64 {
        match run(source) {
            Ok(stack) => match stack.last() {
                Some(Value::Int(n)) => *n,
                other => panic!("{source:?} left {other:?}"),
            },
            Err(e) => panic!("{source:?} failed: {e}"),
        }
    }

    fn text(source: &str) -> String {
        match run(source) {
            Ok(stack) => match stack.last() {
                Some(Value::Str(s)) => s.to_string(),
                other => panic!("{source:?} left {other:?}"),
            },
            Err(e) => panic!("{source:?} failed: {e}"),
        }
    }

    #[test]
    fn arithmetic_is_postfix_and_wrapping() {
        assert_eq!(int("2 3 +"), 5);
        assert_eq!(int("3 2 - "), 1);
        assert_eq!(int("2 3 *"), 6);
        assert_eq!(int("3 2 /"), 1);
        assert_eq!(int("3 2 %"), 1);
        assert_eq!(int("-3 2 /"), -1, "truncates toward zero");
        assert_eq!(
            int("2147483647 1 +"),
            2_147_483_648,
            "the VM is 64-bit, matching PalaceChat's 8-byte IntegerToken"
        );
    }

    #[test]
    fn division_by_zero_is_zero_not_an_error() {
        assert_eq!(int("1 0 /"), 0);
        assert_eq!(int("0 0 /"), 0);
        assert_eq!(int("1 0 %"), 0);
    }

    #[test]
    fn plus_is_polymorphic_and_ampersand_stringifies_a_number() {
        assert_eq!(text("\"ab\" \"cd\" +"), "abcd");
        assert_eq!(text("\"ab\" \"cd\" &"), "abcd");
        assert_eq!(
            text("1 \"cd\" &"),
            "1cd",
            "& stringifies the number when the other operand is a string"
        );
        assert_eq!(text("1 2 &"), "12", "& stringifies both numbers");
        assert!(run("1 \"cd\" +").is_err(), "+ needs matching kinds");
    }

    #[test]
    fn comparisons_are_case_insensitive_and_inequality_negates_equality() {
        assert_eq!(int("\"ABC\" \"abc\" =="), 1, "== is case-insensitive");
        assert_eq!(int("\"ABC\" \"abc\" !="), 0, "!= negates ==");
        assert_eq!(int("\"abc\" \"ABC\" <>"), 0, "<> is != ");
        assert_eq!(int("2 3 <"), 1);
        assert_eq!(int("2 3 >="), 0);
        assert_eq!(int("1 1 == 2 2 == AND"), 1);
        assert_eq!(int("0 1 OR"), 1);
        assert_eq!(int("5 NOT"), 0);
        assert_eq!(int("0 !"), 1, "! is a synonym for NOT");
    }

    #[test]
    fn variables_autovivify_to_zero_and_assign_value_then_name() {
        assert_eq!(int("x"), 0);
        assert_eq!(int("5 x = x"), 5);
        assert_eq!(int("3 x = x 1 + x = x"), 4);
        assert_eq!(int("x ++ x"), 1);
        assert_eq!(int("5 x = x -- x"), 4);
        assert_eq!(int("3 x = 4 x += x"), 7);
        assert_eq!(int("3 x = 4 x -= x"), -1);
        assert_eq!(int("3 x = 4 x *= x"), 12);
        assert_eq!(int("12 x = 4 x /= x"), 3);
        assert_eq!(int("12 x = 5 x %= x"), 2);
        assert_eq!(text("\"a\" x = \"b\" x += x"), "ab");
    }

    #[test]
    fn assignment_dereferences_but_compound_assignment_needs_a_variable() {
        assert_eq!(int("1 y = 2 x = x y = y"), 2);
        assert!(run("1 2 +").is_ok());
        assert!(run("1 2 + 3 +").is_ok());
    }

    #[test]
    fn stack_ops() {
        assert_eq!(int("1 2 DUP + +"), 5);
        assert_eq!(int("1 2 SWAP - "), 1);
        assert_eq!(int("1 2 OVER - - "), 0, "OVER copies the second item");
        assert_eq!(int("10 20 30 2 PICK"), 10);
        assert_eq!(int("10 20 30 0 PICK"), 30);
        assert_eq!(int("1 2 3 STACKDEPTH"), 3);
        assert_eq!(int("1 POP 2"), 2);
    }

    #[test]
    fn toptype_and_vartype_codes() {
        assert_eq!(int("5 TOPTYPE"), 1);
        assert_eq!(int("5 x = x TOPTYPE"), 2);
        assert_eq!(int("5 x = x VARTYPE"), 1);
        assert_eq!(int("{1} TOPTYPE"), 3);
        assert_eq!(int("\"s\" TOPTYPE"), 4);
        assert_eq!(int("[ TOPTYPE"), 5);
        assert_eq!(int("[] TOPTYPE"), 6);
        assert_eq!(int("TOPTYPE"), 0, "empty stack peeks as 0");
    }

    #[test]
    fn conditionals_use_body_first_operand_order() {
        assert_eq!(int("1 { 7 } 1 IF"), 7);
        assert_eq!(int("1 { 7 } 0 IF"), 1);
        assert_eq!(int("{ 7 } { 9 } 1 IFELSE"), 7);
        assert_eq!(int("{ 7 } { 9 } 0 IFELSE"), 9);
    }

    #[test]
    fn while_is_body_first_then_condition() {
        assert_eq!(int("0 x = { x ++ } { x 3 < } WHILE x"), 3);
        assert_eq!(
            int("0 x = { x ++ } { x 10 > } WHILE x"),
            0,
            "the test runs before the body"
        );
    }

    #[test]
    fn break_leaves_a_while_loop() {
        assert_eq!(int("0 x = { x ++ { BREAK } x 3 == IF } { 1 } WHILE x"), 3);
    }

    #[test]
    fn a_trailing_minus_with_nothing_after_it_is_integer_zero() {
        assert_eq!(int("5 -"), 0, "the reference tokenizer reads '-' as NaN");
        assert_eq!(int("5 3 - "), 2, "with a following space it is subtraction");
    }

    #[test]
    fn an_endless_while_is_stopped_by_the_iteration_cap() {
        let limits = Limits {
            while_iterations: 10,
            ..Limits::default()
        };
        let err = Engine::new(NullHost)
            .with_limits(limits)
            .run_source("{ 1 } { 1 } WHILE");
        assert!(matches!(
            err,
            Err(IptError::WhileLimitExceeded { limit: 10 })
        ));
    }

    #[test]
    fn unbounded_recursion_is_stopped_by_the_nesting_cap() {
        let limits = Limits {
            nesting: 8,
            ..Limits::default()
        };
        let err = Engine::new(NullHost)
            .with_limits(limits)
            .run_source("{ myself GLOBAL myself EXEC } myself DEF myself GLOBAL myself EXEC");
        assert!(err.is_err());
        assert_eq!(
            err.unwrap_err().category(),
            "budget",
            "EXEC recursion must hit the nesting cap, not overflow the Rust stack"
        );
    }

    #[test]
    fn a_long_straight_line_script_is_stopped_by_the_step_budget() {
        let limits = Limits {
            steps: 50,
            ..Limits::default()
        };
        let source = "1 ".repeat(200);
        let err = Engine::new(NullHost)
            .with_limits(limits)
            .run_source(&source);
        assert!(
            matches!(err, Err(IptError::StepBudgetExhausted { limit: 50 })),
            "{err:?}"
        );
    }

    #[test]
    fn exec_runs_a_chunk_and_zero_is_a_silent_no_op() {
        assert_eq!(int("1 { 2 + } EXEC"), 3);
        assert_eq!(int("1 0 EXEC"), 1, "EXEC on integer zero does nothing");
        assert!(run("1 \"nope\" EXEC").is_err());
    }

    #[test]
    fn exec_of_an_unset_name_pushes_nothing_so_the_consumer_underflows() {
        // The reference's `EXEC` pops and dereferences its operand
        // (`EXECCommand.as:14`), and an unassigned variable dereferences to the
        // integer zero (`IptVariable.as:34-35`), on which `EXEC` returns without
        // pushing (`EXECCommand.as:17-18`). The consumer then pops an empty stack
        // (`IptTokenStack.as:31-34`), which `AssignOperator.as:11-12` and
        // `ConcatOperator.as:11-12` surface. Five real-host handlers do exactly
        // this: `prar` (144_hs1/hs2, defined by 144_hs0's ON ENTER), `hnd`/`vlus`
        // (9211_hs2, defined by 9211_hs0's ON ENTER), `iam` (5308_hs4, 889_hs0,
        // defined by 893_hs1:58) and `tavstand` (889_hs0, defined at offset 2970
        // after `rollit EXEC` runs it). This pins the behaviour so the residual
        // cannot be "fixed" by inventing a value the reference never pushes.
        assert_eq!(
            run("prar EXEC proparray ="),
            Err(IptError::CommandFailed {
                command: "Assign".to_owned(),
                source: Box::new(IptError::StackUnderflow {
                    needed: 1,
                    available: 0,
                }),
            })
        );
        assert_eq!(
            run("\"x\" iam EXEC &"),
            Err(IptError::CommandFailed {
                command: "Concat".to_owned(),
                source: Box::new(IptError::StackUnderflow {
                    needed: 1,
                    available: 0,
                }),
            })
        );
        assert_eq!(
            run("4 { 1 + } EXEC"),
            Ok(vec![Value::Int(5)]),
            "a defined chunk still supplies its value"
        );
    }

    #[test]
    fn a_global_chunk_defined_by_an_earlier_activation_satisfies_a_later_exec() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("{ 7 } prar DEF prar GLOBAL").unwrap();
        assert_eq!(
            engine
                .run_source_resolved("prar GLOBAL prar EXEC proparray = proparray")
                .unwrap(),
            vec![Value::Int(7)],
            "the defining handler and the consumer share one session's globals"
        );
    }

    #[test]
    fn def_binds_a_chunk_and_global_shares_it() {
        assert_eq!(int("{ 41 1 + } f DEF f EXEC"), 42);
    }

    #[test]
    fn foreach_pushes_each_element() {
        assert_eq!(int("0 s = { s += } [ 1 2 3 ] FOREACH s"), 6);
    }

    #[test]
    fn arrays() {
        assert_eq!(int("[ 10 20 30 ] 1 GET"), 20);
        assert_eq!(int("[ 10 20 30 ] LENGTH"), 3);
        assert_eq!(int("[ 1 2 3 ] a = 7 a 0 PUT a 0 GET"), 7);
        assert_eq!(int("3 ARRAY LENGTH"), 3);
        assert_eq!(int("3 ARRAY 0 GET"), 0);
        assert_eq!(int("-1 ARRAY"), 0, "negative ARRAY pushes 0, not an array");
        assert!(run("[ 1 2 3 ] 5 GET").is_err());
    }

    #[test]
    fn arrays_evaluate_their_contents() {
        assert_eq!(int("[ { 1 } { 2 } 0 IFELSE ] 0 GET"), 2);
        assert_eq!(int("[ 1 2 + ] 0 GET"), 3);
    }

    #[test]
    fn strings_and_their_commands() {
        assert_eq!(int("\"hello\" \"ell\" SUBSTR"), 1);
        assert_eq!(int("\"hello\" \"xyz\" SUBSTR"), 0);
        assert_eq!(text("\"hello\" 1 3 SUBSTRING"), "ell");
        assert_eq!(int("\"hello\" \"ll\" STRINDEX"), 2);
        assert_eq!(int("\"hello\" \"zz\" STRINDEX"), -1);
        assert_eq!(int("\"hello\" STRLEN"), 5);
        assert_eq!(text("\"HELLO\" LOWERCASE"), "hello");
        assert_eq!(text("\"hello\" UPPERCASE"), "HELLO");
        assert_eq!(int("\"42x\" ATOI"), 42);
        assert_eq!(int("\"nope\" ATOI"), 0);
        assert_eq!(text("42 ITOA"), "42");
        assert_eq!(text("-7 ITOA"), "-7");
    }

    #[test]
    fn strtoatom_compiles_and_runs() {
        assert_eq!(int("\"1 2 +\" STRTOATOM EXEC"), 3);
    }

    #[test]
    fn squareroot_is_the_integer_part() {
        assert_eq!(int("0 SQUAREROOT"), 0);
        assert_eq!(int("9 SQUAREROOT"), 3);
        assert_eq!(int("16 SQUAREROOT"), 4);
        assert_eq!(int("20 SQUAREROOT"), 4, "integer part of 4.47…");
        assert_eq!(int("15 SQUAREROOT"), 3);
        assert_eq!(int("2147483647 SQUAREROOT"), 46340, "no i32 overflow");
    }

    #[test]
    fn squareroot_of_a_negative_is_zero() {
        assert_eq!(int("-1 SQUAREROOT"), 0);
        assert_eq!(int("-16 SQUAREROOT"), 0);
    }

    #[test]
    fn squareroot_rejects_a_non_number() {
        assert!(run("\"x\" SQUAREROOT").is_err());
    }

    #[test]
    fn trig_is_fixed_point_x1000() {
        assert_eq!(int("0 SINE"), 0);
        assert_eq!(int("90 SINE"), 1000);
        assert_eq!(int("0 COSINE"), 1000);
        assert_eq!(int("45 TANGENT"), 1000);
    }

    #[test]
    fn return_exits_the_current_atomlist() {
        assert_eq!(int("0 x = { 1 x = RETURN 2 x = } EXEC x"), 1);
    }

    #[test]
    fn exit_stops_the_whole_script() {
        assert_eq!(int("1 EXIT 2"), 1);
    }

    #[test]
    fn to_int64_truncates_toward_zero() {
        assert_eq!(to_int64(0.0), 0);
        assert_eq!(to_int64(-0.5), 0);
        assert_eq!(to_int64(1.9), 1);
        assert_eq!(to_int64(-1000.0), -1000);
        assert_eq!(to_int64(f64::NAN), 0);
        assert_eq!(to_int64(f64::INFINITY), 0);
        assert_eq!(to_int64(4294967296.0), 4_294_967_296);
    }

    #[test]
    fn parse_int_js_matches_parse_int() {
        assert_eq!(parse_int_js("42"), 42);
        assert_eq!(parse_int_js("  -7"), -7);
        assert_eq!(parse_int_js("012"), 12);
        assert_eq!(parse_int_js("0x10"), 16);
        assert_eq!(parse_int_js("99 bottles"), 99);
        assert_eq!(parse_int_js("bottles"), 0);
        assert_eq!(parse_int_js(""), 0);
        assert_eq!(parse_int_js("+3"), 3);
    }

    #[test]
    fn globals_survive_between_activations_but_locals_do_not() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("7 g GLOBAL g =").unwrap();
        assert_eq!(
            engine.run_source_resolved("g GLOBAL g").unwrap(),
            vec![Value::Int(7)]
        );
        engine.run_source("5 local =").unwrap();
        assert_eq!(
            engine.run_source_resolved("local").unwrap(),
            vec![Value::Int(0)]
        );
    }

    #[test]
    fn global_promotes_an_existing_local_value() {
        assert_eq!(int("9 x = x GLOBAL x"), 9);
    }

    #[test]
    fn a_globalized_name_materialises_in_the_shared_store() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("boubou GLOBAL").unwrap();
        assert_eq!(
            engine.globals_snapshot(),
            vec![("BOUBOU".to_owned(), Value::Int(0))],
            "GLOBAL creates the shared slot the reference's store does"
        );
    }

    #[test]
    fn globalize_does_not_clobber_a_shared_value_with_an_unset_local() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("boubou GLOBAL 1 boubou =").unwrap();
        engine.run_source("boubou GLOBAL").unwrap();
        assert_eq!(
            engine.globals_snapshot(),
            vec![("BOUBOU".to_owned(), Value::Int(1))],
            "a later GLOBAL keeps the value when its local is uninitialised"
        );
    }

    #[test]
    fn a_global_assignment_lands_in_the_shared_store() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("X GLOBAL 2 X =").unwrap();
        assert_eq!(
            engine.globals_snapshot(),
            vec![("X".to_owned(), Value::Int(2))],
            "X GLOBAL must link X to the shared slot the assignment writes"
        );
        assert_eq!(
            engine.run_source_resolved("X GLOBAL X").unwrap(),
            vec![Value::Int(2)],
            "a later handler that globalizes X reads the assigned value"
        );
    }

    #[test]
    fn a_non_globalized_name_does_not_see_the_global_store() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("7 g GLOBAL g =").unwrap();
        assert_eq!(
            engine.run_source_resolved("g").unwrap(),
            vec![Value::Int(0)],
            "the reference only reaches the global store through GLOBAL"
        );
    }

    #[test]
    fn a_local_shadowing_a_global_does_not_write_through() {
        let mut engine = Engine::new(NullHost);
        engine.run_source("7 g GLOBAL g =").unwrap();
        engine.run_source("9 g =").unwrap();
        assert_eq!(
            engine.run_source_resolved("g GLOBAL g").unwrap(),
            vec![Value::Int(7)],
            "the un-globalized assignment wrote a discarded local"
        );
    }

    #[test]
    fn a_captured_local_reports_what_the_handler_wrote() {
        let mut engine = Engine::new(NullHost);
        let chunk =
            parse_body("\"!\" CHATSTR =", &engine.commands, &engine.limits).expect("parses");
        let captured = engine
            .run_handler_capture(&chunk, 0, &["CHATSTR"])
            .expect("runs");
        assert_eq!(
            captured.captured,
            vec![Some(Value::str("!"))],
            "the handler read and rewrote CHATSTR"
        );
        assert!(captured.steps > 0);
    }

    #[test]
    fn a_captured_local_is_none_when_the_handler_never_wrote_it() {
        let mut engine = Engine::new(NullHost);
        let chunk = parse_body("1 2 +", &engine.commands, &engine.limits).expect("parses");
        let captured = engine
            .run_handler_capture(&chunk, 0, &["CHATSTR", "OTHER"])
            .expect("runs");
        assert_eq!(captured.captured, vec![None, None]);
    }
}

# `iptscrae` — the IPTSCRAE language

IPTSCRAE is the stack language The Palace used in the 1990s for server scripts,
room and hotspot scripts, and per-user "cyborg" scripts. It looks like Forth:
operands push, commands pop and push, and there are no floats. This crate is a
fresh Rust implementation of the language core — lexer, parser, virtual machine
and a capability trait — with no network, no UI and no Palace commands, so it can
be tested against a real corpus on its own.

* **`iptscrae`** (this crate) is Palace-agnostic: it knows the language.
* **`iptscrae-palace`** adds the Palace command surface and the `PalaceHost`
  capability trait that Palace commands are written against.

```rust
use iptscrae::testing::eval_int;

assert_eq!(eval_int("2 3 +").unwrap(), 5);                       // postfix, integer only
assert_eq!(eval_int("4 x = x 1 + x = x").unwrap(), 5);           // value first: `value name =`
assert_eq!(eval_int("1 { 7 } 1 IF").unwrap(), 7);                // `{body} cond IF`
assert_eq!(eval_int("0 n = { n ++ } { n 3 < } WHILE n").unwrap(), 3); // `{body} {test} WHILE`
```

## Contents

1. [Grammar](#grammar)
2. [The stack and the data model](#the-stack-and-the-data-model)
3. [Semantics](#semantics)
4. [Commands](#commands)
5. [Resource budgets](#resource-budgets)
6. [The capability trait](#the-capability-trait)
7. [Corpus run](#corpus-run)
8. [Divergences and uncertainties](#divergences-and-uncertainties)
9. [Testing](#testing)

---

## Grammar

IPTSCRAE is whitespace-delimited and postfix, so lexing and parsing are one pass:
there is no operator precedence and no expression tree. The only structure is
atomlist nesting.

### Tokens

| Token | Spelling | Notes |
|---|---|---|
| whitespace | space, tab, CR, LF | separators only |
| comment | `#` or `;` to end of line | inside or outside atomlists |
| atomlist | `{ ... }` | nests; **no whitespace required around the braces** |
| string | `"..."` | may span newlines; `\` escapes the next character |
| array | `[ ... ]` | pushes a mark; the `]` collects the elements above it |
| integer | `-?[0-9]+` | 32-bit signed, wraps; no floats, no radix prefixes |
| symbol | `[A-Za-z0-9_]+` | upper-cased; a registered name is a command, anything else a variable |
| operator | `! != <> == = + ++ += - -- -= * *= / /= % %= & &= < <= > >=` | recognised by one character of lookahead |

### Rules worth naming

* **A `-` immediately before a digit is part of a negative literal.** `A-5` is
  the symbol `A` then the integer `-5`, not a subtraction. Inherited from the
  reference tokenizer; see [divergences](#divergences-and-uncertainties).
* **A `-` with nothing after it is the integer `0`** (`parseInt("-")` is `NaN`,
  which coerces to 0). So `5 3 - ` subtracts, but a source ending in `-` pushes a
  zero.
* **Braces need no whitespace.** `{0 0 MOVE`, `{GLOBAL}` and `{i 5 < }` all
  occur in real scripts, so `{` and `}` are delimiters even when glued to code.
* **Strings may span newlines.** A chat message in a real script contains them.
* **A NUL byte ends the script**, matching the reference client's
  `charCodeAt(0) != 0` guard.
* Anything else — `(`, `)`, `@`, `$`, `'`, `.`, `|`, … — is a lexing error.
  `$1`…`$9` only ever appear *inside* strings in the corpus, where they are
  protected as literal text.

### `ON NAME { ... }` handlers

A script file is a sequence of `ON NAME { body }` blocks. The splitter is part
of this crate; **dispatch is not** — deciding which handler runs is the host's
job.

Top-level scanning is deliberately lenient, exactly like the reference
`parseEventHandlers`: anything that is not the literal pattern
`ON <whitespace> <name> {` is skipped. Real harvested scripts contain trailing
junk, stray brackets and wholly commented-out handlers, and the reference client
tolerates all of it. Handler **bodies** are parsed strictly, so a genuine syntax
error inside a handler is still an error. A file with no `ON` block yields an
empty `Script`, not an error.

Handler names keep their source casing; lookup is case-insensitive.

## The stack and the data model

Values live on a LIFO stack. There are six kinds, distinguished by `TOPTYPE`:

| Kind | `TOPTYPE` | Truthiness | Rust type |
|---|---|---|---|
| integer | 1 | false **only** at 0 | `i32` |
| variable reference | 2 | the referenced value's truthiness | a name |
| atomlist | 3 | always true | `Chunk` (shared, immutable) |
| string | 4 | **always true — even `""`** | `Rc<str>` |
| array mark (`[`) | 5 | always true | marker |
| array | 6 | always true | `Rc<RefCell<Vec<Value>>>` |

Two surprises, both faithful to the reference:

* **Only integer zero is false.** An empty string is *true*, so `"" IF` runs the
  clause. Use a comparison, not truthiness, to test a string.
* **Symbols push variable *references*, not values.** A binary operator
  dereferences its operands — which is why `x 1 +` works — while `=`, `+=`,
  `GLOBAL` and `PICK` need the raw reference and therefore do not.

Sharing follows the reference: `DUP`, `OVER`, `PICK` and assignment copy the
*handle*. Atomlists and arrays are reference-shared, so `PUT` is visible through
every handle.

## Semantics

### Arithmetic

Integers are 32-bit and **wrap**. Division truncates toward zero. **Division and
modulo by zero yield `0`, not an error** — the reference coerces `Infinity`/`NaN`
through `int()`. The only float in the language is the intermediate inside
`SINE`/`COSINE`/`TANGENT`; converting it back follows ECMAScript `ToInt32`
(truncate toward zero, wrap modulo 2³², `NaN`/`±Infinity` → 0), after
`Math.round` as the reference does.

### Strings

`+` is polymorphic: two integers add, two strings concatenate, anything else is
a type error. `&` is **strict** string concatenation.

`==` compares strings **case-insensitively**; `!=`/`<>` compare them
**case-sensitively**. `<`, `<=`, `>`, `>=` compare strings upper-cased. This
asymmetry is the reference's, not an accident (see [divergences](#divergences-and-uncertainties)).
Type mismatches: `==` yields `0`, `!=` yields `1`, and the ordering operators
raise a type error.

`SUBSTR` is a case-insensitive **containment test** (pushes 0/1), not an
extractor. `SUBSTRING` extracts. `STRINDEX` finds an offset. `STRTOATOM` compiles
a string into an executable atomlist.

### Variables

A variable is auto-vivified to integer `0` on first mention. Assignment is
**value first, name second**: `5 x =`. Compound assignment (`+=`, `-=`, …) and
`++`/`--` take the variable *on top of the stack*, so the source order is
`value name +=` and `name ++`.

`GLOBAL` promotes a variable into the shared store so a later handler (a later
*activation*) sees it: if the local was assigned, its value is copied into the
global, then the local is rebound to the global. `SGLOBAL` is PalaceChat's
spot-scoped spelling of the same operation; the corpus uses it interchangeably
with `GLOBAL`, so this crate registers it as an alias.

### Control flow

| Form | Source order | Notes |
|---|---|---|
| `IF` | `{body} condition IF` | runs the body when the condition is non-zero |
| `IFELSE` | `{true} {false} condition IFELSE` | **true clause is pushed first** |
| `WHILE` | `{body} {condition} WHILE` | **body first, then the test**; the test runs before each iteration |
| `FOREACH` | `{body} [array] FOREACH` | pushes one element per iteration |
| `EXEC` | `{body} EXEC` | **integer `0` is a silent no-op** — an intentional idiom |
| `DEF` | `{body} name DEF` | same operator as `=`; binds the atomlist to the name |
| `RETURN` | | leaves the current atomlist |
| `BREAK` | | leaves the enclosing loop |
| `EXIT` | | stops the whole script |

`BREAK`/`RETURN`/`EXIT` outside a loop are not errors: `RETURN` ends the current
atomlist, `EXIT` ends the script, and `BREAK` ends the current atomlist.

### Host-seeded variables

Some names are values the client owns rather than commands the script calls. The
canonical one is `CHATSTR`, the chat text an `ON OUTCHAT`/`ON INCHAT` handler
reads and rewrites. The host declares them through
`Host::initial_variables`; the bundled harness seeds `CHATSTR` as an empty
string, without which many corpus scripts fail on `CHATSTR LOWERCASE …`.

## Commands

Every command is either a **builtin** (implemented here, host-independent) or a
**host command** (served by the capability trait). The registry is consulted
while lexing, so adding a host dictionary changes what lexes as a command.

### Builtins (72 names)

| Group | Names |
|---|---|
| stack | `DUP` `POP` `SWAP` `OVER` `PICK` `STACKDEPTH` `TOPTYPE` `VARTYPE` |
| arithmetic | `+` `-` `*` `/` `%` `++` `--` `+=` `-=` `*=` `/=` `%=` |
| host numerics | `RANDOM` `TICKS` `DATETIME` `SINE` `COSINE` `TANGENT` |
| strings | `&` `&=` `ATOI` `ITOA` `STRLEN` `STRINDEX` `SUBSTR` `SUBSTRING` `LOWERCASE` `UPPERCASE` `STRTOATOM` |
| regex | `GREPSTR` `GREPSUB` |
| logic | `AND` `OR` `NOT` `!` |
| comparison | `==` `!=` `<>` `<` `<=` `>` `>=` |
| control flow | `IF` `IFELSE` `WHILE` `FOREACH` `EXEC` `RETURN` `BREAK` `EXIT` `ALARMEXEC` |
| variables | `=` `DEF` `GLOBAL` |
| arrays | `ARRAY` `[` `]` `GET` `PUT` `LENGTH` |
| misc | `IPTVERSION` `_TRACE` `TRACESTACK` `_BREAKPOINT` `DELAY` `BEEP` |

Notable stack effects (operands listed top-first):

| Command | Pops | Pushes |
|---|---|---|
| `PICK` | `n` | a copy of the `n`-th item (0 = top) |
| `TOPTYPE` | nothing | the top item's type code |
| `VARTYPE` | nothing | the type code of the top item *dereferenced* |
| `ARRAY` | `n` | an array of `n` zeros; **negative `n` pushes integer `0`** |
| `GET` | `index`, `array` | the element |
| `PUT` | `index`, `array`, `data` | nothing |
| `SUBSTRING` | `len`, `offset`, `"str"` | the substring |
| `GREPSTR` | `pattern`, `text` | `0`/`1` |
| `GREPSUB` | `"source"` | the source with `$n` substituted |

`_TRACE`/`_BREAKPOINT` are the registered spellings; a bare `TRACE` lexes as a
variable, as in the reference.

### Host commands

The VM hands a host command its operands through the trait. For the Palace
surface, see `crates/iptscrae-palace/README.md`.

## Resource budgets

Scripts arrive from the server, so every loop, allocation and recursion is
bounded. Exceeding a bound is an `IptError`, never a hang.

```rust
use iptscrae::{Engine, Limits, StackDialect};

let limits = Limits::default().with_dialect(StackDialect::OpenPalace);
```

| Limit | Default | Windows | PalaceChat | OpenPalace |
|---|---|---|---|---|
| stack depth | 1024 | 256 | 1024 | 2048 |
| nesting / recursion | 256 | 64 | 256 | 256 |
| `WHILE` iterations | 7500 | 7500 | 7500 | 7500 |
| instructions per activation | 1 000 000 | — | — | — |
| instructions per time slice | 10 000 | — | — | — |
| array elements | 256 | 256 | 256 | 256 |
| string bytes | 65 536 | — | — | — |
| variables per activation | 4096 | — | — | — |

**The default stack depth is 1024 (PalaceChat).** The original Windows/Mac client
allowed 256 items, PalaceChat 1024 and OpenPalace 2048; the corpus was harvested
from a live server, so the smallest dialect that can run all of it is the right
default. Pick a dialect with `StackDialect`, or set any field directly.

The `WHILE` cap and the step budget are **deliberate additions**: neither the
guide nor OpenPalace bounds loop iterations, and OpenPalace is cooperatively
stepped with no total budget. Because this VM is meant to run untrusted remote
code inside a client, it fails closed instead.

## The capability trait

The VM has no filesystem, no network and no clock of its own; it can only call
`Host`. That trait is the security boundary:

```rust
pub trait Host {
    fn random(&mut self, bound: i64) -> i64;                 // RANDOM
    fn ticks(&self) -> i64;                                   // TICKS
    fn datetime(&self) -> i64;                                // DATETIME
    fn trace(&mut self, message: &str);                       // _TRACE / TRACESTACK
    fn grep_match(&mut self, pattern: &str, text: &str)       // GREPSTR
        -> Result<Option<Vec<String>>>;
    fn schedule_alarm(&mut self, ticks: i64, body: Chunk, spot: i64) -> Result<()>;
    fn initial_variables(&self) -> Vec<(String, Value)>;      // CHATSTR and friends
    fn command_pops(&self, name: &str) -> usize;              // host command arity
    fn command(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>>;
}
```

Every method has a deterministic default, so a minimal host is
`impl Host for MyHost {}` and a script still lexes, parses and executes. The
defaults fail closed where a wrong answer would be dangerous (regex, alarms) and
are inert elsewhere.

Host commands receive **dereferenced operands in push order** and return the
values to push; the data stack itself stays private to the VM. That is what
makes the boundary auditable: a command cannot inspect or corrupt the stack, and
a host that declares the wrong arity fails loudly instead of silently
desynchronising it.

## Corpus run

The harness (`iptscrae-palace`) runs all 2,400 harvested per-hotspot scripts.
Every Palace command is stubbed with the *documented* operand count and neutral
results, so the run measures the VM core — control flow, arithmetic, the stack —
on real code rather than Palace semantics.

```bash
cargo run -p iptscrae-palace --bin iptscrae -- corpus $CORPUS/scripts_clean/by_script
```

Current result (PalaceChat dialect, seed 0):

| Metric | Value |
|---|---|
| files | 2400 |
| parsed | **2396 (99.8%)** |
| handlers | 3805 |
| ran clean | **3791 (99.6%)** |
| files fully clean | 2382 (99.4%) |

**Parse failures (4).** All four are genuinely malformed source, not tokenizer
gaps: a stray `(` (`11054_hs1.txt`), two stray `)` (`13009_hs0.txt`,
`7665_hs0.txt`) and a stray `@` outside a string (`9211_hs29.txt`). The
reference tokenizer throws on exactly these characters too.

**Run failures (14).** Three causes, none a VM bug:

1. **Extended commands that this milestone does not implement.** Six failures
   (`7022_hs2`, `7028_hs0`, `7030_hs0`, `7031_hs0`, `7034_hs0`, `7035_hs0`, all
   `ON ROOMREADY`) use `HTTPGET`, a PalaceChat-5 extension with no documented
   signature. It lexes as a variable — exactly as it would in OpenPalace, which
   does not know it either — so the operands it should have consumed shift the
   stack. A later milestone adds it.
2. **Cross-script global state.** Five failures (`144_hs1`, `144_hs2`, `889_hs0`,
   `5308_hs4`, `14463_hs2`, `14463_hs3`) read globals such as `prar`, `iam` and
   `cname` that are set by a *different script* in the live server. The harness
   isolates globals per script file, so those reads see `0`. In a real session
   with all scripts loaded they resolve.
3. **A documented divergence.** One failure (`167_hs0`) does
   `"…id=" TOPPROP &`, concatenating `TOPPROP`'s integer with a string using `&`.
   The guide and OpenPalace both make `&` strict, so this fails in the reference
   implementation too; the live client apparently coerced. See the next section.

## Divergences and uncertainties

Where the guide, the reference implementations and the corpus disagree, the
resolution is recorded here. "Reference" means OpenPalace's ActionScript VM
(`AS3Iptscrae/`, the primary design reference); "guide" means *The Palace
Iptscrae Language Guide* (Communities.com, February 2000).

### Deliberate divergences (safety)

| # | Behaviour | Reference | This crate | Why |
|---|---|---|---|---|
| 1 | stack overflow | at 256 items items quietly "fall off" | error (`StackOverflow`) | untrusted remote code should not silently corrupt state |
| 2 | unbalanced `]` | drains the stack, still pushes an array | lex error (`UnmatchedClose`) | fail closed |
| 3 | loop iterations | unbounded | `WHILE` cap (7500) + step budget | guarantee termination |
| 4 | recursion | 256-list call-stack limit | frame-depth cap, checked before push | explicit, no native recursion |
| 5 | errors from `WHILE`/`FOREACH` frames | **silently swallowed** (a reference bug) | always propagated | a swallowed error is worse than a reported one |
| 6 | `EXIT` flag | never cleared; unwinds everything | same effect, via frame truncation | equivalent, clearer |

### Faithful-to-reference quirks (keep them)

| # | Behaviour | Note |
|---|---|---|
| 7 | `A-5` is `A`, `-5` | `-` before a digit is a literal sign |
| 8 | trailing `-` is integer `0` | `parseInt("-")` → `NaN` → `0` |
| 9 | `==` case-insensitive, `!=`/`<>` case-sensitive | reference asymmetry; the guide says all are case-insensitive — we follow the reference |
| 10 | only integer `0` is false; `""` is true | `StringToken` does not override `toBoolean` |
| 11 | `EXEC` on integer `0` is a no-op | intentional idiom, not a bug |
| 12 | `ARRAY n` with `n < 0` pushes integer `0` | not an empty array |
| 13 | `SUBSTR` is a containment test | `SUBSTRING` extracts |
| 14 | division/modulo by zero yield `0` | `int(Infinity)`/`int(NaN)` |
| 15 | `IFELSE` true clause first; `WHILE` body first | unusual operand order, per the guide |
| 16 | `PICK -1` is an error | the reference's `uint` coercion |
| 17 | `TOPTYPE` of a symbol is `2`; `VARTYPE` dereferences | |
| 18 | integers wrap at 32 bits | |
| 19 | `ATOI` auto-detects `0x` hex | AS3 `parseInt` without a radix |
| 20 | `SINE`/`COSINE`/`TANGENT` are `round(x*1000)` | fixed point |
| 21 | NUL ends the script | reference loop guard |

### Uncertainties and judgements

| # | Question | Resolution |
|---|---|---|
| 22 | Is `&` strict or does it coerce integers? | **Strict**, matching the guide and OpenPalace. Corpus counterexample: `167_hs0.txt` (`TOPPROP &`). Recorded, not silently "fixed". |
| 23 | Is there a 31-character symbol limit? | The guide says yes; OpenPalace enforces nothing. We do not enforce it either, and document `MAX_SYMBOL_LEN = 31`. No corpus symbol exceeds 18 characters. |
| 24 | What is a `\x` escape with fewer than two hex digits? | Decoded as a Windows-1252 byte with the digits present; zero digits is byte 0, matching `writeByte(parseInt("0x"))`. |
| 25 | Do handler names compare case-sensitively? | The reference preserves case and compares exactly; we preserve case and compare case-insensitively (a superset that cannot change a well-formed script). |
| 26 | Are `\f`/`\v` whitespace? | The reference's main tokenizer accepts only space/tab/CR/LF; its `ON` lookahead uses `/^\s/`, which also accepts `\f`/`\v`. We use the four-character set everywhere, so `ON\fENTER` is not recognised. No corpus occurrence. |
| 27 | `SGLOBAL` | Not in OpenPalace. The corpus uses `sym SGLOBAL` exactly as `sym GLOBAL` (54 `DUP GLOBAL` vs 12 `DUP SGLOBAL`, always immediately before a condition), and IF only balances if `SGLOBAL` consumes one operand. Registered as an alias of `GLOBAL`. |
| 28 | Extended PalaceChat commands | Not registered. This crate has no source for their stack effects; registering a guessed arity would corrupt the stack. As variables they behave as they do in OpenPalace. |
| 29 | Regex | `GREPSTR` is host-provided. The bundled engine supports `^ $ . […] [^…] * + ? () |` and `\xNN`, is step-bounded, and returns only the whole match, so `GREPSUB` substitutes `$0` but leaves `$1`…`$9` alone. A production host supplies a complete engine. |
| 30 | `STRLEN`/`STRINDEX`/`SUBSTRING` units | AS3 counts UTF-16 code units; we match (`encode_utf16().count()`), except `SUBSTRING` which slices by `char`. Identical for ASCII, which is all the corpus contains. |
| 31 | `ITOA`/`LOWERCASE`/`UPPERCASE` case mapping | Rust's Unicode mapping, not AS3's. Identical for ASCII. |
| 32 | `DATETIME`/`TICKS`/`RANDOM` | Host-provided and therefore deterministic in tests and the corpus run (fixed clock, seeded generator). The VM itself never reads a clock or an entropy source. |
| 33 | `DELAY`, `BEEP`, `_BREAKPOINT` | No-ops here, as in the reference. `ALARMEXEC`/`SETALARM` are handed to the host. |
| 34 | Where exactly does a command error get attributed? | `IptError::in_command` wraps a fault with the command that raised it, so diagnostics read `GET: array index 5 out of range`. Category is inherited from the underlying fault. |

## Testing

```bash
cargo test -p iptscrae          # unit + conformance + hostile-input suites
cargo test -p iptscrae-palace   # Palace surface (and the corpus test if available)
cargo clippy --workspace --all-targets
```

* `tests/conformance.rs` — one case per operator, command and control-flow form.
* `tests/hostile.rs` — deterministic random token soup and random bytes through
  the lexer, parser and VM (no panic), plus direct tests of every budget:
  endless `WHILE`, hidden recursion, oversized array, unbounded string, deep
  nesting, stack overflow.
* `iptscrae-palace/tests/corpus.rs` — runs the harvested corpus when
  `IPTSCRAE_CORPUS` points at it, asserting ≥99% parse and ≥99% clean.

Panic freedom is enforced structurally as well as by test: the crate is
`#![forbid(unsafe_code)]`, denies `unwrap`/`expect`/`panic` outside tests, uses
no indexing on parsed data, and returns `IptError` from every fallible path.

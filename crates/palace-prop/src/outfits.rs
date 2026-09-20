//! Outfits file: named sets of worn props, stored as one portable file.
//!
//! An **outfit** is a named list of prop references — the props to wear at once.
//! The concept comes from the reference client's `PropBag.savedAvatars`
//! (`saveCurrentAvatar`), which keeps a list of id arrays in a shared object.
//! PalaceChat keeps the same idea in a `macro` file beside its bag; this module
//! stores it as one readable file of its own instead.
//!
//! # The file
//!
//! **This is not the `.prp` binary format.** It is an app-owned, hand-editable,
//! portable document that is deliberately readable and diffable. Version 1 is
//! JSON with a version field and one array of outfits:
//!
//! ```json
//! {
//!   "version": 1,
//!   "outfits": [
//!     {
//!       "name": "Party Hat",
//!       "props": [
//!         [100, 123456789],
//!         [100, 42],
//!         [-7, 0]
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! A prop reference is `[id, crc]` — the [`PropKey`] identity pair, because an
//! id alone is not unique: several records may share an id and differ in crc.
//! Order is significant and preserved; it is the order the props are worn in.
//!
//! The file lives at [`crate::bag_folder::outfits_path`] (`Outfits.prp` in the
//! bag root). Outfits are never embedded in a `.prp` and are never bound to
//! keyboard keys: an outfit is applied by name, not by an F-key macro.
//!
//! # Loading, and what a bad file does
//!
//! [`OutfitStore::open`] never fails and never panics. A missing file is an
//! empty store with no error. An unreadable file (I/O failure, invalid UTF-8)
//! or one that does not match the schema yields an **empty list plus a clear
//! error** ([`OutfitStore::load_error`]) — and no code path here ever writes to
//! a file it could not parse.
//!
//! Within a well-formed document:
//!
//! * Duplicate outfit names resolve deterministically: the **first** occurrence
//!   wins and later ones are dropped (counted by
//!   [`OutfitStore::dropped_duplicates`]).
//! * An empty name, a version other than [`OUTFITS_VERSION`], or a prop
//!   reference that is not an `[id, crc]` integer pair makes the whole document
//!   invalid, so it loads as empty plus an error. Nothing is "repaired" on disk.
//! * A duplicate key inside a JSON object resolves like a duplicate outfit
//!   name: the first occurrence wins.
//! * Unknown keys are ignored, so a newer writer's extra fields do not make an
//!   older reader fail.
//!
//! Names are stored exactly as written in the file. Names passed *in* by a
//! caller are trimmed first, so `save_worn("  Hat ")` stores `Hat`.
//!
//! # Saving
//!
//! Every mutating operation ([`OutfitStore::save_worn`],
//! [`OutfitStore::rename`], [`OutfitStore::delete`],
//! [`OutfitStore::duplicate`]) rebuilds the document and replaces the file
//! through [`BagContext::atomic_write`], the bag folder's write-to-temp,
//! `fsync`, rename sequence. A refused or failed write leaves the file exactly
//! as it was, and the in-memory list is rolled back so memory and disk cannot
//! disagree.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::bag_folder::{outfits_path, BagContext, BagRole};
use crate::error::{PropError, Result};
use crate::prp::PropKey;

/// The outfits document version this build writes and the only one it accepts.
pub const OUTFITS_VERSION: u32 = 1;

/// Largest nesting depth the JSON reader accepts.
///
/// The schema needs three levels (`object` → `outfits` array → outfit object →
/// `props` array → pair); the limit exists so a broken or hostile file of
/// `[[[[…` cannot exhaust the stack.
const MAX_JSON_DEPTH: usize = 32;

/// One named set of worn props.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outfit {
    /// The outfit's name; unique among the outfits of one store.
    pub name: String,
    /// The props to wear, in wearing order.
    pub props: Vec<PropKey>,
}

/// The outfits file plus the list it parsed.
///
/// Built by [`OutfitStore::open`], which never fails: inspect
/// [`OutfitStore::load_error`] to tell "empty file" from "unreadable file".
#[derive(Debug, Clone)]
pub struct OutfitStore {
    /// `None` when no bag folder is configured; every write then fails and the
    /// attempted load records an error.
    path: Option<PathBuf>,
    /// The bag folder that guards reads and writes.
    context: BagContext,
    /// The parsed outfits, in file order.
    outfits: Vec<Outfit>,
    /// Why the last load produced an empty list, when it did.
    load_error: Option<PropError>,
    /// How many outfits a load dropped because their name was already taken.
    dropped_duplicates: usize,
}

impl OutfitStore {
    /// Open the outfits file of `context`'s bag folder and parse it.
    ///
    /// Never fails and never panics: a missing file is an empty store, an
    /// unreadable or corrupt one is an empty store plus a
    /// [`OutfitStore::load_error`]. The file is only read, never touched.
    #[must_use]
    pub fn open(context: BagContext) -> Self {
        let path = context.bag_root.as_deref().map(outfits_path);
        let mut store = Self {
            path,
            context,
            outfits: Vec::new(),
            load_error: None,
            dropped_duplicates: 0,
        };
        store.reload();
        store
    }

    /// [`OutfitStore::open`] over the environment-discovered bag folder.
    #[must_use]
    pub fn discover() -> Self {
        Self::open(BagContext::discover())
    }

    /// Re-read and re-parse the file, replacing the in-memory list.
    ///
    /// The same policy as [`OutfitStore::open`]: never fails, never panics,
    /// never writes. Useful when the file may have been edited on disk.
    pub fn reload(&mut self) {
        self.outfits.clear();
        self.load_error = None;
        self.dropped_duplicates = 0;

        let Some(path) = self.path.clone() else {
            self.load_error = Some(no_bag_folder());
            return;
        };
        if self.context.classify(&path) != BagRole::MyBag {
            self.load_error = Some(PropError::BagIo {
                detail: format!(
                    "{}: refusing to read; not a bag-owned outfits file",
                    path.display()
                ),
            });
            return;
        }

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            // A missing file is a normal first run, not an error.
            Err(err) if err.kind() == ErrorKind::NotFound => return,
            Err(err) => {
                self.load_error = Some(io_error(&path, err));
                return;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(err) => {
                self.load_error = Some(bag_error(&path, format!("file is not valid UTF-8: {err}")));
                return;
            }
        };
        match parse_outfits(text, &path) {
            Ok((outfits, dropped)) => {
                self.outfits = outfits;
                self.dropped_duplicates = dropped;
            }
            Err(err) => self.load_error = Some(err),
        }
    }

    /// The file this store reads and writes; `None` when no bag folder is
    /// configured.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Why the last load produced an empty list; `None` after a good load or a
    /// missing file.
    #[must_use]
    pub fn load_error(&self) -> Option<&PropError> {
        self.load_error.as_ref()
    }

    /// How many outfits the last load dropped because a previous outfit already
    /// used the same name (the first occurrence wins).
    #[must_use]
    pub fn dropped_duplicates(&self) -> usize {
        self.dropped_duplicates
    }

    /// Every outfit, in file order.
    #[must_use]
    pub fn outfits(&self) -> &[Outfit] {
        &self.outfits
    }

    /// Every outfit name, in file order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.outfits
            .iter()
            .map(|outfit| outfit.name.as_str())
            .collect()
    }

    /// The outfit with this exact name, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Outfit> {
        self.outfits
            .iter()
            .find(|outfit| outfit.name.as_str() == name)
    }

    /// Number of outfits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.outfits.len()
    }

    /// Whether there are no outfits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outfits.is_empty()
    }

    /// The exact document text [`OutfitStore::save`] writes, for inspection.
    #[must_use]
    pub fn to_json(&self) -> String {
        render(&self.outfits)
    }

    /// Record `worn` as the outfit called `name`, then save the file.
    ///
    /// The name is trimmed first; an empty name is refused. An existing outfit
    /// with the same name is replaced in place (the reference client's
    /// `saveCurrentAvatar` appends, but a stable list is friendlier to edit); a
    /// new name is appended.
    pub fn save_worn(&mut self, name: &str, worn: &[PropKey]) -> Result<()> {
        let name = clean_name(name)?;
        let worn = worn.to_vec();
        self.mutate(|outfits| {
            match outfits
                .iter_mut()
                .find(|outfit| outfit.name.as_str() == name)
            {
                Some(outfit) => outfit.props = worn,
                None => outfits.push(Outfit { name, props: worn }),
            }
            Ok(())
        })
    }

    /// Rename `from` to `to`, then save the file.
    ///
    /// Refused when `from` is unknown, when `to` belongs to a different outfit,
    /// or when `to` is empty. Renaming an outfit to its own name is a no-op.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let from = from.trim();
        let to = clean_name(to)?;
        self.mutate(|outfits| {
            let Some(index) = outfits
                .iter()
                .position(|outfit| outfit.name.as_str() == from)
            else {
                return Err(outfit_not_found(from));
            };
            if outfits
                .get(index)
                .map(|outfit| outfit.name.as_str() == to.as_str())
                .unwrap_or(false)
            {
                return Ok(());
            }
            if outfits.iter().any(|outfit| outfit.name.as_str() == to) {
                return Err(name_taken(&to));
            }
            match outfits.get_mut(index) {
                Some(outfit) => {
                    outfit.name = to;
                    Ok(())
                }
                None => Err(outfit_not_found(from)),
            }
        })
    }

    /// Delete the outfit called `name`, then save the file.
    ///
    /// Refused when the name is unknown.
    pub fn delete(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        self.mutate(|outfits| {
            let Some(index) = outfits
                .iter()
                .position(|outfit| outfit.name.as_str() == name)
            else {
                return Err(outfit_not_found(name));
            };
            outfits.remove(index);
            Ok(())
        })
    }

    /// Copy the outfit called `source` to a new outfit called `target`, then
    /// save the file.
    ///
    /// The copy is inserted directly after its source. Refused when `source` is
    /// unknown, when `target` is already taken, or when `target` is empty.
    pub fn duplicate(&mut self, source: &str, target: &str) -> Result<()> {
        let source = source.trim();
        let target = clean_name(target)?;
        self.mutate(|outfits| {
            let Some(index) = outfits
                .iter()
                .position(|outfit| outfit.name.as_str() == source)
            else {
                return Err(outfit_not_found(source));
            };
            if outfits.iter().any(|outfit| outfit.name.as_str() == target) {
                return Err(name_taken(&target));
            }
            let props = match outfits.get(index) {
                Some(outfit) => outfit.props.clone(),
                None => return Err(outfit_not_found(source)),
            };
            outfits.insert(
                index.saturating_add(1),
                Outfit {
                    name: target,
                    props,
                },
            );
            Ok(())
        })
    }

    /// Write the current list to the file atomically.
    ///
    /// The mutating operations call this themselves; it is public for callers
    /// that edited the list through [`OutfitStore::outfits`]'s builder methods
    /// (not possible through the shared slice) or simply want to force a write.
    pub fn save(&self) -> Result<()> {
        self.write(&self.outfits)
    }

    /// Apply `change` to a candidate copy, write it, and only then commit it.
    ///
    /// The candidate indirection is what makes a refused or failed write leave
    /// the in-memory list untouched: memory and disk move together or not at all.
    fn mutate<F>(&mut self, change: F) -> Result<()>
    where
        F: FnOnce(&mut Vec<Outfit>) -> Result<()>,
    {
        let mut candidate = self.outfits.clone();
        change(&mut candidate)?;
        self.write(&candidate)?;
        self.outfits = candidate;
        Ok(())
    }

    /// Serialise `outfits` and replace the file in one atomic step.
    fn write(&self, outfits: &[Outfit]) -> Result<()> {
        let path = self.path.as_ref().ok_or_else(no_bag_folder)?;
        self.context.atomic_write(path, render(outfits).as_bytes())
    }
}

/// Trim a caller-supplied outfit name and refuse it when nothing is left.
fn clean_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(PropError::BagIo {
            detail: "outfits: the name must not be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

/// The error for a rename/delete/duplicate over a name that is not stored.
fn outfit_not_found(name: &str) -> PropError {
    PropError::BagIo {
        detail: format!("outfits: no outfit named \"{name}\""),
    }
}

/// The error for introducing a name another outfit already uses.
fn name_taken(name: &str) -> PropError {
    PropError::BagIo {
        detail: format!("outfits: an outfit named \"{name}\" already exists"),
    }
}

/// The error for a store with no bag folder behind it.
fn no_bag_folder() -> PropError {
    PropError::BagIo {
        detail: "outfits: no bag folder configured (set PALACE_PROP_BAG_DIR or a home directory)"
            .to_string(),
    }
}

/// An outfits error naming the file it concerns.
fn bag_error(path: &Path, detail: impl fmt::Display) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {detail}", path.display()),
    }
}

/// An I/O error naming the file it concerns.
fn io_error(path: &Path, err: std::io::Error) -> PropError {
    PropError::BagIo {
        detail: format!("{}: {err}", path.display()),
    }
}

/// Serialise the outfits as the version 1 document.
///
/// Pretty-printed so the file is comfortable to hand-edit and diff; the output
/// always ends with a newline.
fn render(outfits: &[Outfit]) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"version\": {OUTFITS_VERSION},");
    if outfits.is_empty() {
        out.push_str("  \"outfits\": []\n}\n");
        return out;
    }

    out.push_str("  \"outfits\": [\n");
    for (index, outfit) in outfits.iter().enumerate() {
        out.push_str("    {\n");
        let _ = writeln!(out, "      \"name\": {},", quote(&outfit.name));
        if outfit.props.is_empty() {
            out.push_str("      \"props\": []\n");
        } else {
            out.push_str("      \"props\": [\n");
            for (position, key) in outfit.props.iter().enumerate() {
                let comma = if position + 1 == outfit.props.len() {
                    ""
                } else {
                    ","
                };
                let _ = writeln!(out, "        [{}, {}]{comma}", key.id, key.crc);
            }
            out.push_str("      ]\n");
        }
        let comma = if index + 1 == outfits.len() { "" } else { "," };
        let _ = writeln!(out, "    }}{comma}");
    }
    out.push_str("  ]\n}\n");
    out
}

/// Quote and escape a string as a JSON string literal.
///
/// Control characters become `\uXXXX` escapes, so the file is always valid
/// UTF-8 text even for a name pasted from somewhere strange.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Parse and validate a version 1 document.
///
/// Returns the outfits in file order together with the number of duplicate
/// names dropped. Any schema violation is an error and yields no outfits at
/// all: a partially understood file is never half-loaded.
fn parse_outfits(text: &str, path: &Path) -> Result<(Vec<Outfit>, usize)> {
    let document = JsonParser::new(text)
        .parse_document()
        .map_err(|err| bag_error(path, err))?;

    let Json::Object(root) = document else {
        return Err(bag_error(path, "the top level is not an object"));
    };
    let version = match object_field(&root, "version") {
        Some(Json::Int(version)) => *version,
        Some(_) => return Err(bag_error(path, "the version is not an integer")),
        None => return Err(bag_error(path, "the version field is missing")),
    };
    if version != i64::from(OUTFITS_VERSION) {
        return Err(bag_error(
            path,
            format!("unsupported version {version} (this build reads version {OUTFITS_VERSION})"),
        ));
    }
    let entries = match object_field(&root, "outfits") {
        Some(Json::Array(entries)) => entries,
        Some(_) => return Err(bag_error(path, "the outfits field is not an array")),
        None => return Err(bag_error(path, "the outfits field is missing")),
    };

    let mut parsed: Vec<Outfit> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let Json::Object(entry) = entry else {
            return Err(bag_error(path, format!("outfit {index} is not an object")));
        };
        let name = match object_field(entry, "name") {
            Some(Json::String(name)) => name.clone(),
            Some(_) => {
                return Err(bag_error(
                    path,
                    format!("outfit {index}: name is not a string"),
                ))
            }
            None => return Err(bag_error(path, format!("outfit {index}: name is missing"))),
        };
        if name.trim().is_empty() {
            return Err(bag_error(path, format!("outfit {index}: name is empty")));
        }
        let references = match object_field(entry, "props") {
            Some(Json::Array(references)) => references,
            Some(_) => {
                return Err(bag_error(
                    path,
                    format!("outfit {index}: props is not an array"),
                ));
            }
            None => return Err(bag_error(path, format!("outfit {index}: props is missing"))),
        };

        let mut props = Vec::with_capacity(references.len());
        for (position, reference) in references.iter().enumerate() {
            let Json::Array(pair) = reference else {
                return Err(bag_error(
                    path,
                    format!("outfit {index}: prop {position} is not an [id, crc] pair"),
                ));
            };
            if pair.len() != 2 {
                return Err(bag_error(
                    path,
                    format!("outfit {index}: prop {position} does not have exactly two numbers"),
                ));
            }
            let id = i32::try_from(require_int(pair.first(), "prop id", path, index)?).map_err(
                |_| {
                    bag_error(
                        path,
                        format!("outfit {index}: prop id is outside the i32 range"),
                    )
                },
            )?;
            let crc = u32::try_from(require_int(pair.get(1), "prop crc", path, index)?).map_err(
                |_| {
                    bag_error(
                        path,
                        format!("outfit {index}: prop crc is outside the u32 range"),
                    )
                },
            )?;
            props.push(PropKey::new(id, crc));
        }

        parsed.push(Outfit { name, props });
    }

    // Duplicate names resolve deterministically: the first occurrence wins and
    // later ones are dropped rather than merged or numbered.
    let mut outfits: Vec<Outfit> = Vec::with_capacity(parsed.len());
    let mut dropped = 0usize;
    for outfit in parsed {
        if outfits.iter().any(|existing| existing.name == outfit.name) {
            dropped += 1;
        } else {
            outfits.push(outfit);
        }
    }
    Ok((outfits, dropped))
}

/// The int stored under `key`, or a schema error naming the outfit.
fn require_int(value: Option<&Json>, what: &str, path: &Path, index: usize) -> Result<i64> {
    match value {
        Some(Json::Int(number)) => Ok(*number),
        Some(_) => Err(bag_error(
            path,
            format!("outfit {index}: {what} is not an integer"),
        )),
        None => Err(bag_error(
            path,
            format!("outfit {index}: {what} is missing"),
        )),
    }
}

/// A field of an object; a repeated key resolves to its first occurrence.
fn object_field<'a>(object: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    object
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// Why a document could not be parsed, and where.
#[derive(Debug)]
struct JsonError {
    /// Byte offset of the failure.
    position: usize,
    /// Human-readable description (static; the offset carries the specifics).
    message: &'static str,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid JSON at byte {}: {}",
            self.position, self.message
        )
    }
}

/// The parser's result type.
type JsonResult<T> = std::result::Result<T, JsonError>;

/// A minimal JSON value: enough for this file, and deliberately nothing more.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// A byte-level recursive-descent reader for the JSON subset this file needs.
///
/// Hand-written on purpose: the crate carries no JSON dependency, and the
/// format is small enough that a reader with one job beats a general one. It
/// works on bytes and is fully bounds-checked — no input can make it panic.
struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    /// A parser over `text`.
    fn new(text: &'a str) -> Self {
        Self {
            input: text.as_bytes(),
            position: 0,
            depth: 0,
        }
    }

    /// Parse one complete document: a single value, then end of input.
    fn parse_document(&mut self) -> JsonResult<Json> {
        self.skip_whitespace();
        let value = self.parse_value()?;
        self.skip_whitespace();
        if self.peek().is_some() {
            return self.fail("trailing data after the top-level value");
        }
        Ok(value)
    }

    /// Parse any value, recursing only through objects and arrays.
    fn parse_value(&mut self) -> JsonResult<Json> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(Json::String),
            Some(b't') => self.parse_keyword("true", Json::Bool(true)),
            Some(b'f') => self.parse_keyword("false", Json::Bool(false)),
            Some(b'n') => self.parse_keyword("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => self.fail("unexpected character"),
            None => self.fail("unexpected end of input"),
        }
    }

    /// Parse an object into its key/value list, first occurrence of a key first.
    fn parse_object(&mut self) -> JsonResult<Json> {
        self.expect(b'{')?;
        self.enter()?;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.advance();
            self.leave();
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.advance(),
                Some(b'}') => {
                    self.advance();
                    self.leave();
                    return Ok(Json::Object(entries));
                }
                Some(_) => return self.fail("expected ',' or '}' in an object"),
                None => return self.fail("unterminated object"),
            }
        }
    }

    /// Parse an array into its values.
    fn parse_array(&mut self) -> JsonResult<Json> {
        self.expect(b'[')?;
        self.enter()?;
        let mut values: Vec<Json> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.advance();
            self.leave();
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.advance(),
                Some(b']') => {
                    self.advance();
                    self.leave();
                    return Ok(Json::Array(values));
                }
                Some(_) => return self.fail("expected ',' or ']' in an array"),
                None => return self.fail("unterminated array"),
            }
        }
    }

    /// Parse a string starting at the opening quote, decoding escapes.
    fn parse_string(&mut self) -> JsonResult<String> {
        self.expect(b'"')?;
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    self.advance();
                    self.parse_escape(&mut bytes)?;
                }
                Some(byte) if byte < 0x20 => {
                    return self.fail("unescaped control character in a string");
                }
                Some(byte) => {
                    bytes.push(byte);
                    self.advance();
                }
                None => return self.fail("unterminated string"),
            }
        }
        match String::from_utf8(bytes) {
            Ok(string) => Ok(string),
            Err(_) => self.fail("string is not valid UTF-8"),
        }
    }

    /// Decode one backslash escape, appending its UTF-8 bytes.
    fn parse_escape(&mut self, out: &mut Vec<u8>) -> JsonResult<()> {
        match self.peek() {
            Some(b'"' | b'\\' | b'/') => {
                if let Some(byte) = self.peek() {
                    out.push(byte);
                }
                self.advance();
            }
            Some(b'b') => {
                out.push(0x08);
                self.advance();
            }
            Some(b'f') => {
                out.push(0x0c);
                self.advance();
            }
            Some(b'n') => {
                out.push(b'\n');
                self.advance();
            }
            Some(b'r') => {
                out.push(b'\r');
                self.advance();
            }
            Some(b't') => {
                out.push(b'\t');
                self.advance();
            }
            Some(b'u') => {
                self.advance();
                let character = self.parse_unicode_escape()?;
                let mut buffer = [0u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            Some(_) => return self.fail("unknown escape sequence"),
            None => return self.fail("unterminated escape sequence"),
        }
        Ok(())
    }

    /// Decode the four hex digits after `\u`, combining surrogate pairs.
    fn parse_unicode_escape(&mut self) -> JsonResult<char> {
        let first = self.parse_hex4()?;
        if (0xd800..=0xdbff).contains(&first) {
            self.expect(b'\\')?;
            self.expect(b'u')?;
            let second = self.parse_hex4()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return self.fail("high surrogate without a low surrogate");
            }
            let combined = 0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            return match char::from_u32(combined) {
                Some(character) => Ok(character),
                None => self.fail("invalid unicode code point"),
            };
        }
        if (0xdc00..=0xdfff).contains(&first) {
            return self.fail("lone low surrogate");
        }
        match char::from_u32(first) {
            Some(character) => Ok(character),
            None => self.fail("invalid unicode code point"),
        }
    }

    /// Read exactly four hexadecimal digits as one number.
    fn parse_hex4(&mut self) -> JsonResult<u32> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(byte @ b'0'..=b'9') => u32::from(byte - b'0'),
                Some(byte @ b'a'..=b'f') => u32::from(byte - b'a') + 10,
                Some(byte @ b'A'..=b'F') => u32::from(byte - b'A') + 10,
                Some(_) => return self.fail("invalid hex digit in a \\u escape"),
                None => return self.fail("truncated \\u escape"),
            };
            value = (value << 4) | digit;
            self.advance();
        }
        Ok(value)
    }

    /// Scan an integer or float. Both are accepted here; the schema later
    /// insists on integers where integers are required.
    fn parse_number(&mut self) -> JsonResult<Json> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.advance();
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.advance();
        }
        if self.peek() == Some(b'.') {
            self.advance();
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.advance();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.advance();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.advance();
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.advance();
            }
        }

        let digits = self
            .input
            .get(start..self.position)
            .and_then(|slice| std::str::from_utf8(slice).ok());
        let Some(text) = digits else {
            return self.fail("invalid number");
        };
        if text.is_empty() || text == "-" {
            return self.fail("invalid number");
        }
        if let Ok(integer) = text.parse::<i64>() {
            return Ok(Json::Int(integer));
        }
        match text.parse::<f64>() {
            Ok(float) if float.is_finite() => Ok(Json::Float(float)),
            _ => self.fail("invalid number"),
        }
    }

    /// Consume a bare keyword (`true`, `false`, `null`).
    fn parse_keyword(&mut self, word: &str, value: Json) -> JsonResult<Json> {
        let expected = word.as_bytes();
        if self.input.len().saturating_sub(self.position) < expected.len() {
            return self.fail("unexpected end of input");
        }
        for (offset, byte) in expected.iter().enumerate() {
            if self.input.get(self.position + offset) != Some(byte) {
                return self.fail("unexpected character");
            }
        }
        self.position += expected.len();
        Ok(value)
    }

    /// Consume exactly `byte`, or fail.
    fn expect(&mut self, byte: u8) -> JsonResult<()> {
        if self.peek() == Some(byte) {
            self.advance();
            Ok(())
        } else {
            self.fail("unexpected character")
        }
    }

    /// The byte under the cursor, if any.
    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    /// Move past one byte.
    fn advance(&mut self) {
        self.position = self.position.saturating_add(1);
    }

    /// Skip spaces, tabs and newlines.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.advance();
        }
    }

    /// Count one level of object/array nesting and enforce the cap.
    fn enter(&mut self) -> JsonResult<()> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_JSON_DEPTH {
            return self.fail("nesting is too deep");
        }
        Ok(())
    }

    /// Leave one level of nesting.
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Build a failure at the cursor.
    fn fail<T>(&self, message: &'static str) -> JsonResult<T> {
        Err(JsonError {
            position: self.position,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<(Vec<Outfit>, usize)> {
        parse_outfits(text, Path::new("Outfits.prp"))
    }

    #[test]
    fn render_and_parse_round_trip_awkward_names() {
        let outfits = vec![
            Outfit {
                name: "Quote \" backslash \\ newline\n tab\t control\u{1}".to_string(),
                props: vec![PropKey::new(-5, 7), PropKey::new(-5, 8)],
            },
            Outfit {
                name: "Ünïcode ✓ 𝄞".to_string(),
                props: Vec::new(),
            },
        ];
        let text = render(&outfits);
        let (parsed, dropped) = parse(&text).expect("our own output must parse");
        assert_eq!(dropped, 0);
        assert_eq!(parsed, outfits);
    }

    #[test]
    fn quote_escapes_control_characters() {
        assert_eq!(quote("plain"), "\"plain\"");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote("\n\r\t"), "\"\\n\\r\\t\"");
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
    }

    #[test]
    fn a_bad_document_is_an_error_not_a_panic() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let cases: Vec<String> = vec![
            String::new(),
            "{".to_string(),
            "[]".to_string(),
            "{ }".to_string(),
            "{\"outfits\": []}".to_string(),
            "{\"version\": 1}".to_string(),
            "{\"version\": \"1\", \"outfits\": []}".to_string(),
            "{\"version\": 2, \"outfits\": []}".to_string(),
            "{\"version\": 1, \"outfits\": {}}".to_string(),
            "{\"version\": 1, \"outfits\": [41]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"\", \"props\": []}]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\"}]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\", \"props\": [[1]]}]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\", \"props\": [[1, 2, 3]]}]}"
                .to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\", \"props\": [[1, -1]]}]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\", \"props\": [[1, 2.5]]}]}".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\", \"props\": [[2147483648, 0]]}]}"
                .to_string(),
            "{\"version\": 1, \"outfits\": []} trailing".to_string(),
            "{\"version\": 1, \"outfits\": []".to_string(),
            "{\"version\": 1, \"outfits\": [{\"name\": \"A\\q\", \"props\": []}]}".to_string(),
            deep,
        ];
        for text in cases {
            let outcome = parse(&text);
            assert!(outcome.is_err(), "must be rejected: {text:?}");
        }
    }

    #[test]
    fn unknown_keys_and_boundaries_are_accepted() {
        let text = r#"{
            "version": 1,
            "note": "added by a newer build",
            "outfits": [
                {"name": "A", "props": [[-2147483648, 0], [2147483647, 4294967295]], "extra": null}
            ]
        }"#;
        let (outfits, dropped) = parse(text).expect("unknown keys must not fail the load");
        assert_eq!(dropped, 0);
        assert_eq!(
            outfits,
            vec![Outfit {
                name: "A".to_string(),
                props: vec![PropKey::new(i32::MIN, 0), PropKey::new(i32::MAX, u32::MAX),],
            }]
        );
    }

    #[test]
    fn duplicate_names_resolve_to_the_first_occurrence() {
        let text = r#"{
            "version": 1,
            "outfits": [
                {"name": "Twin", "props": [[1, 2]]},
                {"name": "Other", "props": []},
                {"name": "Twin", "props": [[3, 4]]},
                {"name": "Twin", "props": []}
            ]
        }"#;
        let (outfits, dropped) = parse(text).expect("duplicates are a policy, not corruption");
        assert_eq!(dropped, 2);
        assert_eq!(
            outfits
                .iter()
                .map(|outfit| outfit.name.as_str())
                .collect::<Vec<&str>>(),
            vec!["Twin", "Other"]
        );
        assert_eq!(outfits[0].props, vec![PropKey::new(1, 2)]);
    }

    #[test]
    fn duplicate_object_keys_resolve_to_the_first_occurrence() {
        let text = r#"{"version": 1, "version": 99, "outfits": []}"#;
        let (outfits, _dropped) = parse(text).expect("the first version wins");
        assert!(outfits.is_empty());
    }
}

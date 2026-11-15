//! Reading a manifest (C# `StyleManifestDecoder` in `StyleManifest.cs`): the size bound, the raw-key prescan (`Scan`: depth,
//! escaped, banned, long or duplicate keys and `schema` first, token by token as .NET's `Utf8JsonReader` meets them), the closed
//! structure (`Structure`, every field in C#'s order so the first refusal is C#'s) and then the cross-references
//! ([`crate::validator`]). Owner: W9-D.

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;

use crate::manifest::{self, StyleManifest, StyleManifestError, Vocabulary, banned, length};

/// `StyleManifestDecoder.MaximumBytes`.
pub const MAXIMUM_BYTES: usize = 262_144;

/// `StyleManifestDecoder.Codes`: every code a manifest is refused with (the decoder's own and the registry's E_ID_COLLISION).
pub const CODES: [&str; 54] = [
    "E_TOO_LARGE",
    "E_TOO_DEEP",
    "E_DUPLICATE_KEY",
    "E_SCHEMA_NOT_FIRST",
    "E_KEY_ESCAPE",
    "E_NOT_JSON",
    "E_SCHEMA_MISSING",
    "E_SCHEMA_VERSION",
    "E_UNKNOWN_FIELD",
    "E_MISSING_FIELD",
    "E_TYPE",
    "E_STRING_LENGTH",
    "E_CONTROL_CHAR",
    "E_RESERVED_SEPARATOR",
    "E_ID_SHAPE",
    "E_RESERVED_ID",
    "E_RESERVED_NAME",
    "E_DUPLICATE_ID",
    "E_LIMIT",
    "E_PROMPT_PLACEHOLDER",
    "E_TAKES_TEXT_MISMATCH",
    "E_FOLD_TEXT",
    "E_PROMPT_RECOGNITION",
    "E_UNKNOWN_FLAG",
    "E_UNKNOWN_REFERENCE",
    "E_ALIAS_COLLISION",
    "E_UNKNOWN_RULE",
    "E_RULE_INCOMPLETE",
    "E_START_PHASE",
    "E_PHASE_RULE_NONE",
    "E_ENTER_ACTION_TEXT",
    "E_UNKNOWN_CAPABILITY",
    "E_CAPABILITY_UNDECLARED",
    "E_CAPABILITY_MAP",
    "E_UNKNOWN_TINT",
    "E_UNKNOWN_ICON",
    "E_UNKNOWN_PROBE",
    "E_PROBE_NAME_SHAPE",
    "E_SCOPES",
    "E_AUTOALLOW_SERVER",
    "E_AUTOALLOW_SHAPE",
    "E_AUTOALLOW_FOREIGN_SERVER",
    "E_AUTOALLOW_TOOLSEARCH_BUNDLED",
    "E_AUTOALLOW_QUESTION",
    "E_AUTOALLOW_DUPLICATE",
    "E_PLACEHOLDER_INITIAL",
    "E_JOB_MATCHER_LITERAL",
    "E_ID_COLLISION",
    "E_STATE_PARSER",
    "E_STATE_WIDGET",
    "E_STATE_AGGREGATE",
    "E_STATE_PATH_ESCAPE",
    "E_STATE_RUN_EVENT",
    "E_STATE_RUN_STATE",
];

/// `ReservedIds`: `cli` is the wire word for no style; `paperthin` stays reserved after v7 removed it.
pub const RESERVED_IDS: [&str; 3] = ["cli", "ouroboros", "paperthin"];

/// `ReservedNames`: every bundled style's name, as its manifest spells it.
pub const RESERVED_NAMES: [&str; 2] = ["Ouroboros", "클러드 플랜"];

const ICONS: [&str; 33] = [
    "point.3.connected.trianglepath.dotted",
    "questionmark.bubble",
    "wand.and.stars",
    "leaf",
    "play.fill",
    "checkmark.seal",
    "arrow.triangle.2.circlepath",
    "infinity",
    "gauge.with.dots.needle.33percent",
    "lightbulb",
    "square.grid.2x2",
    "arrow.up.message",
    "questionmark.square.dashed",
    "arrow.right",
    "arrow.clockwise",
    "arrow.triangle.branch",
    "bolt",
    "book",
    "bookmark",
    "calendar",
    "chart.bar",
    "cube",
    "doc.text",
    "flag",
    "folder",
    "hammer",
    "list.bullet",
    "magnifyingglass",
    "map",
    "paintbrush",
    "puzzlepiece",
    "sparkles",
    "tray",
];
const TINTS: [&str; 9] = ["accent", "purple", "teal", "indigo", "mint", "orange", "green", "red", "secondary"];

pub(crate) type R<T> = Result<T, StyleManifestError>;

pub(crate) fn fail<T>(code: &str, detail: &str) -> R<T> {
    Err(StyleManifestError::new(code, detail))
}

static STYLE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z0-9][a-z0-9-]{0,39}$").expect("id"));
static ACTION_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9][A-Za-z0-9_.:-]{0,63}$").expect("action id"));
static PROBE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9_][A-Za-z0-9@._-]{0,63}$").expect("probe"));

/// `SafeRelativePath`: not blank, no leading `/`, `~` or `\`, no `:`, no `..` part.
pub fn safe_relative_path(path: &str) -> bool {
    !manifest::trim(path).is_empty()
        && !path.starts_with(['/', '~', '\\'])
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|part| part == "..")
}

/// `StyleManifestDecoder.Decode(bytes, source, capabilities)`: the manifest, or the first refusal. `capabilities` is the vocabulary of
/// built-in features (the app's own, empty, outside the tests).
pub fn decode(bytes: &[u8], source: &str, capabilities: Option<&Vocabulary>) -> R<StyleManifest> {
    if bytes.len() > MAXIMUM_BYTES {
        return fail("E_TOO_LARGE", "");
    }
    scan(bytes)?;
    let root: Value = match serde_json::from_slice(bytes) {
        Ok(root) => root,
        Err(_) => return fail("E_NOT_JSON", ""),
    };
    if !root.is_object() {
        return fail("E_NOT_JSON", "");
    }
    structure(&root)?;
    let manifest = StyleManifest::new(root);
    let app = manifest::app_vocabulary();
    crate::validator::validate(&manifest, source, capabilities.unwrap_or(&app))?;
    Ok(manifest)
}

// ── The prescan ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A JSON syntax error: `E_NOT_JSON` (C# catches the reader's `JsonException`).
struct Syntax;

enum Token {
    Start(bool),
    End,
    Name { name: String, escaped: bool },
    Value,
}

struct Lexer<'a> {
    b: &'a [u8],
    at: usize,
    /// Each open container: an object, and whether a value (or member) was read in it.
    stack: Vec<(bool, bool)>,
    /// In an object, after a name and its colon: the member's value comes next.
    after_name: bool,
    done: bool,
}

impl<'a> Lexer<'a> {
    fn skip(&mut self) {
        while matches!(self.b.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// The next token, as `Utf8JsonReader.Read` returns it; None at the end of a complete document.
    fn next(&mut self) -> Result<Option<Token>, Syntax> {
        self.skip();
        if self.done {
            return if self.at == self.b.len() { Ok(None) } else { Err(Syntax) };
        }
        let Some(&(object, seen)) = self.stack.last() else {
            // The root value.
            if self.at >= self.b.len() {
                return Err(Syntax);
            }
            return self.value();
        };
        if self.after_name {
            self.after_name = false;
            return self.value();
        }
        let c = *self.b.get(self.at).ok_or(Syntax)?;
        if c == if object { b'}' } else { b']' } {
            self.at += 1;
            self.stack.pop();
            self.finished();
            return Ok(Some(Token::End));
        }
        if seen {
            if c != b',' {
                return Err(Syntax);
            }
            self.at += 1;
            self.skip();
        }
        if object {
            if self.b.get(self.at) != Some(&b'"') {
                return Err(Syntax);
            }
            let (name, escaped) = self.string()?;
            self.skip();
            if self.b.get(self.at) != Some(&b':') {
                return Err(Syntax);
            }
            self.at += 1;
            if let Some(top) = self.stack.last_mut() {
                top.1 = true;
            }
            self.after_name = true;
            Ok(Some(Token::Name { name, escaped }))
        } else {
            if let Some(top) = self.stack.last_mut() {
                top.1 = true;
            }
            self.value()
        }
    }

    /// A value just ended: the document is done when it was the root.
    fn finished(&mut self) {
        if self.stack.is_empty() {
            self.done = true;
        }
    }

    fn value(&mut self) -> Result<Option<Token>, Syntax> {
        self.skip();
        let c = *self.b.get(self.at).ok_or(Syntax)?;
        match c {
            b'{' | b'[' => {
                self.at += 1;
                self.stack.push((c == b'{', false));
                Ok(Some(Token::Start(c == b'{')))
            }
            b'"' => {
                self.string()?;
                self.finished();
                Ok(Some(Token::Value))
            }
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => {
                self.number()?;
                self.finished();
                Ok(Some(Token::Value))
            }
            _ => Err(Syntax),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<Option<Token>, Syntax> {
        if !self.b[self.at..].starts_with(word) {
            return Err(Syntax);
        }
        self.at += word.len();
        self.finished();
        Ok(Some(Token::Value))
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.b.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<(), Syntax> {
        if self.b.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.b.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(Syntax),
        }
        if self.b.get(self.at) == Some(&b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return Err(Syntax);
            }
        }
        if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.b.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(Syntax);
            }
        }
        // A number runs into nothing but a separator or white space.
        match self.b.get(self.at) {
            None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') => Ok(()),
            _ => Err(Syntax),
        }
    }

    /// A string at `"`: its text and whether it held an escape.
    fn string(&mut self) -> Result<(String, bool), Syntax> {
        self.at += 1;
        let mut out: Vec<u16> = Vec::new();
        let mut raw: Vec<u8> = Vec::new();
        let mut escaped = false;
        loop {
            let c = *self.b.get(self.at).ok_or(Syntax)?;
            match c {
                b'"' => {
                    self.at += 1;
                    break;
                }
                b'\\' => {
                    escaped = true;
                    flush(&mut raw, &mut out)?;
                    let e = *self.b.get(self.at + 1).ok_or(Syntax)?;
                    self.at += 2;
                    let unit = match e {
                        b'"' => 0x22,
                        b'\\' => 0x5c,
                        b'/' => 0x2f,
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => 0x0a,
                        b'r' => 0x0d,
                        b't' => 0x09,
                        b'u' => {
                            let hex = self.b.get(self.at..self.at + 4).ok_or(Syntax)?;
                            let text = std::str::from_utf8(hex).map_err(|_| Syntax)?;
                            if !text.bytes().all(|h| h.is_ascii_hexdigit()) {
                                return Err(Syntax);
                            }
                            self.at += 4;
                            u16::from_str_radix(text, 16).map_err(|_| Syntax)?
                        }
                        _ => return Err(Syntax),
                    };
                    out.push(unit);
                }
                0..=0x1f => return Err(Syntax),
                _ => {
                    raw.push(c);
                    self.at += 1;
                }
            }
        }
        flush(&mut raw, &mut out)?;
        Ok((String::from_utf16_lossy(&out), escaped))
    }
}

/// Raw bytes of a string, checked as UTF-8 (the reader refuses invalid UTF-8).
fn flush(raw: &mut Vec<u8>, out: &mut Vec<u16>) -> Result<(), Syntax> {
    if !raw.is_empty() {
        out.extend(std::str::from_utf8(raw).map_err(|_| Syntax)?.encode_utf16());
        raw.clear();
    }
    Ok(())
}

/// C# `Scan`: the raw keys before anything is parsed, so duplicate or escaped keys cannot show different permissions to a reader.
fn scan(bytes: &[u8]) -> R<()> {
    let mut lexer = Lexer { b: bytes, at: 0, stack: Vec::new(), after_name: false, done: false };
    let mut keys: Vec<(bool, HashSet<String>)> = Vec::new();
    loop {
        let token = match lexer.next() {
            Ok(Some(token)) => token,
            Ok(None) => return Ok(()),
            Err(Syntax) => return fail("E_NOT_JSON", ""),
        };
        match token {
            Token::Start(object) => {
                keys.push((object, HashSet::new()));
                if keys.len() > 8 {
                    return fail("E_TOO_DEEP", "");
                }
            }
            Token::End => {
                keys.pop();
            }
            Token::Name { name, escaped } => {
                if escaped {
                    return fail("E_KEY_ESCAPE", &name);
                }
                if name.chars().any(|c| banned(c as u32)) {
                    return fail("E_CONTROL_CHAR", &name);
                }
                if length(&name) > 64 {
                    return fail("E_STRING_LENGTH", &name);
                }
                let depth = keys.len();
                let Some((_, seen)) = keys.last_mut() else { return fail("E_NOT_JSON", "") };
                if seen.contains(&name) {
                    return fail("E_DUPLICATE_KEY", &name);
                }
                if depth == 1 && seen.is_empty() && name != "schema" {
                    return fail("E_SCHEMA_NOT_FIRST", "");
                }
                seen.insert(name);
            }
            Token::Value => {}
        }
    }
}

// ── The closed structure ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// C#'s `Reader`: an object's fields taken one by one; what is left over is an unknown field.
struct Reader {
    fields: IndexMap<String, Value>,
    path: String,
}

impl Reader {
    fn new(e: Option<Value>, path: &str) -> R<Reader> {
        let Some(e) = e else { return fail("E_MISSING_FIELD", path) };
        let Value::Object(map) = e else { return fail("E_TYPE", path) };
        Ok(Reader { fields: map.into_iter().collect(), path: path.to_string() })
    }
    fn take(&mut self, name: &str) -> Option<Value> {
        self.fields.shift_remove(name)
    }
    fn at(&self, name: &str) -> String {
        if self.path.is_empty() { name.to_string() } else { format!("{}.{name}", self.path) }
    }
    fn s(&mut self, name: &str, min: usize, max: usize, separator: bool) -> R<String> {
        let at = self.at(name);
        string(self.take(name).as_ref(), &at, min, max, separator)
    }
    fn o(&mut self, name: &str, min: usize, max: usize, separator: bool) -> R<Option<String>> {
        let at = self.at(name);
        match self.take(name) {
            None => Ok(None),
            Some(v) => string(Some(&v), &at, min, max, separator).map(Some),
        }
    }
    fn end(&self) -> R<()> {
        match self.fields.keys().min_by(|a, b| manifest::ordinal(a, b)) {
            Some(first) => fail("E_UNKNOWN_FIELD", &self.at(first)),
            None => Ok(()),
        }
    }
}

/// C# `Str`.
fn string(v: Option<&Value>, p: &str, min: usize, max: usize, separator: bool) -> R<String> {
    let Some(v) = v else { return fail("E_MISSING_FIELD", p) };
    let Some(s) = v.as_str() else { return fail("E_TYPE", p) };
    if s.chars().any(|c| banned(c as u32)) {
        return fail("E_CONTROL_CHAR", p);
    }
    if !separator && s.contains('·') {
        return fail("E_RESERVED_SEPARATOR", p);
    }
    let n = length(s);
    if n < min || n > max || n > 400 {
        return fail("E_STRING_LENGTH", p);
    }
    Ok(s.to_string())
}

fn boolean(v: Option<Value>, p: &str, fallback: Option<bool>) -> R<bool> {
    match v {
        None => match fallback {
            Some(b) => Ok(b),
            None => fail("E_MISSING_FIELD", p),
        },
        Some(Value::Bool(b)) => Ok(b),
        Some(_) => fail("E_TYPE", p),
    }
}

fn integer(v: Option<Value>, p: &str) -> R<f64> {
    let Some(v) = v else { return fail("E_MISSING_FIELD", p) };
    match v.as_f64() {
        Some(d) if v.is_number() && d.is_finite() && d == d.trunc() && d.abs() <= 9_007_199_254_740_992.0 => Ok(d),
        _ => fail("E_TYPE", p),
    }
}

fn array(v: Option<Value>, p: &str, max: usize, min: usize) -> R<Vec<Value>> {
    let Some(v) = v else { return fail("E_MISSING_FIELD", p) };
    let Value::Array(a) = v else { return fail("E_TYPE", p) };
    if a.len() < min {
        return fail("E_MISSING_FIELD", p);
    }
    if a.len() > max {
        return fail("E_LIMIT", p);
    }
    Ok(a)
}

fn optional_array(v: Option<Value>, p: &str, max: usize) -> R<Vec<Value>> {
    match v {
        None => Ok(Vec::new()),
        v => array(v, p, max, 0),
    }
}

fn id(s: String, action: bool) -> R<String> {
    let shape = if action { &*ACTION_ID } else { &*STYLE_ID };
    if !shape.is_match(&s) {
        return fail("E_ID_SHAPE", &s);
    }
    Ok(s)
}

pub(crate) fn known(value: &str, allowed: &[&str], code: &str, p: &str) -> R<()> {
    if allowed.contains(&value) { Ok(()) } else { fail(code, if p.is_empty() { value } else { p }) }
}

fn presentation(r: &mut Reader) -> R<()> {
    if let Some(icon) = r.o("icon", 1, 400, true)? {
        known(&icon, &ICONS, "E_UNKNOWN_ICON", "")?;
    }
    if let Some(tint) = r.o("tint", 1, 400, true)? {
        known(&tint, &TINTS, "E_UNKNOWN_TINT", "")?;
    }
    Ok(())
}

fn guidance(s: Option<&str>, p: &str) -> R<()> {
    if let Some(s) = s
        && s.replace("{phase}", "").contains(['{', '}'])
    {
        return fail("E_PROMPT_PLACEHOLDER", p);
    }
    Ok(())
}

fn unique(seen: &mut HashSet<String>, id: &str, p: &str) -> R<()> {
    if seen.insert(id.to_string()) { Ok(()) } else { fail("E_DUPLICATE_ID", p) }
}

/// C# `Structure`.
fn structure(root: &Value) -> R<()> {
    let mut r = Reader::new(Some(root.clone()), "")?;
    let Some(schema) = r.take("schema") else { return fail("E_SCHEMA_MISSING", "") };
    if integer(Some(schema), "schema")? != 1.0 {
        return fail("E_SCHEMA_VERSION", "");
    }
    id(r.s("id", 1, 40, true)?, false)?;
    r.s("name", 1, 40, false)?;
    r.s("summary", 1, 240, true)?;
    r.s("subtitle", 1, 80, true)?;

    let mut p = Reader::new(r.take("placeholders"), "placeholders")?;
    p.s("idle", 0, 120, true)?;
    p.s("answering", 0, 120, true)?;
    p.o("initial", 0, 120, true)?;
    p.o("running", 0, 120, true)?;
    let at = p.at("phases");
    phase_lines(p.take("phases"), &at, 120)?;
    p.end()?;

    let mut p = Reader::new(r.take("guidance"), "guidance")?;
    let mut lines: Vec<(Option<String>, String)> = Vec::new();
    for k in ["start", "next", "running"] {
        let text = p.o(k, 0, 160, true)?;
        lines.push((text, p.at(k)));
    }
    let at = p.at("phases");
    lines.extend(phase_lines(p.take("phases"), &at, 160)?);
    p.end()?;
    for (text, path) in &lines {
        guidance(text.as_deref(), path)?;
    }

    let mut p = Reader::new(r.take("prerequisites"), "prerequisites")?;
    known(&p.s("mode", 1, 16, true)?, &["all", "any"], "E_UNKNOWN_RULE", "")?;
    if let Some(report) = p.o("report", 1, 16, true)? {
        known(&report, &["first", "all"], "E_UNKNOWN_RULE", "")?;
    }
    let at = p.at("probes");
    for (index, e) in array(p.take("probes"), &at, 16, 0)?.into_iter().enumerate() {
        let mut q = Reader::new(Some(e), &format!("prerequisites.probes[{index}]"))?;
        let kind = q.s("kind", 1, 32, true)?;
        q.s("missing", 1, 200, true)?;
        q.o("hint", 0, 200, true)?;
        let at = q.at("install");
        boolean(q.take("install"), &at, Some(true))?;
        known(&kind, &["plugin", "executable", "skill"], "E_UNKNOWN_PROBE", "")?;
        let name = q.s(if kind == "plugin" { "prefix" } else { "name" }, 1, 64, true)?;
        if !PROBE_NAME.is_match(&name) {
            return fail("E_PROBE_NAME_SHAPE", &name);
        }
        if kind == "skill" {
            let at = q.at("scopes");
            let a = array(q.take("scopes"), &at, 2, 0)?;
            if a.is_empty() {
                return fail("E_SCOPES", "");
            }
            let mut seen = HashSet::new();
            for s in a {
                match s.as_str() {
                    Some(scope @ ("user" | "workspace")) if seen.insert(scope.to_string()) => {}
                    _ => return fail("E_SCOPES", ""),
                }
            }
        }
        q.end()?;
    }
    p.end()?;

    if let Some(install) = r.take("install") {
        let mut p = Reader::new(Some(install), "install")?;
        p.s("command", 1, 400, true)?;
        p.s("paneTitle", 1, 40, false)?;
        p.end()?;
    }

    let mut ids = HashSet::new();
    let mut orders = HashSet::new();
    for (index, e) in array(r.take("phases"), "phases", 16, 0)?.into_iter().enumerate() {
        let mut p = Reader::new(Some(e), &format!("phases[{index}]"))?;
        let phase = id(p.s("id", 1, 40, true)?, false)?;
        unique(&mut ids, &phase, "phases")?;
        p.s("title", 1, 24, false)?;
        let at = p.at("order");
        let n = integer(p.take("order"), &at)?;
        if !(0.0..=99.0).contains(&n) {
            return fail("E_TYPE", &at);
        }
        unique(&mut orders, &(n as i64).to_string(), &at)?;
        p.end()?;
    }

    let mut ids = HashSet::new();
    for (index, e) in array(r.take("groups"), "groups", 16, 1)?.into_iter().enumerate() {
        let mut p = Reader::new(Some(e), &format!("groups[{index}]"))?;
        let group = id(p.s("id", 1, 40, true)?, false)?;
        unique(&mut ids, &group, "groups")?;
        p.s("title", 1, 24, true)?;
        p.o("axis", 0, 60, true)?;
        p.o("question", 0, 200, true)?;
        let at = p.at("actions");
        for s in array(p.take("actions"), &at, 100, 1)? {
            string(Some(&s), &at, 1, 64, true)?;
        }
        p.end()?;
    }

    let mut ids = HashSet::new();
    for (index, e) in array(r.take("actions"), "actions", 100, 1)?.into_iter().enumerate() {
        let mut p = Reader::new(Some(e), &format!("actions[{index}]"))?;
        let action = id(p.s("id", 1, 64, true)?, true)?;
        p.s("title", 1, 40, true)?;
        p.s("help", 0, 400, true)?;
        p.o("scope", 0, 120, true)?;
        let prompt = p.s("prompt", 1, 400, true)?;
        let at = p.at("takesText");
        let takes = boolean(p.take("takesText"), &at, None)?;
        let at = p.at("requiresText");
        boolean(p.take("requiresText"), &at, Some(false))?;
        let fold = p.o("foldText", 1, 16, true)?;
        p.o("match", 1, 64, true)?;
        p.o("phase", 1, 40, true)?;
        let at = p.at("flags");
        let mut flags = HashSet::new();
        for f in optional_array(p.take("flags"), &at, 2)? {
            match f.as_str() {
                Some(flag @ ("userInvoked" | "readOnly")) if flags.insert(flag.to_string()) => {}
                _ => return fail("E_UNKNOWN_FLAG", ""),
            }
        }
        presentation(&mut p)?;
        if let Some(glyph) = p.take("glyph")
            && !glyph.as_str().is_some_and(manifest::emoji::valid)
        {
            return fail("E_TYPE", &p.at("glyph"));
        }
        p.o("requestTitle", 1, 40, false)?;
        p.end()?;
        if takes {
            let Some(fold) = fold else { return fail("E_MISSING_FIELD", &p.at("foldText")) };
            known(&fold, &["trimOnly", "oneLine"], "E_UNKNOWN_RULE", "")?;
        } else if fold.is_some() {
            return fail("E_FOLD_TEXT", &action);
        }
        let rest = prompt.replace("{text}", "");
        let count = (prompt.encode_utf16().count() - rest.encode_utf16().count()) / 6;
        if count > 1 || rest.contains(['{', '}']) {
            return fail("E_PROMPT_PLACEHOLDER", &action);
        }
        if takes != (count == 1) {
            return fail("E_TAKES_TEXT_MISMATCH", &action);
        }
        unique(&mut ids, &action, "actions")?;
    }

    let mut ids = HashSet::new();
    for (index, e) in array(r.take("aliases"), "aliases", 64, 0)?.into_iter().enumerate() {
        let mut p = Reader::new(Some(e), &format!("aliases[{index}]"))?;
        let name = p.s("name", 1, 64, true)?;
        unique(&mut ids, &name, "aliases")?;
        p.s("phase", 1, 40, true)?;
        p.end()?;
    }

    let mut p = Reader::new(r.take("recognition"), "recognition")?;
    let at = p.at("prefixes");
    for e in array(p.take("prefixes"), &at, 4, 1)? {
        string(Some(&e), &at, 1, 32, true)?;
    }
    let at = p.at("lowercase");
    boolean(p.take("lowercase"), &at, None)?;
    p.end()?;

    rules(r.take("rules"))?;
    for e in array(r.take("capabilities"), "capabilities", 4, 0)? {
        string(Some(&e), "capabilities", 1, 64, true)?;
    }
    for (index, e) in array(r.take("autoAllow"), "autoAllow", 32, 0)?.into_iter().enumerate() {
        let mut p = Reader::new(Some(e), &format!("autoAllow[{index}]"))?;
        p.o("server", 1, 64, true)?;
        p.s("tool", 1, 64, true)?;
        p.end()?;
    }
    let mut p = Reader::new(r.take("presentation"), "presentation")?;
    presentation(&mut p)?;
    p.end()?;

    if let Some(job) = r.take("job") {
        let mut p = Reader::new(Some(job), "job")?;
        for key in ["open", "close"] {
            let at = p.at(key);
            for (index, e) in array(p.take(key), &at, 32, 0)?.into_iter().enumerate() {
                let mut q = Reader::new(Some(e), &format!("job.{key}[{index}]"))?;
                q.s("tool", 1, 64, true)?;
                let a = q.o("contains", 1, 400, true)?;
                let b = q.o("notContains", 1, 400, true)?;
                q.end()?;
                if a.is_some() && b.is_some() {
                    return fail("E_JOB_MATCHER_LITERAL", &q.path);
                }
            }
        }
        let at = p.at("whileOpen");
        for e in optional_array(p.take("whileOpen"), &at, 100)? {
            string(Some(&e), &at, 1, 64, true)?;
        }
        let text = p.o("guidance", 0, 160, true)?;
        guidance(text.as_deref(), &p.at("guidance"))?;
        p.end()?;
    }

    if let Some(state) = r.take("stateSources") {
        let mut p = Reader::new(Some(state), "stateSources")?;
        let at = p.at("files");
        for (index, e) in optional_array(p.take("files"), &at, 8)?.into_iter().enumerate() {
            let mut q = Reader::new(Some(e), &format!("stateSources.files[{index}]"))?;
            let path = q.s("path", 1, 400, true)?;
            known(&q.s("parser", 1, 40, true)?, &["markdownChecklist", "json"], "E_STATE_PARSER", "")?;
            known(&q.s("widget", 1, 40, true)?, &["progressBar", "list", "label"], "E_STATE_WIDGET", "")?;
            q.end()?;
            if !safe_relative_path(&path) {
                return fail("E_STATE_PATH_ESCAPE", &path);
            }
        }
        let at = p.at("runEvents");
        for (index, e) in optional_array(p.take("runEvents"), &at, 8)?.into_iter().enumerate() {
            let mut q = Reader::new(Some(e), &format!("stateSources.runEvents[{index}]"))?;
            known(&q.s("event", 1, 40, true)?, &["subagent.start", "subagent.finish", "tool.call"], "E_STATE_RUN_EVENT", "")?;
            known(&q.s("aggregate", 1, 40, true)?, &["count", "lastValue"], "E_STATE_AGGREGATE", "")?;
            known(&q.s("widget", 1, 40, true)?, &["progressBar", "list", "label"], "E_STATE_WIDGET", "")?;
            q.end()?;
        }
        let at = p.at("runState");
        for (index, e) in optional_array(p.take("runState"), &at, 4)?.into_iter().enumerate() {
            let mut q = Reader::new(Some(e), &format!("stateSources.runState[{index}]"))?;
            let source = q.s("source", 1, 40, true)?;
            let widget = q.s("widget", 1, 40, true)?;
            q.end()?;
            known(&source, &["todos", "background"], "E_STATE_RUN_STATE", "")?;
            let allowed: &[&str] =
                if source == "todos" { &["progressBar", "list", "label"] } else { &["taskList", "label", "progressBar"] };
            known(&widget, allowed, "E_STATE_WIDGET", "")?;
        }
        p.end()?;
    }

    // §1.17 (v6): every new request in the style starts in this permission mode (closed: plan).
    if let Some(launch) = r.take("launch") {
        let mut p = Reader::new(Some(launch), "launch")?;
        let mode = p.s("permissionMode", 1, 32, true)?;
        p.end()?;
        known(&mode, &["plan"], "E_UNKNOWN_RULE", "launch.permissionMode")?;
    }
    r.end()
}

/// C# `PhaseLines`: `{ "<phase id>": { "idle"?, "running"? } }`, keys in ordinal order; the keys are checked against the phases later.
fn phase_lines(value: Option<Value>, path: &str, max: usize) -> R<Vec<(Option<String>, String)>> {
    let mut lines = Vec::new();
    let Some(value) = value else { return Ok(lines) };
    let Value::Object(map) = value else { return fail("E_TYPE", path) };
    if map.len() > 16 {
        return fail("E_LIMIT", path);
    }
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|a, b| manifest::ordinal(&a.0, &b.0));
    for (name, entry) in entries {
        let mut r = Reader::new(Some(entry), &format!("{path}.{name}"))?;
        let idle = r.o("idle", 0, max, true)?;
        lines.push((idle, r.at("idle")));
        let running = r.o("running", 0, max, true)?;
        lines.push((running, r.at("running")));
        r.end()?;
    }
    Ok(lines)
}

/// C# `Rules`.
fn rules(value: Option<Value>) -> R<()> {
    let mut r = Reader::new(value, "rules")?;
    for name in ["start", "phase", "next", "enter", "recommend", "initialGroup"] {
        let at = r.at(name);
        let mut p = Reader::new(r.take(name), &at)?;
        let kind = p.s("kind", 1, 32, true)?;
        let allowed: &[&str] = match name {
            "start" => &["none", "actions"],
            "phase" => &["none", "lastRecognisedAction", "planState"],
            "next" => &["byGroup", "byPhase"],
            "enter" => &["verbatim", "rewriteBareDraftTo"],
            "recommend" => &["none", "capability"],
            _ => &["fixed", "capabilityState"],
        };
        known(&kind, allowed, "E_UNKNOWN_RULE", &p.path.clone())?;
        match kind.as_str() {
            "actions" => {
                p.s("phase", 1, 40, true)?;
                let at = p.at("actions");
                for e in array(p.take("actions"), &at, 100, 1)? {
                    string(Some(&e), &at, 1, 64, true)?;
                }
                p.o("resetTitle", 1, 24, true)?;
            }
            "lastRecognisedAction" => {
                p.s("default", 1, 40, true)?;
                let at = p.at("stateOverrides");
                for (i, e) in optional_array(p.take("stateOverrides"), &at, 16)?.into_iter().enumerate() {
                    let mut q = Reader::new(Some(e), &p.at(&format!("stateOverrides[{i}]")))?;
                    let at = q.at("sourceIndex");
                    integer(q.take("sourceIndex"), &at)?;
                    known(&q.s("condition", 1, 32, true)?, &["fileExists", "allChecked"], "E_UNKNOWN_RULE", "")?;
                    q.s("phase", 1, 40, true)?;
                    q.end()?;
                }
            }
            "planState" => {
                let at = p.at("map");
                map(p.take("map"), &at, false)?;
            }
            "byPhase" => {
                let at = p.at("map");
                map(p.take("map"), &at, true)?;
            }
            "rewriteBareDraftTo" => {
                p.s("action", 1, 64, true)?;
                p.s("phase", 1, 40, true)?;
            }
            "capability" | "capabilityState" => {
                p.s("capability", 1, 64, true)?;
                let at = p.at("map");
                map(p.take("map"), &at, false)?;
                if kind == "capability" {
                    p.o("group", 1, 40, true)?;
                }
            }
            "fixed" => {
                p.s("group", 1, 40, true)?;
            }
            _ => {}
        }
        p.end()?;
    }
    r.end()
}

/// C# `Map`.
fn map(value: Option<Value>, path: &str, arrays: bool) -> R<()> {
    let Some(value) = value else { return fail("E_MISSING_FIELD", path) };
    let Value::Object(entries) = value else { return fail("E_TYPE", path) };
    for (name, e) in entries {
        let at = format!("{path}.{name}");
        if arrays {
            for s in array(Some(e), &at, 100, 0)? {
                string(Some(&s), &at, 1, 64, true)?;
            }
        } else {
            string(Some(&e), &at, 1, 64, true)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prescan_meets_problems_in_document_order() {
        assert_eq!(scan(br#"{"schema":1,"a":1,"a":2}"#).unwrap_err().code, "E_DUPLICATE_KEY");
        assert_eq!(scan(br#"{"a":1}"#).unwrap_err().code, "E_SCHEMA_NOT_FIRST");
        assert_eq!(scan(br#"{"schema":1,"\u0061":1}"#).unwrap_err().code, "E_KEY_ESCAPE");
        assert_eq!(scan(br#"{"schema":1,"a":[[[[[[[[1]]]]]]]]}"#).unwrap_err().code, "E_TOO_DEEP");
        assert_eq!(scan(br#"{"schema":1,}"#).unwrap_err().code, "E_NOT_JSON");
        assert_eq!(scan(br#"{"schema":1} x"#).unwrap_err().code, "E_NOT_JSON");
        assert_eq!(scan(br#"{"schema":01}"#).unwrap_err().code, "E_NOT_JSON");
        assert!(scan(br#" {"schema":1,"a":{"b":[1,-2.5e3,true,null,"x\n"]}} "#).is_ok());
        assert!(safe_relative_path("docs/plans/*.md") && !safe_relative_path("C:\\x") && !safe_relative_path("a/../b"));
    }
}

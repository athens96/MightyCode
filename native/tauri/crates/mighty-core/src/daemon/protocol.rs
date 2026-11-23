//! The app↔daemon wire (MC-3), in the control API's style (MC-9, [`crate::control::protocol`]): newline-delimited JSON over the
//! profile's daemon socket (the Mac, 0600) or owner-only named pipe (Windows), each line at most [`MAX_LINE_BYTES`].
//!
//! - The first request of a connection is `hello` with the token (`<data>/daemon/token`) and the protocol version:
//!   `{"id":1,"cmd":"hello","protocol":1,"app":"0.6.0","token":"<64 hex>"}`. A wrong token is answered `unauthorized` and the
//!   connection closed; anything before `hello` too. The answer names the daemon's protocol and version and whether it `accepted`
//!   the client: a client of another protocol may then only ask `status` and `shutdown` (both kept the same in every version), so an
//!   updated app can see and stop an old daemon's work but never drive it.
//! - Each later request is answered by one line, `{"id":n,"ok":true,"result":{…}}` or `{"id":n,"ok":false,"error":{…}}`, in any
//!   order; between them the daemon writes `{"id":0,"event":{…}}` lines for the processes the connection attached to: `output`
//!   (where in the stream it starts, which stream, the bytes in base64), `replayed` (what was kept has all been sent; live output
//!   follows) and `exit`.
//! - Bytes travel as base64 in chunks of at most [`MAX_CHUNK_BYTES`]; a process's environment, arguments and meta are capped.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{Map, Value, json};

pub use crate::control::protocol::{
    ControlError, ErrorCode, LineBuffer, Reply, error_line, event_line, is_token, ok_line, parse_reply, token_matches,
};

use super::ring::Stream;

/// The protocol's version: an app and a daemon of different versions never drive each other.
pub const PROTOCOL: u64 = 1;
/// The longest line either side reads.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The most bytes one `output` event or `write` carries.
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
/// What a run's ring keeps (its stdout and stderr together), and a terminal's (about its scrollback).
pub const RUN_RING_BYTES: usize = 8 * 1024 * 1024;
pub const PTY_RING_BYTES: usize = 2 * 1024 * 1024;
/// The processes one daemon holds at once (running or exited and not yet released).
pub const MAX_PROCESSES: usize = 256;
/// A process's arguments and environment variables, and the longest meta.
pub const MAX_ARGS: usize = 4_096;
pub const MAX_ENV: usize = 4_096;
pub const MAX_META_BYTES: usize = 64 * 1024;

/// What a process is: an agent run's CLI (pipes) or a terminal (a pseudo console).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Run,
    Pty,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Run => "run",
            Kind::Pty => "pty",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "run" => Some(Kind::Run),
            "pty" => Some(Kind::Pty),
            _ => None,
        }
    }
}

/// A run's child as the platform's `SpawnRequest` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub shell_command: Option<String>,
}

/// A terminal as the platform's `PtyRequest` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PtySpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub columns: u16,
    pub rows: u16,
}

/// What a request asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Hello {
        protocol: u64,
        app: String,
        token: String,
    },
    /// Starts a run's child. `key` names what it belongs to (its pane) so a later app finds it; `meta` is the app's own, kept as given.
    Spawn {
        key: Option<String>,
        meta: Value,
        spec: SpawnSpec,
    },
    PtyOpen {
        key: Option<String>,
        meta: Value,
        spec: PtySpec,
    },
    /// Output from `from` on (what is still kept), then live output, as events.
    Attach {
        proc: String,
        from: u64,
    },
    Detach {
        proc: String,
    },
    Write {
        proc: String,
        data: Vec<u8>,
    },
    /// A run's stdin closed (the CLI reads its end).
    CloseStdin {
        proc: String,
    },
    Resize {
        proc: String,
        columns: u16,
        rows: u16,
    },
    /// Ends the process and everything it started.
    Kill {
        proc: String,
    },
    /// Whether a terminal runs a command now.
    Busy {
        proc: String,
    },
    /// An exited process forgotten, its output with it.
    Release {
        proc: String,
    },
    /// The app's own notes on a process, laid over its meta (e.g. where a later app takes up a kept run's output).
    Meta {
        proc: String,
        meta: Value,
    },
    List,
    Status,
    /// Every process ended, then the daemon exits.
    Shutdown,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Hello { .. } => "hello",
            Command::Spawn { .. } => "spawn",
            Command::PtyOpen { .. } => "pty.open",
            Command::Attach { .. } => "attach",
            Command::Detach { .. } => "detach",
            Command::Write { .. } => "write",
            Command::CloseStdin { .. } => "stdin.close",
            Command::Resize { .. } => "resize",
            Command::Kill { .. } => "kill",
            Command::Busy { .. } => "busy",
            Command::Release { .. } => "release",
            Command::Meta { .. } => "meta",
            Command::List => "list",
            Command::Status => "status",
            Command::Shutdown => "shutdown",
        }
    }

    /// Whether a client the daemon did not accept (another protocol) may ask it: only what every version keeps the same.
    pub fn version_free(&self) -> bool {
        matches!(self, Command::Hello { .. } | Command::Status | Command::Shutdown)
    }

    /// The request as one line (no newline).
    pub fn line(&self, id: u64) -> String {
        let mut out = Map::new();
        out.insert("id".into(), json!(id));
        out.insert("cmd".into(), json!(self.name()));
        let proc = |out: &mut Map<String, Value>, proc: &str| {
            out.insert("proc".into(), json!(proc));
        };
        match self {
            Command::Hello { protocol, app, token } => {
                out.insert("protocol".into(), json!(protocol));
                out.insert("app".into(), json!(app));
                out.insert("token".into(), json!(token));
            }
            Command::Spawn { key, meta, spec } => {
                out.insert("key".into(), json!(key));
                out.insert("meta".into(), meta.clone());
                out.insert("program".into(), json!(spec.program));
                out.insert("args".into(), json!(spec.args));
                out.insert("cwd".into(), json!(spec.cwd));
                out.insert("env".into(), json!(spec.env));
                out.insert("envRemove".into(), json!(spec.env_remove));
                out.insert("shellCommand".into(), json!(spec.shell_command));
            }
            Command::PtyOpen { key, meta, spec } => {
                out.insert("key".into(), json!(key));
                out.insert("meta".into(), meta.clone());
                out.insert("program".into(), json!(spec.program));
                out.insert("args".into(), json!(spec.args));
                out.insert("cwd".into(), json!(spec.cwd));
                out.insert("env".into(), json!(spec.env));
                out.insert("columns".into(), json!(spec.columns));
                out.insert("rows".into(), json!(spec.rows));
            }
            Command::Attach { proc: p, from } => {
                proc(&mut out, p);
                out.insert("from".into(), json!(from));
            }
            Command::Write { proc: p, data } => {
                proc(&mut out, p);
                out.insert("data".into(), json!(encode(data)));
            }
            Command::Meta { proc: p, meta } => {
                proc(&mut out, p);
                out.insert("meta".into(), meta.clone());
            }
            Command::Resize { proc: p, columns, rows } => {
                proc(&mut out, p);
                out.insert("columns".into(), json!(columns));
                out.insert("rows".into(), json!(rows));
            }
            Command::Detach { proc: p }
            | Command::CloseStdin { proc: p }
            | Command::Kill { proc: p }
            | Command::Busy { proc: p }
            | Command::Release { proc: p } => proc(&mut out, p),
            Command::List | Command::Status | Command::Shutdown => {}
        }
        Value::Object(out).to_string()
    }
}

pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

fn bad(message: impl Into<String>) -> ControlError {
    ControlError::bad(message)
}

fn text(object: &Map<String, Value>, name: &str) -> Result<String, ControlError> {
    object.get(name).and_then(Value::as_str).map(str::to_string).ok_or_else(|| bad(format!("'{name}' is missing")))
}

/// A process or pane id: letters, digits, `-`, `_`, `.`, `:`, at most 200 characters.
pub fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn id(object: &Map<String, Value>, name: &str) -> Result<String, ControlError> {
    let value = text(object, name)?;
    if !is_id(&value) {
        return Err(bad(format!("'{name}' is not an id")));
    }
    Ok(value)
}

fn key(object: &Map<String, Value>) -> Result<Option<String>, ControlError> {
    match object.get("key") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => id(object, "key").map(Some),
    }
}

fn meta(object: &Map<String, Value>) -> Result<Value, ControlError> {
    let meta = object.get("meta").cloned().unwrap_or(Value::Null);
    if !(meta.is_null() || meta.is_object()) {
        return Err(bad("'meta' must be an object"));
    }
    if meta.to_string().len() > MAX_META_BYTES {
        return Err(ControlError::new(ErrorCode::TooLarge, "'meta' is too large"));
    }
    Ok(meta)
}

fn strings(object: &Map<String, Value>, name: &str, max: usize) -> Result<Vec<String>, ControlError> {
    let items = match object.get(name) {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(bad(format!("'{name}' must be a list"))),
    };
    if items.len() > max {
        return Err(ControlError::new(ErrorCode::TooLarge, format!("'{name}' has too many items")));
    }
    items.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(format!("'{name}' must hold text")))).collect()
}

fn environment(object: &Map<String, Value>) -> Result<BTreeMap<String, String>, ControlError> {
    let entries = match object.get("env") {
        None | Some(Value::Null) => return Ok(BTreeMap::new()),
        Some(Value::Object(entries)) => entries,
        Some(_) => return Err(bad("'env' must be an object")),
    };
    if entries.len() > MAX_ENV {
        return Err(ControlError::new(ErrorCode::TooLarge, "'env' has too many variables"));
    }
    let mut env = BTreeMap::new();
    for (name, value) in entries {
        let value = value.as_str().ok_or_else(|| bad("'env' must hold text"))?;
        if name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0') {
            return Err(bad("'env' holds a name or value that cannot be set"));
        }
        env.insert(name.clone(), value.to_string());
    }
    Ok(env)
}

fn size(object: &Map<String, Value>, name: &str) -> Result<u16, ControlError> {
    let value = object.get(name).and_then(Value::as_u64).ok_or_else(|| bad(format!("'{name}' must be a whole number")))?;
    Ok(value.clamp(1, 1000) as u16)
}

fn program(object: &Map<String, Value>) -> Result<(String, Vec<String>, String), ControlError> {
    let program = text(object, "program")?;
    let cwd = text(object, "cwd")?;
    if program.is_empty() || program.contains('\0') || cwd.is_empty() || cwd.contains('\0') {
        return Err(bad("'program' and 'cwd' must be paths"));
    }
    let args = strings(object, "args", MAX_ARGS)?;
    if args.iter().any(|a| a.contains('\0')) {
        return Err(bad("an argument holds a NUL"));
    }
    Ok((program, args, cwd))
}

/// Reads one request line: its id and command. The token is not checked here (the daemon does).
pub fn parse_request(line: &[u8]) -> Result<(u64, Command), ControlError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ControlError::new(ErrorCode::TooLarge, "the request is too large"));
    }
    let value: Value = serde_json::from_slice(line).map_err(|_| bad("the request is not JSON"))?;
    let object = value.as_object().ok_or_else(|| bad("the request is not an object"))?;
    let request_id = object.get("id").and_then(Value::as_u64).unwrap_or(0);
    let cmd = object.get("cmd").and_then(Value::as_str).ok_or_else(|| bad("'cmd' is missing"))?;
    let command = match cmd {
        "hello" => Command::Hello {
            protocol: object.get("protocol").and_then(Value::as_u64).ok_or_else(|| bad("'protocol' is missing"))?,
            app: object.get("app").and_then(Value::as_str).unwrap_or("").chars().take(64).collect(),
            token: object.get("token").and_then(Value::as_str).unwrap_or("").to_string(),
        },
        "spawn" => {
            let (program, args, cwd) = program(object)?;
            let shell_command = object.get("shellCommand").and_then(Value::as_str).map(str::to_string);
            if shell_command.as_deref().is_some_and(|c| c.contains('\0')) {
                return Err(bad("the shell command holds a NUL"));
            }
            Command::Spawn {
                key: key(object)?,
                meta: meta(object)?,
                spec: SpawnSpec {
                    program,
                    args,
                    cwd,
                    env: environment(object)?,
                    env_remove: strings(object, "envRemove", MAX_ENV)?,
                    shell_command,
                },
            }
        }
        "pty.open" => {
            let (program, args, cwd) = program(object)?;
            Command::PtyOpen {
                key: key(object)?,
                meta: meta(object)?,
                spec: PtySpec {
                    program,
                    args,
                    cwd,
                    env: environment(object)?,
                    columns: size(object, "columns")?,
                    rows: size(object, "rows")?,
                },
            }
        }
        "attach" => Command::Attach {
            proc: id(object, "proc")?,
            from: object.get("from").and_then(Value::as_u64).ok_or_else(|| bad("'from' must be a whole number"))?,
        },
        "detach" => Command::Detach { proc: id(object, "proc")? },
        "write" => {
            let data = decode(&text(object, "data")?).ok_or_else(|| bad("'data' is not base64"))?;
            if data.len() > MAX_CHUNK_BYTES {
                return Err(ControlError::new(ErrorCode::TooLarge, "a write is at most 64 KB"));
            }
            Command::Write { proc: id(object, "proc")?, data }
        }
        "stdin.close" => Command::CloseStdin { proc: id(object, "proc")? },
        "resize" => Command::Resize { proc: id(object, "proc")?, columns: size(object, "columns")?, rows: size(object, "rows")? },
        "kill" => Command::Kill { proc: id(object, "proc")? },
        "busy" => Command::Busy { proc: id(object, "proc")? },
        "release" => Command::Release { proc: id(object, "proc")? },
        "meta" => {
            let meta = meta(object)?;
            if !meta.is_object() {
                return Err(bad("'meta' must be an object"));
            }
            Command::Meta { proc: id(object, "proc")?, meta }
        }
        "list" => Command::List,
        "status" => Command::Status,
        "shutdown" => Command::Shutdown,
        other => return Err(bad(format!("unknown command '{}'", other.chars().take(40).collect::<String>()))),
    };
    Ok((request_id, command))
}

/// What the daemon tells an attached client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Output {
        proc: String,
        offset: u64,
        stream: Stream,
        data: Vec<u8>,
    },
    /// Everything kept up to `offset` has been sent; what follows is live.
    Replayed {
        proc: String,
        offset: u64,
    },
    /// The process ended: its exit code, or why it could not be read.
    Exit {
        proc: String,
        code: Option<i32>,
        error: Option<String>,
    },
}

impl Event {
    pub fn proc(&self) -> &str {
        match self {
            Event::Output { proc, .. } | Event::Replayed { proc, .. } | Event::Exit { proc, .. } => proc,
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Event::Output { proc, offset, stream, data } => {
                json!({ "type": "output", "proc": proc, "offset": offset, "stream": stream.name(), "data": encode(data) })
            }
            Event::Replayed { proc, offset } => json!({ "type": "replayed", "proc": proc, "offset": offset }),
            Event::Exit { proc, code, error } => json!({ "type": "exit", "proc": proc, "code": code, "error": error }),
        }
    }

    /// The event as one line (no newline).
    pub fn line(&self) -> String {
        event_line(0, self.to_json())
    }

    pub fn parse(value: &Value) -> Option<Event> {
        let proc = value.get("proc")?.as_str()?.to_string();
        match value.get("type")?.as_str()? {
            "output" => Some(Event::Output {
                proc,
                offset: value.get("offset")?.as_u64()?,
                stream: Stream::parse(value.get("stream")?.as_str()?)?,
                data: decode(value.get("data")?.as_str()?)?,
            }),
            "replayed" => Some(Event::Replayed { proc, offset: value.get("offset")?.as_u64()? }),
            "exit" => Some(Event::Exit {
                proc,
                code: value.get("code").and_then(Value::as_i64).map(|c| c as i32),
                error: value.get("error").and_then(Value::as_str).map(str::to_string),
            }),
            _ => None,
        }
    }
}

/// A process as `list` gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub proc: String,
    pub kind: Kind,
    pub key: Option<String>,
    pub meta: Value,
    pub pid: u32,
    pub running: bool,
    pub code: Option<i32>,
    /// The stream's end now, and its oldest kept byte.
    pub end: u64,
    pub start: u64,
    /// When it started, in milliseconds since 1970.
    pub started_at: u64,
    pub attached: usize,
}

impl ProcInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "proc": self.proc,
            "kind": self.kind.name(),
            "key": self.key,
            "meta": self.meta,
            "pid": self.pid,
            "running": self.running,
            "code": self.code,
            "end": self.end,
            "start": self.start,
            "startedAt": self.started_at,
            "attached": self.attached,
        })
    }

    pub fn parse(value: &Value) -> Option<ProcInfo> {
        Some(ProcInfo {
            proc: value.get("proc")?.as_str()?.to_string(),
            kind: Kind::parse(value.get("kind")?.as_str()?)?,
            key: value.get("key").and_then(Value::as_str).map(str::to_string),
            meta: value.get("meta").cloned().unwrap_or(Value::Null),
            pid: value.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            running: value.get("running").and_then(Value::as_bool).unwrap_or(false),
            code: value.get("code").and_then(Value::as_i64).map(|c| c as i32),
            end: value.get("end").and_then(Value::as_u64).unwrap_or(0),
            start: value.get("start").and_then(Value::as_u64).unwrap_or(0),
            started_at: value.get("startedAt").and_then(Value::as_u64).unwrap_or(0),
            attached: value.get("attached").and_then(Value::as_u64).unwrap_or(0) as usize,
        })
    }
}

/// The `hello` answer: the daemon's protocol and version, and whether it took the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    pub protocol: u64,
    pub app: String,
    pub pid: u32,
    pub accepted: bool,
}

impl Welcome {
    pub fn to_json(&self) -> Value {
        json!({ "protocol": self.protocol, "app": self.app, "pid": self.pid, "accepted": self.accepted })
    }

    pub fn parse(value: &Value) -> Option<Welcome> {
        Some(Welcome {
            protocol: value.get("protocol")?.as_u64()?,
            app: value.get("app").and_then(Value::as_str).unwrap_or("").to_string(),
            pid: value.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            accepted: value.get("accepted").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn round(command: Command) -> Command {
        let (id, parsed) = parse_request(command.line(7).as_bytes()).unwrap();
        assert_eq!(id, 7);
        parsed
    }

    #[test]
    fn every_command_goes_over_the_wire_and_back() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("EMPTY".to_string(), String::new());
        let commands = [
            Command::Hello { protocol: PROTOCOL, app: "0.6.0".into(), token: TOKEN.into() },
            Command::Spawn {
                key: Some("pane-1".into()),
                meta: json!({ "provider": "claude" }),
                spec: SpawnSpec {
                    program: "/bin/claude".into(),
                    args: vec!["-p".into(), "안녕\nworld".into()],
                    cwd: "/tmp".into(),
                    env: env.clone(),
                    env_remove: vec!["GIT_DIR".into()],
                    shell_command: None,
                },
            },
            Command::Spawn {
                key: None,
                meta: Value::Null,
                spec: SpawnSpec { program: "/bin/sh".into(), cwd: "/".into(), ..Default::default() },
            },
            Command::PtyOpen {
                key: Some("pane-2".into()),
                meta: json!({}),
                spec: PtySpec { program: "/bin/zsh".into(), args: vec!["-l".into()], cwd: "/tmp".into(), env, columns: 120, rows: 40 },
            },
            Command::Attach { proc: "p1".into(), from: 12_345 },
            Command::Detach { proc: "p1".into() },
            Command::Write { proc: "p1".into(), data: vec![0, 1, 2, 0xEA, 0xB0, 0x80, b'\n'] },
            Command::CloseStdin { proc: "p1".into() },
            Command::Resize { proc: "p1".into(), columns: 80, rows: 24 },
            Command::Kill { proc: "p1".into() },
            Command::Busy { proc: "p1".into() },
            Command::Release { proc: "p1".into() },
            Command::Meta { proc: "p1".into(), meta: json!({ "resumeAt": 42 }) },
            Command::List,
            Command::Status,
            Command::Shutdown,
        ];
        for command in commands {
            assert_eq!(round(command.clone()), command);
        }
    }

    #[test]
    fn bad_requests_and_limits_are_refused() {
        let parse = |text: &str| parse_request(text.as_bytes()).map(|(_, c)| c);
        assert_eq!(parse("nope").unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"format c:"}"#).unwrap_err().message, "unknown command 'format c:'");
        assert_eq!(parse(r#"{"cmd":"hello"}"#).unwrap_err().message, "'protocol' is missing");
        assert_eq!(parse(r#"{"cmd":"attach","proc":"../x","from":0}"#).unwrap_err().message, "'proc' is not an id");
        assert_eq!(parse(r#"{"cmd":"attach","proc":"p","from":-1}"#).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"write","proc":"p","data":"!!"}"#).unwrap_err().message, "'data' is not base64");
        assert_eq!(parse(r#"{"cmd":"spawn","program":"/x","cwd":"/","env":{"A=B":"c"}}"#).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"spawn","program":"/x","cwd":"/","meta":[1]}"#).unwrap_err().code, ErrorCode::BadRequest);
        let big = Command::Write { proc: "p".into(), data: vec![0; MAX_CHUNK_BYTES + 1] };
        assert_eq!(parse_request(big.line(1).as_bytes()).unwrap_err().code, ErrorCode::TooLarge);
        let huge = vec![b' '; MAX_LINE_BYTES + 1];
        assert_eq!(parse_request(&huge).unwrap_err().code, ErrorCode::TooLarge);
        let meta = Command::Spawn {
            key: None,
            meta: json!({ "x": "y".repeat(MAX_META_BYTES) }),
            spec: SpawnSpec { program: "/x".into(), cwd: "/".into(), ..Default::default() },
        };
        assert_eq!(parse_request(meta.line(1).as_bytes()).unwrap_err().code, ErrorCode::TooLarge);
        let args = Command::Spawn {
            key: None,
            meta: Value::Null,
            spec: SpawnSpec { program: "/x".into(), cwd: "/".into(), args: vec!["a".into(); MAX_ARGS + 1], ..Default::default() },
        };
        assert_eq!(parse_request(args.line(1).as_bytes()).unwrap_err().code, ErrorCode::TooLarge);
        // A terminal's grid is clamped, as the platform clamps it.
        let pty = r#"{"cmd":"pty.open","program":"/bin/sh","cwd":"/","columns":99999,"rows":0}"#;
        let Command::PtyOpen { spec, .. } = parse(pty).unwrap() else { panic!() };
        assert_eq!((spec.columns, spec.rows), (1000, 1));
    }

    #[test]
    fn only_hello_status_and_shutdown_are_version_free() {
        assert!(Command::Status.version_free() && Command::Shutdown.version_free());
        assert!(Command::Hello { protocol: 9, app: String::new(), token: String::new() }.version_free());
        assert!(!Command::List.version_free());
        assert!(!Command::Kill { proc: "p".into() }.version_free());
        assert!(!Command::Attach { proc: "p".into(), from: 0 }.version_free());
    }

    #[test]
    fn events_and_listings_read_back() {
        let events = [
            Event::Output { proc: "p1".into(), offset: 42, stream: Stream::Err, data: "가\u{0}".as_bytes().to_vec() },
            Event::Replayed { proc: "p1".into(), offset: 50 },
            Event::Exit { proc: "p1".into(), code: Some(-9), error: None },
            Event::Exit { proc: "p1".into(), code: None, error: Some("gone".into()) },
        ];
        for event in events {
            let Reply::Event(value) = parse_reply(event.line().as_bytes()).unwrap() else { panic!() };
            assert_eq!(Event::parse(&value), Some(event.clone()));
            assert_eq!(event.proc(), "p1");
        }
        let info = ProcInfo {
            proc: "p1".into(),
            kind: Kind::Pty,
            key: Some("pane".into()),
            meta: json!({ "a": 1 }),
            pid: 77,
            running: true,
            code: None,
            end: 9,
            start: 2,
            started_at: 1_700_000_000_000,
            attached: 1,
        };
        assert_eq!(ProcInfo::parse(&info.to_json()), Some(info));
        let welcome = Welcome { protocol: PROTOCOL, app: "0.6.0".into(), pid: 5, accepted: true };
        assert_eq!(Welcome::parse(&welcome.to_json()), Some(welcome));
        assert!(is_id("a-b_c.d:1") && !is_id("") && !is_id("a/b") && !is_id(&"x".repeat(201)));
    }
}

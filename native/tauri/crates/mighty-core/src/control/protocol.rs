//! The local control API's wire (MC-9): newline-delimited JSON over the app's socket (the Mac) or named pipe (Windows).
//!
//! A request is one line: `{"v":1,"id":<n>,"token":"<64 hex>","cmd":"<command>", …its fields}`. Its answer is one line,
//! `{"id":<n>,"ok":true,"result":{…}}` or `{"id":<n>,"ok":false,"error":{"code":"…","message":"…"}}`; `events.subscribe` answers
//! `ok` and then writes `{"id":<n>,"event":{…}}` lines until the client goes. A line longer than [`MAX_LINE_BYTES`] is refused and
//! the connection closed; so is a request without the right token.

use std::time::Duration;

use serde_json::{Map, Value, json};

use super::events::WaitUntil;

pub const VERSION: u64 = 1;
/// The longest request line (an `agent.send`'s text is at most [`MAX_TEXT_BYTES`]; JSON escapes may double it).
pub const MAX_LINE_BYTES: usize = 256 * 1024;
/// The longest text `agent.send` takes (the deep link's limit).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The most lines `pane.read` gives, and how many when none is asked.
pub const MAX_READ_LINES: usize = 2_000;
pub const DEFAULT_READ_LINES: usize = 50;
/// The longest a wait may last.
pub const MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// The longest pattern `pane.waitOutput` takes.
pub const MAX_REGEX_CHARS: usize = 1_000;
/// A token is 64 lowercase hex digits.
pub const TOKEN_CHARS: usize = 64;

/// Why a request failed; its `code` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    NotFound,
    TooLarge,
    Timeout,
    Failed,
}

impl ErrorCode {
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::NotFound => "not_found",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> ErrorCode {
        match text {
            "bad_request" => ErrorCode::BadRequest,
            "unauthorized" => ErrorCode::Unauthorized,
            "not_found" => ErrorCode::NotFound,
            "too_large" => ErrorCode::TooLarge,
            "timeout" => ErrorCode::Timeout,
            _ => ErrorCode::Failed,
        }
    }
}

/// A failed request: its code and a short message (never a token, a prompt or a file's text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ControlError {
        ControlError { code, message: message.into() }
    }

    pub fn bad(message: impl Into<String>) -> ControlError {
        ControlError::new(ErrorCode::BadRequest, message)
    }

    pub fn not_found(what: &str) -> ControlError {
        ControlError::new(ErrorCode::NotFound, format!("{what} not found"))
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.name(), self.message)
    }
}

/// What a request asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status,
    WorkspaceList,
    PaneList {
        workspace: Option<String>,
    },
    PaneRead {
        pane: String,
        lines: usize,
    },
    AgentSend {
        pane: String,
        text: String,
    },
    AgentNew {
        workspace: String,
        agent: String,
        worktree: bool,
    },
    AgentWait {
        pane: String,
        until: WaitUntil,
        timeout: Option<Duration>,
    },
    PaneWaitOutput {
        pane: String,
        regex: String,
        timeout: Option<Duration>,
    },
    Events {
        pane: Option<String>,
        types: Vec<String>,
    },
    /// MC-3 headless server: how this app runs (started without a window or not, its window now, version, pid, uptime).
    ServerStatus,
    /// The app quits through its normal quit (the background daemon's work kept as its setting says; `stop_all` ends it too).
    ServerStop {
        stop_all: bool,
    },
    /// The app's window shown (made first when the app started without one).
    AppShow,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Status => "status",
            Command::WorkspaceList => "workspace.list",
            Command::PaneList { .. } => "pane.list",
            Command::PaneRead { .. } => "pane.read",
            Command::AgentSend { .. } => "agent.send",
            Command::AgentNew { .. } => "agent.new",
            Command::AgentWait { .. } => "agent.wait",
            Command::PaneWaitOutput { .. } => "pane.waitOutput",
            Command::Events { .. } => "events.subscribe",
            Command::ServerStatus => "server.status",
            Command::ServerStop { .. } => "server.stop",
            Command::AppShow => "app.show",
        }
    }

    /// The command's fields as the request line carries them.
    fn fields(&self, out: &mut Map<String, Value>) {
        let timeout = |out: &mut Map<String, Value>, t: &Option<Duration>| {
            if let Some(t) = t {
                out.insert("timeoutMs".into(), json!(t.as_millis() as u64));
            }
        };
        match self {
            Command::Status | Command::WorkspaceList | Command::ServerStatus | Command::AppShow => {}
            Command::ServerStop { stop_all } => {
                out.insert("stopAll".into(), json!(stop_all));
            }
            Command::PaneList { workspace } => {
                if let Some(w) = workspace {
                    out.insert("workspace".into(), json!(w));
                }
            }
            Command::PaneRead { pane, lines } => {
                out.insert("pane".into(), json!(pane));
                out.insert("lines".into(), json!(lines));
            }
            Command::AgentSend { pane, text } => {
                out.insert("pane".into(), json!(pane));
                out.insert("text".into(), json!(text));
            }
            Command::AgentNew { workspace, agent, worktree } => {
                out.insert("workspace".into(), json!(workspace));
                out.insert("agent".into(), json!(agent));
                out.insert("worktree".into(), json!(worktree));
            }
            Command::AgentWait { pane, until, timeout: t } => {
                out.insert("pane".into(), json!(pane));
                out.insert("until".into(), json!(until.name()));
                timeout(out, t);
            }
            Command::PaneWaitOutput { pane, regex, timeout: t } => {
                out.insert("pane".into(), json!(pane));
                out.insert("regex".into(), json!(regex));
                timeout(out, t);
            }
            Command::Events { pane, types } => {
                if let Some(p) = pane {
                    out.insert("pane".into(), json!(p));
                }
                if !types.is_empty() {
                    out.insert("types".into(), json!(types));
                }
            }
        }
    }
}

/// One request read off the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u64,
    pub token: String,
    pub command: Command,
}

impl Request {
    /// The request as one line (no newline).
    pub fn line(&self) -> String {
        let mut out = Map::new();
        out.insert("v".into(), json!(VERSION));
        out.insert("id".into(), json!(self.id));
        out.insert("token".into(), json!(self.token));
        out.insert("cmd".into(), json!(self.command.name()));
        self.command.fields(&mut out);
        Value::Object(out).to_string()
    }
}

/// Whether `given` is the token, compared in full whatever its length.
pub fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    !a.is_empty() && a.len() == b.len() && a.iter().zip(b).fold(0u8, |differ, (x, y)| differ | (x ^ y)) == 0
}

/// Whether `text` looks like a token (64 lowercase hex digits).
pub fn is_token(text: &str) -> bool {
    text.len() == TOKEN_CHARS && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn text_field(object: &Map<String, Value>, name: &str) -> Result<String, ControlError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ControlError::bad(format!("'{name}' is missing")))
}

fn optional_text(object: &Map<String, Value>, name: &str) -> Option<String> {
    object.get(name).and_then(Value::as_str).map(str::to_string).filter(|t| !t.trim().is_empty())
}

/// A pane or workspace id: what the app makes (letters, digits, `-`, `_`, `.`, `:`), at most 200 characters.
fn id_field(object: &Map<String, Value>, name: &str) -> Result<String, ControlError> {
    let id = text_field(object, name)?;
    if id.len() > 200 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')) {
        return Err(ControlError::bad(format!("'{name}' is not an id")));
    }
    Ok(id)
}

fn timeout_field(object: &Map<String, Value>) -> Result<Option<Duration>, ControlError> {
    match object.get("timeoutMs") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let ms = value.as_u64().ok_or_else(|| ControlError::bad("'timeoutMs' must be a whole number"))?;
            Ok(Some(Duration::from_millis(ms).min(MAX_WAIT)))
        }
    }
}

/// Reads one request line. The token is not checked here (the server does, against the file's).
pub fn parse_request(line: &[u8]) -> Result<Request, ControlError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ControlError::new(ErrorCode::TooLarge, "the request is too large"));
    }
    let value: Value = serde_json::from_slice(line).map_err(|_| ControlError::bad("the request is not JSON"))?;
    let object = value.as_object().ok_or_else(|| ControlError::bad("the request is not an object"))?;
    let id = object.get("id").and_then(Value::as_u64).unwrap_or(0);
    if let Some(version) = object.get("v").and_then(Value::as_u64)
        && version != VERSION
    {
        return Err(ControlError::bad(format!("protocol version {version} is not supported")));
    }
    let token = object.get("token").and_then(Value::as_str).unwrap_or("").to_string();
    let cmd = object.get("cmd").and_then(Value::as_str).ok_or_else(|| ControlError::bad("'cmd' is missing"))?;
    let command = match cmd {
        "status" => Command::Status,
        "workspace.list" => Command::WorkspaceList,
        "pane.list" => Command::PaneList {
            workspace: match optional_text(object, "workspace") {
                Some(_) => Some(id_field(object, "workspace")?),
                None => None,
            },
        },
        "pane.read" => {
            let lines = match object.get("lines") {
                None | Some(Value::Null) => DEFAULT_READ_LINES,
                Some(v) => v.as_u64().ok_or_else(|| ControlError::bad("'lines' must be a whole number"))? as usize,
            };
            Command::PaneRead { pane: id_field(object, "pane")?, lines: lines.clamp(1, MAX_READ_LINES) }
        }
        "agent.send" => {
            let text = text_field(object, "text")?;
            if text.len() > MAX_TEXT_BYTES {
                return Err(ControlError::new(ErrorCode::TooLarge, "the text is longer than 64 KB"));
            }
            Command::AgentSend { pane: id_field(object, "pane")?, text }
        }
        "agent.new" => Command::AgentNew {
            workspace: id_field(object, "workspace")?,
            agent: optional_text(object, "agent").unwrap_or_else(|| "claude".into()),
            worktree: object.get("worktree").and_then(Value::as_bool).unwrap_or(false),
        },
        "agent.wait" => {
            let until = text_field(object, "until")?;
            Command::AgentWait {
                pane: id_field(object, "pane")?,
                until: WaitUntil::parse(&until).ok_or_else(|| ControlError::bad("'until' must be idle, blocked or done"))?,
                timeout: timeout_field(object)?,
            }
        }
        "pane.waitOutput" => {
            let regex = text_field(object, "regex")?;
            if regex.chars().count() > MAX_REGEX_CHARS {
                return Err(ControlError::new(ErrorCode::TooLarge, "the pattern is longer than 1,000 characters"));
            }
            Command::PaneWaitOutput { pane: id_field(object, "pane")?, regex, timeout: timeout_field(object)? }
        }
        "events.subscribe" => {
            let types = match object.get("types") {
                None | Some(Value::Null) => vec![],
                Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                Some(_) => return Err(ControlError::bad("'types' must be a list")),
            };
            if let Some(unknown) = types.iter().find(|t| !super::events::ControlEvent::TYPES.contains(&t.as_str())) {
                return Err(ControlError::bad(format!("unknown event type '{unknown}'")));
            }
            Command::Events {
                pane: match optional_text(object, "pane") {
                    Some(_) => Some(id_field(object, "pane")?),
                    None => None,
                },
                types,
            }
        }
        "server.status" => Command::ServerStatus,
        "server.stop" => Command::ServerStop { stop_all: object.get("stopAll").and_then(Value::as_bool).unwrap_or(false) },
        "app.show" => Command::AppShow,
        other => return Err(ControlError::bad(format!("unknown command '{}'", other.chars().take(40).collect::<String>()))),
    };
    Ok(Request { id, token, command })
}

/// The `ok` answer line (no newline).
pub fn ok_line(id: u64, result: Value) -> String {
    json!({ "id": id, "ok": true, "result": result }).to_string()
}

/// The error answer line (no newline).
pub fn error_line(id: u64, error: &ControlError) -> String {
    json!({ "id": id, "ok": false, "error": { "code": error.code.name(), "message": error.message } }).to_string()
}

/// One event line of a subscription (no newline).
pub fn event_line(id: u64, event: Value) -> String {
    json!({ "id": id, "event": event }).to_string()
}

/// An answer line as the client reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Ok(Value),
    Error(ControlError),
    Event(Value),
}

pub fn parse_reply(line: &[u8]) -> Result<Reply, ControlError> {
    let value: Value = serde_json::from_slice(line).map_err(|_| ControlError::new(ErrorCode::Failed, "the app's answer is not JSON"))?;
    if let Some(event) = value.get("event") {
        return Ok(Reply::Event(event.clone()));
    }
    match value.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(Reply::Ok(value.get("result").cloned().unwrap_or(Value::Null))),
        Some(false) => {
            let error = value.get("error");
            let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).map_or(ErrorCode::Failed, ErrorCode::parse);
            let message = error.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("").to_string();
            Ok(Reply::Error(ControlError { code, message }))
        }
        None => Err(ControlError::new(ErrorCode::Failed, "the app's answer has no 'ok'")),
    }
}

/// Splits bytes read off a connection into lines, each at most `max` bytes (its `\r\n` or `\n` dropped).
#[derive(Debug)]
pub struct LineBuffer {
    pending: Vec<u8>,
    max: usize,
}

impl LineBuffer {
    pub fn new(max: usize) -> LineBuffer {
        LineBuffer { pending: Vec::new(), max }
    }

    /// Takes `bytes` and gives the lines they complete; Err once a line (complete or not) is longer than the limit.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ControlError> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.pending);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if !line.iter().all(u8::is_ascii_whitespace) {
                    lines.push(line);
                }
                continue;
            }
            if self.pending.len() >= self.max {
                return Err(ControlError::new(ErrorCode::TooLarge, "the line is too long"));
            }
            self.pending.push(byte);
        }
        Ok(lines)
    }

    /// Whether part of a line waits for its end.
    pub fn has_partial(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn round(command: Command) -> Command {
        let line = Request { id: 3, token: TOKEN.into(), command }.line();
        let request = parse_request(line.as_bytes()).unwrap();
        assert_eq!((request.id, request.token.as_str()), (3, TOKEN));
        request.command
    }

    #[test]
    fn every_command_goes_over_the_wire_and_back() {
        let commands = [
            Command::Status,
            Command::WorkspaceList,
            Command::PaneList { workspace: None },
            Command::PaneList { workspace: Some("w-1".into()) },
            Command::PaneRead { pane: "p-1".into(), lines: 20 },
            Command::AgentSend { pane: "p-1".into(), text: "run the tests\nthen report".into() },
            Command::AgentNew { workspace: "w-1".into(), agent: "codex".into(), worktree: true },
            Command::AgentWait { pane: "p-1".into(), until: WaitUntil::Blocked, timeout: Some(Duration::from_millis(1500)) },
            Command::AgentWait { pane: "p-1".into(), until: WaitUntil::Done, timeout: None },
            Command::PaneWaitOutput { pane: "p-1".into(), regex: "^ok\\b".into(), timeout: Some(Duration::from_secs(2)) },
            Command::Events { pane: Some("p-1".into()), types: vec!["pane.state".into()] },
            Command::Events { pane: None, types: vec![] },
            Command::ServerStatus,
            Command::ServerStop { stop_all: false },
            Command::ServerStop { stop_all: true },
            Command::AppShow,
        ];
        for command in commands {
            assert_eq!(round(command.clone()), command);
        }
    }

    #[test]
    fn bad_requests_are_refused_with_their_reason() {
        let parse = |text: &str| parse_request(text.as_bytes()).map(|r| r.command);
        assert_eq!(parse("not json").unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse("[1]").unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"token":"x"}"#).unwrap_err().message, "'cmd' is missing");
        assert_eq!(parse(r#"{"cmd":"rm -rf"}"#).unwrap_err().message, "unknown command 'rm -rf'");
        assert_eq!(parse(r#"{"v":2,"cmd":"status"}"#).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"pane.read"}"#).unwrap_err().message, "'pane' is missing");
        assert_eq!(parse(r#"{"cmd":"pane.read","pane":"../x"}"#).unwrap_err().message, "'pane' is not an id");
        assert_eq!(parse(r#"{"cmd":"agent.wait","pane":"p","until":"soon"}"#).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"events.subscribe","types":["secrets"]}"#).unwrap_err().code, ErrorCode::BadRequest);
        assert_eq!(parse(r#"{"cmd":"agent.wait","pane":"p","until":"idle","timeoutMs":-1}"#).unwrap_err().code, ErrorCode::BadRequest);
        // A stop without `stopAll` keeps what the daemon setting keeps.
        assert_eq!(parse(r#"{"cmd":"server.stop"}"#).unwrap(), Command::ServerStop { stop_all: false });
        assert_eq!(parse(r#"{"cmd":"server.stop","stopAll":"yes"}"#).unwrap(), Command::ServerStop { stop_all: false });
    }

    #[test]
    fn limits_hold() {
        let long_text = "a".repeat(MAX_TEXT_BYTES + 1);
        let line = Request { id: 1, token: TOKEN.into(), command: Command::AgentSend { pane: "p".into(), text: long_text } }.line();
        assert_eq!(parse_request(line.as_bytes()).unwrap_err().code, ErrorCode::TooLarge);
        let huge = vec![b' '; MAX_LINE_BYTES + 1];
        assert_eq!(parse_request(&huge).unwrap_err().code, ErrorCode::TooLarge);
        let pattern = "x".repeat(MAX_REGEX_CHARS + 1);
        let line =
            Request { id: 1, token: TOKEN.into(), command: Command::PaneWaitOutput { pane: "p".into(), regex: pattern, timeout: None } }
                .line();
        assert_eq!(parse_request(line.as_bytes()).unwrap_err().code, ErrorCode::TooLarge);
        assert_eq!(
            round(Command::PaneRead { pane: "p".into(), lines: 1_000_000 }),
            Command::PaneRead { pane: "p".into(), lines: MAX_READ_LINES }
        );
        assert_eq!(round(Command::PaneRead { pane: "p".into(), lines: 0 }), Command::PaneRead { pane: "p".into(), lines: 1 });
        let wait = round(Command::AgentWait { pane: "p".into(), until: WaitUntil::Idle, timeout: Some(Duration::from_secs(10 * 86_400)) });
        assert_eq!(wait, Command::AgentWait { pane: "p".into(), until: WaitUntil::Idle, timeout: Some(MAX_WAIT) });
    }

    #[test]
    fn the_line_buffer_splits_lines_and_caps_them() {
        let mut buffer = LineBuffer::new(8);
        assert_eq!(buffer.push(b"abc").unwrap(), Vec::<Vec<u8>>::new());
        assert!(buffer.has_partial());
        assert_eq!(buffer.push(b"d\r\n\nxy\n").unwrap(), vec![b"abcd".to_vec(), b"xy".to_vec()]);
        assert!(!buffer.has_partial());
        assert_eq!(buffer.push(b"123456789").unwrap_err().code, ErrorCode::TooLarge);
    }

    #[test]
    fn tokens_compare_in_full() {
        assert!(token_matches(TOKEN, TOKEN));
        assert!(!token_matches(TOKEN, &TOKEN[..63]));
        assert!(!token_matches(TOKEN, &format!("{}0", &TOKEN[..63])));
        assert!(!token_matches("", ""));
        assert!(is_token(TOKEN) && !is_token(&TOKEN.to_uppercase()) && !is_token("abc"));
    }

    #[test]
    fn replies_read_back() {
        assert_eq!(parse_reply(ok_line(1, json!({"a": 1})).as_bytes()).unwrap(), Reply::Ok(json!({"a": 1})));
        let error = ControlError::new(ErrorCode::Timeout, "timed out");
        assert_eq!(parse_reply(error_line(2, &error).as_bytes()).unwrap(), Reply::Error(error));
        assert_eq!(
            parse_reply(event_line(3, json!({"type": "attention"})).as_bytes()).unwrap(),
            Reply::Event(json!({"type": "attention"}))
        );
        assert!(parse_reply(b"{}").is_err());
    }
}

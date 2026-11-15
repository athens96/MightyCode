//! The help capture's canned CLI (C# `WinUI\HelpDemoCli.cs`; plan §1.8 "Help capture", W9-H): the app's own binary in another mode,
//! `--help-demo-cli <script.json> <CLI arguments…>`, started through the real run path in the `claude` and `codex` CLIs' place by the help
//! capture (src-tauri help_capture.rs) and the perf scenario (perf.rs). Served by main.rs's `exe_mode` before anything else, beside
//! [`crate::fake_claude`]. Owner: W9-H.
//!
//! Nothing runs and nothing goes on the network but the run's own loopback mod bridge: a run prints the demo profile's canned stream lines
//! for the request it was given, so the panes show what a real run would. The script is `{"runs": [{"provider", "input", "frames",
//! "marker"}]}`; the run whose input the prompt holds plays (the last such entry wins), and its `marker` file is written once every line is
//! out. A Claude run answers the host's `initialize`, says `session.start` to the run's mod bridge, then waits for its input to close, as
//! the CLI does while the host may still answer it; a Codex run (`exec --json`) reads its prompt to the end and leaves at once.
//! `--version` gives `2.1.271 (Claude Code)`; `--no-session-persistence` (the app's model-list probe) exits 0 at once.

use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::Duration;

use mighty_core::json::JsonOut;
use serde_json::{Value, json};

/// The mode's first argument (C# `HelpDemoCli.Argument`).
pub const ARGUMENT: &str = "--help-demo-cli";
/// The CLI version it says it is (C# `Console.WriteLine("2.1.271 (Claude Code)")`).
pub const VERSION: &str = "2.1.271";
/// The pause after each canned line (C# `Thread.Sleep(15)`), so the host reads them as a stream.
const FRAME_GAP: Duration = Duration::from_millis(15);
/// The mod bridge's `session.start` gives up after this (C# `HttpClient.Timeout`).
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Serves the mode when `args` start with it and an absolute script path, with the exit code; None otherwise (the arguments then reach the
/// normal parser, which refuses them).
pub fn try_run(args: &[String]) -> Option<i32> {
    let [first, script, rest @ ..] = args else { return None };
    if first != ARGUMENT || !mighty_core::normalize::fully_qualified(script) {
        return None;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    Some(run(Path::new(script), rest, stdin.lock(), stdout.lock(), &Bridge::from_environment()))
}

/// The run's mod bridge as the host handed it in the environment (`MIGHTY_CLAUDE_BRIDGE_URL`, `_TOKEN`, `MIGHTY_CLAUDE_RUN_ID`).
#[derive(Default)]
pub struct Bridge {
    url: Option<String>,
    token: Option<String>,
    run_id: Option<String>,
}

impl std::fmt::Debug for Bridge {
    /// The token never shows.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Bridge").field("url", &self.url).field("run_id", &self.run_id).finish_non_exhaustive()
    }
}

impl Bridge {
    pub fn from_environment() -> Bridge {
        let read = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Bridge { url: read("MIGHTY_CLAUDE_BRIDGE_URL"), token: read("MIGHTY_CLAUDE_BRIDGE_TOKEN"), run_id: read("MIGHTY_CLAUDE_RUN_ID") }
    }

    /// C# `SayStarted`: one `session.start` to the run's mod bridge, as the real CLI's mod sends, so the run does not report its mod silent.
    /// The session is the run's first canned `session_id`, else a new one. Errors are let go.
    fn say_started(&self, run: &Value) {
        let (Some(url), Some(token), Some(id)) = (&self.url, &self.token, &self.run_id) else { return };
        let session = frames(run)
            .iter()
            .find_map(|frame| frame.get("session_id").and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let body = text(&json!({"version": 1, "runId": id, "claudeSessionId": session, "event": "session.start"}));
        let _ = post(url, token, &body);
    }
}

/// A POST of `body` (JSON) to the loopback `http` address `url` (refused unless it resolves to loopback), the bearer `token` with it; the
/// response's head is read and let go.
fn post(url: &str, token: &str, body: &str) -> std::io::Result<()> {
    let invalid = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, why.to_string());
    let address = url::Url::parse(url).map_err(|_| invalid("not an address"))?;
    if address.scheme() != "http" {
        return Err(invalid("the bridge is plain http on loopback"));
    }
    let host = address.host_str().ok_or_else(|| invalid("no host"))?;
    let port = address.port_or_known_default().ok_or_else(|| invalid("no port"))?;
    let target = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?.next().ok_or_else(|| invalid("no address"))?;
    // The run's mod bridge listens on loopback only; nothing else is ever sent the token.
    if !target.ip().is_loopback() {
        return Err(invalid("the bridge is not on loopback"));
    }
    let mut stream = std::net::TcpStream::connect_timeout(&target, BRIDGE_TIMEOUT)?;
    stream.set_read_timeout(Some(BRIDGE_TIMEOUT))?;
    stream.set_write_timeout(Some(BRIDGE_TIMEOUT))?;
    let path = match address.query() {
        Some(query) => format!("{}?{query}", address.path()),
        None => address.path().to_string(),
    };
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    // The status line and headers only (at most 8 KiB): the bridge answers 204 with no body.
    let mut answer = Vec::new();
    let mut buffer = [0u8; 1024];
    while answer.len() < 8192 && !answer.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => answer.extend_from_slice(&buffer[..read]),
        }
    }
    Ok(())
}

/// A JSON value as `JsonNode.ToJsonString()` writes it (the default encoder), as C# prints each canned line.
fn text(value: &Value) -> String {
    let mut out = JsonOut::new();
    out.value(value);
    out.finish()
}

fn frames(run: &Value) -> &[Value] {
    run.get("frames").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// C# `Find`: the last run of `provider` whose (non-empty) input the prompt holds.
fn find<'a>(runs: &'a [Value], provider: &str, prompt: &str) -> Option<&'a Value> {
    runs.iter().rev().find(|run| {
        run.get("provider").and_then(Value::as_str) == Some(provider)
            && run.get("input").and_then(Value::as_str).is_some_and(|input| !input.is_empty() && prompt.contains(input))
    })
}

/// C# `Play`: every canned line, a short pause after each, then the run's marker file (empty).
fn play(run: &Value, output: &mut impl Write) {
    let line_end = mighty_run::os::new_line();
    for frame in frames(run) {
        let _ = write!(output, "{}{line_end}", text(frame));
        let _ = output.flush();
        std::thread::sleep(FRAME_GAP);
    }
    if let Some(marker) = run.get("marker").and_then(Value::as_str).filter(|m| !m.is_empty()) {
        let _ = std::fs::write(marker, "");
    }
}

/// A user message's prompt: its content as a string, or its parts' texts one a line (C#'s `string.Join("\n", …)`).
fn prompt_of(message: &Value) -> String {
    match message.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            parts.iter().map(|part| part.get("text").and_then(Value::as_str).unwrap_or("")).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

/// C# `HelpDemoCli.Run(scriptPath, arguments)` over the given streams: the exit code (0, whatever the script held).
pub fn run(script: &Path, arguments: &[String], mut input: impl BufRead, mut output: impl Write, bridge: &Bridge) -> i32 {
    let has = |flag: &str| arguments.iter().any(|a| a == flag);
    if has("--version") {
        let _ = write!(output, "{VERSION} (Claude Code){}", mighty_run::os::new_line());
        let _ = output.flush();
        return 0;
    }
    // The app's model-list probe: leaving at once makes it use its built-in list.
    if has("--no-session-persistence") {
        return 0;
    }
    let script: Value = std::fs::read(script).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null);
    let runs = script.get("runs").and_then(Value::as_array).cloned().unwrap_or_default();
    if has("exec") {
        let mut prompt = String::new();
        let _ = input.read_to_string(&mut prompt);
        if let Some(run) = find(&runs, "codex", &prompt) {
            play(run, &mut output);
        }
        return 0;
    }
    let mut played = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_slice::<Value>(&line) else { continue };
        let kind = message.get("type").and_then(Value::as_str);
        if kind == Some("control_request")
            && let Some(id) = message.get("request_id").and_then(Value::as_str)
            && message.get("request").and_then(|r| r.get("subtype")).and_then(Value::as_str) == Some("initialize")
        {
            let answer = json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                "response": {"commands": [], "models": []}}});
            let _ = write!(output, "{}{}", text(&answer), mighty_run::os::new_line());
            let _ = output.flush();
            continue;
        }
        if kind != Some("user") || played {
            continue;
        }
        let Some(run) = find(&runs, "claude", &prompt_of(&message)) else { continue };
        played = true;
        bridge.say_started(run);
        play(run, &mut output);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mighty-help-demo-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    /// The script the capture writes: two Claude runs (the later one wins a prompt both match) and a Codex run, each with its marker.
    fn script(dir: &Path) -> std::path::PathBuf {
        let marker = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let runs = json!({"runs": [
            {"provider": "claude", "input": "fix the bug", "frames": [{"type": "system", "session_id": "s-1"}, {"type": "result", "result": "old"}], "marker": marker("old.done")},
            {"provider": "claude", "input": "fix the bug", "frames": [{"type": "system", "session_id": "s-2"}, {"type": "assistant", "text": "고침"}], "marker": marker("claude.done")},
            {"provider": "codex", "input": "review it", "frames": [{"type": "thread.started", "thread_id": "t-1"}], "marker": marker("codex.done")},
            {"provider": "claude", "input": "", "frames": [{"type": "never"}], "marker": marker("empty.done")},
        ]});
        let path = dir.join("script.json");
        std::fs::write(&path, runs.to_string()).unwrap();
        path
    }

    fn lines(output: &[u8]) -> Vec<Value> {
        String::from_utf8(output.to_vec()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn version_and_the_model_list_probe_answer_at_once() {
        let dir = folder("version");
        let mut out = Vec::new();
        assert_eq!(run(&dir.join("none.json"), &arguments(&["--version"]), &b""[..], &mut out, &Bridge::default()), 0);
        assert_eq!(String::from_utf8(out).unwrap(), format!("2.1.271 (Claude Code){}", mighty_run::os::new_line()));
        let mut out = Vec::new();
        assert_eq!(run(&dir.join("none.json"), &arguments(&["-p", "--no-session-persistence"]), &b""[..], &mut out, &Bridge::default()), 0);
        assert!(out.is_empty(), "the probe gets nothing and uses the built-in list");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// C#'s Claude half: `initialize` answered, the first user message whose prompt holds a run's input plays the last such run and writes
    /// its marker; a later message plays nothing more; the CLI leaves once its input closes.
    #[test]
    fn a_claude_run_answers_initialize_and_plays_the_matching_run_once() {
        let dir = folder("claude");
        let input = [
            json!({"type": "control_request", "request_id": "init-1", "request": {"subtype": "initialize"}}).to_string(),
            "not json".to_string(),
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "Please"}, {"type": "text", "text": "fix the bug now"}]}}).to_string(),
            json!({"type": "user", "message": {"role": "user", "content": "fix the bug again"}}).to_string(),
        ]
        .join("\n");
        let mut out = Vec::new();
        assert_eq!(
            run(&script(&dir), &arguments(&["-p", "--output-format", "stream-json"]), input.as_bytes(), &mut out, &Bridge::default()),
            0
        );
        let printed = lines(&out);
        assert_eq!(
            printed[0],
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "init-1", "response": {"commands": [], "models": []}}})
        );
        assert_eq!(printed[1..], [json!({"type": "system", "session_id": "s-2"}), json!({"type": "assistant", "text": "고침"})]);
        assert!(dir.join("claude.done").exists(), "the played run's marker is written");
        assert!(!dir.join("old.done").exists() && !dir.join("empty.done").exists(), "only the last matching run plays, and only once");
        // The line as JsonNode.ToJsonString() writes it: non-ASCII escaped.
        assert!(String::from_utf8(out).unwrap().contains("\\uACE0\\uCE68"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_codex_exec_run_reads_its_prompt_to_the_end_and_plays() {
        let dir = folder("codex");
        let mut out = Vec::new();
        let prompt = "Context first.\nreview it please\n";
        assert_eq!(run(&script(&dir), &arguments(&["exec", "--json", "-"]), prompt.as_bytes(), &mut out, &Bridge::default()), 0);
        assert_eq!(lines(&out), [json!({"type": "thread.started", "thread_id": "t-1"})]);
        assert!(dir.join("codex.done").exists());
        let mut out = Vec::new();
        assert_eq!(run(&script(&dir), &arguments(&["exec", "--json"]), &b"nothing it knows"[..], &mut out, &Bridge::default()), 0);
        assert!(out.is_empty(), "a prompt no run matches plays nothing");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A missing or unreadable script plays nothing and still exits 0, as C#'s empty `runs` does.
    #[test]
    fn a_missing_script_plays_nothing() {
        let dir = folder("missing");
        let mut out = Vec::new();
        let input = json!({"type": "user", "message": {"content": "fix the bug"}}).to_string();
        assert_eq!(run(&dir.join("nothing.json"), &[], input.as_bytes(), &mut out, &Bridge::default()), 0);
        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The mode is served only for `--help-demo-cli <absolute script>`; anything else reaches the parser.
    #[test]
    fn only_the_mode_with_an_absolute_script_is_served() {
        assert_eq!(try_run(&arguments(&["--help-demo-cli"])), None);
        assert_eq!(try_run(&arguments(&["--help-demo-cli", "relative.json", "--version"])), None);
        assert_eq!(try_run(&arguments(&["--fake-claude", "/x"])), None);
        assert_eq!(try_run(&arguments(&[])), None);
    }

    /// `session.start` reaches a loopback bridge as the real mod sends it: POST, bearer token, JSON naming the run and the canned session.
    #[test]
    fn session_start_reaches_the_bridge() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/events", listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut seen = Vec::new();
            let mut buffer = [0u8; 4096];
            while !String::from_utf8_lossy(&seen).contains("\"event\"") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                seen.extend_from_slice(&buffer[..read]);
            }
            stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").unwrap();
            String::from_utf8(seen).unwrap()
        });
        let bridge = Bridge { url: Some(url), token: Some("t".repeat(64)), run_id: Some("run-1".into()) };
        bridge.say_started(&json!({"frames": [{"type": "system"}, {"type": "system", "session_id": "s-9"}]}));
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /events HTTP/1.1\r\n"), "{request}");
        assert!(request.contains(&format!("Authorization: Bearer {}\r\n", "t".repeat(64))));
        assert!(request.contains("Content-Type: application/json"));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"version": 1, "runId": "run-1", "claudeSessionId": "s-9", "event": "session.start"}));
        assert!(!format!("{bridge:?}").contains("tttt"), "the token never shows in Debug");
    }

    /// The token goes to loopback only: an address elsewhere is refused before anything connects.
    #[test]
    fn the_bridge_is_reached_on_loopback_only() {
        let refused = post("http://192.0.2.1:9/events", "secret", "{}").unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
        assert!(post("https://127.0.0.1:9/events", "secret", "{}").is_err(), "plain http only");
    }
}

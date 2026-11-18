//! The scripted Claude (C# Core/DelegationFakeClaude.cs; M/AppStore+DelegationSmoke.swift:7-12, the Mac's Tests/FakeClaude): the app's
//! own exe in a second mode, `--fake-claude <absolute root> <the CLI's arguments>`, which the delegation smoke and the scenarios start in
//! the CLI's place. Kept in the shipping exe because docs/tauri-conversion.md section 1 lists the mode. Owner: W7-C.
//!
//! There is no model: what the "agent" does is read from a script. It speaks the CLI's stream-json, answers the host's `initialize`,
//! asks for permissions the CLI's way (`can_use_tool`), reaches the delegation MCP server named in the app-written `--mcp-config` with
//! the environment it inherited (so the pane token never leaves the environment), and writes its transcript where Claude Code does.
//!
//! Under `<root>`, keyed by the Claude project folder name of the folder it runs in: `scripts/<key>.json` `{"turns": [[step]]}` is
//! what a run in that folder plays, the n-th turn for the n-th prompt; without one, `child-scripts.json` `[{"phrase", "steps"}]` gives
//! the first turn of a run whose first prompt holds the phrase (the last such entry wins). `logs/<key>.jsonl` is what it saw and did;
//! `claude-config/projects/<key>/<session>.jsonl` its transcript.
//!
//! Steps: `{"say"}`; `{"call", "arguments", "server"?, "ask"?}` a tools/call, asked for first when `ask`; `{"permission", "input"}` a
//! built-in tool use, asked for; `{"run": [[argv]], "ask"?}` a Bash tool use running each command in the working folder (`git` is the
//! git delegation uses, with fixture settings); `{"write", "text"}` a file under the working folder; `{"sleep"}` seconds; `{"exit"}`
//! leave at once with this code. `--version` gives `2.1.271 (Claude Code)`; `--no-session-persistence` (the model-list probe) exits 0.
//!
//! Every child (the MCP server, a `run` step's commands) starts through the given `ProcessHost`: it ships in the exe, so no
//! `Command::new`, and its children go with the job its own process is in.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::json::JsonOut;
use mighty_platform::process::{ChildReader, ChildWriter};
use mighty_platform::{Child, ProcessHost, SpawnRequest};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::mcp::delegation::SERVER_NAME as DELEGATION_SERVER;

/// `DelegationFakeClaude.Argument`.
pub const ARGUMENT: &str = "--fake-claude";
/// The CLI version it says it is.
pub const VERSION: &str = "2.1.271";

/// `DelegationFakeClaude.RunAsync(root, args)`: `root` is the fully qualified folder after [`ARGUMENT`], `args` the CLI's arguments
/// after it; the process's exit code.
pub fn run(root: &Path, args: &[String], processes: Arc<dyn ProcessHost>) -> i32 {
    if args.iter().any(|a| a == "--version") {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "{VERSION} (Claude Code){}", mighty_run::os::new_line());
        let _ = out.flush();
        return 0;
    }
    // The app's model-list probe: leaving at once makes it use its built-in list.
    if args.iter().any(|a| a == "--no-session-persistence") {
        return 0;
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return 1 };
    runtime.block_on(async {
        let mut fake = FakeClaude::new(root, args, processes, stdin_messages(), Arc::new(std::sync::Mutex::new(std::io::stdout())));
        fake.run().await
    })
}

/// `SessionHistory.ClaudeProjectFolder` (mighty-history's `claude_project_folder`): every character but an ASCII letter or digit
/// made `-`.
fn project_folder(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// A JSON value as `JsonNode.ToJsonString()` writes it (the default encoder).
fn text(value: &Value) -> String {
    let mut out = JsonOut::new();
    out.value(value);
    out.finish()
}

/// Stdin's JSON objects, one a line, read on a thread of their own until it closes.
fn stdin_messages() -> mpsc::UnboundedReceiver<Map<String, Value>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("fake-claude-stdin".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            let mut line = Vec::new();
            loop {
                line.clear();
                match input.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&line);
                if let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text.trim_end_matches(['\r', '\n']))
                    && sender.send(message).is_err()
                {
                    break;
                }
            }
        })
        .expect("the fake's stdin reader");
    receiver
}

type Output = Arc<std::sync::Mutex<dyn Write + Send>>;

/// `DelegationFakeClaude`.
pub struct FakeClaude {
    root: PathBuf,
    cwd: String,
    key: String,
    session_id: String,
    arguments: Vec<String>,
    processes: Arc<dyn ProcessHost>,
    output: Output,
    incoming: mpsc::UnboundedReceiver<Map<String, Value>>,
    waiting: VecDeque<Map<String, Value>>,
    servers: Vec<(String, Connection)>,
    configured: Vec<String>,
    last_uuid: Option<String>,
    counter: u64,
    started: bool,
}

impl FakeClaude {
    /// A fake in the current folder reading `incoming` and writing `output`.
    pub fn new(
        root: &Path,
        arguments: &[String],
        processes: Arc<dyn ProcessHost>,
        incoming: mpsc::UnboundedReceiver<Map<String, Value>>,
        output: Output,
    ) -> FakeClaude {
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let value = |option: &str| arguments.iter().position(|a| a == option).and_then(|i| arguments.get(i + 1)).cloned();
        FakeClaude {
            root: root.to_path_buf(),
            key: project_folder(&cwd),
            cwd,
            session_id: value("--resume").unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            arguments: arguments.to_vec(),
            processes,
            output,
            incoming,
            waiting: VecDeque::new(),
            servers: vec![],
            configured: vec![],
            last_uuid: None,
            counter: 0,
            started: false,
        }
    }

    fn value(&self, option: &str) -> Option<&str> {
        let index = self.arguments.iter().position(|a| a == option)?;
        self.arguments.get(index + 1).map(String::as_str)
    }

    /// `Run`: plays the script turn by turn until stdin closes or a step exits; the exit code.
    pub async fn run(&mut self) -> i32 {
        self.log(json!({"event": "argv", "arguments": self.arguments}));
        self.connect_servers().await;
        let mut turn = 0;
        let mut turns: Option<Vec<Vec<Map<String, Value>>>> = None;
        while let Some(message) = self.next(|_| true).await {
            match message.get("type").and_then(Value::as_str) {
                Some("control_request") => {
                    let Some(id) = message.get("request_id").and_then(Value::as_str).map(str::to_string) else { continue };
                    if message.get("request").and_then(|r| r.get("subtype")).and_then(Value::as_str) == Some("initialize") {
                        self.emit(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"commands": [], "models": []}}}));
                        self.log(json!({"event": "initialize"}));
                    } else {
                        self.emit(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "Unsupported control request."}}));
                    }
                }
                Some("user") => {
                    let prompt = message.get("message").and_then(|m| m.get("content")).cloned().unwrap_or(json!(""));
                    if turns.is_none() {
                        turns = Some(self.script(&text_of(&prompt)));
                    }
                    let steps = turns.as_ref().and_then(|t| t.get(turn)).cloned().unwrap_or_default();
                    turn += 1;
                    if let Some(code) = self.play(steps, &prompt).await {
                        return self.finish(code).await;
                    }
                }
                _ => {}
            }
        }
        self.finish(0).await
    }

    /// `Next`: the first message `matching` takes, waiting for one; None once stdin closed with none left.
    async fn next(&mut self, matching: impl Fn(&Map<String, Value>) -> bool) -> Option<Map<String, Value>> {
        if let Some(index) = self.waiting.iter().position(&matching) {
            return self.waiting.remove(index);
        }
        while let Some(message) = self.incoming.recv().await {
            if matching(&message) {
                return Some(message);
            }
            self.waiting.push_back(message);
        }
        None
    }

    /// `Script(prompt)`: this folder's turns, else the first turn of the last child script whose phrase the prompt holds.
    fn script(&self, prompt: &str) -> Vec<Vec<Map<String, Value>>> {
        fn turns(value: Option<&Value>) -> Vec<Vec<Map<String, Value>>> {
            match value {
                Some(Value::Array(list)) => list
                    .iter()
                    .map(|turn| match turn {
                        Value::Array(steps) => steps.iter().filter_map(|s| s.as_object().cloned()).collect(),
                        _ => vec![],
                    })
                    .collect(),
                _ => vec![],
            }
        }
        let own = self.root.join("scripts").join(format!("{}.json", self.key));
        if own.is_file() {
            return turns(read_json(&own).as_ref().and_then(|v| v.get("turns")));
        }
        let Some(Value::Array(entries)) = read_json(&self.root.join("child-scripts.json")) else { return vec![] };
        let found = entries
            .iter()
            .filter_map(Value::as_object)
            .rfind(|entry| prompt.contains(entry.get("phrase").and_then(Value::as_str).unwrap_or("\0")));
        match found {
            Some(entry) => {
                let steps = entry.get("steps").cloned().unwrap_or(Value::Null);
                turns(Some(&Value::Array(vec![steps]))).into_iter().take(1).collect()
            }
            None => vec![],
        }
    }

    /// `ConnectServers`: the delegation server of `--mcp-config` (inline JSON or a file), started and initialized.
    async fn connect_servers(&mut self) {
        let Some(value) = self.value("--mcp-config").map(str::to_string) else { return };
        let config = if value.starts_with('{') { serde_json::from_str(&value).ok() } else { read_json(Path::new(&value)) };
        let Some(Value::Object(entries)) = config.and_then(|c| c.get("mcpServers").cloned()) else { return };
        let mut names: Vec<String> = entries.keys().cloned().collect();
        names.sort();
        self.configured = names;
        // The delegation server is the one these scripts call.
        let Some(Value::Object(entry)) = entries.get(DELEGATION_SERVER) else { return };
        let Some(command) = entry.get("command").and_then(Value::as_str) else { return };
        let arguments: Vec<String> = match entry.get("args") {
            Some(Value::Array(list)) => list.iter().map(|item| item.as_str().unwrap_or("").to_string()).collect(),
            _ => vec![],
        };
        match Connection::start(self.processes.as_ref(), command, &arguments, &self.cwd).await {
            Ok(connection) => {
                self.log(json!({"event": "mcp", "server": DELEGATION_SERVER, "tools": connection.tools}));
                self.servers.push((DELEGATION_SERVER.to_string(), connection));
            }
            Err(error) => self.log(json!({"event": "mcp", "server": DELEGATION_SERVER, "error": error})),
        }
    }

    /// `Play`: one turn; the code to leave with when a step said so.
    async fn play(&mut self, steps: Vec<Map<String, Value>>, prompt: &Value) -> Option<i32> {
        if !self.started {
            self.started = true;
            let mut tools: Vec<String> = ["Bash", "Edit", "Read", "Write"].map(String::from).to_vec();
            for (server, connection) in &self.servers {
                tools.extend(connection.tools.iter().map(|tool| format!("mcp__{server}__{tool}")));
            }
            let servers: Vec<Value> = self
                .configured
                .iter()
                .map(|name| {
                    let status = if self.servers.iter().any(|(s, _)| s == name) { "connected" } else { "pending" };
                    json!({"name": name, "status": status})
                })
                .collect();
            let mode = self.value("--permission-mode").unwrap_or("default").to_string();
            self.emit(json!({
                "type": "system", "subtype": "init", "session_id": self.session_id, "cwd": self.cwd, "model": "claude-fake-1",
                "permissionMode": mode, "tools": tools, "mcp_servers": servers,
            }));
        }
        self.state_changed("running");
        self.record(json!({"type": "user", "message": {"role": "user", "content": prompt}}));
        let mut said = String::new();
        for step in steps {
            let text_of_step = |name: &str| step.get(name).and_then(Value::as_str).map(str::to_string);
            if let Some(text) = text_of_step("say") {
                said = text.clone();
                self.assistant(json!({"type": "text", "text": text}));
            } else if let Some(tool) = text_of_step("call") {
                let server = text_of_step("server").unwrap_or_else(|| DELEGATION_SERVER.to_string());
                let input = step.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
                let name = format!("mcp__{server}__{tool}");
                let id = self.tool_use_id();
                self.assistant(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                if step.get("ask") == Some(&Value::Bool(true))
                    && let Some(denial) = self.ask(&name, &input, &id).await
                {
                    self.tool_result(&id, json!(denial), true);
                    continue;
                }
                let Some(index) = self.servers.iter().position(|(s, _)| *s == server) else {
                    self.tool_result(&id, json!(format!("No MCP server named {server}.")), true);
                    continue;
                };
                let result = self.servers[index].1.call(&tool, &input).await;
                self.log(json!({"event": "call", "server": server, "tool": tool, "arguments": input, "result": result}));
                let error = result.get("isError") == Some(&Value::Bool(true));
                self.tool_result(&id, result.get("content").cloned().unwrap_or(json!([])), error);
            } else if let Some(built_in) = text_of_step("permission") {
                let input = step.get("input").and_then(Value::as_object).cloned().unwrap_or_default();
                let id = self.tool_use_id();
                self.assistant(json!({"type": "tool_use", "id": id, "name": built_in, "input": input}));
                match self.ask(&built_in, &input, &id).await {
                    Some(denial) => self.tool_result(&id, json!(denial), true),
                    None => self.tool_result(&id, json!(format!("{built_in} ran.")), false),
                }
            } else if let Some(Value::Array(commands)) = step.get("run") {
                let argv: Vec<Vec<String>> = commands
                    .iter()
                    .filter_map(Value::as_array)
                    .map(|command| command.iter().map(|part| part.as_str().unwrap_or("").to_string()).collect())
                    .collect();
                let command = argv.iter().map(|c| c.join(" ")).collect::<Vec<_>>().join(" && ");
                let mut input = Map::new();
                input.insert("command".into(), json!(command));
                let id = self.tool_use_id();
                self.assistant(json!({"type": "tool_use", "id": id, "name": "Bash", "input": input}));
                if step.get("ask") == Some(&Value::Bool(true))
                    && let Some(denial) = self.ask("Bash", &input, &id).await
                {
                    self.tool_result(&id, json!(denial), true);
                    continue;
                }
                let (mut code, mut shown) = (0, String::new());
                for part in &argv {
                    (code, shown) = self.shell(part).await;
                    if code != 0 {
                        break;
                    }
                }
                self.log(json!({"event": "bash", "command": command, "code": code}));
                self.tool_result(&id, json!(shown), code != 0);
            } else if let Some(path) = text_of_step("write") {
                let joined = Path::new(&self.cwd).join(&path);
                let file = mighty_core::workspace_files::full_path(&joined).unwrap_or(joined);
                if let Some(parent) = file.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&file, text_of_step("text").unwrap_or_default().as_bytes());
                self.log(json!({"event": "write", "path": path}));
            } else if let Some(seconds) = step.get("sleep").and_then(Value::as_f64) {
                tokio::time::sleep(Duration::from_secs_f64(seconds.max(0.0))).await;
            } else if let Some(code) = step.get("exit").and_then(Value::as_i64) {
                return Some(code as i32);
            }
        }
        self.emit(json!({
            "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "num_turns": 1, "result": said,
            "session_id": self.session_id, "total_cost_usd": 0, "usage": {"input_tokens": 1, "output_tokens": 1},
        }));
        self.state_changed("idle");
        None
    }

    /// `Ask`: whether `tool` may run, asked the way the CLI asks under `--permission-prompt-tool stdio`; None when allowed, else why not.
    async fn ask(&mut self, tool: &str, input: &Map<String, Value>, tool_use_id: &str) -> Option<String> {
        if self.value("--input-format") != Some("stream-json") || self.value("--permission-prompt-tool") != Some("stdio") {
            self.log(json!({"event": "permission", "tool": tool, "behavior": "deny"}));
            return Some(format!("Permission to use {tool} was not granted."));
        }
        let request_id = format!("perm-{}", self.number());
        self.emit(json!({
            "type": "control_request", "request_id": request_id,
            "request": {"subtype": "can_use_tool", "tool_name": tool, "input": input, "tool_use_id": tool_use_id, "permission_suggestions": []},
        }));
        let wanted = request_id.clone();
        let Some(answer) = self
            .next(move |m| {
                m.get("type").and_then(Value::as_str) == Some("control_response")
                    && m.get("response").and_then(|r| r.get("request_id")).and_then(Value::as_str) == Some(wanted.as_str())
            })
            .await
        else {
            self.log(json!({"event": "permission", "tool": tool, "behavior": "unanswered"}));
            return Some("The permission request was not answered.".into());
        };
        let response = answer.get("response");
        let decision = response.and_then(|r| r.get("response")).and_then(Value::as_object).cloned().unwrap_or_default();
        let behavior = if response.and_then(|r| r.get("subtype")).and_then(Value::as_str) == Some("success") {
            decision.get("behavior").and_then(Value::as_str).unwrap_or("deny").to_string()
        } else {
            "error".to_string()
        };
        self.log(json!({"event": "permission", "tool": tool, "behavior": behavior, "toolUseID": decision.get("toolUseID").cloned()}));
        if behavior == "allow" {
            return None;
        }
        Some(
            decision
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Permission to use {tool} was denied.")),
        )
    }

    /// `Shell`: one command in the working folder, as the CLI's Bash tool runs it; git with fixture settings, away from the user's and
    /// the system's. Its exit code and its output then its errors.
    async fn shell(&self, command: &[String]) -> (i32, String) {
        let Some(first) = command.first() else { return (127, "No command.".into()) };
        let program = if first == "git" { mighty_run::git::executable().or_else(|| on_path("git")) } else { on_path(first) };
        let Some(program) = program else { return (127, format!("The command {first} was not found.")) };
        let mut request = SpawnRequest::new(program, command[1..].iter().cloned(), &self.cwd);
        request.env.remove("NO_COLOR");
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.len() >= 4 && name[..4].eq_ignore_ascii_case("GIT_") {
                request.env_remove.push(name.into_owned());
            }
        }
        let settings = self.root.join("fixture.gitconfig");
        if !settings.exists() {
            let _ = std::fs::write(&settings, "");
        }
        request.env.insert("GIT_CONFIG_GLOBAL".into(), settings.to_string_lossy().into_owned());
        request.env.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        request.env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        for who in ["GIT_AUTHOR", "GIT_COMMITTER"] {
            request.env.insert(format!("{who}_NAME"), "Fake Claude".into());
            request.env.insert(format!("{who}_EMAIL"), "fake-claude@example.invalid".into());
        }
        match mighty_platform::process::capture(self.processes.as_ref(), request, None, Duration::from_secs(3600), 64 << 20).await {
            Ok(captured) => {
                (captured.exit_code, format!("{}{}", String::from_utf8_lossy(&captured.stdout), String::from_utf8_lossy(&captured.stderr)))
            }
            Err(error) => (127, error.to_string()),
        }
    }

    fn assistant(&mut self, content: Value) {
        let message = json!({
            "id": format!("msg_fake_{}", self.number()), "type": "message", "role": "assistant", "model": "claude-fake-1",
            "content": [content], "usage": {"input_tokens": 1, "output_tokens": 1},
        });
        let uuid = self.record(json!({"type": "assistant", "message": message}));
        self.emit(
            json!({"type": "assistant", "message": message, "parent_tool_use_id": null, "session_id": self.session_id, "uuid": uuid}),
        );
    }

    fn tool_result(&mut self, id: &str, content: Value, is_error: bool) {
        let message =
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error}]});
        let uuid = self.record(json!({"type": "user", "message": message}));
        self.emit(json!({"type": "user", "message": message, "parent_tool_use_id": null, "session_id": self.session_id, "uuid": uuid}));
    }

    fn state_changed(&self, state: &str) {
        if std::env::var("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS").as_deref() == Ok("1") {
            self.emit(json!({"type": "system", "subtype": "session_state_changed", "state": state, "session_id": self.session_id}));
        }
    }

    /// `Emit`: one stream-json line on stdout (`WriteLine`: the OS's line end).
    fn emit(&self, line: Value) {
        let mut out = self.output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = write!(out, "{}{}", text(&line), mighty_run::os::new_line());
        let _ = out.flush();
    }

    /// `Record`: one line of the session's transcript, linked to the line before it as Claude Code links them; its uuid.
    fn record(&mut self, fields: Value) -> String {
        let uuid = uuid::Uuid::new_v4().to_string();
        let mut line = Map::new();
        line.insert("parentUuid".into(), self.last_uuid.clone().map_or(Value::Null, Value::String));
        line.insert("isSidechain".into(), json!(false));
        line.insert("userType".into(), json!("external"));
        line.insert("cwd".into(), json!(self.cwd));
        line.insert("sessionId".into(), json!(self.session_id));
        line.insert("version".into(), json!(VERSION));
        line.insert("uuid".into(), json!(uuid));
        line.insert("timestamp".into(), json!(mighty_core::time::now_o()));
        if let Value::Object(fields) = fields {
            for (name, value) in fields {
                line.insert(name, value);
            }
        }
        self.last_uuid = Some(uuid.clone());
        let file = self.root.join("claude-config").join("projects").join(&self.key).join(format!("{}.jsonl", self.session_id));
        append(&file, &Value::Object(line));
        uuid
    }

    fn log(&self, entry: Value) {
        append(&self.root.join("logs").join(format!("{}.jsonl", self.key)), &entry);
    }

    fn number(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn tool_use_id(&mut self) -> String {
        format!("toolu_fake_{}", self.number())
    }

    async fn finish(&mut self, code: i32) -> i32 {
        for (_, server) in self.servers.drain(..) {
            server.close().await;
        }
        self.log(json!({"event": "exit", "code": code}));
        code
    }
}

/// `TextOf(prompt)`: a string prompt, or its text blocks joined by line ends.
fn text_of(prompt: &Value) -> String {
    match prompt {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `File.ReadAllText` then `JsonNode.Parse` (a byte order mark skipped); None when it cannot be read.
fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned();
    serde_json::from_str(&text).ok()
}

/// `File.AppendAllText(path, line + "\n")`, the folder made first.
fn append(path: &Path, line: &Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(format!("{}\n", text(line)).as_bytes());
    }
}

/// A command as `ProcessStartInfo` without the shell finds it: a full path as it is, else the first folder of PATH holding it (on
/// Windows also with `.exe` when it names no extension).
fn on_path(name: &str) -> Option<PathBuf> {
    if mighty_core::normalize::fully_qualified(name) {
        return Some(PathBuf::from(name));
    }
    let mut names = vec![name.to_string()];
    if mighty_run::os::windows() && Path::new(name).extension().is_none() {
        names.push(format!("{name}.exe"));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|folder| folder.is_absolute())
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)).collect::<Vec<_>>())
        .find(|candidate| candidate.is_file())
}

/// One stdio MCP server from the config, spoken to over JSON-RPC lines with the environment this process inherited.
struct Connection {
    child: Child,
    input: Option<ChildWriter>,
    output: BufReader<ChildReader>,
    next_id: i64,
    tools: Vec<String>,
}

impl Connection {
    /// Started and initialized, its tools listed; Err with why not.
    async fn start(processes: &dyn ProcessHost, command: &str, arguments: &[String], cwd: &str) -> Result<Connection, String> {
        let program = on_path(command).ok_or_else(|| format!("The command {command} was not found."))?;
        let mut request = SpawnRequest::new(program, arguments.iter().cloned(), cwd);
        request.env.remove("NO_COLOR");
        let mut child = processes.spawn(request).await.map_err(|error| error.to_string())?;
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().ok_or("The MCP server did not start.")?);
        // Its errors are read and dropped, so it never blocks on a full pipe.
        if let Some(mut errors) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = tokio::io::sink();
                let _ = tokio::io::copy(&mut errors, &mut sink).await;
            });
        }
        let mut connection = Connection { child, input, output, next_id: 1, tools: vec![] };
        let hello = connection
            .request(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": VERSION}}),
            )
            .await;
        if hello.as_ref().and_then(|h| h.get("result")).is_none() {
            connection.close().await;
            return Err("The MCP server did not initialize.".into());
        }
        connection.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        let listed = connection.request("tools/list", json!({})).await;
        connection.tools = listed
            .as_ref()
            .and_then(|l| l.get("result"))
            .and_then(|r| r.get("tools"))
            .and_then(Value::as_array)
            .map(|tools| tools.iter().map(|t| t.get("name").and_then(Value::as_str).unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        Ok(connection)
    }

    /// `Call`: the `tools/call` result, or an error result when none came.
    async fn call(&mut self, tool: &str, arguments: &Map<String, Value>) -> Value {
        let error = |text: &str| json!({"content": [{"type": "text", "text": text}], "isError": true});
        let Some(answer) = self.request("tools/call", json!({"name": tool, "arguments": arguments})).await else {
            return error("mighty-delegation closed without answering.");
        };
        match answer.get("result") {
            Some(result @ Value::Object(_)) => result.clone(),
            _ => error(
                answer
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("mighty-delegation answered with an error."),
            ),
        }
    }

    async fn send(&mut self, message: &Value) {
        if let Some(input) = &mut self.input {
            let line = format!("{}{}", text(message), mighty_run::os::new_line());
            let _ = input.write_all(line.as_bytes()).await;
            let _ = input.flush().await;
        }
    }

    /// One request; the answer of its id, or None once the server's output ended first.
    async fn request(&mut self, method: &str, parameters: Value) -> Option<Map<String, Value>> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": parameters})).await;
        let mut line = String::new();
        loop {
            line.clear();
            match self.output.read_line(&mut line).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            if let Ok(Value::Object(message)) = serde_json::from_str::<Value>(line.trim_end())
                && message.get("id").and_then(Value::as_i64) == Some(id)
            {
                return Some(message);
            }
        }
    }

    /// `Close`: its stdin closed, up to 10 s for it to leave, then it and what it started end.
    async fn close(mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = input.shutdown().await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        let _ = self.child.close().await;
    }
}

//! The scripted Claude (C# DelegationFakeClaude, the checks' `--fake-claude <root>`): it answers `--version`, plays its folder's script
//! over stream-json, asks its permissions the CLI's way, reaches the delegation server of `--mcp-config` on the pane's token and writes
//! its log and transcript under its root. W7-C.

mod delegation_io;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[allow(clippy::disallowed_methods, reason = "the check starts the checks' own exe, as C#'s Verification.Self does")]
fn fake(root: &Path, args: &[&str], cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mighty-delegation-fake"));
    command.arg("--fake-claude").arg(root).args(args).current_dir(cwd);
    command
}

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mighty-fake-claude-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    // The Mac's temp folder is under `/var`, which the fake's own working folder reads as `/private/var`: its scripts and transcripts
    // are keyed by that name.
    #[cfg(unix)]
    let path = std::fs::canonicalize(&path).unwrap();
    path
}

/// The project folder name Claude Code gives a working folder (`SessionHistory.ClaudeProjectFolder`).
fn key(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn the_fake_says_its_version_and_leaves_the_model_probe() {
    let root = folder("version");
    let out = fake(&root, &["--version"], &root).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "2.1.271 (Claude Code)");
    assert_eq!(out.status.code(), Some(0));
    let probe = fake(&root, &["--no-session-persistence", "-p", "x"], &root).output().unwrap();
    assert!(probe.status.success() && probe.stdout.is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// A run plays its turn: `initialize` answered, a permission asked and allowed, a file written, a said line and the result; the log
/// and the transcript where Claude Code keeps them.
#[test]
fn the_fake_plays_its_script_over_stream_json() {
    let root = folder("play");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    // The folder as the fake's own current folder names it (macOS's temp folder is /var, a link to /private/var); Windows' canonical
    // form would add `\\?\`, which the process's current folder never has.
    let work = if cfg!(windows) { work } else { work.canonicalize().unwrap() };
    let script = json!({"turns": [[
        {"permission": "Edit", "input": {"file_path": "a.txt"}},
        {"write": "notes/a.txt", "text": "한글"},
        {"say": "Done."},
    ]]});
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts").join(format!("{}.json", key(&work))), script.to_string()).unwrap();
    let mut child = fake(&root, &["--input-format", "stream-json", "--permission-prompt-tool", "stdio", "--resume", "s-1"], &work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut send = |message: Value| writeln!(input, "{message}").unwrap();
    send(json!({"type": "control_request", "request_id": "init", "request": {"subtype": "initialize"}}));
    send(json!({"type": "user", "message": {"role": "user", "content": "Do it"}}));
    let mut seen = vec![];
    let mut line = String::new();
    while output.read_line(&mut line).unwrap() > 0 {
        let message: Value = serde_json::from_str(line.trim_end()).unwrap();
        line.clear();
        if message["type"] == "control_request" && message["request"]["subtype"] == "can_use_tool" {
            assert_eq!(message["request"]["tool_name"], "Edit");
            let id = message["request_id"].clone();
            send(
                json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "toolUseID": "t"}}}),
            );
        }
        let done = message["type"] == "result";
        seen.push(message);
        if done {
            break;
        }
    }
    drop(send);
    drop(input);
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(seen[0]["type"] == "control_response" && seen[0]["response"]["subtype"] == "success", "initialize answered first");
    assert!(seen.iter().any(|m| m["type"] == "system" && m["subtype"] == "init" && m["session_id"] == "s-1"));
    assert!(seen.iter().any(|m| m["type"] == "user" && m["message"]["content"][0]["content"] == "Edit ran."), "the allowed tool ran");
    let result = seen.last().unwrap();
    assert!(result["result"] == "Done." && result["session_id"] == "s-1");
    assert_eq!(std::fs::read_to_string(work.join("notes").join("a.txt")).unwrap(), "한글");
    let log = lines(&root.join("logs").join(format!("{}.jsonl", key(&work))));
    let events: Vec<&str> = log.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(events, ["argv", "initialize", "permission", "write", "exit"]);
    assert_eq!(log[2]["behavior"], "allow");
    let transcript = lines(&root.join("claude-config").join("projects").join(key(&work)).join("s-1.jsonl"));
    assert!(transcript.len() >= 3 && transcript[0]["parentUuid"].is_null() && transcript[1]["parentUuid"] == transcript[0]["uuid"]);
    let _ = std::fs::remove_dir_all(root);
}

/// The delegation server of `--mcp-config` is started with the fake's own environment (the pane's token and pipe) and a scripted
/// call reaches the app's listener; its answer is the tool result. The fake Claude starts its MCP server through the platform's process
/// host, over the OS's transport.
#[tokio::test(flavor = "multi_thread")]
async fn the_fake_calls_the_delegation_server_on_the_panes_token() {
    use std::sync::Arc;
    use std::time::Duration;

    use delegation_io::{Canned, executable, temp};
    use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
    use mighty_platform::BoxFuture;

    struct Nothing;
    impl TerminalHandler for Nothing {
        fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err(String::new()) })
        }
    }
    let root = folder("call");
    let canned = Arc::new(Canned::new(Duration::ZERO));
    let calls = canned.calls.clone();
    let pipe = AgentIo::start(delegation_io::os_transport().0, Arc::new(Nothing)).unwrap();
    let binding = pipe.bind("parent", "ws", &temp(), "claude", &executable()).unwrap();
    binding.connect_delegation(Some(canned), true, "");
    let mut arguments = vec![];
    binding.append_arguments(&mut arguments);
    // The config names the checks' exe, as the app's names MightyCode.exe.
    let config = std::fs::read_to_string(&arguments[1])
        .unwrap()
        .replace(&executable().replace('\\', "\\\\"), &env!("CARGO_BIN_EXE_mighty-delegation-fake").replace('\\', "\\\\"));
    let script = json!({"turns": [[{"call": "delegate", "arguments": {"task": "Write it", "mode": "plan"}}, {"say": "Delegated."}]]});
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts").join(format!("{}.json", key(&root))), script.to_string()).unwrap();
    let root_for = root.clone();
    let environment = binding.environment();
    let finished = tokio::task::spawn_blocking(move || {
        let mut child = fake(&root_for, &["--mcp-config", &config], &root_for)
            .envs(environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", json!({"type": "user", "message": {"role": "user", "content": "Delegate"}})).unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut messages = vec![];
        let mut line = String::new();
        while output.read_line(&mut line).unwrap() > 0 {
            let message: Value = serde_json::from_str(line.trim_end()).unwrap();
            line.clear();
            let done = message["type"] == "result";
            messages.push(message);
            if done {
                break;
            }
        }
        drop(input);
        (child.wait().unwrap().code(), messages)
    });
    let (code, messages) = tokio::time::timeout(Duration::from_secs(60), finished).await.unwrap().unwrap();
    assert_eq!(code, Some(0));
    let init = messages.iter().find(|m| m["subtype"] == "init").unwrap_or_else(|| panic!("no init message: {messages:?}"));
    assert!(init["tools"].as_array().unwrap().iter().any(|t| t == "mcp__mighty-delegation__delegate"), "{init}");
    let result = messages.iter().find(|m| m["type"] == "user").unwrap();
    assert_eq!(result["message"]["content"][0]["is_error"], false, "{result}");
    let recorded = calls.lock().unwrap().clone();
    assert!(recorded.len() == 1 && recorded[0].0.tool == "delegate" && recorded[0].1.pane_id == "parent", "the call reached the app");
    let log = lines(&root.join("logs").join(format!("{}.jsonl", key(&root))));
    let mcp = log.iter().find(|e| e["event"] == "mcp").unwrap();
    assert_eq!(mcp["tools"].as_array().unwrap().len(), 6, "the six tools: {mcp}");
    pipe.close().await;
    let _ = std::fs::remove_dir_all(root);
}

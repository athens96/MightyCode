//! `mighty-workspace`'s stdio MCP server (`--agent-workspace-mcp`, `MC-14`): on-demand, strictly read-only lookup across the other
//! local workspaces registered in Mighty Code (`list_workspaces`, `search_workspace`, `read_workspace_file`,
//! `recent_workspace_activity`), forwarded over the authenticated `AgentIo` pipe (`MIGHTY_PANE_TOKEN`) without changing
//! `mighty-terminal`'s four tools or `mighty-delegation`'s six tools.

use std::sync::Arc;

use indexmap::IndexMap;
use mighty_platform::BoxFuture;
use mighty_run::text_json::{escape_relaxed, relaxed};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::serve::{self, Handle};
use crate::agent_io::wire::{Lenient, MAXIMUM_BYTES, Unreadable, raw_member};
use crate::request::prefix;

/// MCP server name in `--mcp-config` / Codex `mcp_servers`.
pub const SERVER_NAME: &str = "mighty-workspace";
/// Headless CLI flag served by `main.rs` (`exe_mode`).
pub const HEADLESS_ARGUMENT: &str = crate::modes::WORKSPACE_MCP_ARGUMENT;

/// System-level MCP instructions sent on `initialize`.
pub const GUIDANCE: &str = "Use mighty-workspace only when the user asks about another workspace/project or mentions `@workspace:<name>`, or when cross-repo APIs/contracts must be checked. Start with list_workspaces to discover local workspaces, then use search_workspace, read_workspace_file, or recent_workspace_activity on demand. All tools are read-only and automatically omit secrets (.env*, keys, credentials).";

/// Tool Error when started outside a live Mighty Code pane binding.
pub const NO_CONNECTION: &str = "This MCP server was started without a Mighty Code pane connection, so it cannot reach the app.";
/// Tool Error when cross-workspace lookup is switched off in Settings.
pub const DETACHED_MESSAGE: &str = "Cross-workspace lookup is currently disabled in Mighty Code Settings.";

/// One argument of a `mighty-workspace` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceArgument {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// One tool in `mighty-workspace`'s manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceTool {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [WorkspaceArgument],
}

pub const LIST_WORKSPACES: WorkspaceTool = WorkspaceTool {
    name: "list_workspaces",
    description: "List the local workspaces registered in Mighty Code (marking your current workspace), with each workspace's id, name, folder basename, git branch, open agent pane count, and latest session summary. Call this first when looking up another workspace or resolving an `@workspace:<name>` reference.",
    arguments: &[],
};

pub const SEARCH_WORKSPACE: WorkspaceTool = WorkspaceTool {
    name: "search_workspace",
    description: "Search a local workspace's text files and/or recent agent conversation summaries on demand (case-insensitive). Skips .git, node_modules, target, binary files, and sensitive files (.env*, *.pem, *.key, credentials).",
    arguments: &[
        WorkspaceArgument {
            name: "workspace",
            description: "Target workspace id or name (from list_workspaces, or after `@workspace:`).",
            required: true,
        },
        WorkspaceArgument { name: "query", description: "Case-insensitive text query to search for.", required: true },
        WorkspaceArgument {
            name: "scope",
            description: "Optional search scope: 'all' (default), 'files', or 'conversations'.",
            required: false,
        },
    ],
};

pub const READ_WORKSPACE_FILE: WorkspaceTool = WorkspaceTool {
    name: "read_workspace_file",
    description: "Read a bounded line range (up to 250 lines per call) of a UTF-8 text file inside a registered local workspace. Refuses path traversal ('..'), symlinks escaping the workspace root, binary files, and secret/credential files.",
    arguments: &[
        WorkspaceArgument { name: "workspace", description: "Target workspace id or name.", required: true },
        WorkspaceArgument {
            name: "path",
            description: "Workspace-relative file path (e.g. 'src/api/types.ts' or 'CLAUDE.md').",
            required: true,
        },
        WorkspaceArgument { name: "start_line", description: "Optional 1-based start line (default '1').", required: false },
        WorkspaceArgument {
            name: "end_line",
            description: "Optional 1-based inclusive end line (at most 250 lines returned).",
            required: false,
        },
    ],
};

pub const RECENT_WORKSPACE_ACTIVITY: WorkspaceTool = WorkspaceTool {
    name: "recent_workspace_activity",
    description: "Read a registered local workspace's recent activity: its open agent panes (title, provider, status, latest request and assistant summary) and recent git branch/commits/modified files.",
    arguments: &[WorkspaceArgument { name: "workspace", description: "Target workspace id or name.", required: true }],
};

/// All four read-only tools exposed by `mighty-workspace`.
pub const MANIFEST: [&WorkspaceTool; 4] = [&LIST_WORKSPACES, &SEARCH_WORKSPACE, &READ_WORKSPACE_FILE, &RECENT_WORKSPACE_ACTIVITY];

pub fn named(name: &str) -> Option<&'static WorkspaceTool> {
    MANIFEST.iter().copied().find(|tool| tool.name == name)
}

/// A validated call forwarded over the `AgentIo` pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRequest {
    pub tool: String,
    pub arguments: IndexMap<String, String>,
}

impl WorkspaceRequest {
    pub fn new(tool: impl Into<String>) -> Self {
        WorkspaceRequest { tool: tool.into(), arguments: IndexMap::new() }
    }
}

/// The authenticated pane making the cross-workspace query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCaller {
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub provider: String,
}

/// The app's answer over the pipe (`ok: true` for normal content, `ok: false` for a tool-level error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceResponse {
    pub ok: bool,
    pub text: String,
}

impl WorkspaceResponse {
    pub fn ok(text: impl Into<String>) -> Self {
        WorkspaceResponse { ok: true, text: text.into() }
    }

    pub fn failure(text: impl Into<String>) -> Self {
        WorkspaceResponse { ok: false, text: text.into() }
    }
}

/// Implemented in `src-tauri/src/workspace_mcp.rs` to answer cross-workspace queries from `Host` state and disk.
pub trait WorkspaceHandler: Send + Sync {
    fn handle(&self, request: WorkspaceRequest, caller: WorkspaceCaller) -> BoxFuture<'static, WorkspaceResponse>;
}

/// Transport from the stdio `WorkspaceServer` subprocess to the app's `AgentIo` pipe.
pub type WorkspaceTransport = Arc<dyn Fn(WorkspaceRequest) -> BoxFuture<'static, WorkspaceResponse> + Send + Sync>;

struct Late;

impl From<Unreadable> for Late {
    fn from(_: Unreadable) -> Self {
        Late
    }
}

/// Stdio JSON-RPC 2.0 MCP server for `--agent-workspace-mcp`.
#[derive(Clone)]
pub struct WorkspaceServer {
    transport: Option<WorkspaceTransport>,
}

impl WorkspaceServer {
    pub fn new(transport: Option<WorkspaceTransport>) -> WorkspaceServer {
        WorkspaceServer { transport }
    }

    pub fn handler(&self) -> Handle {
        let server = self.clone();
        Arc::new(move |line: String| {
            let server = server.clone();
            Box::pin(async move { server.handle(&line).await })
        })
    }

    pub async fn handle(&self, line: &str) -> Option<String> {
        let line = line.trim_end_matches('\r');
        if line.chars().all(|c| c == ' ' || c == '\t') {
            return None;
        }
        if line.len() > MAXIMUM_BYTES {
            return Some(error("null", -32700, "Message exceeds 1 MB."));
        }
        let Some(document) = Lenient::parse(line) else { return Some(error("null", -32700, "Parse error")) };
        match self.answer(&document, line).await {
            Ok(answer) => answer,
            Err(Late) => Some(error("null", -32700, "Parse error")),
        }
    }

    async fn answer(&self, document: &Lenient, line: &str) -> Result<Option<String>, Late> {
        let json = &document.value;
        let valid_id = |value: Option<&Value>| -> Result<Option<String>, Late> {
            Ok(match value {
                Some(Value::String(text)) => Some(escape_relaxed(document.read(text)?)),
                Some(Value::Number(number)) => Some(raw_member(line, "id").unwrap_or_else(|| number.to_string())),
                _ => None,
            })
        };
        let Value::Object(request) = json else { return Ok(Some(error("null", -32600, "Invalid Request"))) };
        document.keys(request)?;
        let version_ok = match request.get("jsonrpc") {
            Some(Value::String(version)) => document.read(version)? == "2.0",
            _ => false,
        };
        let Some(Value::String(named_method)) = request.get("method").filter(|_| version_ok) else {
            let id = valid_id(request.get("id"))?;
            return Ok(Some(error(id.as_deref().unwrap_or("null"), -32600, "Invalid Request")));
        };
        if !request.contains_key("id") {
            return Ok(None);
        }
        let Some(id) = valid_id(request.get("id"))? else { return Ok(Some(error("null", -32600, "Invalid Request"))) };
        let method = document.read(named_method)?;
        let empty = Map::new();
        let parameters = match request.get("params") {
            Some(Value::Object(parameters)) => parameters,
            _ => &empty,
        };
        Ok(Some(match method {
            "initialize" => {
                document.keys(parameters)?;
                let asked = document.text(parameters.get("protocolVersion"))?;
                reply(
                    &id,
                    "result",
                    &json!({
                        "protocolVersion": serve::protocol_version(asked),
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": SERVER_NAME, "title": "Mighty Code workspace lookup", "version": "1.0.0"},
                        "instructions": GUIDANCE,
                    }),
                )
            }
            "ping" => reply(&id, "result", &json!({})),
            "tools/list" => reply(&id, "result", &json!({"tools": MANIFEST.iter().copied().map(definition).collect::<Vec<_>>()})),
            "tools/call" => {
                document.keys(parameters)?;
                let Some(name) = document.text(parameters.get("name"))? else {
                    return Ok(Some(error(&id, -32602, "Unknown tool: none")));
                };
                let Some(spec) = named(name) else {
                    return Ok(Some(error(&id, -32602, &format!("Unknown tool: {}", prefix(name, 80)))));
                };
                let arguments = match parameters.get("arguments") {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(arguments)) => arguments.clone(),
                    _ => return Ok(Some(reply(&id, "result", &tool_error("arguments must be an object.")))),
                };
                let answer = self.call(document, spec, &arguments).await?;
                reply(&id, "result", &answer)
            }
            _ => error(&id, -32601, &format!("Method not found: {}", prefix(method, 80))),
        }))
    }

    async fn call(&self, document: &Lenient, spec: &WorkspaceTool, arguments: &Map<String, Value>) -> Result<Value, Late> {
        document.keys(arguments)?;
        let mut values = IndexMap::new();
        for (key, value) in arguments {
            if !spec.arguments.iter().any(|arg| arg.name == key) {
                return Ok(tool_error(&format!("Unknown argument '{key}' for {}.", spec.name)));
            }
            match value {
                Value::Null => {}
                Value::String(text) => {
                    let read = document.read(text)?;
                    if read.contains('\0') {
                        return Ok(tool_error(&format!("Argument '{key}' must not contain NUL bytes.")));
                    }
                    values.insert(key.clone(), read.to_string());
                }
                Value::Number(number) => {
                    values.insert(key.clone(), number.to_string());
                }
                Value::Bool(flag) => {
                    values.insert(key.clone(), flag.to_string());
                }
                _ => return Ok(tool_error(&format!("Argument '{key}' of {} must be a string or number.", spec.name))),
            }
        }
        for required in spec.arguments.iter().filter(|arg| arg.required) {
            if values.get(required.name).is_none_or(|v| v.trim().is_empty()) {
                return Ok(tool_error(&format!("Missing required argument '{}' for {}.", required.name, spec.name)));
            }
        }
        let Some(transport) = &self.transport else { return Ok(tool_error(NO_CONNECTION)) };
        let response = transport(WorkspaceRequest { tool: spec.name.to_string(), arguments: values }).await;
        if response.ok { Ok(json!({"content": [{"type": "text", "text": response.text}]})) } else { Ok(tool_error(&response.text)) }
    }
}

fn reply(id: &str, key: &str, value: &Value) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"{key}\":{}}}", relaxed(value, false))
}

fn error(id: &str, code: i64, message: &str) -> String {
    reply(id, "error", &json!({"code": code, "message": message}))
}

pub fn definition(tool: &WorkspaceTool) -> Value {
    let mut properties = Map::new();
    for argument in tool.arguments {
        properties.insert(argument.name.into(), json!({"type": "string", "description": argument.description}));
    }
    let required: Vec<&str> = tool.arguments.iter().filter(|a| a.required).map(|a| a.name).collect();
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
    })
}

pub fn tool_error(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_server_lists_its_four_tools_and_validates_arguments() {
        let server = WorkspaceServer::new(Some(Arc::new(|req| {
            Box::pin(async move { WorkspaceResponse::ok(format!("called {} {:?}", req.tool, req.arguments)) })
        })));
        let init =
            server.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#).await.unwrap();
        let init_val: Value = serde_json::from_str(&init).unwrap();
        assert_eq!(init_val["result"]["serverInfo"]["name"], "mighty-workspace");
        assert!(init_val["result"]["instructions"].as_str().unwrap().contains("list_workspaces"));

        let listed = server.handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await.unwrap();
        let listed_val: Value = serde_json::from_str(&listed).unwrap();
        let names: Vec<&str> = listed_val["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["list_workspaces", "search_workspace", "read_workspace_file", "recent_workspace_activity"]);

        // Numeric line numbers are accepted ergonomically alongside strings.
        let read_call = server
            .handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_workspace_file","arguments":{"workspace":"api","path":"schema.ts","start_line":10,"end_line":50}}}"#)
            .await
            .unwrap();
        let read_val: Value = serde_json::from_str(&read_call).unwrap();
        assert!(read_val["result"]["isError"].is_null());
        assert!(read_val["result"]["content"][0]["text"].as_str().unwrap().contains("\"start_line\": \"10\""));

        // Missing required argument returns isError: true.
        let missing = server
            .handle(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_workspace","arguments":{"workspace":"api"}}}"#,
            )
            .await
            .unwrap();
        let missing_val: Value = serde_json::from_str(&missing).unwrap();
        assert_eq!(missing_val["result"]["isError"], true);
    }
}

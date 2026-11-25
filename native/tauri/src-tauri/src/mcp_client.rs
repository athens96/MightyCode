//! MC-98: the app's own call to a tool of one of the person's MCP servers, for an MCP trigger (automation/triggers.rs). The server is
//! found by its name in the same configs the MCP hub (agent_context.rs) lists — the workspace's and the person's Claude Code, Codex
//! and Antigravity configs — and reached the way the hub signs in: Streamable HTTP, with the config's own headers, else the hub's
//! OAuth token (refreshed when it ran out), else the config's bearer token variable, else the token Claude Code keeps for it.
//!
//! One call is `initialize`, `notifications/initialized` and `tools/call` on a fresh session; the answer is JSON or an SSE stream,
//! and a tool's result is its `structuredContent`, else its first text read as JSON. A server run as a command (stdio) is not
//! reached: an MCP trigger names an HTTP one. Nothing here knows any server by name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mighty_core::locale::Locale;
use mighty_platform::http::HttpsRequest;
use serde_json::{Value, json};
use tauri::Manager as _;

use crate::agent_context::{self, ExternalMcpServerState, ScanPaths, StoredMcpOAuthToken};
use crate::host::Host;

/// How long one request may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest answer read.
const MAX_ANSWER: usize = 4 * 1024 * 1024;

/// Why a call did not come back with a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// No config names the server.
    NotConfigured,
    /// The server is run as a command, not reached over HTTP.
    Stdio,
    /// The server answered 401 or 403: a sign-in is needed.
    AuthRequired,
    /// The server has no such tool.
    NoTool,
    /// The tool's result has no list where the trigger looks.
    NoList(String),
    /// The server answered another failure status.
    Http(u16),
    /// The tool or the protocol failed (its words).
    Failed(String),
}

impl McpError {
    /// The words the trigger's status and the event inbox show.
    pub fn words(&self, locale: &Locale, server: &str, tool: &str) -> String {
        match self {
            McpError::NotConfigured => locale.get_with("automation.mcp.notConfigured", &[("server", server)]),
            McpError::Stdio => locale.get_with("automation.mcp.stdio", &[("server", server)]),
            McpError::AuthRequired => locale.get_with("automation.mcp.auth", &[("server", server)]),
            McpError::NoTool => locale.get_with("automation.mcp.noTool", &[("server", server), ("tool", tool)]),
            McpError::NoList(list) => locale.get_with("automation.mcp.noList", &[("tool", tool), ("list", list)]),
            McpError::Http(status) => {
                locale.get_with("automation.mcp.failed", &[("server", server), ("reason", &format!("HTTP {status}"))])
            }
            McpError::Failed(reason) => locale.get_with("automation.mcp.failed", &[("server", server), ("reason", reason)]),
        }
    }
}

/// The server `name` among the configs: an HTTP entry first (the workspace's before the person's, as the hub lists them).
pub fn pick_server(servers: Vec<ExternalMcpServerState>, name: &str) -> Result<ExternalMcpServerState, McpError> {
    let named: Vec<ExternalMcpServerState> = servers.into_iter().filter(|s| s.name == name.trim()).collect();
    if named.is_empty() {
        return Err(McpError::NotConfigured);
    }
    named.into_iter().find(|s| s.transport == "http" && s.url.as_deref().is_some_and(|u| !u.trim().is_empty())).ok_or(McpError::Stdio)
}

/// Either a plain JSON-RPC object or an SSE `data: {...}` stream (the message with a result or an error, else the last one).
pub fn parse_answer(raw: &str) -> Result<Value, McpError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str::<Value>(trimmed).map_err(|e| McpError::Failed(format!("not JSON: {e}")));
    }
    let mut last = None;
    for line in trimmed.lines() {
        let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else { continue };
        if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
            if value.get("result").is_some() || value.get("error").is_some() {
                return Ok(value);
            }
            last = Some(value);
        }
    }
    last.ok_or_else(|| McpError::Failed("an answer that is neither JSON nor an event stream".into()))
}

/// A `tools/call` answer's result: `structuredContent`, else the first text content read as JSON (or as text); an error, a tool's
/// `isError` or an unknown tool as such.
pub fn tool_result(rpc: &Value) -> Result<Value, McpError> {
    if let Some(error) = rpc.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("MCP error").to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("tool") && (lower.contains("unknown") || lower.contains("not found") || lower.contains("no such")) {
            return Err(McpError::NoTool);
        }
        return Err(McpError::Failed(message.chars().take(300).collect()));
    }
    let result = rpc.get("result").ok_or_else(|| McpError::Failed("no result".into()))?;
    let first_text =
        || result.get("content").and_then(Value::as_array).and_then(|c| c.iter().find_map(|i| i.get("text").and_then(Value::as_str)));
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = first_text().unwrap_or("the tool failed");
        let lower = text.to_ascii_lowercase();
        if lower.contains("unknown tool") || lower.contains("tool not found") {
            return Err(McpError::NoTool);
        }
        return Err(McpError::Failed(text.chars().take(300).collect()));
    }
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return Ok(structured.clone());
    }
    if let Some(text) = first_text() {
        return Ok(serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string())));
    }
    Ok(result.clone())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The hub's token refreshed through its token endpoint, kept again; None when it cannot be.
async fn refreshed(host: &Host, https: &dyn mighty_platform::http::HttpsSend, saved: &StoredMcpOAuthToken) -> Option<String> {
    let refresh = saved.refresh_token.as_deref().filter(|t| !t.is_empty())?;
    let endpoint = saved.token_endpoint.as_deref().filter(|u| !u.is_empty())?;
    let client = saved.client_id.as_deref().filter(|c| !c.is_empty())?;
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh)
        .append_pair("client_id", client)
        .finish();
    let request = HttpsRequest {
        url: endpoint.to_string(),
        headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into()), ("Accept".into(), "application/json".into())],
        timeout: HTTP_TIMEOUT,
    };
    let (answer, _) = https.post_json(request, form.into_bytes(), 65_536).await.ok()?;
    if !(200..300).contains(&answer.status) {
        return None;
    }
    let parsed: Value = serde_json::from_str(&answer.body).ok()?;
    let access = parsed.get("access_token")?.as_str()?.trim().to_string();
    if access.is_empty() {
        return None;
    }
    let expires_in = parsed.get("expires_in").and_then(Value::as_i64).unwrap_or(3600).clamp(0, 10 * 365 * 24 * 3600);
    let next = StoredMcpOAuthToken {
        access_token: access.clone(),
        refresh_token: parsed.get("refresh_token").and_then(Value::as_str).map(str::to_string).or_else(|| saved.refresh_token.clone()),
        expires_at_ms: Some(now_ms().saturating_add(expires_in.saturating_mul(1000))),
        ..saved.clone()
    };
    if host.writes() {
        let _ = agent_context::save_mcp_oauth_token(&host.data_dir, next);
    }
    Some(access)
}

/// The headers that sign a request to `server`: its config's own, else a bearer token (the hub's, its variable's, Claude Code's).
async fn sign_in_headers(
    host: &Host,
    https: &dyn mighty_platform::http::HttpsSend,
    server: &ExternalMcpServerState,
    folder: Option<&Path>,
) -> Vec<(String, String)> {
    let own = server.headers.0.clone();
    if own.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization")) {
        return own;
    }
    let url = server.url.clone().unwrap_or_default();
    let mut token = None;
    if let Some(saved) = agent_context::hub_oauth_token(&host.data_dir, &server.name, &url) {
        let fresh = saved.expires_at_ms.is_none_or(|at| at > now_ms() + 45_000);
        token = if fresh { Some(saved.access_token.clone()) } else { refreshed(host, https, &saved).await };
    }
    if token.is_none()
        && let Some(variable) = &server.bearer_token_env_var
    {
        token = crate::repo_tasks::login_shell_env(host, folder).await.get(variable).cloned().filter(|t| !t.trim().is_empty());
    }
    if token.is_none() && host.smoke.is_none() {
        let (name, at) = (server.name.clone(), url.clone());
        token = tauri::async_runtime::spawn_blocking(move || {
            agent_context::claude_credentials_blob().and_then(|blob| agent_context::claude_mcp_access_token(&blob, &name, &at))
        })
        .await
        .ok()
        .flatten();
    }
    let mut headers = own;
    if let Some(token) = token {
        headers.push(("Authorization".into(), format!("Bearer {}", token.trim())));
    }
    headers
}

/// Calls `tool` with `args` on the MCP server `server` as the configs of the workspace at `folder` (and the person's) name it.
pub async fn call_tool(host: &Host, folder: Option<&Path>, server: &str, tool: &str, args: &Value) -> Result<Value, McpError> {
    let paths = {
        let home = crate::accounts::home();
        ScanPaths {
            home_dir: (!home.trim().is_empty()).then(|| PathBuf::from(home)),
            codex_home: std::env::var("CODEX_HOME").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from),
            data_dir: host.data_dir.clone(),
            // The keychain is read only when a token is wanted from it.
            claude_keychain_blob: None,
        }
    };
    let root = folder.map(Path::to_path_buf);
    let servers = tauri::async_runtime::spawn_blocking(move || agent_context::scan_external_mcp_servers(root.as_deref(), &paths))
        .await
        .map_err(|e| McpError::Failed(e.to_string()))?;
    let found = pick_server(servers, server)?;
    let url = found.url.clone().unwrap_or_default();
    let https = host
        .app
        .try_state::<crate::platform::Ports>()
        .map(|ports| ports.https.clone())
        .ok_or_else(|| McpError::Failed("HTTP transport unavailable".into()))?;
    let signed = sign_in_headers(host, https.as_ref(), &found, folder).await;
    let headers = |session: Option<&str>| {
        let mut all: Vec<(String, String)> =
            vec![("Content-Type".into(), "application/json".into()), ("Accept".into(), "application/json, text/event-stream".into())];
        all.extend(signed.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("content-type") && !n.eq_ignore_ascii_case("accept")).cloned());
        if let Some(session) = session.filter(|s| !s.is_empty()) {
            all.push(("Mcp-Session-Id".into(), session.to_string()));
        }
        all
    };
    let post = |body: Value, session: Option<String>| {
        let request = HttpsRequest { url: url.clone(), headers: headers(session.as_deref()), timeout: HTTP_TIMEOUT };
        let https = https.clone();
        async move {
            let bytes = serde_json::to_vec(&body).map_err(|e| McpError::Failed(e.to_string()))?;
            let (answer, session) = https.post_json(request, bytes, MAX_ANSWER).await.map_err(|e| McpError::Failed(e.to_string()))?;
            match answer.status {
                401 | 403 => Err(McpError::AuthRequired),
                status if !(200..300).contains(&status) => Err(McpError::Http(status)),
                _ => Ok((answer.body, session)),
            }
        }
    };
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "Mighty Code", "version": host.version } }
    });
    let (_, session) = post(initialize, None).await?;
    if session.is_some() {
        let _ = post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), session.clone()).await;
    }
    let arguments = if args.is_object() { args.clone() } else { json!({}) };
    let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": tool, "arguments": arguments } });
    let (body, _) = post(call, session).await?;
    tool_result(&parse_answer(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, transport: &str, url: Option<&str>) -> ExternalMcpServerState {
        ExternalMcpServerState {
            provider: "claude".into(),
            name: name.into(),
            scope: "user".into(),
            transport: transport.into(),
            target: url.unwrap_or("cmd").into(),
            url: url.map(str::to_string),
            command: None,
            args: vec![],
            bearer_token_env_var: None,
            source: "~/.claude.json".into(),
            oauth_connected: false,
            headers: agent_context::SecretHeaders(vec![("Authorization".into(), "Bearer secret".into())]),
        }
    }

    #[test]
    fn a_server_is_found_by_its_name_over_http_first() {
        let all =
            vec![server("tracker", "stdio", None), server("tracker", "http", Some("https://t.dev/mcp")), server("x", "http", Some("u"))];
        assert_eq!(pick_server(all.clone(), "tracker").unwrap().url.as_deref(), Some("https://t.dev/mcp"));
        assert_eq!(pick_server(all.clone(), "nope"), Err(McpError::NotConfigured));
        assert_eq!(pick_server(vec![server("cmd", "stdio", None)], "cmd"), Err(McpError::Stdio));
        assert!(!format!("{:?}", all[0].headers).contains("secret"), "a header's value is never printed");
        assert!(serde_json::to_value(&all[0]).unwrap().get("headers").is_none(), "nor sent to the page");
    }

    #[test]
    fn answers_and_tool_results_are_read() {
        let plain = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"tasks\":[{\"key\":\"MC-1\"}]}"}]}}"#;
        assert_eq!(tool_result(&parse_answer(plain).unwrap()).unwrap()["tasks"][0]["key"], "MC-1");
        let sse = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"structuredContent\":{\"tasks\":[]}}}\r\n\r\n";
        assert_eq!(tool_result(&parse_answer(sse).unwrap()).unwrap(), json!({"tasks": []}));
        let unknown = json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32602, "message": "Unknown tool: list_tasks"}});
        assert_eq!(tool_result(&unknown), Err(McpError::NoTool));
        let failed = json!({"result": {"isError": true, "content": [{"type": "text", "text": "project not found"}]}});
        assert_eq!(tool_result(&failed), Err(McpError::Failed("project not found".into())));
        assert!(parse_answer("<html>").is_err());
        let text = json!({"result": {"content": [{"type": "text", "text": "plain words"}]}});
        assert_eq!(tool_result(&text).unwrap(), json!("plain words"));
    }

    #[test]
    fn claude_code_s_own_token_is_found_by_name_or_address() {
        let later = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let blob = json!({"mcpOAuth": {"a|1": {"serverName": "tracker", "serverUrl": "https://t.dev/mcp/", "accessToken": "tok",
            "expiresAt": later}, "b|2": {"serverName": "old", "accessToken": "gone", "expiresAt": 1}}})
        .to_string();
        assert_eq!(agent_context::claude_mcp_access_token(&blob, "tracker", "").as_deref(), Some("tok"));
        assert_eq!(agent_context::claude_mcp_access_token(&blob, "other", "https://T.dev/mcp").as_deref(), Some("tok"));
        assert_eq!(agent_context::claude_mcp_access_token(&blob, "old", ""), None, "expired");
    }
}

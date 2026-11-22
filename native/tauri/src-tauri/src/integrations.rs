//! MightyWork (Streamable HTTP MCP + OAuth 2.1 PKCE) & GitHub (`gh` CLI) workspace integration (`MC-11`):
//! - Per-workspace MightyWork project key & GitHub `owner/repo` mapping with automatic repository detection
//! - Browsing & searching MightyWork tasks, MightyWork wiki pages, and GitHub issues
//! - Attaching rich context to the composer or launching directly in a new pane / isolated git worktree
//! - Optional automatic MightyWork pipeline stage sync (`in_progress` on start, `review` on PR, `done` on merge)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use mighty_core::model::{LinkedTask, PaneWorktree, RunSession, Workspace};
use mighty_platform::SpawnRequest;
use mighty_platform::http::HttpsRequest;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tauri::Manager as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::host::{Host, Registry, lock};
use crate::methods::{flag, text};

const DEFAULT_MIGHTYWORK_URL: &str = "https://mightywork.duckdns.org/mcp";
const AUTH_FILE_NAME: &str = "mightywork-auth.json";
const MAX_HTTP_BODY: usize = 2 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

const QUERY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredMightyWorkAuth {
    #[serde(default)]
    server_url: String,
    #[serde(default)]
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    token_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    expires_at_ms: Option<i64>,
}

#[derive(Default)]
pub struct IntegrationsState {
    auth: Mutex<Option<StoredMightyWorkAuth>>,
    loaded: Mutex<bool>,
}

fn state(host: &Host) -> Arc<IntegrationsState> {
    if let Some(existing) = host.app.try_state::<Arc<IntegrationsState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(IntegrationsState::default());
    host.app.manage(created.clone());
    host.app.try_state::<Arc<IntegrationsState>>().map(|s| s.inner().clone()).unwrap_or(created)
}

fn load_saved_auth(host: &Host) -> Option<StoredMightyWorkAuth> {
    let st = state(host);
    {
        let mut loaded = lock(&st.loaded);
        if !*loaded {
            let path = host.data_dir.join(AUTH_FILE_NAME);
            let parsed = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<StoredMightyWorkAuth>(&b).ok());
            *lock(&st.auth) = parsed.filter(|a| !a.access_token.trim().is_empty());
            *loaded = true;
        }
    }
    lock(&st.auth).clone()
}

fn save_auth(host: &Host, auth: Option<StoredMightyWorkAuth>) -> Result<(), String> {
    let st = state(host);
    *lock(&st.auth) = auth.clone();
    *lock(&st.loaded) = true;
    if !host.writes() {
        return Ok(());
    }
    let path = host.data_dir.join(AUTH_FILE_NAME);
    match auth {
        Some(record) => {
            std::fs::create_dir_all(&host.data_dir).map_err(|e| e.to_string())?;
            let mut bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            let temp = host.data_dir.join(format!("{AUTH_FILE_NAME}.{}.tmp", std::process::id()));
            std::fs::write(&temp, &bytes).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
            }
            if let Err(err) = std::fs::rename(&temp, &path) {
                let _ = std::fs::remove_file(&temp);
                return Err(err.to_string());
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn encode_form(pairs: &[(&str, &str)]) -> Vec<u8> {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", utf8_percent_encode(k, QUERY_ENCODE_SET), utf8_percent_encode(v, QUERY_ENCODE_SET)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

/// Discovers MightyWork MCP URL & optional inline Bearer header from workspace or user MCP configs.
fn discover_mcp_endpoint(workspace_path: Option<&str>, override_url: Option<&str>) -> (String, Option<String>) {
    if let Some(url) = override_url.map(str::trim).filter(|u| !u.is_empty()) {
        return (url.to_string(), None);
    }
    let extract_json = |root: &Value| -> Option<(String, Option<String>)> {
        let mw = root.get("mcpServers")?.get("mightywork")?;
        let url = mw.get("url").or_else(|| mw.get("httpUrl")).and_then(Value::as_str)?.trim().to_string();
        if url.is_empty() {
            return None;
        }
        let header = mw
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|h| h.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).and_then(|(_, v)| v.as_str()))
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")).or(Some(v)))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Some((url, header))
    };

    if let Some(ws) = workspace_path.filter(|p| !p.is_empty()) {
        let mcp_json = Path::new(ws).join(".mcp.json");
        if let Ok(text) = std::fs::read_to_string(&mcp_json)
            && let Ok(val) = serde_json::from_str::<Value>(&text)
            && let Some(found) = extract_json(&val)
        {
            return found;
        }
    }

    let home = crate::accounts::home();
    if !home.is_empty() {
        let claude_path = Path::new(&home).join(".claude.json");
        if let Ok(text) = std::fs::read_to_string(&claude_path)
            && let Ok(val) = serde_json::from_str::<Value>(&text)
        {
            if let Some(ws) = workspace_path
                && let Some(proj) = val.get("projects").and_then(|p| p.get(ws))
                && let Some(found) = extract_json(proj)
            {
                return found;
            }
            if let Some(found) = extract_json(&val) {
                return found;
            }
        }
        let codex_path = Path::new(&home).join(".codex").join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&codex_path)
            && let Ok(doc) = text.parse::<toml::Table>()
            && let Some(mw) = doc.get("mcp_servers").and_then(|m| m.get("mightywork")).and_then(|v| v.as_table())
            && let Some(url) = mw.get("url").and_then(|v| v.as_str()).map(str::trim).filter(|u| !u.is_empty())
        {
            let bearer = mw
                .get("bearer_token_env_var")
                .and_then(|v| v.as_str())
                .and_then(|var| std::env::var(var).ok())
                .filter(|t| !t.trim().is_empty());
            return (url.to_string(), bearer);
        }
    }

    (DEFAULT_MIGHTYWORK_URL.to_string(), None)
}

/// Non-interactive fallback check for an unexpired `mcpOAuth` access token in Claude Code's credentials.
fn read_claude_mcp_oauth_token(mcp_url: &str) -> Option<String> {
    let parse_blob = |raw: &str| -> Option<String> {
        let root: Value = serde_json::from_str(raw).ok()?;
        let map = root.get("mcpOAuth")?.as_object()?;
        let now = Utc::now().timestamp_millis();
        for entry in map.values() {
            let name = entry.get("serverName").and_then(Value::as_str).unwrap_or("");
            let url = entry.get("serverUrl").and_then(Value::as_str).unwrap_or("");
            if name == "mightywork" || (!mcp_url.is_empty() && url == mcp_url) {
                let expires = entry.get("expiresAt").and_then(Value::as_f64).map(|n| n as i64).unwrap_or(i64::MAX);
                if expires > now + 15_000
                    && let Some(tok) = entry.get("accessToken").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty())
                {
                    return Some(tok.to_string());
                }
            }
        }
        None
    };

    if let Some(reader) = crate::platform::claude_credentials()
        && let Ok(Some(raw)) = reader.read(None, false)
        && let Some(tok) = parse_blob(&raw)
    {
        return Some(tok);
    }

    let home = crate::accounts::home();
    if !home.is_empty() {
        let cred_file = Path::new(&home).join(".claude").join(".credentials.json");
        if let Ok(raw) = std::fs::read_to_string(&cred_file)
            && let Some(tok) = parse_blob(&raw)
        {
            return Some(tok);
        }
    }
    None
}

async fn refresh_saved_oauth(host: &Host, saved: &StoredMightyWorkAuth) -> Option<String> {
    let refresh_token = saved.refresh_token.as_deref().filter(|t| !t.is_empty())?;
    let token_endpoint = saved.token_endpoint.as_deref().filter(|u| !u.is_empty())?;
    let client_id = saved.client_id.as_deref().filter(|c| !c.is_empty())?;
    let ports = host.app.try_state::<crate::platform::Ports>()?;
    let body = encode_form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)]);
    let req = HttpsRequest {
        url: token_endpoint.to_string(),
        headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into()), ("Accept".into(), "application/json".into())],
        timeout: HTTP_TIMEOUT,
    };
    let (resp, _) = ports.https.post_json(req, body, 65_536).await.ok()?;
    if !(200..300).contains(&resp.status) {
        return None;
    }
    let parsed: Value = serde_json::from_str(&resp.body).ok()?;
    let access_token = parsed.get("access_token")?.as_str()?.trim().to_string();
    if access_token.is_empty() {
        return None;
    }
    let expires_in = parsed.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);
    let next = StoredMightyWorkAuth {
        server_url: saved.server_url.clone(),
        access_token: access_token.clone(),
        refresh_token: parsed.get("refresh_token").and_then(Value::as_str).map(str::to_string).or_else(|| saved.refresh_token.clone()),
        token_endpoint: saved.token_endpoint.clone(),
        client_id: saved.client_id.clone(),
        expires_at_ms: Some(now_ms() + expires_in * 1000),
    };
    let _ = save_auth(host, Some(next));
    Some(access_token)
}

async fn resolve_bearer_token(host: &Host, mcp_url: &str, inline_header: Option<String>) -> Option<String> {
    if let Some(saved) = load_saved_auth(host) {
        let expired = saved.expires_at_ms.is_some_and(|exp| exp <= now_ms() + 45_000);
        if !expired {
            return Some(saved.access_token);
        }
        if let Some(refreshed) = refresh_saved_oauth(host, &saved).await {
            return Some(refreshed);
        }
    }
    if let Some(inline) = inline_header.filter(|t| !t.trim().is_empty()) {
        return Some(inline);
    }
    read_claude_mcp_oauth_token(mcp_url)
}

/// Parses either a plain JSON-RPC object or an SSE `data: {...}` stream from a Streamable HTTP MCP response.
pub fn parse_mcp_http_payload(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str::<Value>(trimmed).map_err(|e| format!("Invalid MCP JSON: {e}"));
    }
    let mut last_json = None;
    for line in trimmed.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            let candidate = data.trim();
            if candidate.starts_with('{')
                && let Ok(val) = serde_json::from_str::<Value>(candidate)
            {
                if val.get("result").is_some() || val.get("error").is_some() {
                    return Ok(val);
                }
                last_json = Some(val);
            }
        }
    }
    last_json.ok_or_else(|| "Unrecognized MCP HTTP response payload.".to_string())
}

fn extract_tool_result(rpc: &Value) -> Result<Value, String> {
    if let Some(err) = rpc.get("error") {
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("MCP error");
        return Err(msg.to_string());
    }
    let Some(result) = rpc.get("result") else {
        return Err("Missing result in MCP response.".into());
    };
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("Tool execution failed.");
        return Err(text.to_string());
    }
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return Ok(structured.clone());
    }
    if let Some(text) =
        result.get("content").and_then(Value::as_array).and_then(|c| c.first()).and_then(|item| item.get("text")).and_then(Value::as_str)
    {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            return Ok(parsed);
        }
        return Ok(Value::String(text.to_string()));
    }
    Ok(result.clone())
}

async fn mcp_call_tool(
    host: &Host,
    workspace_path: Option<&str>,
    override_url: Option<&str>,
    tool_name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let (mcp_url, inline_token) = discover_mcp_endpoint(workspace_path, override_url);
    let token = resolve_bearer_token(host, &mcp_url, inline_token).await;
    let Some(ports) = host.app.try_state::<crate::platform::Ports>() else {
        return Err("HTTP transport unavailable.".into());
    };

    let build_headers = |session_id: Option<&str>, tok: Option<&str>| -> Vec<(String, String)> {
        let mut h =
            vec![("Content-Type".into(), "application/json".into()), ("Accept".into(), "application/json, text/event-stream".into())];
        if let Some(t) = tok.filter(|s| !s.is_empty()) {
            h.push(("Authorization".into(), format!("Bearer {t}")));
        }
        if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
            h.push(("Mcp-Session-Id".into(), sid.to_string()));
        }
        h
    };

    let init_body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "MightyCode", "version": host.version }
        }
    }))
    .map_err(|e| e.to_string())?;

    let init_req = HttpsRequest { url: mcp_url.clone(), headers: build_headers(None, token.as_deref()), timeout: HTTP_TIMEOUT };
    let (init_resp, session_id) = ports.https.post_json(init_req, init_body, MAX_HTTP_BODY).await.map_err(|e| e.to_string())?;
    if init_resp.status == 401 || init_resp.status == 403 {
        return Err("AUTH_REQUIRED".into());
    }
    if !(200..300).contains(&init_resp.status) {
        return Err(format!("MightyWork MCP HTTP {}", init_resp.status));
    }

    // Best-effort initialized notification when stateful session id is present.
    if session_id.is_some() {
        let notif_body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap_or_default();
        let notif_req = HttpsRequest {
            url: mcp_url.clone(),
            headers: build_headers(session_id.as_deref(), token.as_deref()),
            timeout: Duration::from_secs(5),
        };
        let _ = ports.https.post_json(notif_req, notif_body, 65_536).await;
    }

    let call_body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        }
    }))
    .map_err(|e| e.to_string())?;

    let call_req = HttpsRequest { url: mcp_url, headers: build_headers(session_id.as_deref(), token.as_deref()), timeout: HTTP_TIMEOUT };
    let (call_resp, _) = ports.https.post_json(call_req, call_body, MAX_HTTP_BODY).await.map_err(|e| e.to_string())?;
    if call_resp.status == 401 || call_resp.status == 403 {
        return Err("AUTH_REQUIRED".into());
    }
    if !(200..300).contains(&call_resp.status) {
        return Err(format!("MightyWork MCP HTTP {}", call_resp.status));
    }
    let rpc = parse_mcp_http_payload(&call_resp.body)?;
    extract_tool_result(&rpc)
}

/// Interactive OAuth 2.1 + PKCE (`S256`) browser sign-in against the MightyWork authorization server.
async fn oauth_login(host: &Arc<Host>, mcp_url: &str) -> Result<(), String> {
    if !host.may_change_machine() {
        return Err(crate::setup::verification_mode());
    }
    let parsed_mcp = url::Url::parse(mcp_url).map_err(|_| "Invalid MightyWork MCP URL.".to_string())?;
    let origin = format!("{}://{}", parsed_mcp.scheme(), parsed_mcp.host_str().ok_or("Missing host")?);
    let metadata_url = format!("{origin}/.well-known/oauth-authorization-server");

    let ports = host.app.try_state::<crate::platform::Ports>().ok_or("HTTP transport unavailable.")?;
    let meta_resp = ports
        .https
        .get(HttpsRequest { url: metadata_url, headers: vec![("Accept".into(), "application/json".into())], timeout: HTTP_TIMEOUT }, 65_536)
        .await
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&meta_resp.status) {
        return Err(format!("OAuth discovery failed (HTTP {}).", meta_resp.status));
    }
    let meta: Value = serde_json::from_str(&meta_resp.body).map_err(|e| e.to_string())?;
    let auth_endpoint = meta.get("authorization_endpoint").and_then(Value::as_str).ok_or("Missing authorization_endpoint")?;
    let token_endpoint = meta.get("token_endpoint").and_then(Value::as_str).ok_or("Missing token_endpoint")?;
    let reg_endpoint = meta.get("registration_endpoint").and_then(Value::as_str).ok_or("Missing registration_endpoint")?;

    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let reg_body = serde_json::to_vec(&json!({
        "client_name": "Mighty Code",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none"
    }))
    .map_err(|e| e.to_string())?;
    let (reg_resp, _) = ports
        .https
        .post_json(
            HttpsRequest {
                url: reg_endpoint.to_string(),
                headers: vec![("Content-Type".into(), "application/json".into()), ("Accept".into(), "application/json".into())],
                timeout: HTTP_TIMEOUT,
            },
            reg_body,
            65_536,
        )
        .await
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&reg_resp.status) {
        return Err(format!("OAuth client registration failed (HTTP {}).", reg_resp.status));
    }
    let reg_json: Value = serde_json::from_str(&reg_resp.body).map_err(|e| e.to_string())?;
    let client_id = reg_json.get("client_id").and_then(Value::as_str).ok_or("Missing client_id")?.to_string();

    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let csrf_state = uuid::Uuid::new_v4().simple().to_string();

    let mut auth_url = url::Url::parse(auth_endpoint).map_err(|e| e.to_string())?;
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &csrf_state)
        .append_pair("scope", "mightywork:read mightywork:write");

    let launch_url = auth_url.to_string();
    tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&launch_url)).await.map_err(|e| e.to_string())??;

    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(180), listener.accept())
        .await
        .map_err(|_| "Timed out waiting for browser sign-in.".to_string())?
        .map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf))
        .await
        .map_err(|_| "Timed out reading OAuth callback.".to_string())?
        .map_err(|e| e.to_string())?;
    let req_line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
    let path_part = req_line.split_whitespace().nth(1).unwrap_or("/");
    let callback_url = url::Url::parse(&format!("http://127.0.0.1:{port}{path_part}")).map_err(|e| e.to_string())?;
    let mut code = None;
    let mut returned_state = None;
    for (k, v) in callback_url.query_pairs() {
        if k == "code" {
            code = Some(v.into_owned());
        } else if k == "state" {
            returned_state = Some(v.into_owned());
        }
    }

    let html = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><html><body style=\"font-family:system-ui;padding:32px\"><h3>MightyWork sign-in complete.</h3><p>You can return to Mighty Code and close this tab.</p><script>window.close()</script></body></html>";
    let _ = socket.write_all(html.as_bytes()).await;
    let _ = socket.shutdown().await;

    if returned_state.as_deref() != Some(csrf_state.as_str()) {
        return Err("OAuth state mismatch.".into());
    }
    let code = code.filter(|c| !c.is_empty()).ok_or("Authorization code not returned.")?;

    let token_body = encode_form(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect_uri),
        ("client_id", &client_id),
        ("code_verifier", &verifier),
    ]);
    let (tok_resp, _) = ports
        .https
        .post_json(
            HttpsRequest {
                url: token_endpoint.to_string(),
                headers: vec![
                    ("Content-Type".into(), "application/x-www-form-urlencoded".into()),
                    ("Accept".into(), "application/json".into()),
                ],
                timeout: HTTP_TIMEOUT,
            },
            token_body,
            65_536,
        )
        .await
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&tok_resp.status) {
        return Err(format!("OAuth token exchange failed (HTTP {}).", tok_resp.status));
    }
    let tok_json: Value = serde_json::from_str(&tok_resp.body).map_err(|e| e.to_string())?;
    let access_token = tok_json.get("access_token").and_then(Value::as_str).ok_or("Missing access_token")?.to_string();
    let refresh_token = tok_json.get("refresh_token").and_then(Value::as_str).map(str::to_string);
    let expires_in = tok_json.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);

    save_auth(
        host,
        Some(StoredMightyWorkAuth {
            server_url: mcp_url.to_string(),
            access_token,
            refresh_token,
            token_endpoint: Some(token_endpoint.to_string()),
            client_id: Some(client_id),
            expires_at_ms: Some(now_ms() + expires_in * 1000),
        }),
    )
}

/// Scans `<workspace>/AGENTS.md` or `<workspace>/CLAUDE.md` for a MightyWork project key hint (e.g. `MC`).
pub fn detect_mightywork_project(workspace_path: &Path) -> Option<String> {
    for file_name in ["AGENTS.md", "CLAUDE.md"] {
        let path = workspace_path.join(file_name);
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        if !content.to_ascii_lowercase().contains("mightywork") {
            continue;
        }
        for line in content.lines().take(80) {
            if line.contains("project") {
                // Match patterns like `project: "MC"`, `project="MC"`, or `` (`project`): `MC` ``.
                for token in line.split(['`', '"', '\'']) {
                    let t = token.trim();
                    if (2..=8).contains(&t.len()) && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && t != "MCP" {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }
    None
}

async fn detect_github_repo(host: &Host, workspace_path: &Path) -> Option<String> {
    let git = crate::workspace_create::git_executable()?;
    if !workspace_path.is_dir() {
        return None;
    }
    let req = SpawnRequest::new(git, ["remote", "get-url", "origin"], workspace_path.to_path_buf()).env("GIT_TERMINAL_PROMPT", "0");
    let captured = mighty_platform::process::capture(host.processes.as_ref(), req, None, Duration::from_secs(3), 4096).await.ok()?;
    if captured.exit_code != 0 {
        return None;
    }
    let origin = String::from_utf8_lossy(&captured.stdout).trim().to_string();
    mighty_delegation::pane_worktree::github_repository(&origin)
}

fn gh_executable(host: &Host) -> Option<PathBuf> {
    let env = host.runs().map(|r| r.catalog.environment().current().values).unwrap_or_default();
    crate::platform::cli_copies("gh", &env).into_iter().next().map(PathBuf::from)
}

/// Generates a git-worktree-safe session id (`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$`) embedding the task/issue key.
pub fn make_task_session_id(key: &str) -> String {
    let cleaned: String = key
        .trim_start_matches('#')
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let prefix = if cleaned.is_empty() { "task".to_string() } else { cleaned.chars().take(24).collect() };
    let suffix: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
    format!("{prefix}-{suffix}")
}

/// Formats rich markdown context for composer attachment or initial agent prompt.
pub fn format_item_context(
    source: &str,
    key: &str,
    title: &str,
    stage: Option<&str>,
    url: Option<&str>,
    body: &str,
    checklist: &[(String, bool)],
) -> String {
    let mut lines = Vec::new();
    let header_tag = match source {
        "mightywork_task" => format!("[MightyWork {key}]"),
        "mightywork_wiki" => "[MightyWork Wiki]".to_string(),
        "github_issue" => format!("[GitHub {key}]"),
        _ => format!("[{key}]"),
    };
    lines.push(format!("{header_tag} {title}"));
    if let Some(st) = stage.filter(|s| !s.is_empty()) {
        lines.push(format!("Status: {st}"));
    }
    if let Some(u) = url.filter(|s| !s.is_empty()) {
        lines.push(format!("URL: {u}"));
    }
    if !body.trim().is_empty() {
        lines.push(String::new());
        lines.push(body.trim().to_string());
    }
    if !checklist.is_empty() {
        lines.push(String::new());
        lines.push("Checklist:".into());
        for (item_text, done) in checklist {
            lines.push(format!("- [{}] {}", if *done { "x" } else { " " }, item_text));
        }
    }
    lines.join("\n")
}

/// Asynchronous best-effort MightyWork stage & comment sync when `mightywork_auto_sync_on()` is active.
pub fn spawn_stage_sync(
    host: &Arc<Host>,
    workspace_id: &str,
    linked: &LinkedTask,
    stage: &'static str,
    comment: Option<String>,
    pane_id_for_note: Option<String>,
) {
    if linked.source != "mightywork" || !host.may_change_machine() {
        return;
    }
    let cfg = {
        let view = host.view();
        let Some(ws) = view.snapshot.workspace(workspace_id) else { return };
        if !ws.mightywork_auto_sync_on() {
            return;
        }
        let project = linked
            .project
            .clone()
            .or_else(|| ws.mightywork_project.clone())
            .or_else(|| detect_mightywork_project(Path::new(ws.path())))
            .unwrap_or_default();
        if project.is_empty() {
            return;
        }
        (project, ws.path().to_string(), ws.mightywork_url.clone())
    };

    let host = host.clone();
    let task_key = linked.key.clone();
    tauri::async_runtime::spawn(async move {
        let (project, ws_path, mw_url) = cfg;
        let updated = mcp_call_tool(
            &host,
            Some(&ws_path),
            mw_url.as_deref(),
            "update_task",
            json!({ "project": project, "task": task_key, "stage": stage }),
        )
        .await;
        if let Some(body) = comment.filter(|b| !b.is_empty()) {
            let _ = mcp_call_tool(
                &host,
                Some(&ws_path),
                mw_url.as_deref(),
                "add_comment",
                json!({ "project": project, "task": task_key, "body": body }),
            )
            .await;
        }
        if updated.is_ok()
            && let Some(pane_id) = pane_id_for_note
        {
            let note = match stage {
                "in_progress" => host.locale.get_with("integrations.sync.started", &[("key", &task_key)]),
                "review" => host.locale.get_with("integrations.sync.review", &[("key", &task_key)]),
                "done" => host.locale.get_with("integrations.sync.done", &[("key", &task_key)]),
                _ => return,
            };
            crate::pane_worktree::append_system_note(&host, &pane_id, note);
        }
    });
}

/// Hooked from `pane_worktree.rs` when a worktree's PR push succeeds.
pub fn on_worktree_pr(host: &Arc<Host>, pane_id: &str, pane: &PaneWorktree, compare_url: Option<&str>) {
    // Only a pane that works on a worktree branch of its own moves its task: a record without one is no pull request of that task.
    if pane.branch.as_deref().is_none_or(str::is_empty) {
        return;
    }
    let (ws_id, linked) = {
        let view = host.view();
        let session = view.snapshot.session(pane_id);
        let ws_id = session.map(|s| s.workspace_id().to_string()).unwrap_or_default();
        let linked = pane.linked_task.clone().or_else(|| session.and_then(|s| s.linked_task.clone()));
        (ws_id, linked)
    };
    let Some(linked) = linked else { return };
    let comment = match compare_url {
        Some(url) => format!("Pushed worktree branch `{}` (base `{}`) for PR review: {url}", pane.branch(), pane.base_branch()),
        None => format!("Pushed worktree branch `{}` (base `{}`) for review.", pane.branch(), pane.base_branch()),
    };
    spawn_stage_sync(host, &ws_id, &linked, "review", Some(comment), Some(pane_id.to_string()));
}

/// Hooked from `pane_worktree.rs` when an open worktree pane merges into its base branch.
pub fn on_worktree_merged(host: &Arc<Host>, pane_id: &str, pane: &PaneWorktree, commit: &str) {
    let (ws_id, linked) = {
        let view = host.view();
        let session = view.snapshot.session(pane_id);
        let ws_id = session.map(|s| s.workspace_id().to_string()).unwrap_or_default();
        let linked = pane.linked_task.clone().or_else(|| session.and_then(|s| s.linked_task.clone()));
        (ws_id, linked)
    };
    let Some(linked) = linked else { return };
    let short: String = commit.chars().take(8).collect();
    let comment = format!("Merged worktree branch `{}` into `{}` (`{short}`).", pane.branch(), pane.base_branch());
    spawn_stage_sync(host, &ws_id, &linked, "done", Some(comment), Some(pane_id.to_string()));
}

/// Hooked from `pane_worktree.rs` when a kept (closed) worktree merges from the Worktree List sheet.
pub fn on_kept_worktree_merged(host: &Arc<Host>, workspace_id: &str, pane: &PaneWorktree, commit: &str) {
    let Some(linked) = pane.linked_task.as_ref() else { return };
    let short: String = commit.chars().take(8).collect();
    let comment = format!("Merged kept worktree branch `{}` into `{}` (`{short}`).", pane.branch(), pane.base_branch());
    spawn_stage_sync(host, workspace_id, linked, "done", Some(comment), None);
}

fn workspace_snapshot(host: &Host, requested_id: Option<&str>) -> Result<Workspace, String> {
    let view = host.view();
    let target = requested_id
        .filter(|id| !id.is_empty())
        .or(view.snapshot.active_workspace_id.as_deref())
        .and_then(|id| view.snapshot.workspace(id))
        .cloned();
    target.ok_or_else(|| host.locale.get("window.error.addWorkspaceFirst"))
}

async fn build_status(host: &Arc<Host>, ws: &Workspace) -> Value {
    let local_path = (ws.remote.is_none() && !ws.path().is_empty()).then(|| PathBuf::from(ws.path()));
    let detected_mw = local_path.as_deref().and_then(detect_mightywork_project).unwrap_or_default();
    let detected_gh = match local_path.as_deref() {
        Some(p) => detect_github_repo(host, p).await.unwrap_or_default(),
        None => String::new(),
    };
    let is_git = match local_path.as_deref() {
        Some(p) => crate::host::read_git(host.processes.as_ref(), &p.to_string_lossy()).await.is_some(),
        None => false,
    };
    let (mcp_url, inline_tok) = discover_mcp_endpoint(ws.path.as_deref(), ws.mightywork_url.as_deref());
    let has_token = resolve_bearer_token(host, &mcp_url, inline_tok).await.is_some();

    let mut projects = Vec::new();
    let mut connected = false;
    if has_token && let Ok(res) = mcp_call_tool(host, ws.path.as_deref(), ws.mightywork_url.as_deref(), "list_projects", json!({})).await {
        connected = true;
        let arr = res.get("projects").and_then(Value::as_array).or_else(|| res.as_array());
        if let Some(list) = arr {
            for p in list {
                let key = p.get("key").and_then(Value::as_str).unwrap_or("").to_string();
                let name = p.get("name").and_then(Value::as_str).unwrap_or(&key).to_string();
                if !key.is_empty() {
                    projects.push(json!({ "key": key, "name": name }));
                }
            }
        }
    }

    json!({
        "workspaceId": ws.id(),
        "workspaceName": ws.name(),
        "isGitRepo": is_git,
        "mightyworkProject": ws.mightywork_project.clone().unwrap_or_else(|| detected_mw.clone()),
        "detectedMightyworkProject": detected_mw,
        "mightyworkUrl": mcp_url,
        "mightyworkConnected": connected,
        "mightyworkProjects": projects,
        "githubRepo": ws.github_repo.clone().unwrap_or_else(|| detected_gh.clone()),
        "detectedGithubRepo": detected_gh,
        "mightyworkAutoSync": ws.mightywork_auto_sync_on(),
    })
}

async fn fetch_detail(host: &Arc<Host>, ws: &Workspace, source: &str, key: &str) -> Result<(Value, LinkedTask, String), String> {
    let mw_project = ws
        .mightywork_project
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| detect_mightywork_project(Path::new(ws.path())))
        .unwrap_or_default();

    match source {
        "mightywork_task" => {
            if mw_project.is_empty() {
                return Err("MightyWork project key is not configured.".into());
            }
            let raw = mcp_call_tool(
                host,
                ws.path.as_deref(),
                ws.mightywork_url.as_deref(),
                "get_task",
                json!({ "project": mw_project, "task": key }),
            )
            .await?;
            let item_key = raw.get("key").and_then(Value::as_str).unwrap_or(key).to_string();
            let title = raw.get("title").and_then(Value::as_str).unwrap_or(&item_key).to_string();
            let stage = raw.get("stage").and_then(Value::as_str).map(str::to_string);
            let priority = raw.get("priority").and_then(Value::as_str).map(str::to_string);
            let url = raw.get("url").and_then(Value::as_str).map(str::to_string);
            let body = raw.get("description").and_then(Value::as_str).unwrap_or("").to_string();
            let tags: Vec<String> = raw
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            let checklist: Vec<(String, bool)> = raw
                .get("checklist")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|c| {
                            Some((c.get("text")?.as_str()?.to_string(), c.get("done").and_then(Value::as_bool).unwrap_or(false)))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let formatted = format_item_context(source, &item_key, &title, stage.as_deref(), url.as_deref(), &body, &checklist);
            let checklist_json: Vec<Value> = checklist.iter().map(|(t, d)| json!({ "text": t, "done": d })).collect();
            let linked = LinkedTask {
                source: "mightywork".into(),
                project: Some(mw_project),
                key: item_key.clone(),
                title: Some(title.clone()),
                url: url.clone(),
            };
            Ok((
                json!({
                    "source": source,
                    "key": item_key,
                    "title": title,
                    "stage": stage,
                    "priority": priority,
                    "tags": tags,
                    "url": url,
                    "body": body,
                    "checklist": checklist_json,
                    "formattedContext": formatted,
                }),
                linked,
                formatted,
            ))
        }
        "mightywork_wiki" => {
            if mw_project.is_empty() {
                return Err("MightyWork project key is not configured.".into());
            }
            let raw = mcp_call_tool(
                host,
                ws.path.as_deref(),
                ws.mightywork_url.as_deref(),
                "get_wiki_page",
                json!({ "project": mw_project, "page": key }),
            )
            .await?;
            let page_id = raw.get("id").and_then(Value::as_str).unwrap_or(key).to_string();
            let title = raw.get("title").and_then(Value::as_str).unwrap_or(key).to_string();
            let stage = raw.get("status").and_then(Value::as_str).map(str::to_string);
            let url = raw.get("url").and_then(Value::as_str).map(str::to_string);
            let body = raw.get("content").and_then(Value::as_str).unwrap_or("").to_string();
            let formatted = format_item_context(source, &page_id, &title, stage.as_deref(), url.as_deref(), &body, &[]);
            let linked = LinkedTask {
                source: "mightywork_wiki".into(),
                project: Some(mw_project),
                key: page_id.clone(),
                title: Some(title.clone()),
                url: url.clone(),
            };
            Ok((
                json!({
                    "source": source,
                    "key": page_id,
                    "title": title,
                    "stage": stage,
                    "tags": raw.get("tags").cloned().unwrap_or_else(|| json!([])),
                    "url": url,
                    "body": body,
                    "checklist": [],
                    "formattedContext": formatted,
                }),
                linked,
                formatted,
            ))
        }
        "github_issue" => {
            let repo = match ws.github_repo.clone().filter(|r| !r.trim().is_empty()) {
                Some(r) => r,
                None => detect_github_repo(host, Path::new(ws.path())).await.unwrap_or_default(),
            };
            if repo.is_empty() {
                return Err("GitHub repository is not configured.".into());
            }
            let Some(gh) = gh_executable(host) else {
                return Err("GitHub CLI (gh) is not installed.".into());
            };
            let num = key.trim_start_matches('#');
            let cwd = if ws.path().is_empty() { std::env::temp_dir() } else { PathBuf::from(ws.path()) };
            let req = SpawnRequest::new(
                gh,
                ["issue", "view", num, "--repo", &repo, "--json", "number,title,body,state,labels,url,updatedAt"],
                cwd,
            )
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1");
            let captured = mighty_platform::process::capture(host.processes.as_ref(), req, None, Duration::from_secs(10), 512_000)
                .await
                .map_err(|e| e.to_string())?;
            if captured.exit_code != 0 {
                return Err(String::from_utf8_lossy(&captured.stderr).trim().to_string());
            }
            let raw: Value = serde_json::from_slice(&captured.stdout).map_err(|e| e.to_string())?;
            let issue_key =
                format!("#{}", raw.get("number").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_else(|| num.to_string()));
            let title = raw.get("title").and_then(Value::as_str).unwrap_or(&issue_key).to_string();
            let stage = raw.get("state").and_then(Value::as_str).map(|s| s.to_ascii_lowercase());
            let url = raw.get("url").and_then(Value::as_str).map(str::to_string);
            let body = raw.get("body").and_then(Value::as_str).unwrap_or("").to_string();
            let tags: Vec<String> = raw
                .get("labels")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
                .unwrap_or_default();
            let formatted = format_item_context(source, &issue_key, &title, stage.as_deref(), url.as_deref(), &body, &[]);
            let linked = LinkedTask {
                source: "github".into(),
                project: Some(repo),
                key: issue_key.clone(),
                title: Some(title.clone()),
                url: url.clone(),
            };
            Ok((
                json!({
                    "source": source,
                    "key": issue_key,
                    "title": title,
                    "stage": stage,
                    "tags": tags,
                    "url": url,
                    "body": body,
                    "checklist": [],
                    "formattedContext": formatted,
                }),
                linked,
                formatted,
            ))
        }
        _ => Err("Unknown integration item source.".into()),
    }
}

pub fn register(methods: &mut Registry) {
    methods.add("integrationsStatus", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsSaveSettings", |host: Arc<Host>, args| async move {
        let ws_id = workspace_snapshot(&host, text(&args, "workspaceId"))?.id().to_string();
        let mw_proj = text(&args, "mightyworkProject").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mw_url = text(&args, "mightyworkUrl").map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != DEFAULT_MIGHTYWORK_URL);
        let gh_repo = text(&args, "githubRepo").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let auto_sync = args.get("mightyworkAutoSync").and_then(Value::as_bool);

        host.view().update(|snapshot| {
            if let Some(ws) = snapshot.workspaces_mut().iter_mut().find(|w| w.id() == ws_id) {
                ws.mightywork_project = mw_proj;
                ws.mightywork_url = mw_url;
                ws.github_repo = gh_repo;
                ws.mightywork_auto_sync = auto_sync;
            }
        })?;
        host.schedule();
        let ws = workspace_snapshot(&host, Some(&ws_id))?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsOAuthLogin", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId")).ok();
        let (mcp_url, _) = discover_mcp_endpoint(
            ws.as_ref().and_then(|w| w.path.as_deref()),
            text(&args, "mightyworkUrl").or_else(|| ws.as_ref().and_then(|w| w.mightywork_url.as_deref())),
        );
        match oauth_login(&host, &mcp_url).await {
            Ok(()) => Ok(json!({ "connected": true })),
            Err(error) => Ok(json!({ "connected": false, "error": error })),
        }
    });

    methods.add("integrationsSaveToken", |host: Arc<Host>, args| async move {
        let token = text(&args, "token").unwrap_or("").trim().to_string();
        if token.is_empty() {
            return Err("Token must not be empty.".into());
        }
        let ws = workspace_snapshot(&host, text(&args, "workspaceId")).ok();
        let (mcp_url, _) = discover_mcp_endpoint(
            ws.as_ref().and_then(|w| w.path.as_deref()),
            text(&args, "mightyworkUrl").or_else(|| ws.as_ref().and_then(|w| w.mightywork_url.as_deref())),
        );
        save_auth(
            &host,
            Some(StoredMightyWorkAuth {
                server_url: mcp_url,
                access_token: token,
                refresh_token: None,
                token_endpoint: None,
                client_id: None,
                expires_at_ms: None,
            }),
        )?;
        Ok(json!({ "connected": true }))
    });

    methods.add("integrationsLogout", |host: Arc<Host>, _| async move {
        save_auth(&host, None)?;
        Ok(json!({ "connected": false }))
    });

    methods.add("integrationsList", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let tab = text(&args, "tab").unwrap_or("tasks");
        let query = text(&args, "query").unwrap_or("").trim().to_lowercase();
        let filter = text(&args, "filter").unwrap_or("all");

        match tab {
            "tasks" => {
                let project = ws
                    .mightywork_project
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| detect_mightywork_project(Path::new(ws.path())))
                    .unwrap_or_default();
                if project.is_empty() {
                    return Ok(json!({ "items": [], "unconfigured": "mightywork" }));
                }
                let mut call_args = json!({ "project": project, "limit": 80 });
                if matches!(filter, "todo" | "in_progress" | "backlog" | "review" | "qa" | "done") {
                    call_args["stage"] = Value::String(filter.into());
                }
                match mcp_call_tool(&host, ws.path.as_deref(), ws.mightywork_url.as_deref(), "list_tasks", call_args).await {
                    Ok(res) => {
                        let empty = Vec::new();
                        let list = res.get("tasks").and_then(Value::as_array).unwrap_or(&empty);
                        let items: Vec<Value> = list
                            .iter()
                            .filter_map(|t| {
                                let key = t.get("key")?.as_str()?.to_string();
                                let title = t.get("title")?.as_str()?.to_string();
                                let stage = t.get("stage").and_then(Value::as_str).unwrap_or("");
                                if filter == "all" && stage == "done" && query.is_empty() {
                                    // Keep active tasks prominent by default when browsing all without a search query, or include all:
                                }
                                if !query.is_empty() && !key.to_lowercase().contains(&query) && !title.to_lowercase().contains(&query) {
                                    return None;
                                }
                                Some(json!({
                                    "source": "mightywork_task",
                                    "key": key,
                                    "title": title,
                                    "stage": stage,
                                    "priority": t.get("priority").and_then(Value::as_str),
                                    "tags": t.get("tags").cloned().unwrap_or_else(|| json!([])),
                                }))
                            })
                            .collect();
                        Ok(json!({ "items": items }))
                    }
                    Err(err) => Ok(json!({ "items": [], "error": err })),
                }
            }
            "wiki" => {
                let project = ws
                    .mightywork_project
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| detect_mightywork_project(Path::new(ws.path())))
                    .unwrap_or_default();
                if project.is_empty() {
                    return Ok(json!({ "items": [], "unconfigured": "mightywork" }));
                }
                match mcp_call_tool(
                    &host,
                    ws.path.as_deref(),
                    ws.mightywork_url.as_deref(),
                    "list_wiki_pages",
                    json!({ "project": project }),
                )
                .await
                {
                    Ok(res) => {
                        let rows = if let Some(arr) = res.get("pages").and_then(Value::as_array) {
                            arr.clone()
                        } else if let Some(arr) = res.as_array() {
                            arr.clone()
                        } else if res.get("id").is_some() {
                            vec![res]
                        } else {
                            Vec::new()
                        };
                        let items: Vec<Value> = rows
                            .iter()
                            .filter_map(|p| {
                                let id = p.get("id")?.as_str()?.to_string();
                                let title = p.get("title")?.as_str()?.to_string();
                                if !query.is_empty() && !title.to_lowercase().contains(&query) {
                                    return None;
                                }
                                Some(json!({
                                    "source": "mightywork_wiki",
                                    "key": id,
                                    "title": title,
                                    "stage": p.get("status").and_then(Value::as_str),
                                    "tags": p.get("tags").cloned().unwrap_or_else(|| json!([])),
                                    "updatedAt": p.get("updated_at").and_then(Value::as_str),
                                }))
                            })
                            .collect();
                        Ok(json!({ "items": items }))
                    }
                    Err(err) => Ok(json!({ "items": [], "error": err })),
                }
            }
            "github" => {
                let repo = match ws.github_repo.clone().filter(|r| !r.trim().is_empty()) {
                    Some(r) => r,
                    None => detect_github_repo(&host, Path::new(ws.path())).await.unwrap_or_default(),
                };
                if repo.is_empty() {
                    return Ok(json!({ "items": [], "unconfigured": "github" }));
                }
                let Some(gh) = gh_executable(&host) else {
                    return Ok(json!({ "items": [], "unconfigured": "github" }));
                };
                let state_arg = if filter == "all" { "all" } else { "open" };
                let cwd = if ws.path().is_empty() { std::env::temp_dir() } else { PathBuf::from(ws.path()) };
                let mut cmd_args = vec![
                    "issue".to_string(),
                    "list".to_string(),
                    "--repo".to_string(),
                    repo,
                    "--state".to_string(),
                    state_arg.to_string(),
                    "--limit".to_string(),
                    "50".to_string(),
                    "--json".to_string(),
                    "number,title,state,labels,url,updatedAt".to_string(),
                ];
                if !query.is_empty() {
                    cmd_args.push("--search".to_string());
                    cmd_args.push(query);
                }
                let req = SpawnRequest::new(gh, cmd_args, cwd).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
                match mighty_platform::process::capture(host.processes.as_ref(), req, None, Duration::from_secs(10), 512_000).await {
                    Ok(captured) if captured.exit_code == 0 => {
                        let list: Vec<Value> = serde_json::from_slice(&captured.stdout).unwrap_or_default();
                        let items: Vec<Value> = list
                            .iter()
                            .filter_map(|i| {
                                let num = i.get("number")?.as_i64()?;
                                let title = i.get("title")?.as_str()?.to_string();
                                let tags: Vec<String> = i
                                    .get("labels")
                                    .and_then(Value::as_array)
                                    .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
                                    .unwrap_or_default();
                                Some(json!({
                                    "source": "github_issue",
                                    "key": format!("#{num}"),
                                    "title": title,
                                    "stage": i.get("state").and_then(Value::as_str).map(|s| s.to_ascii_lowercase()),
                                    "tags": tags,
                                    "url": i.get("url").and_then(Value::as_str),
                                    "updatedAt": i.get("updatedAt").and_then(Value::as_str),
                                }))
                            })
                            .collect();
                        Ok(json!({ "items": items }))
                    }
                    Ok(captured) => Ok(json!({ "items": [], "error": String::from_utf8_lossy(&captured.stderr).trim() })),
                    Err(err) => Ok(json!({ "items": [], "error": err.to_string() })),
                }
            }
            _ => Err("Unknown tab.".into()),
        }
    });

    methods.add("integrationsDetail", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let source = text(&args, "source").unwrap_or("mightywork_task");
        let key = text(&args, "key").unwrap_or("");
        let (detail, _, _) = fetch_detail(&host, &ws, source, key).await?;
        Ok(json!({ "item": detail }))
    });

    methods.add("integrationsStart", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let mode = text(&args, "mode").unwrap_or("worktree");
        let auto_send = flag(&args, "autoSend");
        let source = text(&args, "source").unwrap_or("mightywork_task");
        let key = text(&args, "key").unwrap_or("");

        let (detail, linked, formatted_context) = fetch_detail(&host, &ws, source, key).await?;
        let item_key = detail.get("key").and_then(Value::as_str).unwrap_or(key);
        let item_title = detail.get("title").and_then(Value::as_str).unwrap_or(item_key);
        let pane_title: String = format!("{item_key} · {item_title}").chars().take(120).collect();
        let prompt = format!("Please implement the following task and verify all requirements:\n\n{formatted_context}");

        let provider = {
            let view = host.view();
            view.snapshot
                .active_session_id
                .as_deref()
                .and_then(|id| view.snapshot.session(id))
                .filter(|s| s.kind() == "claude")
                .map(|s| s.provider().to_string())
                .unwrap_or_else(|| "claude".to_string())
        };

        let session_id = make_task_session_id(item_key);
        let persist_linked = matches!(linked.source.as_str(), "mightywork" | "github").then(|| linked.clone());
        let initial_draft = if auto_send { String::new() } else { prompt.clone() };

        let created = if mode == "worktree" {
            crate::pane_worktree::open_task_in_worktree(
                &host,
                &ws,
                &provider,
                session_id.clone(),
                pane_title,
                initial_draft,
                persist_linked.clone(),
            )
            .await?
        } else {
            let shaped_id = session_id.clone();
            let shaped_linked = persist_linked.clone();
            crate::frame::add_pane_to(&host, Some(ws.id()), "claude", &provider, None, move |pane| RunSession {
                id: Some(shaped_id),
                title: Some(pane_title),
                title_mode: Some("fixed".into()),
                draft: Some(initial_draft),
                linked_task: shaped_linked,
                ..pane
            })?
        };

        if !created {
            return Ok(json!({ "paneId": session_id, "started": false }));
        }

        {
            let mut view = host.view();
            let _ = crate::panes::ensure(&mut view, Some(&session_id));
            view.sync();
        }
        host.schedule();

        if let Some(ref task_link) = persist_linked {
            let comment = if mode == "worktree" {
                Some(format!("Started implementation in Mighty Code worktree branch `mighty/{session_id}`."))
            } else {
                Some("Started implementation in Mighty Code.".to_string())
            };
            spawn_stage_sync(&host, ws.id(), task_link, "in_progress", comment, Some(session_id.clone()));
        }

        let mut started_run = false;
        if auto_send {
            started_run = crate::composer::start_run(&host, &session_id, &prompt, vec![], None, false, None).await.is_ok();
        }

        Ok(json!({ "paneId": session_id, "started": started_run }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_and_sse_mcp_payloads() {
        let plain = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"total\":1}"}]}}"#;
        let rpc = parse_mcp_http_payload(plain).unwrap();
        assert_eq!(extract_tool_result(&rpc).unwrap()["total"], 1);

        let sse = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"structuredContent\":{\"key\":\"MC-11\"}}}\r\n\r\n";
        let rpc2 = parse_mcp_http_payload(sse).unwrap();
        assert_eq!(extract_tool_result(&rpc2).unwrap()["key"], "MC-11");
    }

    #[test]
    fn detects_project_key_and_builds_safe_worktree_session_ids() {
        let dir = std::env::temp_dir().join(format!("mighty-int-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "# MightyWork Rules\n- **기본 프로젝트 키 (`project`)**: `MC`\n").unwrap();
        assert_eq!(detect_mightywork_project(&dir).as_deref(), Some("MC"));
        let _ = std::fs::remove_dir_all(&dir);

        let id = make_task_session_id("MC-11");
        assert!(id.starts_with("mc-11-") && mighty_core::wire::identifier(&id));
        let gh_id = make_task_session_id("#42");
        assert!(gh_id.starts_with("42-") && mighty_core::wire::identifier(&gh_id));
    }

    #[test]
    fn formats_context_with_checklist() {
        let ctx = format_item_context(
            "mightywork_task",
            "MC-11",
            "MightyWork Integration",
            Some("in_progress"),
            Some("https://example.test/MC-11"),
            "Implement picker.",
            &[("Step 1".into(), true), ("Step 2".into(), false)],
        );
        assert!(ctx.contains("[MightyWork MC-11] MightyWork Integration"));
        assert!(ctx.contains("- [x] Step 1") && ctx.contains("- [ ] Step 2"));
    }
}

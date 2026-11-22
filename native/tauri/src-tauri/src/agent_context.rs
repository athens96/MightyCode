//! Unified Agent Context & Multi-CLI MCP Hub (`MC-10`):
//! - Part A: Workspace instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) read, divergence check, atomic write, and 1-click sync
//! - Part B: Built-in Mighty Code runtime MCP status (`mighty-terminal`, `mighty-delegation`, `mighty-workspace`), external MCP scanner
//!   across Claude Code, Codex, and Antigravity (`gemini` / `agy`), OAuth token detection, and Claude Hooks scanner
//! - Part C: One-click multi-CLI MCP registration (preserving existing JSON/TOML keys), removal, OAuth 2.1 + PKCE `S256` browser login,
//!   and `toolkit.json` template persistence

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use mighty_core::model::Workspace;
use mighty_platform::http::HttpsRequest;
use mighty_tools::toolkit::file::{self as toolkit_file, ToolkitFileEntry, ToolkitInstallSpec};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tauri::Manager as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::host::{Host, Registry};
use crate::methods::{flag, text};

/// Allowed workspace instruction files in provider display order.
pub const INSTRUCTION_FILES: [(&str, &str); 3] = [("claude", "CLAUDE.md"), ("codex", "AGENTS.md"), ("gemini", "GEMINI.md")];

/// Maximum size in bytes (`512 KiB`) for each workspace instruction file.
pub const MAX_INSTRUCTION_BYTES: usize = 512 * 1024;

/// Maximum config file size (`8 MiB`) inspected by the MCP / Hook scanner.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// Built-in runtime MCP servers managed by Mighty Code itself (never allowed as external server names).
pub const RESERVED_MCP_NAMES: [&str; 3] = ["mighty-terminal", "mighty-delegation", "mighty-workspace"];

/// File storing OAuth 2.1 tokens acquired through `agentContextMcpOAuthLogin`.
pub const MCP_OAUTH_TOKENS_FILE: &str = "mcp-oauth-tokens.json";

/// Existing MightyWork OAuth token file managed by `integrations.rs`.
const MIGHTYWORK_AUTH_FILE: &str = "mightywork-auth.json";
const DEFAULT_MIGHTYWORK_URL: &str = "https://mightywork.duckdns.org/mcp";
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionFileState {
    pub provider: String,
    pub name: String,
    pub exists: bool,
    pub text: String,
    pub byte_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinMcpServerState {
    pub name: String,
    pub active: bool,
    pub providers: Vec<String>,
    pub tool_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalMcpServerState {
    pub provider: String,
    pub name: String,
    pub scope: String,
    pub transport: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    pub source: String,
    pub oauth_connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeHookState {
    pub event: String,
    pub commands: Vec<String>,
    pub scope: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextPayload {
    pub workspace_path: String,
    pub diverged: bool,
    pub files: Vec<InstructionFileState>,
    pub builtin_mcp_servers: Vec<BuiltinMcpServerState>,
    pub external_mcp_servers: Vec<ExternalMcpServerState>,
    pub claude_hooks: Vec<ClaudeHookState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUpsertSpec {
    Http { url: String, bearer_token_env_var: Option<String> },
    Stdio { command: String, args: Vec<String> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMcpOAuthToken {
    #[serde(default)]
    pub server_name: String,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub token_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct McpOAuthTokenBook {
    #[serde(default)]
    tokens: BTreeMap<String, StoredMcpOAuthToken>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CompatMightyWorkAuth {
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

/// Environment paths injected into scanner and writer routines so unit tests run in isolated temp directories.
#[derive(Debug, Clone)]
pub struct ScanPaths {
    pub home_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub claude_keychain_blob: Option<String>,
}

impl ScanPaths {
    pub fn from_host(host: &Host) -> ScanPaths {
        let home_raw = crate::accounts::home();
        let home_dir = (!home_raw.trim().is_empty()).then(|| PathBuf::from(home_raw));
        let codex_home = std::env::var("CODEX_HOME").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from);
        let claude_keychain_blob = if host.smoke.is_some() {
            None
        } else {
            crate::platform::claude_credentials().and_then(|reader| reader.read(None, false).ok().flatten())
        };
        ScanPaths { home_dir, codex_home, data_dir: host.data_dir.clone(), claude_keychain_blob }
    }

    fn codex_user_config(&self) -> Option<(PathBuf, &'static str)> {
        if let Some(custom) = &self.codex_home {
            return Some((custom.join("config.toml"), "$CODEX_HOME/config.toml"));
        }
        self.home_dir.as_ref().map(|home| (home.join(".codex").join("config.toml"), "~/.codex/config.toml"))
    }
}

fn mcp_name_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").expect("valid MCP name regex"))
}

pub fn is_reserved_mcp_name(name: &str) -> bool {
    RESERVED_MCP_NAMES.iter().any(|reserved| reserved.eq_ignore_ascii_case(name.trim()))
}

pub fn validate_mcp_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if is_reserved_mcp_name(trimmed) {
        return Err(format!("MCP server name '{trimmed}' is reserved for Mighty Code built-in runtime tools."));
    }
    if !mcp_name_regex().is_match(trimmed) {
        return Err("MCP server name must match ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$.".into());
    }
    Ok(trimmed.to_string())
}

pub fn validate_mcp_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if !toolkit_file::valid_mcp_url(trimmed) {
        return Err("MCP URL must start with https:// (or loopback http://127.0.0.1 / http://localhost).".into());
    }
    Ok(trimmed.to_string())
}

pub fn validate_bearer_env_var(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(val) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if !toolkit_file::valid_env_var_name(val) {
        return Err("Bearer token environment variable must match ^[A-Za-z_][A-Za-z0-9_]{0,127}$ (never pass a raw secret token).".into());
    }
    Ok(Some(val.to_string()))
}

pub fn validate_stdio_command(command: &str, args: &[String]) -> Result<(String, Vec<String>), String> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.chars().any(char::is_control) {
        return Err("MCP stdio command must be a non-empty executable name or path.".into());
    }
    if trimmed.contains(';') || trimmed.contains('|') || trimmed.contains('&') || trimmed.contains('`') {
        return Err("MCP stdio command must not contain shell metacharacters.".into());
    }
    if Path::new(trimmed).components().any(|part| matches!(part, Component::ParentDir)) {
        return Err("MCP stdio command must not contain '..' path traversal.".into());
    }
    if args.len() > 64 {
        return Err("MCP stdio arguments must not exceed 64 items.".into());
    }
    let mut cleaned_args = Vec::with_capacity(args.len());
    for arg in args {
        if arg.contains('\0') {
            return Err("MCP stdio arguments must not contain null bytes.".into());
        }
        cleaned_args.push(arg.clone());
    }
    Ok((trimmed.to_string(), cleaned_args))
}

pub fn validate_providers(raw: &[String]) -> Result<Vec<String>, String> {
    let mut selected = BTreeSet::new();
    for item in raw {
        let normalized = match item.trim().to_ascii_lowercase().as_str() {
            "claude" => "claude",
            "codex" => "codex",
            "gemini" | "agy" | "antigravity" => "gemini",
            other => return Err(format!("Unsupported CLI provider '{other}'. Expected 'claude', 'codex', or 'gemini'.")),
        };
        selected.insert(normalized);
    }
    if selected.is_empty() {
        return Err("Select at least one target CLI ('claude', 'codex', or 'gemini').".into());
    }
    let mut ordered = Vec::with_capacity(selected.len());
    for canonical in ["claude", "codex", "gemini"] {
        if selected.contains(canonical) {
            ordered.push(canonical.to_string());
        }
    }
    Ok(ordered)
}

pub fn validate_scope(raw: &str) -> Result<&'static str, String> {
    match raw.trim() {
        "user" => Ok("user"),
        "project" => Ok("project"),
        other => Err(format!("Invalid MCP scope '{other}'. Expected 'user' or 'project'.")),
    }
}

pub fn validate_instruction_name(name: &str) -> Result<&'static str, String> {
    INSTRUCTION_FILES
        .iter()
        .find(|(_, allowed)| *allowed == name)
        .map(|(_, allowed)| *allowed)
        .ok_or_else(|| format!("Instruction file '{name}' is not allowed. Expected CLAUDE.md, AGENTS.md, or GEMINI.md."))
}

// ── Part A: Unified Instruction Files ─────────────────────────────────────────────────────────────────────────────────────

fn ensure_safe_instruction_target(canonical_root: &Path, file_name: &str) -> Result<PathBuf, String> {
    let allowed = validate_instruction_name(file_name)?;
    let candidate = canonical_root.join(allowed);
    if let Ok(sym_meta) = std::fs::symlink_metadata(&candidate) {
        if sym_meta.file_type().is_symlink() {
            let resolved = std::fs::canonicalize(&candidate).map_err(|_| "Refused dangling or unreadable symlink.".to_string())?;
            if !resolved.starts_with(canonical_root) {
                return Err("Refused symlink escaping the workspace root.".into());
            }
        }
        if sym_meta.is_dir() {
            return Err(format!("'{allowed}' is a directory, not a regular file."));
        }
    }
    Ok(candidate)
}

/// Compares trimmed non-empty contents of existing instruction files (matching Swift 0.4 `AgentContext.diverged`).
pub fn files_diverged(files: &[InstructionFileState]) -> bool {
    let distinct: HashSet<&str> = files.iter().filter(|f| f.exists).map(|f| f.text.trim()).filter(|t| !t.is_empty()).collect();
    distinct.len() > 1
}

pub fn read_instruction_files(root: &Path) -> Result<(Vec<InstructionFileState>, bool), String> {
    let canonical_root = std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?;
    let mut out = Vec::with_capacity(INSTRUCTION_FILES.len());
    for (provider, name) in INSTRUCTION_FILES {
        let candidate = ensure_safe_instruction_target(&canonical_root, name)?;
        if !candidate.exists() {
            out.push(InstructionFileState {
                provider: provider.to_string(),
                name: name.to_string(),
                exists: false,
                text: String::new(),
                byte_len: 0,
            });
            continue;
        }
        let meta = std::fs::metadata(&candidate).map_err(|e| format!("Cannot read metadata for {name}: {e}"))?;
        if !meta.is_file() {
            return Err(format!("'{name}' is not a regular file."));
        }
        if meta.len() > MAX_INSTRUCTION_BYTES as u64 {
            return Err(format!("'{name}' exceeds the 512 KiB instruction file limit."));
        }
        let bytes = std::fs::read(&candidate).map_err(|e| format!("Cannot read {name}: {e}"))?;
        if bytes.len() > MAX_INSTRUCTION_BYTES {
            return Err(format!("'{name}' exceeds the 512 KiB instruction file limit."));
        }
        let decoded = String::from_utf8(bytes).map_err(|_| format!("'{name}' is not valid UTF-8 text."))?;
        let byte_len = decoded.len();
        out.push(InstructionFileState { provider: provider.to_string(), name: name.to_string(), exists: true, text: decoded, byte_len });
    }
    let diverged = files_diverged(&out);
    Ok((out, diverged))
}

/// Writes `bytes` to `target` without a half-written file ever being seen: a temporary file beside it, flushed to disk, then renamed
/// over it. A target that is a symlink (a dotfiles manager's link, `CLAUDE.md -> AGENTS.md`) is written where it points, so the link
/// stays a link. The temporary file is created owner-only and then given the target's own permissions (or owner-only for a file
/// holding secrets), so a file the user keeps private never becomes readable by others, not even for a moment.
fn atomic_write_bytes(target: &Path, bytes: &[u8], mode_0600: bool) -> Result<(), String> {
    use std::io::Write as _;
    let target = match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(target).map_err(|_| "Refused dangling or unreadable symlink.".to_string())?
        }
        _ => target.to_path_buf(),
    };
    let parent = target.parent().ok_or_else(|| "Invalid target file path.".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let existing = std::fs::metadata(&target).ok();
    let file_label = target.file_name().and_then(|n| n.to_str()).unwrap_or("config");
    let nonce = uuid::Uuid::new_v4().simple();
    let temp = parent.join(format!(".{file_label}.{nonce}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| -> std::io::Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if mode_0600 { 0o600 } else { existing.as_ref().map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644) };
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = (mode_0600, &existing);
        Ok(())
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(err.to_string());
    }
    if let Err(err) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(err.to_string());
    }
    Ok(())
}

/// What the page read of each instruction file, by name (`None`: it did not exist), sent with a save or a sync.
pub type ExpectedTexts = std::collections::HashMap<String, Option<String>>;

/// Refuses a write when a file it would replace changed on disk since the page read it (an agent appending to CLAUDE.md while the
/// panel was open): the person reloads and decides, instead of the change being overwritten unseen.
pub fn check_unchanged(root: &Path, expected: &ExpectedTexts, names: &[&str]) -> Result<(), String> {
    let canonical_root = std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?;
    for name in names {
        let Some(read) = expected.get(*name) else { continue };
        let path = ensure_safe_instruction_target(&canonical_root, name)?;
        let now = if path.is_file() { Some(std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {name}: {e}"))?) } else { None };
        if now.as_deref() != read.as_deref() {
            return Err(format!("'{name}' changed on disk since it was opened. Reload it before saving."));
        }
    }
    Ok(())
}

fn expected_texts(args: &Value) -> ExpectedTexts {
    args.get("expected")
        .and_then(Value::as_object)
        .map(|map| map.iter().map(|(name, text)| (name.clone(), text.as_str().map(str::to_string))).collect())
        .unwrap_or_default()
}

pub fn write_instruction_file(root: &Path, name: &str, content: &str) -> Result<(), String> {
    let allowed = validate_instruction_name(name)?;
    if content.len() > MAX_INSTRUCTION_BYTES {
        return Err(format!("'{allowed}' exceeds the 512 KiB instruction file limit."));
    }
    let canonical_root = std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?;
    let target = ensure_safe_instruction_target(&canonical_root, allowed)?;
    atomic_write_bytes(&target, content.as_bytes(), false)
}

pub fn sync_instruction_files(
    root: &Path,
    source_name: &str,
    content_override: Option<&str>,
    target_names: Option<&[String]>,
) -> Result<(), String> {
    let allowed_source = validate_instruction_name(source_name)?;
    let canonical_root = std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?;

    let source_text = match content_override {
        Some(text) => {
            if text.len() > MAX_INSTRUCTION_BYTES {
                return Err(format!("'{allowed_source}' exceeds the 512 KiB instruction file limit."));
            }
            text.to_string()
        }
        None => {
            let src_path = ensure_safe_instruction_target(&canonical_root, allowed_source)?;
            if !src_path.is_file() {
                return Err(format!("Source instruction file '{allowed_source}' does not exist."));
            }
            let meta = std::fs::metadata(&src_path).map_err(|e| e.to_string())?;
            if meta.len() > MAX_INSTRUCTION_BYTES as u64 {
                return Err(format!("'{allowed_source}' exceeds the 512 KiB instruction file limit."));
            }
            std::fs::read_to_string(&src_path).map_err(|e| e.to_string())?
        }
    };

    let targets: Vec<&'static str> = match target_names {
        Some(names) if !names.is_empty() => {
            let mut list = Vec::with_capacity(names.len() + 1);
            if content_override.is_some() {
                list.push(allowed_source);
            }
            for raw in names {
                let valid = validate_instruction_name(raw)?;
                if !list.contains(&valid) {
                    list.push(valid);
                }
            }
            list
        }
        _ => INSTRUCTION_FILES.iter().map(|(_, n)| *n).collect(),
    };

    let mut dests = Vec::with_capacity(targets.len());
    for target_name in &targets {
        dests.push(ensure_safe_instruction_target(&canonical_root, target_name)?);
    }
    // A file that is a link to one already written (`CLAUDE.md -> AGENTS.md`) is that file: written once, its link kept.
    let mut written: HashSet<PathBuf> = HashSet::new();
    for dest in dests {
        let real = std::fs::canonicalize(&dest).unwrap_or_else(|_| dest.clone());
        if !written.insert(real) {
            continue;
        }
        atomic_write_bytes(&dest, source_text.as_bytes(), false)?;
    }
    Ok(())
}

// ── Part B: Built-in System MCPs + External Multi-CLI MCP Scanner & Claude Hooks ──────────────────────────────────────────

pub fn builtin_mcp_servers(host: &Host) -> Vec<BuiltinMcpServerState> {
    let cross_workspace = host.view().snapshot.cross_workspace_lookup_on();
    let delegation_active = crate::settings::general::delegation_on(host) && host.delegation_live();
    let providers = vec!["claude".to_string(), "codex".to_string()];
    vec![
        BuiltinMcpServerState { name: "mighty-terminal".into(), active: true, providers: providers.clone(), tool_count: 4 },
        BuiltinMcpServerState { name: "mighty-delegation".into(), active: delegation_active, providers: providers.clone(), tool_count: 6 },
        BuiltinMcpServerState { name: "mighty-workspace".into(), active: cross_workspace, providers, tool_count: 4 },
    ]
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn normalize_url_key(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn is_mightywork_endpoint(url: &str, saved_mw_url: Option<&str>) -> bool {
    let norm = normalize_url_key(url);
    if norm.is_empty() {
        return false;
    }
    if norm == normalize_url_key(DEFAULT_MIGHTYWORK_URL) {
        return true;
    }
    if let Some(saved) = saved_mw_url.filter(|s| !s.trim().is_empty())
        && norm == normalize_url_key(saved)
    {
        return true;
    }
    // Only the default server or the one the integration is set to: a host that merely contains "mightywork" (a staging or
    // look-alike server) is another server, and never takes the integration's token or its scopes.
    false
}

/// The MightyWork server the integration is signed in to (`mightywork-auth.json`), if any.
fn saved_mightywork_url(data_dir: &Path) -> Option<String> {
    let bytes = std::fs::read(data_dir.join(MIGHTYWORK_AUTH_FILE)).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?.get("server_url")?.as_str().map(str::to_string)
}

struct OAuthTokenIndex {
    hub_tokens: Vec<StoredMcpOAuthToken>,
    claude_entries: Vec<(String, String)>,
    mightywork_auth: Option<CompatMightyWorkAuth>,
}

impl OAuthTokenIndex {
    fn load(paths: &ScanPaths) -> OAuthTokenIndex {
        let hub_path = paths.data_dir.join(MCP_OAUTH_TOKENS_FILE);
        let hub_tokens = std::fs::read(&hub_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<McpOAuthTokenBook>(&b).ok())
            .map(|book| {
                let now = now_ms();
                book.tokens
                    .into_values()
                    .filter(|t| {
                        !t.access_token.trim().is_empty()
                            && (t.expires_at_ms.is_none_or(|exp| exp > now + 15_000) || t.refresh_token.is_some())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut claude_entries = Vec::new();
        let mut collect_claude_blob = |raw: &str| {
            let Ok(root) = serde_json::from_str::<Value>(raw) else { return };
            let Some(map) = root.get("mcpOAuth").and_then(Value::as_object) else { return };
            let now = Utc::now().timestamp_millis();
            for entry in map.values() {
                let tok = entry.get("accessToken").and_then(Value::as_str).unwrap_or("").trim();
                if tok.is_empty() {
                    continue;
                }
                let expires = entry.get("expiresAt").and_then(Value::as_f64).map(|n| n as i64).unwrap_or(i64::MAX);
                let has_refresh = entry.get("refreshToken").and_then(Value::as_str).is_some_and(|r| !r.trim().is_empty());
                if expires > now + 15_000 || has_refresh {
                    let s_name = entry.get("serverName").and_then(Value::as_str).unwrap_or("").to_string();
                    let s_url = entry.get("serverUrl").and_then(Value::as_str).unwrap_or("").to_string();
                    claude_entries.push((s_name, s_url));
                }
            }
        };

        if let Some(blob) = &paths.claude_keychain_blob {
            collect_claude_blob(blob);
        }
        if let Some(home) = &paths.home_dir {
            let cred_path = home.join(".claude").join(".credentials.json");
            if let Ok(raw) = read_bounded_string(&cred_path) {
                collect_claude_blob(&raw);
            }
        }

        let mw_path = paths.data_dir.join(MIGHTYWORK_AUTH_FILE);
        let mightywork_auth =
            std::fs::read(&mw_path).ok().and_then(|b| serde_json::from_slice::<CompatMightyWorkAuth>(&b).ok()).filter(|a| {
                !a.access_token.trim().is_empty()
                    && (a.expires_at_ms.is_none_or(|exp| exp > now_ms() + 15_000) || a.refresh_token.is_some())
            });

        OAuthTokenIndex { hub_tokens, claude_entries, mightywork_auth }
    }

    fn has_token(&self, name: &str, url: Option<&str>, has_inline_auth: bool) -> bool {
        if has_inline_auth {
            return true;
        }
        let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
            return false;
        };
        let norm_url = normalize_url_key(url);
        if self
            .hub_tokens
            .iter()
            .any(|t| t.server_name == name || (!t.server_url.is_empty() && normalize_url_key(&t.server_url) == norm_url))
        {
            return true;
        }
        if self.claude_entries.iter().any(|(s_name, s_url)| s_name == name || (!s_url.is_empty() && normalize_url_key(s_url) == norm_url)) {
            return true;
        }
        if let Some(mw) = &self.mightywork_auth
            && (name.eq_ignore_ascii_case("mightywork") || is_mightywork_endpoint(url, Some(&mw.server_url)))
        {
            return true;
        }
        false
    }
}

fn read_bounded_string(path: &Path) -> Result<String, ()> {
    let meta = std::fs::metadata(path).map_err(|_| ())?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return Err(());
    }
    let bytes = std::fs::read(path).map_err(|_| ())?;
    String::from_utf8(bytes).map_err(|_| ())
}

fn read_json_object_opt(path: &Path) -> Option<Map<String, Value>> {
    let raw = read_bounded_string(path).ok()?;
    let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    serde_json::from_str::<Value>(stripped).ok()?.as_object().cloned()
}

fn parse_json_mcp_servers(
    container: &Map<String, Value>,
    provider: &str,
    scope: &str,
    source: &str,
    oauth: &OAuthTokenIndex,
) -> Vec<ExternalMcpServerState> {
    let Some(servers) = container.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        if is_reserved_mcp_name(name) {
            continue;
        }
        let Some(entry) = servers.get(name).and_then(Value::as_object) else { continue };
        let url = entry
            .get("url")
            .or_else(|| entry.get("httpUrl"))
            .or_else(|| entry.get("serverUrl"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        let command = entry.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        let args: Vec<String> = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let bearer_token_env_var =
            entry.get("bearerTokenEnvVar").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let has_inline_auth = entry
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|h| h.iter().find(|(k, _)| k.eq_ignore_ascii_case("authorization")).and_then(|(_, v)| v.as_str()))
            .is_some_and(|v| !v.trim().is_empty() && !v.contains("${"));

        let (transport, target) = if let Some(ref endpoint) = url {
            ("http".to_string(), endpoint.clone())
        } else {
            let mut parts = Vec::new();
            if let Some(ref cmd) = command {
                parts.push(cmd.clone());
            }
            parts.extend(args.iter().cloned());
            ("stdio".to_string(), parts.join(" "))
        };
        let oauth_connected = oauth.has_token(name, url.as_deref(), has_inline_auth);
        result.push(ExternalMcpServerState {
            provider: provider.to_string(),
            name: name.clone(),
            scope: scope.to_string(),
            transport,
            target,
            url,
            command,
            args,
            bearer_token_env_var,
            source: source.to_string(),
            oauth_connected,
        });
    }
    result
}

fn parse_toml_mcp_servers(path: &Path, scope: &str, source: &str, oauth: &OAuthTokenIndex) -> Vec<ExternalMcpServerState> {
    let Ok(raw) = read_bounded_string(path) else { return Vec::new() };
    let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let Ok(doc) = stripped.parse::<toml::Table>() else { return Vec::new() };
    let Some(servers) = doc.get("mcp_servers").and_then(toml::Value::as_table) else { return Vec::new() };

    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        if is_reserved_mcp_name(name) {
            continue;
        }
        let Some(entry) = servers.get(name).and_then(toml::Value::as_table) else { continue };
        let url = entry.get("url").and_then(toml::Value::as_str).map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
        let command = entry.get("command").and_then(toml::Value::as_str).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        let args: Vec<String> = entry
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|items| items.iter().filter_map(toml::Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let bearer_token_env_var =
            entry.get("bearer_token_env_var").and_then(toml::Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);

        let (transport, target) = if let Some(ref endpoint) = url {
            ("http".to_string(), endpoint.clone())
        } else {
            let mut parts = Vec::new();
            if let Some(ref cmd) = command {
                parts.push(cmd.clone());
            }
            parts.extend(args.iter().cloned());
            ("stdio".to_string(), parts.join(" "))
        };
        let oauth_connected = oauth.has_token(name, url.as_deref(), false);
        result.push(ExternalMcpServerState {
            provider: "codex".into(),
            name: name.clone(),
            scope: scope.to_string(),
            transport,
            target,
            url,
            command,
            args,
            bearer_token_env_var,
            source: source.to_string(),
            oauth_connected,
        });
    }
    result
}

pub fn scan_external_mcp_servers(workspace_root: Option<&Path>, paths: &ScanPaths) -> Vec<ExternalMcpServerState> {
    let oauth = OAuthTokenIndex::load(paths);
    let mut out = Vec::new();

    // 1. Claude: <root>/.mcp.json (project), ~/.claude.json projects[root].mcpServers (project), ~/.claude.json mcpServers (user).
    if let Some(root) = workspace_root
        && let Some(obj) = read_json_object_opt(&root.join(".mcp.json"))
    {
        out.extend(parse_json_mcp_servers(&obj, "claude", "project", ".mcp.json", &oauth));
    }
    if let Some(home) = &paths.home_dir
        && let Some(claude_obj) = read_json_object_opt(&home.join(".claude.json"))
    {
        if let Some(root) = workspace_root
            && let Some(projects) = claude_obj.get("projects").and_then(Value::as_object)
        {
            for candidate_key in claude_project_keys(root) {
                if let Some(proj_obj) = projects.get(&candidate_key).and_then(Value::as_object) {
                    out.extend(parse_json_mcp_servers(proj_obj, "claude", "project", "~/.claude.json (local)", &oauth));
                }
            }
        }
        out.extend(parse_json_mcp_servers(&claude_obj, "claude", "user", "~/.claude.json", &oauth));
    }

    // 2. Codex: <root>/.codex/config.toml (project) and $CODEX_HOME/config.toml or ~/.codex/config.toml (user).
    if let Some(root) = workspace_root {
        out.extend(parse_toml_mcp_servers(&root.join(".codex").join("config.toml"), "project", ".codex/config.toml", &oauth));
    }
    if let Some((codex_user_path, codex_source)) = paths.codex_user_config() {
        out.extend(parse_toml_mcp_servers(&codex_user_path, "user", codex_source, &oauth));
    }

    // 3. Antigravity (gemini / agy): project + user mcp_config.json and settings.json (never ~/.gemini/antigravity-cli/settings.json).
    if let Some(root) = workspace_root {
        if let Some(obj) = read_json_object_opt(&root.join(".gemini").join("mcp_config.json")) {
            out.extend(parse_json_mcp_servers(&obj, "gemini", "project", ".gemini/mcp_config.json", &oauth));
        }
        if let Some(obj) = read_json_object_opt(&root.join(".gemini").join("settings.json")) {
            out.extend(parse_json_mcp_servers(&obj, "gemini", "project", ".gemini/settings.json", &oauth));
        }
    }
    if let Some(home) = &paths.home_dir {
        if let Some(obj) = read_json_object_opt(&home.join(".gemini").join("config").join("mcp_config.json")) {
            out.extend(parse_json_mcp_servers(&obj, "gemini", "user", "~/.gemini/config/mcp_config.json", &oauth));
        }
        if let Some(obj) = read_json_object_opt(&home.join(".gemini").join("settings.json")) {
            out.extend(parse_json_mcp_servers(&obj, "gemini", "user", "~/.gemini/settings.json", &oauth));
        }
    }

    out
}

fn parse_claude_hooks_object(container: &Map<String, Value>, scope: &str, source: &str) -> Vec<ClaudeHookState> {
    let Some(hooks_obj) = container.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut events: Vec<&String> = hooks_obj.keys().collect();
    events.sort();
    let mut result = Vec::new();
    for event in events {
        let Some(groups) = hooks_obj.get(event).and_then(Value::as_array) else { continue };
        let mut commands = Vec::new();
        for group in groups {
            let Some(group_obj) = group.as_object() else { continue };
            if let Some(nested) = group_obj.get("hooks").and_then(Value::as_array) {
                for item in nested {
                    if let Some(cmd) = item.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
                        commands.push(cmd.to_string());
                    }
                }
            } else if let Some(cmd) = group_obj.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
                commands.push(cmd.to_string());
            }
        }
        if !commands.is_empty() {
            result.push(ClaudeHookState { event: event.clone(), commands, scope: scope.to_string(), source: source.to_string() });
        }
    }
    result
}

pub fn scan_claude_hooks(workspace_root: Option<&Path>, home_dir: Option<&Path>) -> Vec<ClaudeHookState> {
    let mut out = Vec::new();
    if let Some(root) = workspace_root {
        if let Some(local) = read_json_object_opt(&root.join(".claude").join("settings.local.json")) {
            out.extend(parse_claude_hooks_object(&local, "local", ".claude/settings.local.json"));
        }
        if let Some(project) = read_json_object_opt(&root.join(".claude").join("settings.json")) {
            out.extend(parse_claude_hooks_object(&project, "project", ".claude/settings.json"));
        }
    }
    if let Some(home) = home_dir
        && let Some(user) = read_json_object_opt(&home.join(".claude").join("settings.json"))
    {
        out.extend(parse_claude_hooks_object(&user, "user", "~/.claude/settings.json"));
    }
    out
}

// ── Part C: One-Click Multi-CLI Upsert, Remove, OAuth 2.1 Login & Toolkit Save ────────────────────────────────────────────

/// An agent's config larger than this is not edited by the panel (`~/.claude.json` holds project history and can grow, but not this far).
const MAX_AGENT_CONFIG_BYTES: u64 = 32 * 1024 * 1024;

/// The keys Claude Code may file a project under in `~/.claude.json`: the path as given, its canonical form, and on Windows both with
/// forward slashes (`C:/...`), which is how Claude Code writes them there.
fn claude_project_keys(root: &Path) -> Vec<String> {
    let mut keys = vec![root.to_string_lossy().into_owned()];
    if let Ok(canonical) = std::fs::canonicalize(root) {
        let text = canonical.to_string_lossy().into_owned();
        keys.push(text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text));
    }
    if cfg!(windows) {
        let slashed: Vec<String> = keys.iter().map(|k| k.replace('\\', "/")).collect();
        keys.extend(slashed);
    }
    let mut seen = HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys
}

fn read_existing_json_for_edit(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_AGENT_CONFIG_BYTES) {
        return Err(format!("{} is larger than 32 MiB; edit it by hand.", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8.", path.display()))?;
    let stripped = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if stripped.trim().is_empty() {
        return Ok(Map::new());
    }
    let val: Value = serde_json::from_str(stripped).map_err(|e| format!("Cannot parse {}: {e}", path.display()))?;
    val.as_object().cloned().ok_or_else(|| format!("Expected JSON object at root of {}.", path.display()))
}

fn write_json_object(path: &Path, obj: &Map<String, Value>, mode_0600: bool) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(obj.clone())).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_write_bytes(path, &bytes, mode_0600)
}

fn upsert_json_mcp_entry(path: &Path, name: &str, entry: Value, mode_0600: bool) -> Result<(), String> {
    let mut root = read_existing_json_for_edit(path)?;
    let servers_val = root.entry("mcpServers".to_string()).or_insert_with(|| Value::Object(Map::new()));
    let Some(servers_obj) = servers_val.as_object_mut() else {
        return Err(format!("'mcpServers' in {} is not a JSON object.", path.display()));
    };
    // A server already there keeps what this form does not set (its env, headers, timeouts, enabled tools); only how it is reached
    // (its transport, address, command and arguments) is replaced.
    let merged = match (servers_obj.get(name).and_then(Value::as_object), entry) {
        (Some(existing), Value::Object(new)) => {
            let mut kept = existing.clone();
            for key in TRANSPORT_KEYS {
                kept.remove(*key);
            }
            for (key, value) in new {
                kept.insert(key, value);
            }
            Value::Object(kept)
        }
        (_, entry) => entry,
    };
    servers_obj.insert(name.to_string(), merged);
    write_json_object(path, &root, mode_0600)
}

/// The keys that say how an MCP server is reached, replaced together when it is registered again.
const TRANSPORT_KEYS: &[&str] = &["type", "url", "httpUrl", "serverUrl", "command", "args"];

fn remove_json_mcp_entry(path: &Path, name: &str, mode_0600: bool) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut root = read_existing_json_for_edit(path)?;
    let removed = root.get_mut("mcpServers").and_then(Value::as_object_mut).and_then(|servers| servers.remove(name)).is_some();
    if removed {
        write_json_object(path, &root, mode_0600)?;
    }
    Ok(removed)
}

fn remove_claude_local_project_entry(claude_json_path: &Path, root: &Path, name: &str) -> Result<bool, String> {
    if !claude_json_path.exists() {
        return Ok(false);
    }
    let mut doc = read_existing_json_for_edit(claude_json_path)?;
    let mut changed = false;
    if let Some(projects) = doc.get_mut("projects").and_then(Value::as_object_mut) {
        for key in claude_project_keys(root) {
            if let Some(proj_servers) = projects.get_mut(&key).and_then(Value::as_object_mut).and_then(|p| p.get_mut("mcpServers"))
                && let Some(map) = proj_servers.as_object_mut()
                && map.remove(name).is_some()
            {
                changed = true;
            }
        }
    }
    if changed {
        write_json_object(claude_json_path, &doc, true)?;
    }
    Ok(changed)
}

/// Codex's `config.toml` read for an edit that keeps it as the user wrote it: comments, blank lines and table styles stay.
fn read_toml_for_edit(path: &Path) -> Result<toml_edit::DocumentMut, String> {
    if !path.exists() {
        return Ok(toml_edit::DocumentMut::new());
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_AGENT_CONFIG_BYTES) {
        return Err(format!("{} is larger than 32 MiB; edit it by hand.", path.display()));
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let stripped = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    stripped.parse::<toml_edit::DocumentMut>().map_err(|e| format!("Cannot parse {}: {e}", path.display()))
}

fn upsert_toml_mcp_entry(path: &Path, name: &str, spec: &McpUpsertSpec) -> Result<(), String> {
    let mut doc = read_toml_for_edit(path)?;
    let servers = doc.entry("mcp_servers").or_insert_with(|| {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    let Some(servers_table) = servers.as_table_like_mut() else {
        return Err(format!("'mcp_servers' in {} is not a TOML table.", path.display()));
    };
    if servers_table.get(name).is_none() {
        servers_table.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let Some(server) = servers_table.get_mut(name).and_then(toml_edit::Item::as_table_like_mut) else {
        return Err(format!("'mcp_servers.{name}' in {} is not a TOML table.", path.display()));
    };
    // What this form does not set (env, http_headers, timeouts, enabled_tools) stays; only how the server is reached changes.
    for key in ["url", "command", "args"] {
        server.remove(key);
    }
    match spec {
        McpUpsertSpec::Http { url, bearer_token_env_var } => {
            server.insert("url", toml_edit::value(url.as_str()));
            if let Some(var) = bearer_token_env_var {
                server.insert("bearer_token_env_var", toml_edit::value(var.as_str()));
            }
        }
        McpUpsertSpec::Stdio { command, args } => {
            server.insert("command", toml_edit::value(command.as_str()));
            let mut list = toml_edit::Array::new();
            for arg in args {
                list.push(arg.as_str());
            }
            server.insert("args", toml_edit::value(list));
        }
    }
    atomic_write_bytes(path, doc.to_string().as_bytes(), false)
}

fn remove_toml_mcp_entry(path: &Path, name: &str) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut doc = read_toml_for_edit(path)?;
    let removed =
        doc.get_mut("mcp_servers").and_then(toml_edit::Item::as_table_like_mut).and_then(|servers| servers.remove(name)).is_some();
    if removed {
        atomic_write_bytes(path, doc.to_string().as_bytes(), false)?;
    }
    Ok(removed)
}

/// Every config file an MCP change for `providers` in `scope` writes (Claude, Codex, Gemini; user or project).
fn mcp_target_files(providers: &[String], scope: &str, root: Option<&Path>, paths: &ScanPaths, removing: bool) -> Vec<PathBuf> {
    let mut files = vec![];
    for provider in providers {
        match (provider.as_str(), scope, root) {
            ("claude", "user", _) => files.extend(paths.home_dir.as_ref().map(|h| h.join(".claude.json"))),
            ("claude", "project", Some(root)) => files.push(root.join(".mcp.json")),
            ("codex", "user", _) => files.extend(paths.codex_user_config().map(|(path, _)| path)),
            ("codex", "project", Some(root)) => files.push(root.join(".codex").join("config.toml")),
            ("gemini", "user", _) => {
                if let Some(home) = &paths.home_dir {
                    files.push(home.join(".gemini").join("config").join("mcp_config.json"));
                    if removing {
                        files.push(home.join(".gemini").join("settings.json"));
                    }
                }
            }
            ("gemini", "project", Some(root)) => {
                files.push(root.join(".gemini").join("mcp_config.json"));
                if removing {
                    files.push(root.join(".gemini").join("settings.json"));
                }
            }
            _ => {}
        }
    }
    files
}

/// Reads every file a change would write before writing any: one that does not parse stops the change with nothing written, so a
/// failure never leaves some agents changed and others not.
fn preflight_mcp_files(files: &[PathBuf]) -> Result<(), String> {
    for file in files.iter().filter(|f| f.exists()) {
        if file.extension().is_some_and(|e| e == "toml") {
            read_toml_for_edit(file)?;
        } else {
            read_existing_json_for_edit(file)?;
        }
    }
    Ok(())
}

pub fn upsert_mcp_configs(
    name: &str,
    spec: &McpUpsertSpec,
    scope: &str,
    providers: &[String],
    workspace_root: Option<&Path>,
    paths: &ScanPaths,
) -> Result<(), String> {
    let valid_name = validate_mcp_name(name)?;
    let valid_scope = validate_scope(scope)?;
    let valid_providers = validate_providers(providers)?;

    let canonical_root = match valid_scope {
        "project" => {
            let root = workspace_root.ok_or_else(|| "Project-scope MCP registration requires an active local workspace.".to_string())?;
            Some(std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?)
        }
        _ => None,
    };

    preflight_mcp_files(&mcp_target_files(&valid_providers, valid_scope, canonical_root.as_deref(), paths, false))?;

    for provider in &valid_providers {
        match (provider.as_str(), valid_scope) {
            ("claude", "user") => {
                let home = paths.home_dir.as_ref().ok_or_else(|| "User home directory is unavailable.".to_string())?;
                let entry = claude_mcp_json_value(spec);
                upsert_json_mcp_entry(&home.join(".claude.json"), &valid_name, entry, true)?;
            }
            ("claude", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                let entry = claude_mcp_json_value(spec);
                upsert_json_mcp_entry(&root.join(".mcp.json"), &valid_name, entry, false)?;
            }
            ("codex", "user") => {
                let (codex_path, _) = paths.codex_user_config().ok_or_else(|| "User home directory is unavailable.".to_string())?;
                upsert_toml_mcp_entry(&codex_path, &valid_name, spec)?;
            }
            ("codex", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                upsert_toml_mcp_entry(&root.join(".codex").join("config.toml"), &valid_name, spec)?;
            }
            ("gemini", "user") => {
                let home = paths.home_dir.as_ref().ok_or_else(|| "User home directory is unavailable.".to_string())?;
                let entry = gemini_mcp_json_value(spec);
                upsert_json_mcp_entry(&home.join(".gemini").join("config").join("mcp_config.json"), &valid_name, entry, false)?;
            }
            ("gemini", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                let entry = gemini_mcp_json_value(spec);
                upsert_json_mcp_entry(&root.join(".gemini").join("mcp_config.json"), &valid_name, entry, false)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn claude_mcp_json_value(spec: &McpUpsertSpec) -> Value {
    match spec {
        McpUpsertSpec::Http { url, bearer_token_env_var } => {
            let mut obj = Map::new();
            obj.insert("type".into(), Value::String("http".into()));
            obj.insert("url".into(), Value::String(url.clone()));
            if let Some(var) = bearer_token_env_var {
                obj.insert("bearerTokenEnvVar".into(), Value::String(var.clone()));
            }
            Value::Object(obj)
        }
        McpUpsertSpec::Stdio { command, args } => json!({
            "type": "stdio",
            "command": command,
            "args": args,
        }),
    }
}

fn gemini_mcp_json_value(spec: &McpUpsertSpec) -> Value {
    match spec {
        McpUpsertSpec::Http { url, .. } => json!({
            "url": url,
            "disabled": false,
        }),
        McpUpsertSpec::Stdio { command, args } => json!({
            "command": command,
            "args": args,
            "disabled": false,
        }),
    }
}

pub fn remove_mcp_configs(
    name: &str,
    scope: &str,
    providers: &[String],
    workspace_root: Option<&Path>,
    paths: &ScanPaths,
    allow_user_file_cleanup: bool,
) -> Result<(), String> {
    let valid_name = validate_mcp_name(name)?;
    let valid_scope = validate_scope(scope)?;
    let valid_providers = validate_providers(providers)?;

    let canonical_root = match valid_scope {
        "project" => {
            let root = workspace_root.ok_or_else(|| "Project-scope MCP removal requires an active local workspace.".to_string())?;
            Some(std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?)
        }
        _ => None,
    };

    preflight_mcp_files(&mcp_target_files(&valid_providers, valid_scope, canonical_root.as_deref(), paths, true))?;

    for provider in &valid_providers {
        match (provider.as_str(), valid_scope) {
            ("claude", "user") => {
                if let Some(home) = &paths.home_dir {
                    remove_json_mcp_entry(&home.join(".claude.json"), &valid_name, true)?;
                }
            }
            ("claude", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                remove_json_mcp_entry(&root.join(".mcp.json"), &valid_name, false)?;
                if allow_user_file_cleanup && let Some(home) = &paths.home_dir {
                    let _ = remove_claude_local_project_entry(&home.join(".claude.json"), root, &valid_name);
                }
            }
            ("codex", "user") => {
                if let Some((codex_path, _)) = paths.codex_user_config() {
                    remove_toml_mcp_entry(&codex_path, &valid_name)?;
                }
            }
            ("codex", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                remove_toml_mcp_entry(&root.join(".codex").join("config.toml"), &valid_name)?;
            }
            ("gemini", "user") => {
                if let Some(home) = &paths.home_dir {
                    remove_json_mcp_entry(&home.join(".gemini").join("config").join("mcp_config.json"), &valid_name, false)?;
                    remove_json_mcp_entry(&home.join(".gemini").join("settings.json"), &valid_name, false)?;
                }
            }
            ("gemini", "project") => {
                let root = canonical_root.as_ref().expect("project root verified");
                remove_json_mcp_entry(&root.join(".gemini").join("mcp_config.json"), &valid_name, false)?;
                remove_json_mcp_entry(&root.join(".gemini").join("settings.json"), &valid_name, false)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn save_mcp_to_toolkit(host: &Host, name: &str, spec: &McpUpsertSpec) -> Result<(), String> {
    let toolkit = crate::settings::tools::toolkit_of(host);
    let store = toolkit.store(host);
    let (existing_entries, _) = store.list();
    let matched = existing_entries.into_iter().find(|entry| {
        entry.id == name
            || matches!(&entry.install, ToolkitInstallSpec::Mcp { name: n, .. } | ToolkitInstallSpec::McpHttp { name: n, .. } if n == name)
    });
    let entry_id = matched.as_ref().map(|e| e.id.clone()).unwrap_or_else(|| name.to_string());
    let display_name = matched.as_ref().map(|e| e.display_name.clone()).unwrap_or_else(|| name.to_string());

    let install = match spec {
        McpUpsertSpec::Http { url, bearer_token_env_var } => {
            ToolkitInstallSpec::McpHttp { name: name.to_string(), url: url.clone(), bearer_token_env_var: bearer_token_env_var.clone() }
        }
        McpUpsertSpec::Stdio { command, args } => {
            ToolkitInstallSpec::Mcp { name: name.to_string(), executable: command.clone(), args: args.clone() }
        }
    };
    let candidate = ToolkitFileEntry::new(&entry_id, &display_name, "mcp", install);
    // Verify the serialized entry passes `toolkit::file::parse_entry` before persisting.
    let canonical_val: Value =
        serde_json::from_str(&mighty_tools::toolkit::store::build_canonical_json(&candidate)).map_err(|e| e.to_string())?;
    if toolkit_file::parse_entry(&canonical_val).is_none() {
        return Err("MCP command or identifier is not eligible for toolkit.json template storage.".into());
    }
    store.add(&candidate)?;
    crate::settings::tools::read(host);
    Ok(())
}

pub fn save_mcp_oauth_token(data_dir: &Path, record: StoredMcpOAuthToken) -> Result<(), String> {
    let book_path = data_dir.join(MCP_OAUTH_TOKENS_FILE);
    let mut book = std::fs::read(&book_path).ok().and_then(|b| serde_json::from_slice::<McpOAuthTokenBook>(&b).ok()).unwrap_or_default();
    book.tokens.insert(record.server_name.clone(), record.clone());
    let mut bytes = serde_json::to_vec_pretty(&book).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    atomic_write_bytes(&book_path, &bytes, true)?;

    // Stay compatible with `integrations.rs` when logging into MightyWork.
    if is_mightywork_endpoint(&record.server_url, saved_mightywork_url(data_dir).as_deref()) {
        let compat = CompatMightyWorkAuth {
            server_url: record.server_url,
            access_token: record.access_token,
            refresh_token: record.refresh_token,
            token_endpoint: record.token_endpoint,
            client_id: record.client_id,
            expires_at_ms: record.expires_at_ms,
        };
        let mut mw_bytes = serde_json::to_vec_pretty(&compat).map_err(|e| e.to_string())?;
        mw_bytes.push(b'\n');
        atomic_write_bytes(&data_dir.join(MIGHTYWORK_AUTH_FILE), &mw_bytes, true)?;
    }
    Ok(())
}

fn encode_form(pairs: &[(&str, &str)]) -> Vec<u8> {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", utf8_percent_encode(k, QUERY_ENCODE_SET), utf8_percent_encode(v, QUERY_ENCODE_SET)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

/// Interactive RFC 8414 + RFC 7591 + OAuth 2.1 PKCE (`S256`) browser sign-in for HTTP MCP servers.
async fn mcp_oauth_login(host: &Arc<Host>, server_name: &str, mcp_url: &str) -> Result<(), String> {
    if !host.may_change_machine() {
        return Err(crate::setup::verification_mode());
    }
    let valid_name = validate_mcp_name(server_name)?;
    let valid_url = validate_mcp_url(mcp_url)?;
    let parsed_mcp = url::Url::parse(&valid_url).map_err(|_| "Invalid MCP URL.".to_string())?;
    let host_str = parsed_mcp.host_str().ok_or("Missing host in MCP URL.")?;
    let origin = match parsed_mcp.port() {
        Some(port) => format!("{}://{host_str}:{port}", parsed_mcp.scheme()),
        None => format!("{}://{host_str}", parsed_mcp.scheme()),
    };
    let metadata_url = format!("{origin}/.well-known/oauth-authorization-server");

    let https = host
        .app
        .try_state::<crate::platform::Ports>()
        .map(|ports| ports.https.clone())
        .unwrap_or_else(|| crate::platform::ports(host.smoke.is_some()).https);

    let meta_resp = https
        .get(HttpsRequest { url: metadata_url, headers: vec![("Accept".into(), "application/json".into())], timeout: HTTP_TIMEOUT }, 65_536)
        .await
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&meta_resp.status) {
        return Err(format!("OAuth discovery failed (HTTP {}).", meta_resp.status));
    }
    let meta: Value = serde_json::from_str(&meta_resp.body).map_err(|e| e.to_string())?;
    let auth_endpoint = meta.get("authorization_endpoint").and_then(Value::as_str).ok_or("Missing authorization_endpoint")?;
    let token_endpoint = meta.get("token_endpoint").and_then(Value::as_str).ok_or("Missing token_endpoint")?;
    let default_reg = format!("{origin}/oauth/register");
    let reg_endpoint = meta.get("registration_endpoint").and_then(Value::as_str).unwrap_or(&default_reg);

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

    let (reg_resp, _) = https
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
    {
        let mut pairs = auth_url.query_pairs_mut();
        pairs
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &csrf_state);
        if is_mightywork_endpoint(&valid_url, saved_mightywork_url(&host.data_dir).as_deref()) {
            pairs.append_pair("scope", "mightywork:read mightywork:write");
        }
    }

    let launch_url = auth_url.to_string();
    tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&launch_url)).await.map_err(|e| e.to_string())??;

    // The browser may connect first for something else (a preconnect, the favicon), and another local program may connect too: only
    // a GET of /callback is the sign-in's answer; anything else gets a 404 and the wait goes on, all within the one deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let (mut socket, callback_url) = loop {
        let (mut socket, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| "Timed out waiting for browser sign-in.".to_string())?
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf)).await else { continue };
        let req_line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
        let mut parts = req_line.split_whitespace();
        let (method, path_part) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
        if method != "GET" || !(path_part == "/callback" || path_part.starts_with("/callback?")) {
            let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            let _ = socket.shutdown().await;
            continue;
        }
        match url::Url::parse(&format!("http://127.0.0.1:{port}{path_part}")) {
            Ok(callback_url) => break (socket, callback_url),
            Err(_) => continue,
        }
    };

    let mut code = None;
    let mut returned_state = None;
    let mut oauth_error = None;
    for (k, v) in callback_url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => returned_state = Some(v.into_owned()),
            "error" => oauth_error = Some(v.into_owned()),
            _ => {}
        }
    }

    let html = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><html><body style=\"font-family:system-ui;padding:32px\"><h3>MCP sign-in complete.</h3><p>You can return to Mighty Code and close this tab.</p><script>window.close()</script></body></html>";
    let _ = socket.write_all(html.as_bytes()).await;
    let _ = socket.shutdown().await;

    if let Some(err) = oauth_error.filter(|e| !e.is_empty()) {
        return Err(format!("OAuth authorization refused: {err}"));
    }
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
    let (tok_resp, _) = https
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

    save_mcp_oauth_token(
        &host.data_dir,
        StoredMcpOAuthToken {
            server_name: valid_name,
            server_url: valid_url,
            access_token,
            refresh_token,
            token_endpoint: Some(token_endpoint.to_string()),
            client_id: Some(client_id),
            expires_at_ms: Some(now_ms().saturating_add(expires_in.clamp(0, 10 * 365 * 24 * 3600).saturating_mul(1000))),
        },
    )
}

// ── Host Workspace Resolution & RPC Registration ──────────────────────────────────────────────────────────────────────────

fn resolve_local_workspace(host: &Host, requested_id: Option<&str>) -> Result<Workspace, String> {
    let view = host.view();
    let target = requested_id
        .filter(|id| !id.is_empty())
        .or(view.snapshot.active_workspace_id.as_deref())
        .and_then(|id| view.snapshot.workspace(id))
        .cloned()
        .ok_or_else(|| host.locale.get("window.error.addWorkspaceFirst"))?;
    if target.remote.is_some() || target.path().trim().is_empty() {
        return Err("Agent context requires a local workspace folder.".into());
    }
    Ok(target)
}

fn optional_local_workspace_path(host: &Host, requested_id: Option<&str>) -> Option<PathBuf> {
    resolve_local_workspace(host, requested_id).ok().map(|ws| PathBuf::from(ws.path()))
}

fn build_context_payload(host: &Host, ws: &Workspace) -> Result<AgentContextPayload, String> {
    let root = PathBuf::from(ws.path());
    let (files, diverged) = read_instruction_files(&root)?;
    let builtin_mcp_servers = builtin_mcp_servers(host);
    let scan_paths = ScanPaths::from_host(host);
    let external_mcp_servers = scan_external_mcp_servers(Some(&root), &scan_paths);
    let claude_hooks = scan_claude_hooks(Some(&root), scan_paths.home_dir.as_deref());
    Ok(AgentContextPayload {
        workspace_path: ws.path().to_string(),
        diverged,
        files,
        builtin_mcp_servers,
        external_mcp_servers,
        claude_hooks,
    })
}

fn string_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn register(methods: &mut Registry) {
    methods.add("agentContextGet", |host: Arc<Host>, args| async move {
        let ws = resolve_local_workspace(&host, text(&args, "workspaceId"))?;
        let payload =
            tauri::async_runtime::spawn_blocking(move || build_context_payload(&host, &ws)).await.map_err(|e| e.to_string())??;
        serde_json::to_value(payload).map_err(|e| e.to_string())
    });

    methods.add("agentContextWriteFile", |host: Arc<Host>, args| async move {
        let ws = resolve_local_workspace(&host, text(&args, "workspaceId"))?;
        let root = PathBuf::from(ws.path());
        if !(host.writes() || host.may_touch_repo(&root)) {
            return Err(crate::setup::verification_mode());
        }
        let name = text(&args, "name").unwrap_or("").to_string();
        let body = text(&args, "text").unwrap_or("").to_string();
        let expected = expected_texts(&args);
        let payload = tauri::async_runtime::spawn_blocking(move || -> Result<AgentContextPayload, String> {
            check_unchanged(&root, &expected, &[name.as_str()])?;
            write_instruction_file(&root, &name, &body)?;
            build_context_payload(&host, &ws)
        })
        .await
        .map_err(|e| e.to_string())??;
        serde_json::to_value(payload).map_err(|e| e.to_string())
    });

    methods.add("agentContextSyncFiles", |host: Arc<Host>, args| async move {
        let ws = resolve_local_workspace(&host, text(&args, "workspaceId"))?;
        let root = PathBuf::from(ws.path());
        if !(host.writes() || host.may_touch_repo(&root)) {
            return Err(crate::setup::verification_mode());
        }
        let source_name = text(&args, "sourceName").unwrap_or("").to_string();
        let content_override = args.get("content").and_then(Value::as_str).map(str::to_string);
        let target_names = args
            .get("targetNames")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
        let expected = expected_texts(&args);

        let payload = tauri::async_runtime::spawn_blocking(move || -> Result<AgentContextPayload, String> {
            let names: Vec<&str> = match &target_names {
                Some(list) if !list.is_empty() => list.iter().map(String::as_str).chain(std::iter::once(source_name.as_str())).collect(),
                _ => INSTRUCTION_FILES.iter().map(|(_, n)| *n).collect(),
            };
            check_unchanged(&root, &expected, &names)?;
            sync_instruction_files(&root, &source_name, content_override.as_deref(), target_names.as_deref())?;
            build_context_payload(&host, &ws)
        })
        .await
        .map_err(|e| e.to_string())??;
        serde_json::to_value(payload).map_err(|e| e.to_string())
    });

    methods.add("agentContextMcpUpsert", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let name = validate_mcp_name(text(&args, "name").unwrap_or(""))?;
        let scope = validate_scope(text(&args, "scope").unwrap_or("user"))?;
        let providers = validate_providers(&string_list(&args, "providers"))?;
        let save_to_toolkit = flag(&args, "saveToToolkit");

        let transport = text(&args, "transport").unwrap_or("http").trim();
        let spec = match transport {
            "http" => {
                let url = validate_mcp_url(text(&args, "url").unwrap_or(""))?;
                let bearer_token_env_var = validate_bearer_env_var(text(&args, "bearerTokenEnvVar"))?;
                McpUpsertSpec::Http { url, bearer_token_env_var }
            }
            "stdio" => {
                let raw_cmd = text(&args, "command").unwrap_or("");
                let raw_args = string_list(&args, "args");
                let (command, parsed_args) = validate_stdio_command(raw_cmd, &raw_args)?;
                McpUpsertSpec::Stdio { command, args: parsed_args }
            }
            other => return Err(format!("Unsupported MCP transport '{other}'. Expected 'http' or 'stdio'.")),
        };

        let ws_for_project = if scope == "project" {
            let ws = resolve_local_workspace(&host, ws_id.as_deref())?;
            let root = PathBuf::from(ws.path());
            if !(host.writes() || host.may_touch_repo(&root)) {
                return Err(crate::setup::verification_mode());
            }
            Some(ws)
        } else {
            if !host.may_change_machine() {
                return Err(crate::setup::verification_mode());
            }
            resolve_local_workspace(&host, ws_id.as_deref()).ok()
        };

        if save_to_toolkit && !host.may_change_machine() {
            return Err(crate::setup::verification_mode());
        }

        tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
            let scan_paths = ScanPaths::from_host(&host);
            let root_buf = ws_for_project.as_ref().map(|w| PathBuf::from(w.path()));
            upsert_mcp_configs(&name, &spec, scope, &providers, root_buf.as_deref(), &scan_paths)?;
            if save_to_toolkit {
                save_mcp_to_toolkit(&host, &name, &spec)?;
            }
            if let Some(ws) = ws_for_project {
                let payload = build_context_payload(&host, &ws)?;
                let mut val = serde_json::to_value(payload).map_err(|e| e.to_string())?;
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("savedToToolkit".into(), Value::Bool(save_to_toolkit));
                }
                Ok(val)
            } else {
                Ok(json!({ "ok": true, "savedToToolkit": save_to_toolkit }))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    });

    methods.add("agentContextMcpRemove", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let name = validate_mcp_name(text(&args, "name").unwrap_or(""))?;
        let scope = validate_scope(text(&args, "scope").unwrap_or("user"))?;
        let providers = validate_providers(&string_list(&args, "providers"))?;

        let ws_opt = if scope == "project" {
            let ws = resolve_local_workspace(&host, ws_id.as_deref())?;
            let root = PathBuf::from(ws.path());
            if !(host.writes() || host.may_touch_repo(&root)) {
                return Err(crate::setup::verification_mode());
            }
            Some(ws)
        } else {
            if !host.may_change_machine() {
                return Err(crate::setup::verification_mode());
            }
            resolve_local_workspace(&host, ws_id.as_deref()).ok()
        };

        let allow_user_cleanup = host.may_change_machine();
        tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
            let scan_paths = ScanPaths::from_host(&host);
            let root_buf =
                ws_opt.as_ref().map(|w| PathBuf::from(w.path())).or_else(|| optional_local_workspace_path(&host, ws_id.as_deref()));
            remove_mcp_configs(&name, scope, &providers, root_buf.as_deref(), &scan_paths, allow_user_cleanup)?;
            if let Some(ws) = ws_opt {
                let payload = build_context_payload(&host, &ws)?;
                serde_json::to_value(payload).map_err(|e| e.to_string())
            } else {
                Ok(json!({ "ok": true }))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    });

    methods.add("agentContextMcpOAuthLogin", |host: Arc<Host>, args| async move {
        let name = text(&args, "name").unwrap_or("").to_string();
        let url = text(&args, "url").unwrap_or("").to_string();
        mcp_oauth_login(&host, &name, &url).await?;
        Ok(json!({ "connected": true }))
    });
}

// ── Unit Tests ────────────────────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mighty-agent-ctx-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn codex_toml_keeps_comments_and_a_registered_again_server_keeps_its_env() {
        let dir = temp_dir("codex-toml");
        let config = dir.join("config.toml");
        std::fs::write(&config, "# my settings\nmodel = \"o5\"\n\n# servers\n[mcp_servers.github]\ncommand = \"old\"\nstartup_timeout_sec = 30\n\n[mcp_servers.github.env]\nGITHUB_TOKEN = \"x\"\n").unwrap();
        upsert_toml_mcp_entry(
            &config,
            "github",
            &McpUpsertSpec::Http { url: "https://api.example.com/mcp".into(), bearer_token_env_var: None },
        )
        .unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("# my settings") && text.contains("# servers"), "comments stay: {text}");
        assert!(text.contains("startup_timeout_sec = 30") && text.contains("GITHUB_TOKEN"), "the server's own settings stay: {text}");
        assert!(
            text.contains("url = \"https://api.example.com/mcp\"") && !text.contains("command = \"old\""),
            "only the transport changed: {text}"
        );
        assert!(remove_toml_mcp_entry(&config, "github").unwrap());
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("# my settings") && !text.contains("github"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_json_server_registered_again_keeps_env_and_headers() {
        let dir = temp_dir("json-merge");
        let file = dir.join(".mcp.json");
        std::fs::write(&file, r#"{"mcpServers":{"gh":{"type":"stdio","command":"old","args":["a"],"env":{"T":"1"},"headers":{"X":"y"}}}}"#)
            .unwrap();
        upsert_json_mcp_entry(
            &file,
            "gh",
            claude_mcp_json_value(&McpUpsertSpec::Http { url: "https://h/mcp".into(), bearer_token_env_var: None }),
            false,
        )
        .unwrap();
        let root: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let gh = &root["mcpServers"]["gh"];
        assert_eq!(gh["url"], "https://h/mcp");
        assert_eq!(gh["type"], "http");
        assert_eq!(gh["env"]["T"], "1");
        assert_eq!(gh["headers"]["X"], "y");
        assert!(gh.get("command").is_none() && gh.get("args").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn writes_keep_a_symlink_a_link_and_a_private_file_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("links");
        let real = dir.join("AGENTS.md");
        std::fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.join("CLAUDE.md")).unwrap();
        write_instruction_file(&dir, "CLAUDE.md", "new").unwrap();
        assert!(std::fs::symlink_metadata(dir.join("CLAUDE.md")).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new", "written where it points");
        sync_instruction_files(&dir, "AGENTS.md", Some("synced"), None).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(dir.join("GEMINI.md")).unwrap(), "synced");

        let private = dir.join("config.toml");
        std::fs::write(&private, "a = 1\n").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        upsert_toml_mcp_entry(&private, "x", &McpUpsertSpec::Stdio { command: "tool".into(), args: vec![] }).unwrap();
        assert_eq!(std::fs::metadata(&private).unwrap().permissions().mode() & 0o777, 0o600, "a private file stays private");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_over_a_file_changed_on_disk_is_refused() {
        let dir = temp_dir("unchanged");
        std::fs::write(dir.join("CLAUDE.md"), "as read").unwrap();
        let mut expected = ExpectedTexts::new();
        expected.insert("CLAUDE.md".into(), Some("as read".into()));
        expected.insert("GEMINI.md".into(), None);
        assert!(check_unchanged(&dir, &expected, &["CLAUDE.md", "GEMINI.md"]).is_ok());
        std::fs::write(dir.join("CLAUDE.md"), "an agent appended").unwrap();
        assert!(check_unchanged(&dir, &expected, &["CLAUDE.md"]).is_err());
        std::fs::write(dir.join("GEMINI.md"), "made since").unwrap();
        assert!(check_unchanged(&dir, &expected, &["GEMINI.md"]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_default_or_saved_mightywork_server_is_mightywork() {
        assert!(is_mightywork_endpoint(DEFAULT_MIGHTYWORK_URL, None));
        assert!(!is_mightywork_endpoint("https://mightywork-dev.example.com/mcp", None));
        assert!(is_mightywork_endpoint("https://mw.internal/mcp", Some("https://mw.internal/mcp")));
    }

    #[test]
    fn instruction_files_read_divergence_write_and_one_click_sync() {
        let ws = temp_dir("instructions");

        let (initial, diverged0) = read_instruction_files(&ws).unwrap();
        assert_eq!(initial.len(), 3);
        assert!(!diverged0);
        assert!(initial.iter().all(|f| !f.exists && f.text.is_empty() && f.byte_len == 0));

        write_instruction_file(&ws, "CLAUDE.md", "# Shared Rules\n- Always test\n").unwrap();
        let (after_one, diverged1) = read_instruction_files(&ws).unwrap();
        assert!(!diverged1, "a single existing instruction file is not diverged");
        assert!(after_one[0].exists && !after_one[1].exists && !after_one[2].exists);

        write_instruction_file(&ws, "AGENTS.md", "# Different Codex Rules\n").unwrap();
        let (_, diverged2) = read_instruction_files(&ws).unwrap();
        assert!(diverged2, "different trimmed contents report diverged=true");

        // Sync CLAUDE.md across all 3 instruction files in one call.
        sync_instruction_files(&ws, "CLAUDE.md", Some("# Unified Rules\n\n1. Zero warnings\n"), None).unwrap();
        let (synced, diverged3) = read_instruction_files(&ws).unwrap();
        assert!(!diverged3, "synced instruction files are no longer diverged");
        for file in &synced {
            assert!(file.exists, "{} exists after sync", file.name);
            assert_eq!(file.text, "# Unified Rules\n\n1. Zero warnings\n");
        }

        // Trailing blank lines do not count as divergence (Swift 0.4 parity).
        write_instruction_file(&ws, "GEMINI.md", "# Unified Rules\n\n1. Zero warnings\n\n\n").unwrap();
        let (_, diverged4) = read_instruction_files(&ws).unwrap();
        assert!(!diverged4);

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn instruction_files_refuse_path_traversal_oversized_payload_and_escaping_symlinks() {
        let ws = temp_dir("traversal-ws");
        let outside = temp_dir("traversal-outside");
        std::fs::write(outside.join("secret.md"), "TOP_SECRET").unwrap();

        for disallowed in ["../CLAUDE.md", "sub/CLAUDE.md", ".env", "README.md", "/etc/passwd", ""] {
            assert!(write_instruction_file(&ws, disallowed, "x").is_err(), "must reject {disallowed}");
            assert!(sync_instruction_files(&ws, disallowed, Some("x"), None).is_err(), "must reject source {disallowed}");
        }

        let too_large = "a".repeat(MAX_INSTRUCTION_BYTES + 1);
        assert!(write_instruction_file(&ws, "CLAUDE.md", &too_large).is_err(), "must reject > 512 KiB");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.md"), ws.join("GEMINI.md")).unwrap();
            assert!(read_instruction_files(&ws).is_err(), "must refuse reading symlink escaping workspace root");
            assert!(write_instruction_file(&ws, "GEMINI.md", "overwrite").is_err(), "must refuse writing through escaping symlink");
            assert_eq!(std::fs::read_to_string(outside.join("secret.md")).unwrap(), "TOP_SECRET");
        }

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn rejects_reserved_mighty_mcp_names_and_invalid_inputs() {
        for reserved in ["mighty-terminal", "mighty-delegation", "mighty-workspace", "MIGHTY-TERMINAL"] {
            assert!(validate_mcp_name(reserved).is_err(), "reserved server {reserved} must be rejected");
        }
        assert!(validate_mcp_name("mightywork").is_ok());
        assert!(validate_mcp_name("context7_v2.1-prod").is_ok());
        assert!(validate_mcp_name("-leading-dash").is_err());
        assert!(validate_mcp_name("bad space").is_err());

        assert!(validate_mcp_url("https://mightywork.duckdns.org/mcp").is_ok());
        assert!(validate_mcp_url("http://127.0.0.1:8787/mcp").is_ok());
        assert!(validate_mcp_url("http://localhost:3000/mcp").is_ok());
        assert!(validate_mcp_url("http://evil.example.com/mcp").is_err());
        assert!(validate_mcp_url("file:///etc/passwd").is_err());

        assert_eq!(validate_bearer_env_var(Some("MIGHTYWORK_TOKEN")).unwrap().as_deref(), Some("MIGHTYWORK_TOKEN"));
        assert!(validate_bearer_env_var(Some("sk-live-secret-token")).is_err());
        assert!(validate_bearer_env_var(Some("9INVALID")).is_err());
    }

    #[test]
    fn multi_cli_mcp_upsert_scan_oauth_detection_and_remove_preserve_existing_keys() {
        let ws = temp_dir("mcp-ws");
        let home = temp_dir("mcp-home");
        let data = temp_dir("mcp-data");

        // Seed existing settings across Claude, Codex, Antigravity, and antigravity-cli permission rules.
        std::fs::write(
            home.join(".claude.json"),
            r#"{
  "numStartups": 42,
  "oauthAccount": { "email": "dev@example.com" },
  "mcpServers": {
    "existing-claude": { "type": "stdio", "command": "npx", "args": ["-y", "existing-pkg"] }
  },
  "projects": {
    "other": { "allowedTools": ["Bash"] }
  }
}"#,
        )
        .unwrap();

        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex").join("config.toml"),
            "model = \"gpt-5-codex\"\napproval_policy = \"on-request\"\n\n[mcp_servers.existing_codex]\ncommand = \"uvx\"\nargs = [\"mcp-git\"]\n",
        )
        .unwrap();

        std::fs::create_dir_all(home.join(".gemini").join("config")).unwrap();
        std::fs::write(
            home.join(".gemini").join("config").join("mcp_config.json"),
            r#"{"theme":"dark","mcpServers":{"existing-agy":{"command":"node","args":["srv.js"],"disabled":false}}}"#,
        )
        .unwrap();

        let agy_cli_settings = home.join(".gemini").join("antigravity-cli").join("settings.json");
        std::fs::create_dir_all(agy_cli_settings.parent().unwrap()).unwrap();
        let untouched_agy_cli = r#"{"permissions":{"allow":["Bash(cargo test)"]}}"#;
        std::fs::write(&agy_cli_settings, untouched_agy_cli).unwrap();

        // Seed Claude hooks in project & user settings.
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        std::fs::write(
            ws.join(".claude").join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"./scripts/guard.sh"}]}]}}"#,
        )
        .unwrap();

        let paths = ScanPaths { home_dir: Some(home.clone()), codex_home: None, data_dir: data.clone(), claude_keychain_blob: None };
        let all_providers = vec!["claude".to_string(), "codex".to_string(), "gemini".to_string()];

        // Upsert HTTP MCP across all 3 CLIs in user scope.
        let http_spec =
            McpUpsertSpec::Http { url: "https://mightywork.duckdns.org/mcp".into(), bearer_token_env_var: Some("MW_TOKEN".into()) };
        upsert_mcp_configs("mightywork", &http_spec, "user", &all_providers, Some(&ws), &paths).unwrap();

        // Upsert stdio MCP in project scope across Claude & Antigravity.
        let stdio_spec = McpUpsertSpec::Stdio { command: "npx".into(), args: vec!["-y".into(), "@upstash/context7-mcp".into()] };
        upsert_mcp_configs(
            "context7",
            &stdio_spec,
            "project",
            &["claude".to_string(), "gemini".to_string(), "codex".to_string()],
            Some(&ws),
            &paths,
        )
        .unwrap();

        // Save OAuth token and verify MightyWork compatibility file + oauth_connected flag.
        save_mcp_oauth_token(
            &data,
            StoredMcpOAuthToken {
                server_name: "mightywork".into(),
                server_url: "https://mightywork.duckdns.org/mcp".into(),
                access_token: "tok_test_123".into(),
                refresh_token: Some("ref_test_456".into()),
                token_endpoint: Some("https://mightywork.duckdns.org/oauth/token".into()),
                client_id: Some("cid_1".into()),
                expires_at_ms: Some(now_ms() + 3_600_000),
            },
        )
        .unwrap();
        assert!(data.join(MIGHTYWORK_AUTH_FILE).is_file(), "MightyWork OAuth also writes mightywork-auth.json");

        let scanned = scan_external_mcp_servers(Some(&ws), &paths);
        for provider in ["claude", "codex", "gemini"] {
            let mw = scanned.iter().find(|s| s.provider == provider && s.name == "mightywork" && s.scope == "user").unwrap();
            assert_eq!(mw.transport, "http");
            assert_eq!(mw.url.as_deref(), Some("https://mightywork.duckdns.org/mcp"));
            assert!(mw.oauth_connected, "{provider} mightywork detects OAuth token");

            let c7 = scanned.iter().find(|s| s.provider == provider && s.name == "context7" && s.scope == "project").unwrap();
            assert_eq!(c7.transport, "stdio");
            assert_eq!(c7.command.as_deref(), Some("npx"));
            assert_eq!(c7.args, vec!["-y", "@upstash/context7-mcp"]);
        }

        // Verify Antigravity HTTP format matches `agy mcp add --type http`: `{"url": ..., "disabled": false}`.
        let agy_json: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".gemini/config/mcp_config.json")).unwrap()).unwrap();
        assert_eq!(agy_json["theme"], "dark", "preserved existing top-level key in mcp_config.json");
        assert!(agy_json["mcpServers"]["existing-agy"].is_object(), "preserved existing Antigravity server");
        assert_eq!(agy_json["mcpServers"]["mightywork"], json!({ "url": "https://mightywork.duckdns.org/mcp", "disabled": false }));

        // Verify Claude & Codex pre-existing keys were preserved intact.
        let claude_json: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(claude_json["numStartups"], 42);
        assert_eq!(claude_json["oauthAccount"]["email"], "dev@example.com");
        assert!(claude_json["mcpServers"]["existing-claude"].is_object());

        let codex_toml: toml::Table = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap().parse().unwrap();
        assert_eq!(codex_toml["model"].as_str(), Some("gpt-5-codex"));
        assert!(codex_toml["mcp_servers"]["existing_codex"].is_table());
        assert_eq!(codex_toml["mcp_servers"]["mightywork"]["bearer_token_env_var"].as_str(), Some("MW_TOKEN"));

        // Verify ~/.gemini/antigravity-cli/settings.json was NEVER touched.
        assert_eq!(std::fs::read_to_string(&agy_cli_settings).unwrap(), untouched_agy_cli);

        // Verify Claude Hooks scanner.
        let hooks = scan_claude_hooks(Some(&ws), Some(&home));
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, "PreToolUse");
        assert_eq!(hooks[0].commands, vec!["./scripts/guard.sh"]);
        assert_eq!(hooks[0].scope, "project");

        // Remove mightywork from user scope across all 3 CLIs and verify pre-existing keys remain.
        remove_mcp_configs("mightywork", "user", &all_providers, Some(&ws), &paths, true).unwrap();
        let after_remove = scan_external_mcp_servers(Some(&ws), &paths);
        assert!(!after_remove.iter().any(|s| s.name == "mightywork"));
        assert!(after_remove.iter().any(|s| s.name == "existing-claude"));
        assert!(after_remove.iter().any(|s| s.name == "existing_codex"));
        assert!(after_remove.iter().any(|s| s.name == "existing-agy"));
        assert!(after_remove.iter().any(|s| s.name == "context7"));

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&data);
    }
}

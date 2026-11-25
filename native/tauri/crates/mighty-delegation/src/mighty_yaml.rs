//! Repository shared configuration (`.mighty.yaml` and `.mighty.local.yaml`, `MC-8`): pure-Rust strict YAML subset parser,
//! normalized schema (`MightyRepoConfig`), local overlay merging, and canonical SHA-256 content hashing for trust verification.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::Path;

use indexmap::IndexMap;
use mighty_core::automation::spec::{self as automation, Action, CatchUp, FireMode, GitHubEvent, ScheduleSpec, TriggerSource, TriggerSpec};
use mighty_core::normalize::worktree_copy_paths;
pub use mighty_core::normalize::{has_invisible_format_chars, is_invisible_format_char};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Maximum size in bytes accepted for `.mighty.yaml` or `.mighty.local.yaml` (`256 KiB`).
pub const MAXIMUM_CONFIG_BYTES: usize = 256 * 1024;

/// Maximum normalized character length for `worktree.setup` or a task command.
pub const MAXIMUM_COMMAND_CHARS: usize = 2_000;

/// Maximum entries accepted in `worktree.share` or `worktree.copy`.
pub const MAXIMUM_WORKTREE_RULES: usize = 64;

/// Maximum tasks accepted in `tasks`.
pub const MAXIMUM_TASKS: usize = 128;

/// Maximum named actions accepted in `run`.
pub const MAXIMUM_RUN_ACTIONS: usize = 32;

/// `worktree.teardownTimeoutSeconds` when the file names none, and the most it may name.
pub const DEFAULT_TEARDOWN_TIMEOUT_SECONDS: u32 = 60;
pub const MAXIMUM_TEARDOWN_TIMEOUT_SECONDS: u32 = 3_600;

/// `worktree.ports` (the size of each worktree's port block) when the file names none, and the most it may name; `0` turns the
/// assignment off.
pub const DEFAULT_WORKTREE_PORTS: u32 = 10;
pub const MAXIMUM_WORKTREE_PORTS: u32 = 100;

/// Maximum characters of a fixed `run.<name>.url`.
pub const MAXIMUM_URL_CHARS: usize = 2_048;

/// The variables the app sets for a worktree's scripts, tasks and agent: `worktree.env` may not set them.
pub const RESERVED_ENV_NAMES: [&str; 7] = [
    "MIGHTY_PORT",
    "MIGHTY_PORT_COUNT",
    "MIGHTY_WORKTREE_NAME",
    "MIGHTY_ROOT_PATH",
    "MIGHTY_WORKTREE_PATH",
    "MIGHTY_WORKSPACE_FOLDER",
    "MIGHTY_WORKTREE",
];

/// Maximum nesting depth accepted by the YAML subset parser.
const MAXIMUM_YAML_DEPTH: usize = 16;

/// Base repository config filenames checked in priority order.
pub const BASE_CONFIG_FILES: [&str; 2] = [".mighty.yaml", ".mighty.yml"];

/// Local personal overlay config filenames checked in priority order.
pub const LOCAL_CONFIG_FILES: [&str; 2] = [".mighty.local.yaml", ".mighty.local.yml"];

/// A relative file or directory copy rule for new git worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyRule {
    pub from: String,
    pub to: String,
}

impl CopyRule {
    pub fn same_path(path: impl Into<String>) -> Self {
        let path = path.into();
        Self { from: path.clone(), to: path }
    }

    pub fn mapped(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), to: to.into() }
    }

    /// Human-readable display label (`"path"` when `from == to`, else `"from -> to"`).
    pub fn display_label(&self) -> String {
        if self.from == self.to { self.from.clone() } else { format!("{} -> {}", self.from, self.to) }
    }
}

/// Worktree cache sharing, file copy, setup/teardown commands, port block size, and environment rules from `.mighty.yaml`.
/// The keys added later (`teardown`, `teardownTimeoutSeconds`, `ports`) serialize only when set, so a file without them keeps the
/// `content_hash` it was approved with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub share: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy: Vec<CopyRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
    /// Run in the worktree folder before it is discarded or cleaned up after a merge (`MC-25`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown: Option<String>,
    #[serde(default, rename = "teardownTimeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub teardown_timeout_seconds: Option<u32>,
    /// Ports in each worktree's block (`MIGHTY_PORT` .. `MIGHTY_PORT + ports - 1`); `0` turns the assignment off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<u32>,
}

impl WorktreeConfig {
    /// How long `teardown` may run before it is stopped.
    pub fn teardown_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.teardown_timeout_seconds.unwrap_or(DEFAULT_TEARDOWN_TIMEOUT_SECONDS)))
    }

    /// The size of each worktree's port block (`0`: none is assigned).
    pub fn port_count(&self) -> u32 {
        self.ports.unwrap_or(DEFAULT_WORKTREE_PORTS)
    }
}

/// Where a run action's dev-server address comes from (`run.<name>.url`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunUrl<'a> {
    /// The first `http://localhost:<port>`-style address in the action's output.
    Auto,
    /// No address is shown.
    Off,
    /// This address, as written.
    Fixed(&'a str),
}

fn default_true() -> bool {
    true
}

fn default_url() -> String {
    "auto".into()
}

/// One named action under `run` in `.mighty.yaml` (`MC-25`): a long-running command (a dev server, an end-to-end suite) started
/// and stopped from the task panel. `concurrent: false` stops the same action in the repository's other worktrees first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunActionSpec {
    pub command: String,
    #[serde(default = "default_true")]
    pub concurrent: bool,
    /// `auto`, `off` or a fixed `http(s)://` address.
    #[serde(default = "default_url")]
    pub url: String,
}

impl RunActionSpec {
    pub fn url_mode(&self) -> RunUrl<'_> {
        match self.url.as_str() {
            "auto" => RunUrl::Auto,
            "off" => RunUrl::Off,
            fixed => RunUrl::Fixed(fixed),
        }
    }
}

/// One shared task/script entry under `tasks` (or `scripts`) in `.mighty.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoTaskSpec {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Normalized repository configuration after parsing `.mighty.yaml` / `.mighty.local.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MightyRepoConfig {
    pub version: u32,
    #[serde(default)]
    pub worktree: WorktreeConfig,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub tasks: IndexMap<String, RepoTaskSpec>,
    /// Named run actions (`MC-25`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub run: IndexMap<String, RunActionSpec>,
    /// The status hook (`MC-9`), read from `.mighty.local.yaml` only.
    #[serde(default, skip_serializing_if = "HooksConfig::is_empty")]
    pub hooks: HooksConfig,
    /// Schedules (`MC-32`): agent work started by a cron line or an RRULE, once the configuration is approved.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub schedules: IndexMap<String, ScheduleSpec>,
    /// Event triggers (`MC-32`): GitHub, Linear, a signed webhook or a standing order's shell check, once approved.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub triggers: IndexMap<String, TriggerSpec>,
    /// The verification gate (`MC-48`): commands (and a browser check) run after an agent's turn, once approved.
    #[serde(default, skip_serializing_if = "VerifyConfig::is_empty")]
    pub verify: VerifyConfig,
}

/// The most commands `verify.commands` may name.
pub const MAXIMUM_VERIFY_COMMANDS: usize = 16;
/// `verify.commands.<name>.timeoutSeconds` when the file names none, and the most it may name.
pub const DEFAULT_VERIFY_TIMEOUT_SECONDS: u32 = 300;
pub const MAXIMUM_VERIFY_TIMEOUT_SECONDS: u32 = 3_600;
/// `verify.maxRounds` (automatic repair requests after a failed verification) when the file names none, and the most it may name.
pub const DEFAULT_VERIFY_MAX_ROUNDS: u32 = 2;
pub const MAXIMUM_VERIFY_MAX_ROUNDS: u32 = 5;
/// The most paths `verify.browser.paths` may name, and the longest one.
pub const MAXIMUM_VERIFY_PATHS: usize = 10;
pub const MAXIMUM_VERIFY_PATH_CHARS: usize = 512;

/// One named command under `verify.commands` (`MC-48`): run in the run's folder (or `cwd` inside it), failed past its time limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyCommandSpec {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

impl VerifyCommandSpec {
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.timeout_seconds.unwrap_or(DEFAULT_VERIFY_TIMEOUT_SECONDS)))
    }
}

/// `verify.browser` (`MC-48`): the dev server's address, fixed (`url`, a local address only) or the one a run action printed
/// (`runAction`), and the paths visited there (`/` when none is named).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyBrowserSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, rename = "runAction", skip_serializing_if = "Option::is_none")]
    pub run_action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl VerifyBrowserSpec {
    /// The paths to visit: `paths`, else `/`.
    pub fn visit_paths(&self) -> Vec<String> {
        if self.paths.is_empty() { vec!["/".into()] } else { self.paths.clone() }
    }
}

/// When verification runs after a turn: `always`, or `changed` (only when the request changed files; the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VerifyWhen {
    Always,
    #[default]
    Changed,
}

/// `verify` in `.mighty.yaml` (`MC-48`). Serialized only when set, so a file without it keeps its `content_hash`; runs only once the
/// configuration is approved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyConfig {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub commands: IndexMap<String, VerifyCommandSpec>,
    #[serde(default, rename = "maxRounds", skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<VerifyWhen>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<VerifyBrowserSpec>,
}

impl VerifyConfig {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.max_rounds.is_none() && self.when.is_none() && self.browser.is_none()
    }

    /// Whether there is anything to run: a command or a browser check.
    pub fn has_checks(&self) -> bool {
        !self.commands.is_empty() || self.browser.is_some()
    }

    pub fn rounds(&self) -> u32 {
        self.max_rounds.unwrap_or(DEFAULT_VERIFY_MAX_ROUNDS)
    }

    pub fn when(&self) -> VerifyWhen {
        self.when.unwrap_or_default()
    }
}

/// The pane states a hook follows when `hooks.on` names none.
pub const DEFAULT_HOOK_EVENTS: [&str; 3] = ["blocked", "done", "failed"];
/// The states `hooks.on` may name.
pub const HOOK_EVENTS: [&str; 6] = ["started", "blocked", "done", "failed", "stopped", "automation"];
/// `hooks.timeoutSeconds` when the file names none, and the most it may name.
pub const DEFAULT_HOOK_TIMEOUT_SECONDS: u32 = 20;
pub const MAXIMUM_HOOK_TIMEOUT_SECONDS: u32 = 300;

/// `hooks` in `.mighty.local.yaml` (`MC-9`): a command run when one of this workspace's agent panes moves to a state in `on`, the
/// event in `MIGHTY_*` variables, never a prompt. Personal: a `hooks` key in the shared `.mighty.yaml` is refused. Serialized only
/// when set, so a file without it keeps its `content_hash`; runs only once the configuration is approved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
    #[serde(default, rename = "timeoutSeconds", skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

impl HooksConfig {
    pub fn is_empty(&self) -> bool {
        self.command.is_none() && self.on.is_empty() && self.timeout_seconds.is_none()
    }

    /// The states the hook follows: `on`, else [`DEFAULT_HOOK_EVENTS`].
    pub fn events(&self) -> Vec<String> {
        if self.on.is_empty() { DEFAULT_HOOK_EVENTS.iter().map(|e| e.to_string()).collect() } else { self.on.clone() }
    }

    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.timeout_seconds.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECONDS)))
    }
}

impl Default for MightyRepoConfig {
    fn default() -> Self {
        Self {
            version: 1,
            worktree: WorktreeConfig::default(),
            tasks: IndexMap::new(),
            run: IndexMap::new(),
            hooks: HooksConfig::default(),
            schedules: IndexMap::new(),
            triggers: IndexMap::new(),
            verify: VerifyConfig::default(),
        }
    }
}

impl MightyRepoConfig {
    /// Whether this configuration defines any worktree actions (`share`, `copy`, `setup`, `teardown`, or `env`).
    pub fn has_worktree_actions(&self) -> bool {
        !self.worktree.share.is_empty()
            || !self.worktree.copy.is_empty()
            || self.worktree.setup.is_some()
            || self.worktree.teardown.is_some()
            || !self.worktree.env.is_empty()
    }
}

/// Loaded and merged repository configuration with source file list and canonical SHA-256 `content_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRepoConfig {
    pub config: MightyRepoConfig,
    pub files: Vec<String>,
    pub content_hash: String,
}

impl LoadedRepoConfig {
    /// Short 12-character hex prefix of `content_hash` for compact UI badges and prompts.
    pub fn short_hash(&self) -> &str {
        &self.content_hash[..self.content_hash.len().min(12)]
    }

    /// Whether this configuration defines any worktree actions (`share`, `copy`, `setup`, `teardown`, or `env`).
    pub fn has_worktree_actions(&self) -> bool {
        self.config.has_worktree_actions()
    }
}

#[derive(Debug, Clone)]
struct ParsedLayer {
    config: MightyRepoConfig,
    setup_specified: bool,
    teardown_specified: bool,
}

// ── YAML subset AST & parser ────────────────────────────────────────────────────────────────────────────────────────────

/// Minimal strict YAML subset value supporting comments, scalars, inline/block lists, nested mappings, and `|`/`>` block scalars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlValue {
    Null,
    Scalar(String),
    Seq(Vec<YamlValue>),
    Map(IndexMap<String, YamlValue>),
}

#[derive(Debug, Clone)]
struct RawLine<'a> {
    number: usize,
    raw: &'a str,
}

/// Parses a YAML document bounded to [`MAXIMUM_CONFIG_BYTES`] into a [`YamlValue`].
pub fn parse_yaml(input: &str) -> Result<YamlValue, String> {
    if input.len() > MAXIMUM_CONFIG_BYTES {
        return Err(format!("YAML exceeds maximum size of {MAXIMUM_CONFIG_BYTES} bytes."));
    }
    if input.contains('\0') {
        return Err("YAML must not contain NUL bytes.".into());
    }
    let text = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let raw_lines: Vec<RawLine<'_>> = normalized.split('\n').enumerate().map(|(idx, raw)| RawLine { number: idx + 1, raw }).collect();

    let mut cursor = 0;
    skip_blank_and_comments(&raw_lines, &mut cursor)?;
    if cursor < raw_lines.len() && strip_comment(raw_lines[cursor].raw)?.trim() == "---" {
        cursor += 1;
        skip_blank_and_comments(&raw_lines, &mut cursor)?;
    }
    if cursor >= raw_lines.len() || strip_comment(raw_lines[cursor].raw)?.trim() == "..." {
        return Ok(YamlValue::Map(IndexMap::new()));
    }

    let indent = leading_indent(&raw_lines[cursor])?;
    let value = parse_block(&raw_lines, &mut cursor, indent, 0)?;
    skip_blank_and_comments(&raw_lines, &mut cursor)?;
    if cursor < raw_lines.len() && strip_comment(raw_lines[cursor].raw)?.trim() == "..." {
        cursor += 1;
        skip_blank_and_comments(&raw_lines, &mut cursor)?;
    }
    if cursor < raw_lines.len() {
        return Err(format!("Line {}: unexpected trailing content.", raw_lines[cursor].number));
    }
    Ok(value)
}

fn leading_indent(line: &RawLine<'_>) -> Result<usize, String> {
    let mut count = 0;
    for byte in line.raw.bytes() {
        match byte {
            b' ' => count += 1,
            b'\t' => return Err(format!("Line {}: tab characters are not allowed for YAML indentation.", line.number)),
            _ => break,
        }
    }
    Ok(count)
}

fn skip_blank_and_comments(lines: &[RawLine<'_>], cursor: &mut usize) -> Result<(), String> {
    while *cursor < lines.len() {
        let line = &lines[*cursor];
        let trimmed_ws = line.raw.trim();
        if trimmed_ws.is_empty() || trimmed_ws.starts_with('#') {
            *cursor += 1;
            continue;
        }
        let cleaned = strip_comment(line.raw)?;
        if cleaned.trim().is_empty() {
            *cursor += 1;
            continue;
        }
        if cleaned.trim() == "..." {
            break;
        }
        break;
    }
    Ok(())
}

/// Whether a quote at `index` starts a quoted scalar: at the start of the line or after a `:`, `-`, `?`, `,`, `[` or `{` (and the
/// blanks after it). A quote inside a plain scalar (`Don't`, `say "hi"`) opens nothing.
fn opens_scalar(raw: &str, index: usize) -> bool {
    let before = &raw[..index];
    let trimmed = before.trim_end_matches([' ', '\t']);
    match trimmed.as_bytes().last() {
        None | Some(b',' | b'[' | b'{') => true,
        Some(b':' | b'-' | b'?') => trimmed.len() < before.len(),
        Some(_) => false,
    }
}

/// Strips an unquoted `#` comment (at start of string or preceded by space/tab) and trailing spaces/tabs.
fn strip_comment(raw: &str) -> Result<&str, String> {
    let bytes = raw.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut index = 0;
    while index < bytes.len() {
        let ch = bytes[index];
        if in_single {
            if ch == b'\'' {
                if bytes.get(index + 1) == Some(&b'\'') {
                    index += 2;
                    continue;
                }
                in_single = false;
            }
            index += 1;
            continue;
        }
        if in_double {
            if ch == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if ch == b'"' {
                in_double = false;
            }
            index += 1;
            continue;
        }
        match ch {
            b'\'' if opens_scalar(raw, index) => in_single = true,
            b'"' if opens_scalar(raw, index) => in_double = true,
            b'#' if index == 0 || matches!(bytes[index - 1], b' ' | b'\t') => {
                return Ok(raw[..index].trim_end_matches([' ', '\t']));
            }
            _ => {}
        }
        index += 1;
    }
    if in_single || in_double {
        return Err("Unterminated quoted string.".into());
    }
    Ok(raw.trim_end_matches([' ', '\t']))
}

fn parse_block(lines: &[RawLine<'_>], cursor: &mut usize, expected_indent: usize, depth: usize) -> Result<YamlValue, String> {
    if depth > MAXIMUM_YAML_DEPTH {
        return Err("YAML nesting is too deep.".into());
    }
    skip_blank_and_comments(lines, cursor)?;
    if *cursor >= lines.len() {
        return Ok(YamlValue::Null);
    }
    let line = &lines[*cursor];
    let indent = leading_indent(line)?;
    if indent < expected_indent {
        return Ok(YamlValue::Null);
    }
    let cleaned = strip_comment(line.raw).map_err(|e| format!("Line {}: {e}", line.number))?;
    let content = &cleaned[indent..];
    if content.starts_with('[') || content.starts_with('{') {
        *cursor += 1;
        return parse_inline_value(content, line.number, depth + 1);
    }
    if is_sequence_item(content) {
        parse_sequence(lines, cursor, indent, depth + 1)
    } else {
        parse_mapping(lines, cursor, indent, depth + 1)
    }
}

fn is_sequence_item(content: &str) -> bool {
    content == "-" || content.starts_with("- ") || content.starts_with("-\t")
}

fn parse_sequence(lines: &[RawLine<'_>], cursor: &mut usize, seq_indent: usize, depth: usize) -> Result<YamlValue, String> {
    if depth > MAXIMUM_YAML_DEPTH {
        return Err("YAML nesting is too deep.".into());
    }
    let mut items = Vec::new();
    loop {
        skip_blank_and_comments(lines, cursor)?;
        if *cursor >= lines.len() {
            break;
        }
        let line = &lines[*cursor];
        let cleaned = strip_comment(line.raw).map_err(|e| format!("Line {}: {e}", line.number))?;
        if cleaned.trim() == "..." {
            break;
        }
        let indent = leading_indent(line)?;
        if indent < seq_indent {
            break;
        }
        if indent > seq_indent {
            return Err(format!("Line {}: unexpected indentation inside sequence.", line.number));
        }
        let content = &cleaned[indent..];
        if !is_sequence_item(content) {
            break;
        }
        let line_no = line.number;
        let after_dash = content[1..].trim_start_matches(' ');
        if after_dash.starts_with('\t') {
            return Err(format!("Line {line_no}: tab characters are not allowed after '-'."));
        }
        let item_col = cleaned.len() - after_dash.len();
        *cursor += 1;

        if after_dash.is_empty() {
            skip_blank_and_comments(lines, cursor)?;
            if *cursor < lines.len() {
                let next_indent = leading_indent(&lines[*cursor])?;
                if next_indent > seq_indent && strip_comment(lines[*cursor].raw).map(|s| s.trim() != "...").unwrap_or(true) {
                    items.push(parse_block(lines, cursor, next_indent, depth + 1)?);
                    continue;
                }
            }
            items.push(YamlValue::Null);
            continue;
        }

        if let Some(block_header) = parse_block_scalar_header(after_dash, line_no)? {
            let text = read_block_scalar(lines, cursor, seq_indent, block_header)?;
            items.push(YamlValue::Scalar(text));
            continue;
        }

        if !after_dash.starts_with('[')
            && !after_dash.starts_with('{')
            && let Some((key, val_rest)) = try_split_mapping_entry(after_dash, line_no)?
        {
            let mut map = IndexMap::new();
            let first_val = parse_mapping_rhs(lines, cursor, item_col, val_rest, line_no, depth + 1)?;
            map.insert(key, first_val);
            parse_mapping_into(lines, cursor, item_col, depth + 1, &mut map)?;
            items.push(YamlValue::Map(map));
            continue;
        }

        items.push(parse_inline_value(after_dash, line_no, depth + 1)?);
    }
    Ok(YamlValue::Seq(items))
}

fn parse_mapping(lines: &[RawLine<'_>], cursor: &mut usize, map_indent: usize, depth: usize) -> Result<YamlValue, String> {
    let mut map = IndexMap::new();
    parse_mapping_into(lines, cursor, map_indent, depth, &mut map)?;
    Ok(YamlValue::Map(map))
}

fn parse_mapping_into(
    lines: &[RawLine<'_>],
    cursor: &mut usize,
    map_indent: usize,
    depth: usize,
    map: &mut IndexMap<String, YamlValue>,
) -> Result<(), String> {
    if depth > MAXIMUM_YAML_DEPTH {
        return Err("YAML nesting is too deep.".into());
    }
    loop {
        skip_blank_and_comments(lines, cursor)?;
        if *cursor >= lines.len() {
            break;
        }
        let line = &lines[*cursor];
        let cleaned = strip_comment(line.raw).map_err(|e| format!("Line {}: {e}", line.number))?;
        if cleaned.trim() == "..." {
            break;
        }
        let indent = leading_indent(line)?;
        if indent < map_indent {
            break;
        }
        if indent > map_indent {
            return Err(format!("Line {}: unexpected indentation in mapping.", line.number));
        }
        let content = &cleaned[indent..];
        if is_sequence_item(content) {
            break;
        }
        let line_no = line.number;
        let Some((key, val_rest)) = try_split_mapping_entry(content, line_no)? else {
            return Err(format!("Line {line_no}: expected 'key: value' mapping entry."));
        };
        if map.contains_key(&key) {
            return Err(format!("Line {line_no}: duplicate key '{key}'."));
        }
        *cursor += 1;
        let value = parse_mapping_rhs(lines, cursor, map_indent, val_rest, line_no, depth + 1)?;
        map.insert(key, value);
    }
    Ok(())
}

fn parse_mapping_rhs(
    lines: &[RawLine<'_>],
    cursor: &mut usize,
    parent_indent: usize,
    val_rest: &str,
    line_no: usize,
    depth: usize,
) -> Result<YamlValue, String> {
    let trimmed = val_rest.trim();
    if trimmed.is_empty() {
        skip_blank_and_comments(lines, cursor)?;
        if *cursor < lines.len() {
            let next_cleaned = strip_comment(lines[*cursor].raw).map_err(|e| format!("Line {}: {e}", lines[*cursor].number))?;
            if next_cleaned.trim() != "..." {
                let next_indent = leading_indent(&lines[*cursor])?;
                if next_indent > parent_indent {
                    return parse_block(lines, cursor, next_indent, depth);
                }
                if next_indent == parent_indent && is_sequence_item(&next_cleaned[next_indent..]) {
                    return parse_sequence(lines, cursor, next_indent, depth);
                }
            }
        }
        return Ok(YamlValue::Null);
    }
    if let Some(header) = parse_block_scalar_header(trimmed, line_no)? {
        let text = read_block_scalar(lines, cursor, parent_indent, header)?;
        return Ok(YamlValue::Scalar(text));
    }
    parse_inline_value(trimmed, line_no, depth)
}

#[derive(Debug, Clone, Copy)]
enum BlockStyle {
    Literal,
    Folded,
}

#[derive(Debug, Clone, Copy)]
enum Chomping {
    Clip,
    Strip,
    Keep,
}

#[derive(Debug, Clone, Copy)]
struct BlockScalarHeader {
    style: BlockStyle,
    chomping: Chomping,
}

fn parse_block_scalar_header(token: &str, line_no: usize) -> Result<Option<BlockScalarHeader>, String> {
    let first = match token.chars().next() {
        Some('|') => BlockStyle::Literal,
        Some('>') => BlockStyle::Folded,
        _ => return Ok(None),
    };
    let rest = token[1..].trim();
    let chomping = match rest {
        "" => Chomping::Clip,
        "-" => Chomping::Strip,
        "+" => Chomping::Keep,
        _ => return Err(format!("Line {line_no}: unsupported block scalar header '{token}'.")),
    };
    Ok(Some(BlockScalarHeader { style: first, chomping }))
}

fn read_block_scalar(lines: &[RawLine<'_>], cursor: &mut usize, parent_indent: usize, header: BlockScalarHeader) -> Result<String, String> {
    let mut collected: Vec<(usize, &str)> = Vec::new();
    let mut min_indent: Option<usize> = None;

    while *cursor < lines.len() {
        let line = &lines[*cursor];
        let raw = line.raw;
        let spaces = raw.bytes().take_while(|b| *b == b' ').count();
        let rest = &raw[spaces..];
        if rest.is_empty() {
            collected.push((spaces, ""));
            *cursor += 1;
            continue;
        }
        if rest.starts_with('\t') && spaces <= parent_indent {
            return Err(format!("Line {}: tab characters are not allowed for YAML indentation.", line.number));
        }
        if spaces <= parent_indent {
            break;
        }
        min_indent = Some(min_indent.map_or(spaces, |cur| cur.min(spaces)));
        collected.push((spaces, raw));
        *cursor += 1;
    }

    let base_indent = min_indent.unwrap_or(0);
    let mut trailing_empty = 0usize;
    while collected.last().is_some_and(|(_, raw)| raw.is_empty()) {
        collected.pop();
        trailing_empty += 1;
    }

    if collected.is_empty() {
        return Ok(match header.chomping {
            Chomping::Keep if trailing_empty > 0 => "\n".repeat(trailing_empty),
            _ => String::new(),
        });
    }

    let sliced: Vec<(bool, &str)> = collected
        .into_iter()
        .map(|(spaces, raw)| {
            if raw.is_empty() {
                (false, "")
            } else {
                let extra = spaces > base_indent;
                (extra, &raw[base_indent.min(raw.len())..])
            }
        })
        .collect();

    let mut body = match header.style {
        BlockStyle::Literal => sliced.iter().map(|(_, text)| *text).collect::<Vec<_>>().join("\n"),
        BlockStyle::Folded => fold_block_lines(&sliced),
    };

    match header.chomping {
        Chomping::Strip => {}
        Chomping::Clip => body.push('\n'),
        Chomping::Keep => {
            body.push('\n');
            for _ in 0..trailing_empty {
                body.push('\n');
            }
        }
    }
    Ok(body)
}

fn fold_block_lines(lines: &[(bool, &str)]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() {
        let (extra, text) = lines[index];
        if text.is_empty() {
            let mut empty_run = 0usize;
            while index < lines.len() && lines[index].1.is_empty() {
                empty_run += 1;
                index += 1;
            }
            for _ in 0..empty_run {
                out.push('\n');
            }
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            if extra {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out.push_str(text);
        if extra && index + 1 < lines.len() && !lines[index + 1].1.is_empty() {
            out.push('\n');
        }
        index += 1;
    }
    out
}

/// Attempts to split `content` into `(key, rhs)` if `content` is a mapping entry (`key:` or `key: ...`).
fn try_split_mapping_entry(content: &str, line_no: usize) -> Result<Option<(String, &str)>, String> {
    let trimmed = content.trim_start();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('\'') {
        let (key, rest) = parse_single_quoted_prefix(trimmed, line_no)?;
        let rest_trimmed = rest.trim_start_matches([' ', '\t']);
        if let Some(after_colon) = rest_trimmed.strip_prefix(':')
            && (after_colon.is_empty() || after_colon.starts_with([' ', '\t']))
        {
            return Ok(Some((key, after_colon)));
        }
        return Ok(None);
    }
    if trimmed.starts_with('"') {
        let (key, rest) = parse_double_quoted_prefix(trimmed, line_no)?;
        let rest_trimmed = rest.trim_start_matches([' ', '\t']);
        if let Some(after_colon) = rest_trimmed.strip_prefix(':')
            && (after_colon.is_empty() || after_colon.starts_with([' ', '\t']))
        {
            return Ok(Some((key, after_colon)));
        }
        return Ok(None);
    }
    let bytes = trimmed.as_bytes();
    for idx in 0..bytes.len() {
        if bytes[idx] == b':' && (idx + 1 == bytes.len() || matches!(bytes[idx + 1], b' ' | b'\t')) {
            let raw_key = trimmed[..idx].trim();
            if raw_key.is_empty() || raw_key.contains(['[', ']', '{', '}']) {
                return Ok(None);
            }
            return Ok(Some((raw_key.to_string(), &trimmed[idx + 1..])));
        }
    }
    Ok(None)
}

fn parse_inline_value(raw: &str, line_no: usize, depth: usize) -> Result<YamlValue, String> {
    if depth > MAXIMUM_YAML_DEPTH {
        return Err(format!("Line {line_no}: YAML nesting is too deep."));
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() || matches!(trimmed, "~" | "null" | "Null" | "NULL") {
        return Ok(YamlValue::Null);
    }
    if trimmed.starts_with('[') {
        if !trimmed.ends_with(']') {
            return Err(format!("Line {line_no}: unclosed inline list '['."));
        }
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.is_empty() {
            return Ok(YamlValue::Seq(Vec::new()));
        }
        let parts = split_flow_items(inner, line_no)?;
        let mut items = Vec::with_capacity(parts.len());
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            items.push(parse_inline_value(part, line_no, depth + 1)?);
        }
        return Ok(YamlValue::Seq(items));
    }
    if trimmed.starts_with('{') {
        if !trimmed.ends_with('}') {
            return Err(format!("Line {line_no}: unclosed inline mapping '{{'."));
        }
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.is_empty() {
            return Ok(YamlValue::Map(IndexMap::new()));
        }
        let parts = split_flow_items(inner, line_no)?;
        let mut map = IndexMap::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((key, rhs)) = split_flow_mapping_pair(part, line_no)? else {
                return Err(format!("Line {line_no}: expected 'key: value' inside inline mapping."));
            };
            if map.contains_key(&key) {
                return Err(format!("Line {line_no}: duplicate key '{key}' in inline mapping."));
            }
            map.insert(key, parse_inline_value(rhs, line_no, depth + 1)?);
        }
        return Ok(YamlValue::Map(map));
    }
    if trimmed.starts_with('\'') {
        let (text, rest) = parse_single_quoted_prefix(trimmed, line_no)?;
        if !rest.trim().is_empty() {
            return Err(format!("Line {line_no}: unexpected trailing characters after single-quoted string."));
        }
        return Ok(YamlValue::Scalar(text));
    }
    if trimmed.starts_with('"') {
        let (text, rest) = parse_double_quoted_prefix(trimmed, line_no)?;
        if !rest.trim().is_empty() {
            return Err(format!("Line {line_no}: unexpected trailing characters after double-quoted string."));
        }
        return Ok(YamlValue::Scalar(text));
    }
    Ok(YamlValue::Scalar(trimmed.to_string()))
}

fn split_flow_items(inner: &str, line_no: usize) -> Result<Vec<&str>, String> {
    let bytes = inner.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut depth_bracket = 0i32;
    let mut depth_brace = 0i32;

    while index < bytes.len() {
        let ch = bytes[index];
        if in_single {
            if ch == b'\'' {
                if bytes.get(index + 1) == Some(&b'\'') {
                    index += 2;
                    continue;
                }
                in_single = false;
            }
            index += 1;
            continue;
        }
        if in_double {
            if ch == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if ch == b'"' {
                in_double = false;
            }
            index += 1;
            continue;
        }
        match ch {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'[' => depth_bracket += 1,
            b']' => {
                depth_bracket -= 1;
                if depth_bracket < 0 {
                    return Err(format!("Line {line_no}: unmatched ']' in inline collection."));
                }
            }
            b'{' => depth_brace += 1,
            b'}' => {
                depth_brace -= 1;
                if depth_brace < 0 {
                    return Err(format!("Line {line_no}: unmatched '}}' in inline collection."));
                }
            }
            b',' if depth_bracket == 0 && depth_brace == 0 => {
                parts.push(&inner[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if in_single || in_double || depth_bracket != 0 || depth_brace != 0 {
        return Err(format!("Line {line_no}: malformed inline collection."));
    }
    parts.push(&inner[start..]);
    Ok(parts)
}

fn split_flow_mapping_pair(entry: &str, line_no: usize) -> Result<Option<(String, &str)>, String> {
    let trimmed = entry.trim();
    if trimmed.starts_with('\'') {
        let (key, rest) = parse_single_quoted_prefix(trimmed, line_no)?;
        let rest = rest.trim_start();
        return Ok(rest.strip_prefix(':').map(|rhs| (key, rhs)));
    }
    if trimmed.starts_with('"') {
        let (key, rest) = parse_double_quoted_prefix(trimmed, line_no)?;
        let rest = rest.trim_start();
        return Ok(rest.strip_prefix(':').map(|rhs| (key, rhs)));
    }
    if let Some((raw_key, rhs)) = trimmed.split_once(':') {
        let key = raw_key.trim();
        if key.is_empty() {
            return Ok(None);
        }
        return Ok(Some((key.to_string(), rhs)));
    }
    Ok(None)
}

fn parse_single_quoted_prefix(input: &str, line_no: usize) -> Result<(String, &str), String> {
    let rest = input.strip_prefix('\'').ok_or_else(|| format!("Line {line_no}: expected single quote."))?;
    let mut out = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '\'' {
            if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return Ok((out, &rest[idx + 1..]));
        }
        out.push(ch);
    }
    Err(format!("Line {line_no}: unterminated single-quoted string."))
}

fn parse_double_quoted_prefix(input: &str, line_no: usize) -> Result<(String, &str), String> {
    let rest = input.strip_prefix('"').ok_or_else(|| format!("Line {line_no}: expected double quote."))?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, ch)) = chars.next() {
        if ch == '"' {
            return Ok((out, &rest[idx + 1..]));
        }
        if ch == '\\' {
            let Some((_, esc)) = chars.next() else {
                return Err(format!("Line {line_no}: trailing backslash in double-quoted string."));
            };
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let mut hex = String::with_capacity(4);
                    for _ in 0..4 {
                        let Some((_, h)) = chars.next() else {
                            return Err(format!("Line {line_no}: incomplete \\u escape."));
                        };
                        hex.push(h);
                    }
                    let code = u32::from_str_radix(&hex, 16).map_err(|_| format!("Line {line_no}: invalid \\u{hex} escape."))?;
                    if code == 0 {
                        return Err(format!("Line {line_no}: NUL escape is not allowed."));
                    }
                    let decoded = char::from_u32(code).ok_or_else(|| format!("Line {line_no}: invalid Unicode scalar \\u{hex}."))?;
                    out.push(decoded);
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }
        out.push(ch);
    }
    Err(format!("Line {line_no}: unterminated double-quoted string."))
}

// ── Schema decoding & normalization ─────────────────────────────────────────────────────────────────────────────────────

/// Parses a single `.mighty.yaml` or `.mighty.local.yaml` text into a normalized [`MightyRepoConfig`].
pub fn parse_config(input: &str) -> Result<MightyRepoConfig, String> {
    Ok(parse_layer(input)?.config)
}

fn parse_layer(input: &str) -> Result<ParsedLayer, String> {
    let root = parse_yaml(input)?;
    let map = match root {
        YamlValue::Null => IndexMap::new(),
        YamlValue::Map(map) => map,
        _ => return Err("Top-level YAML value in .mighty.yaml must be a mapping.".into()),
    };

    let mut version = 1u32;
    let mut worktree = WorktreeConfig::default();
    let mut setup_specified = false;
    let mut teardown_specified = false;
    let mut tasks = IndexMap::new();
    let mut saw_tasks = false;
    let mut run = IndexMap::new();
    let mut hooks = HooksConfig::default();
    let mut schedules = IndexMap::new();
    let mut triggers = IndexMap::new();
    let mut verify = VerifyConfig::default();

    for (key, value) in map {
        match key.as_str() {
            "version" => {
                let raw = expect_scalar(&value, "version")?;
                let parsed: u32 = raw.trim().parse().map_err(|_| format!("Unsupported version '{raw}': expected version: 1."))?;
                if parsed != 1 {
                    return Err(format!("Unsupported .mighty.yaml version {parsed}: only version 1 is supported."));
                }
                version = parsed;
            }
            "worktree" => {
                let (wt, has_setup, has_teardown) = decode_worktree(&value)?;
                worktree = wt;
                setup_specified = has_setup;
                teardown_specified = has_teardown;
            }
            "tasks" | "scripts" => {
                if saw_tasks {
                    return Err("Specify only one of 'tasks' or 'scripts', not both.".into());
                }
                saw_tasks = true;
                tasks = decode_tasks(&value)?;
            }
            "run" => {
                run = decode_run_actions(&value)?;
            }
            "hooks" => {
                hooks = decode_hooks(&value)?;
            }
            "schedules" => {
                schedules = decode_schedules(&value)?;
            }
            "triggers" => {
                triggers = decode_triggers(&value)?;
            }
            "verify" => {
                verify = decode_verify(&value)?;
            }
            other => return Err(format!("Unknown top-level key '{other}' in .mighty.yaml.")),
        }
    }

    Ok(ParsedLayer {
        config: MightyRepoConfig { version, worktree, tasks, run, hooks, schedules, triggers, verify },
        setup_specified,
        teardown_specified,
    })
}

/// A schedule's or trigger's action keys (`prompt`, `agent`, `model`, `worktree`, `mode`, `pane`) into `action`; false for any
/// other key.
fn decode_action_key(action: &mut Action, key: &str, item: &YamlValue, field: &str) -> Result<bool, String> {
    let text = |item: &YamlValue| -> Result<Option<String>, String> {
        let raw = expect_scalar(item, &format!("{field}.{key}"))?;
        let trimmed = raw.trim();
        if has_invisible_format_chars(trimmed) {
            return Err(format!("'{field}.{key}' contains invisible formatting characters."));
        }
        Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
    };
    match key {
        "prompt" => action.prompt = text(item)?.unwrap_or_default(),
        "agent" => action.agent = text(item)?,
        "model" => action.model = text(item)?,
        "worktree" => action.worktree = decode_bool(item, &format!("{field}.worktree"))?,
        "mode" => {
            let raw = expect_scalar(item, &format!("{field}.mode"))?;
            action.mode = FireMode::parse(&raw).ok_or_else(|| format!("'{field}.mode' must be new or heartbeat."))?;
        }
        "pane" => action.pane = text(item)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// A named entry of `schedules` or `triggers`: its name and its fields.
type NamedEntry<'a> = (&'a String, &'a IndexMap<String, YamlValue>);

fn named_entries<'a>(value: &'a YamlValue, key: &str, max: usize) -> Result<Vec<NamedEntry<'a>>, String> {
    let map = match value {
        YamlValue::Null => return Ok(vec![]),
        YamlValue::Map(map) => map,
        _ => return Err(format!("'{key}' must be a mapping of names to settings.")),
    };
    if map.len() > max {
        return Err(format!("'{key}' exceeds maximum of {max} entries."));
    }
    map.iter()
        .map(|(name, item)| {
            automation::valid_name(name).map_err(|_| format!("Invalid name '{name}' in '{key}'."))?;
            match item {
                YamlValue::Map(fields) => Ok((name, fields)),
                _ => Err(format!("'{key}.{name}' must be a mapping.")),
            }
        })
        .collect()
}

/// `schedules`: `name: { cron | rrule, prompt, agent, model, worktree, mode, pane, catchUp, enabled }`.
fn decode_schedules(value: &YamlValue) -> Result<IndexMap<String, ScheduleSpec>, String> {
    let mut out = IndexMap::new();
    for (name, fields) in named_entries(value, "schedules", automation::MAX_SCHEDULES)? {
        let field = format!("schedules.{name}");
        let mut spec = ScheduleSpec { name: name.clone(), enabled: true, ..ScheduleSpec::default() };
        for (key, item) in fields {
            match key.as_str() {
                "cron" => spec.cron = Some(expect_scalar(item, &format!("{field}.cron"))?.trim().to_string()),
                "rrule" => spec.rrule = Some(expect_scalar(item, &format!("{field}.rrule"))?.trim().to_string()),
                "catchUp" => {
                    let raw = expect_scalar(item, &format!("{field}.catchUp"))?;
                    spec.catch_up = CatchUp::parse(&raw).ok_or_else(|| format!("'{field}.catchUp' must be skip or once."))?;
                }
                "enabled" => spec.enabled = decode_bool(item, &format!("{field}.enabled"))?,
                other => {
                    if !decode_action_key(&mut spec.action, other, item, &field)? {
                        return Err(format!("Unknown key '{field}.{other}' in .mighty.yaml."));
                    }
                }
            }
        }
        spec.validate(&mighty_core::wire::PROVIDERS).map_err(|e| format!("'{field}': {e}."))?;
        out.insert(name.clone(), spec);
    }
    Ok(out)
}

/// A list (or a comma-separated string) of names.
fn decode_name_list(item: &YamlValue, field: &str) -> Result<Vec<String>, String> {
    let names: Vec<String> = match item {
        YamlValue::Null => vec![],
        YamlValue::Scalar(text) => text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        YamlValue::Seq(items) => items.iter().map(|i| expect_scalar(i, field).map(|t| t.trim().to_string())).collect::<Result<_, _>>()?,
        YamlValue::Map(_) => return Err(format!("'{field}' must be a list.")),
    };
    if names.len() > 16 || names.iter().any(|n| n.chars().count() > 80 || has_invisible_format_chars(n)) {
        return Err(format!("'{field}' has too many or invalid names."));
    }
    Ok(names)
}

/// MC-97: a trigger's `where`: a list of conditions, each the long form (`{ field: labels, op: has, value: bot:dev }`) or the compact
/// one (`labels: { has: bot:dev }`, `base: main`, `author: [kim, lee]` for `in`).
fn decode_where(item: &YamlValue, field: &str) -> Result<Vec<mighty_core::automation::condition::Condition>, String> {
    fn json(value: &YamlValue) -> serde_json::Value {
        match value {
            YamlValue::Null => serde_json::Value::Null,
            YamlValue::Scalar(text) => serde_json::Value::String(text.clone()),
            YamlValue::Seq(items) => serde_json::Value::Array(items.iter().map(json).collect()),
            YamlValue::Map(map) => serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), json(v))).collect()),
        }
    }
    let items = match item {
        YamlValue::Null => return Ok(vec![]),
        YamlValue::Seq(items) => items,
        _ => return Err(format!("'{field}' must be a list of conditions.")),
    };
    let conditions = items
        .iter()
        .map(|c| mighty_core::automation::condition::condition_from(&json(c)).map_err(|e| format!("'{field}': {e}.")))
        .collect::<Result<Vec<_>, _>>()?;
    if conditions.iter().any(|c| c.values.iter().any(|v| has_invisible_format_chars(v))) {
        return Err(format!("'{field}' contains invisible formatting characters."));
    }
    Ok(conditions)
}

/// `triggers`: `name: { github: [pr_opened, review_requested, ci_failed] | linear: [states] | webhook: true | standing: <command>
/// (everyMinutes, dailyCap), where: [conditions], prompt, agent, model, worktree, mode, pane, enabled }`. MC-97: a GitHub trigger's
/// old `author`, `base` and `label` are read as the first of its `where` conditions.
fn decode_triggers(value: &YamlValue) -> Result<IndexMap<String, TriggerSpec>, String> {
    let mut out = IndexMap::new();
    for (name, fields) in named_entries(value, "triggers", automation::MAX_TRIGGERS)? {
        let field = format!("triggers.{name}");
        let mut action = Action::default();
        let mut enabled = true;
        let mut source: Option<TriggerSource> = None;
        let mut every = None;
        let mut cap = None;
        let mut repo: Option<String> = None;
        let mut filter = automation::GitHubFilter::default();
        let mut conditions: Vec<mighty_core::automation::condition::Condition> = vec![];
        let mut outcome: Option<Vec<automation::BotOutcome>> = None;
        let mut set_source = |next: TriggerSource| -> Result<(), String> {
            if source.replace(next).is_some() {
                return Err(format!("'{field}' names more than one of github, linear, webhook, standing, event, bot."));
            }
            Ok(())
        };
        for (key, item) in fields {
            match key.as_str() {
                "github" => {
                    let names = decode_name_list(item, &format!("{field}.github"))?;
                    let events = names
                        .iter()
                        .map(|n| {
                            GitHubEvent::parse(n).ok_or_else(|| {
                                let all = GitHubEvent::ALL.iter().map(|e| e.name()).collect::<Vec<_>>().join(", ");
                                format!("'{field}.github' names '{n}': use {all}.")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let events = GitHubEvent::ALL.into_iter().filter(|e| events.contains(e)).collect();
                    set_source(TriggerSource::Github { events, repo: None })?;
                }
                "linear" => set_source(TriggerSource::Linear { states: decode_name_list(item, &format!("{field}.linear"))? })?,
                "webhook" => {
                    if decode_bool(item, &format!("{field}.webhook"))? {
                        set_source(TriggerSource::Webhook)?;
                    }
                }
                "standing" => {
                    let command = decode_command_value(item, &format!("{field}.standing"), false)?
                        .ok_or_else(|| format!("'{field}.standing' cannot be empty."))?;
                    set_source(TriggerSource::Standing { command, every_minutes: 0, daily_cap: 0 })?;
                }
                "everyMinutes" => {
                    every = Some(decode_bounded_u32(
                        item,
                        &format!("{field}.everyMinutes"),
                        automation::MIN_EVERY_MINUTES,
                        automation::MAX_EVERY_MINUTES,
                    )?)
                }
                "dailyCap" => cap = Some(decode_bounded_u32(item, &format!("{field}.dailyCap"), 1, automation::MAX_DAILY_CAP)?),
                // MC-64: the repository a GitHub trigger watches, when not the workspace's own.
                "repo" => repo = Some(expect_scalar(item, &format!("{field}.repo"))?.trim().to_string()),
                // MC-97: the conditions its events must pass.
                "where" => conditions = decode_where(item, &format!("{field}.where"))?,
                // MC-66: a GitHub trigger's filter; MC-97: read as `where` conditions.
                "author" => filter.author = Some(expect_scalar(item, &format!("{field}.author"))?.trim().to_string()),
                "base" => filter.base = Some(expect_scalar(item, &format!("{field}.base"))?.trim().to_string()),
                "label" => filter.label = Some(expect_scalar(item, &format!("{field}.label"))?.trim().to_string()),
                // MC-66: an event `mighty event emit` sends, and another bot's request ending.
                "event" => set_source(TriggerSource::Event { name: expect_scalar(item, &format!("{field}.event"))?.trim().to_string() })?,
                "bot" => {
                    set_source(TriggerSource::Bot { bot: expect_scalar(item, &format!("{field}.bot"))?.trim().to_string(), on: vec![] })?
                }
                "outcome" => {
                    let names = decode_name_list(item, &format!("{field}.outcome"))?;
                    outcome = Some(
                        names
                            .iter()
                            .map(|n| {
                                automation::BotOutcome::parse(n).ok_or_else(|| format!("'{field}.outcome' names '{n}': use done, failed."))
                            })
                            .collect::<Result<_, _>>()?,
                    );
                }
                "enabled" => enabled = decode_bool(item, &format!("{field}.enabled"))?,
                other => {
                    if !decode_action_key(&mut action, other, item, &field)? {
                        return Err(format!("Unknown key '{field}.{other}' in .mighty.yaml."));
                    }
                }
            }
        }
        let on = match source {
            Some(TriggerSource::Standing { command, .. }) => TriggerSource::Standing {
                command,
                every_minutes: every.unwrap_or(automation::DEFAULT_EVERY_MINUTES),
                daily_cap: cap.unwrap_or(automation::DEFAULT_DAILY_CAP),
            },
            Some(_) if every.is_some() || cap.is_some() => {
                return Err(format!("'{field}': everyMinutes and dailyCap are for a standing order."));
            }
            Some(TriggerSource::Github { events, .. }) => {
                conditions.splice(0..0, std::mem::take(&mut filter).conditions());
                TriggerSource::Github { events, repo: repo.take() }
            }
            Some(TriggerSource::Bot { bot, .. }) => {
                TriggerSource::Bot { bot, on: outcome.take().unwrap_or_else(|| vec![automation::BotOutcome::Done]) }
            }
            Some(other) => other,
            None => return Err(format!("'{field}' needs one of github, linear, webhook, standing, event, bot.")),
        };
        if repo.is_some() || filter != automation::GitHubFilter::default() {
            return Err(format!("'{field}': repo, author, base and label are for a GitHub trigger."));
        }
        if outcome.is_some() {
            return Err(format!("'{field}': outcome is for a bot trigger."));
        }
        let spec = TriggerSpec { name: name.clone(), on, enabled, conditions, action };
        spec.validate(&mighty_core::wire::PROVIDERS).map_err(|e| format!("'{field}': {e}."))?;
        out.insert(name.clone(), spec);
    }
    Ok(out)
}

/// `hooks`: `command` (a string or a list, as `worktree.setup`), `on` (a list or a comma-separated string of [`HOOK_EVENTS`]) and
/// `timeoutSeconds` (1 to 300).
fn decode_hooks(value: &YamlValue) -> Result<HooksConfig, String> {
    let map = match value {
        YamlValue::Null => return Ok(HooksConfig::default()),
        YamlValue::Map(map) => map,
        _ => return Err("'hooks' must be a mapping.".into()),
    };
    let mut hooks = HooksConfig::default();
    for (key, item) in map {
        match key.as_str() {
            "command" => hooks.command = decode_command_value(item, "hooks.command", true)?,
            "on" => {
                let names: Vec<String> = match item {
                    YamlValue::Null => vec![],
                    YamlValue::Scalar(text) => text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                    YamlValue::Seq(items) => {
                        items.iter().map(|i| expect_scalar(i, "hooks.on").map(|t| t.trim().to_string())).collect::<Result<_, _>>()?
                    }
                    YamlValue::Map(_) => return Err("'hooks.on' must be a list of states.".into()),
                };
                if let Some(unknown) = names.iter().find(|n| !HOOK_EVENTS.contains(&n.as_str())) {
                    return Err(format!("'hooks.on' names '{unknown}': use {}.", HOOK_EVENTS.join(", ")));
                }
                hooks.on = HOOK_EVENTS.iter().filter(|e| names.iter().any(|n| n == *e)).map(|e| e.to_string()).collect();
            }
            "timeoutSeconds" => {
                hooks.timeout_seconds = Some(decode_bounded_u32(item, "hooks.timeoutSeconds", 1, MAXIMUM_HOOK_TIMEOUT_SECONDS)?);
            }
            other => return Err(format!("Unknown key 'hooks.{other}' in .mighty.yaml.")),
        }
    }
    Ok(hooks)
}

/// `verify` (`MC-48`): `commands` (a mapping of names to a command or `{ command, cwd, timeoutSeconds }`, or a list of
/// `{ name, command, … }`), `maxRounds` (0 to 5), `when` (`always` or `changed`) and `browser` (`url` or `runAction`, and `paths`).
fn decode_verify(value: &YamlValue) -> Result<VerifyConfig, String> {
    let map = match value {
        YamlValue::Null => return Ok(VerifyConfig::default()),
        YamlValue::Map(map) => map,
        _ => return Err("'verify' must be a mapping (commands, maxRounds, when, browser).".into()),
    };
    let mut verify = VerifyConfig::default();
    for (key, item) in map {
        match key.as_str() {
            "commands" => verify.commands = decode_verify_commands(item)?,
            "maxRounds" => {
                verify.max_rounds = Some(decode_bounded_u32(item, "verify.maxRounds", 0, MAXIMUM_VERIFY_MAX_ROUNDS)?);
            }
            "when" => {
                let raw = expect_scalar(item, "verify.when")?;
                verify.when = Some(match raw.trim().to_ascii_lowercase().as_str() {
                    "always" => VerifyWhen::Always,
                    "changed" => VerifyWhen::Changed,
                    _ => return Err("'verify.when' must be always or changed.".into()),
                });
            }
            "browser" => verify.browser = decode_verify_browser(item)?,
            other => return Err(format!("Unknown key 'verify.{other}' in .mighty.yaml.")),
        }
    }
    Ok(verify)
}

fn decode_verify_command(name: &str, value: &YamlValue) -> Result<VerifyCommandSpec, String> {
    let field = format!("verify.commands.{name}");
    match value {
        YamlValue::Scalar(_) | YamlValue::Seq(_) => {
            let command = decode_command_value(value, &field, false)?.ok_or_else(|| format!("'{field}' cannot be empty."))?;
            Ok(VerifyCommandSpec { command, cwd: None, timeout_seconds: None })
        }
        YamlValue::Map(spec) => {
            let mut command = None;
            let mut cwd = None;
            let mut timeout_seconds = None;
            for (k, v) in spec {
                match k.as_str() {
                    "name" => {}
                    "command" | "run" | "cmd" => {
                        if command.is_some() {
                            return Err(format!("Duplicate command key in '{field}'."));
                        }
                        command = Some(
                            decode_command_value(v, &format!("{field}.{k}"), false)?
                                .ok_or_else(|| format!("'{field}.{k}' cannot be empty."))?,
                        );
                    }
                    "cwd" => {
                        let raw = expect_scalar(v, &format!("{field}.cwd"))?;
                        let trimmed = raw.trim();
                        if !trimmed.is_empty() && trimmed != "." {
                            cwd = Some(normalize_single_rel_path(trimmed, &format!("{field}.cwd"))?);
                        }
                    }
                    "timeoutSeconds" => {
                        timeout_seconds =
                            Some(decode_bounded_u32(v, &format!("{field}.timeoutSeconds"), 1, MAXIMUM_VERIFY_TIMEOUT_SECONDS)?);
                    }
                    other => return Err(format!("Unknown key '{other}' in '{field}'.")),
                }
            }
            let command = command.ok_or_else(|| format!("Verify command '{name}' must define 'command'."))?;
            Ok(VerifyCommandSpec { command, cwd, timeout_seconds })
        }
        YamlValue::Null => Err(format!("Verify command '{name}' cannot be empty.")),
    }
}

fn decode_verify_commands(value: &YamlValue) -> Result<IndexMap<String, VerifyCommandSpec>, String> {
    let mut out = IndexMap::new();
    let mut add = |name: &str, spec: &YamlValue| -> Result<(), String> {
        if !is_valid_task_name(name) {
            return Err(format!("Invalid verify command name '{name}'."));
        }
        if out.contains_key(name) {
            return Err(format!("Verify command '{name}' is named twice."));
        }
        if out.len() >= MAXIMUM_VERIFY_COMMANDS {
            return Err(format!("'verify.commands' exceeds maximum of {MAXIMUM_VERIFY_COMMANDS} entries."));
        }
        let decoded = decode_verify_command(name, spec)?;
        out.insert(name.to_string(), decoded);
        Ok(())
    };
    match value {
        YamlValue::Null => {}
        YamlValue::Map(map) => {
            for (name, spec) in map {
                add(name, spec)?;
            }
        }
        YamlValue::Seq(items) => {
            for item in items {
                let YamlValue::Map(fields) = item else {
                    return Err("Each item of 'verify.commands' must be a mapping with 'name' and 'command'.".into());
                };
                let name = fields
                    .get("name")
                    .map(|n| expect_scalar(n, "verify.commands.name"))
                    .transpose()?
                    .ok_or_else(|| "Each item of 'verify.commands' must have a 'name'.".to_string())?;
                add(name.trim(), item)?;
            }
        }
        YamlValue::Scalar(_) => return Err("'verify.commands' must be a mapping of names to commands.".into()),
    }
    Ok(out)
}

/// Whether `url` is an `http(s)` address on this computer (`localhost`, `127.0.0.1`, `[::1]`) with a port.
pub fn is_local_dev_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        && parsed.port().is_some()
}

fn decode_verify_browser(value: &YamlValue) -> Result<Option<VerifyBrowserSpec>, String> {
    let map = match value {
        YamlValue::Null => return Ok(Some(VerifyBrowserSpec::default())),
        YamlValue::Map(map) => map,
        _ => return Err("'verify.browser' must be a mapping (url or runAction, paths).".into()),
    };
    let mut browser = VerifyBrowserSpec::default();
    for (key, item) in map {
        match key.as_str() {
            "url" => {
                let url = decode_run_url(item, "verify.browser.url")?;
                if url == "auto" || url == "off" {
                    continue;
                }
                if !is_local_dev_url(&url) {
                    return Err(
                        "'verify.browser.url' must be an http address on this computer (localhost or 127.0.0.1 with a port).".into()
                    );
                }
                browser.url = Some(url);
            }
            "runAction" => {
                let raw = expect_scalar(item, "verify.browser.runAction")?;
                let name = raw.trim();
                if !is_valid_task_name(name) {
                    return Err(format!("Invalid run action name '{name}' in 'verify.browser.runAction'."));
                }
                browser.run_action = Some(name.to_string());
            }
            "paths" => {
                let items: Vec<String> = match item {
                    YamlValue::Null => vec![],
                    YamlValue::Scalar(text) => vec![text.trim().to_string()],
                    YamlValue::Seq(items) => items
                        .iter()
                        .map(|i| expect_scalar(i, "verify.browser.paths").map(|t| t.trim().to_string()))
                        .collect::<Result<_, _>>()?,
                    YamlValue::Map(_) => return Err("'verify.browser.paths' must be a list of paths.".into()),
                };
                if items.len() > MAXIMUM_VERIFY_PATHS {
                    return Err(format!("'verify.browser.paths' exceeds maximum of {MAXIMUM_VERIFY_PATHS} entries."));
                }
                for path in &items {
                    if !path.starts_with('/')
                        || path.starts_with("//")
                        || path.chars().count() > MAXIMUM_VERIFY_PATH_CHARS
                        || has_invisible_format_chars(path)
                        || path.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\')
                    {
                        return Err(format!("'verify.browser.paths' entry '{path}' must be a path starting with / without spaces."));
                    }
                }
                browser.paths = items;
            }
            other => return Err(format!("Unknown key 'verify.browser.{other}' in .mighty.yaml.")),
        }
    }
    if browser.url.is_some() && browser.run_action.is_some() {
        return Err("Specify only one of 'verify.browser.url' or 'verify.browser.runAction'.".into());
    }
    Ok(Some(browser))
}

fn decode_worktree(value: &YamlValue) -> Result<(WorktreeConfig, bool, bool), String> {
    let map = match value {
        YamlValue::Null => return Ok((WorktreeConfig::default(), false, false)),
        YamlValue::Map(map) => map,
        _ => return Err("'worktree' must be a mapping.".into()),
    };

    let mut share = Vec::new();
    let mut saw_share = false;
    let mut copy = Vec::new();
    let mut setup = None;
    let mut setup_specified = false;
    let mut env = IndexMap::new();
    let mut teardown = None;
    let mut teardown_specified = false;
    let mut teardown_timeout_seconds = None;
    let mut ports = None;

    for (key, item) in map {
        match key.as_str() {
            "share" | "symlinks" => {
                if saw_share {
                    return Err("Specify only one of 'worktree.share' or 'worktree.symlinks'.".into());
                }
                saw_share = true;
                share = decode_share_list(item)?;
            }
            "copy" => {
                copy = decode_copy_list(item)?;
            }
            "setup" | "bootstrap" => {
                if setup_specified {
                    return Err("Specify only one of 'worktree.setup' or 'worktree.bootstrap'.".into());
                }
                setup_specified = true;
                setup = decode_command_value(item, "worktree.setup", true)?;
            }
            "env" => {
                env = decode_env_map(item)?;
            }
            "teardown" => {
                teardown_specified = true;
                teardown = decode_command_value(item, "worktree.teardown", true)?;
            }
            "teardownTimeoutSeconds" => {
                teardown_timeout_seconds =
                    Some(decode_bounded_u32(item, "worktree.teardownTimeoutSeconds", 1, MAXIMUM_TEARDOWN_TIMEOUT_SECONDS)?);
            }
            "ports" => {
                ports = Some(decode_bounded_u32(item, "worktree.ports", 0, MAXIMUM_WORKTREE_PORTS)?);
            }
            other => return Err(format!("Unknown key 'worktree.{other}' in .mighty.yaml.")),
        }
    }

    Ok((WorktreeConfig { share, copy, setup, env, teardown, teardown_timeout_seconds, ports }, setup_specified, teardown_specified))
}

/// A whole number from `min` to `max` (`ports: 10`, `teardownTimeoutSeconds: "90"`).
fn decode_bounded_u32(value: &YamlValue, field: &str, min: u32, max: u32) -> Result<u32, String> {
    let raw = expect_scalar(value, field)?;
    let trimmed = raw.trim();
    let parsed: u32 = trimmed.parse().map_err(|_| format!("'{field}' must be a whole number from {min} to {max}."))?;
    if parsed < min || parsed > max {
        return Err(format!("'{field}' must be a whole number from {min} to {max}."));
    }
    Ok(parsed)
}

/// `true`/`false` (also `yes`/`no`, `on`/`off`).
fn decode_bool(value: &YamlValue, field: &str) -> Result<bool, String> {
    let raw = expect_scalar(value, field)?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Ok(true),
        "false" | "no" | "off" => Ok(false),
        _ => Err(format!("'{field}' must be true or false.")),
    }
}

/// `auto` (also empty), `off` (also `false`, `none`), or a fixed `http://` / `https://` address without spaces or invisible
/// characters.
fn decode_run_url(value: &YamlValue, field: &str) -> Result<String, String> {
    let raw = match value {
        YamlValue::Null => return Ok("auto".into()),
        _ => expect_scalar(value, field)?,
    };
    let trimmed = raw.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "" | "auto" => return Ok("auto".into()),
        "off" | "false" | "none" | "no" => return Ok("off".into()),
        _ => {}
    }
    if trimmed.chars().count() > MAXIMUM_URL_CHARS {
        return Err(format!("'{field}' exceeds maximum length of {MAXIMUM_URL_CHARS} characters."));
    }
    if has_invisible_format_chars(trimmed) || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("'{field}' must not contain spaces, control or invisible characters."));
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| format!("'{field}' must be auto, off, or an http(s) address."))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return Err(format!("'{field}' must be auto, off, or an http(s) address."));
    }
    Ok(trimmed.to_string())
}

fn decode_run_actions(value: &YamlValue) -> Result<IndexMap<String, RunActionSpec>, String> {
    let map = match value {
        YamlValue::Null => return Ok(IndexMap::new()),
        YamlValue::Map(map) => map,
        _ => return Err("'run' must be a mapping of action names to commands.".into()),
    };
    if map.len() > MAXIMUM_RUN_ACTIONS {
        return Err(format!("'run' exceeds maximum of {MAXIMUM_RUN_ACTIONS} actions."));
    }
    let mut actions = IndexMap::new();
    for (name, spec_val) in map {
        if !is_valid_task_name(name) {
            return Err(format!("Invalid run action name '{name}'."));
        }
        let field = format!("run.{name}");
        let spec = match spec_val {
            YamlValue::Scalar(_) | YamlValue::Seq(_) => {
                let command = decode_command_value(spec_val, &field, false)?.ok_or_else(|| format!("'{field}' cannot be empty."))?;
                RunActionSpec { command, concurrent: true, url: default_url() }
            }
            YamlValue::Map(spec_map) => {
                let mut command = None;
                let mut concurrent = true;
                let mut url = default_url();
                for (k, v) in spec_map {
                    match k.as_str() {
                        "command" | "cmd" => {
                            if command.is_some() {
                                return Err(format!("Duplicate command key in '{field}'."));
                            }
                            command = Some(
                                decode_command_value(v, &format!("{field}.{k}"), false)?
                                    .ok_or_else(|| format!("'{field}.{k}' cannot be empty."))?,
                            );
                        }
                        "concurrent" => concurrent = decode_bool(v, &format!("{field}.concurrent"))?,
                        "url" => url = decode_run_url(v, &format!("{field}.url"))?,
                        other => return Err(format!("Unknown key '{other}' in '{field}'.")),
                    }
                }
                let Some(command) = command else {
                    return Err(format!("Run action '{name}' must define 'command'."));
                };
                RunActionSpec { command, concurrent, url }
            }
            YamlValue::Null => return Err(format!("Run action '{name}' cannot be empty.")),
        };
        actions.insert(name.clone(), spec);
    }
    Ok(actions)
}

fn normalize_single_rel_path(raw: &str, field: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("'{field}' contains an empty path."));
    }
    let Some(mut normalized) = worktree_copy_paths([Some(trimmed)]) else {
        return Err(format!("'{field}' path '{trimmed}' must be a safe relative path inside the repository."));
    };
    Ok(normalized.remove(0))
}

fn decode_share_list(value: &YamlValue) -> Result<Vec<String>, String> {
    let items = match value {
        YamlValue::Null => return Ok(Vec::new()),
        YamlValue::Seq(seq) => seq,
        _ => return Err("'worktree.share' must be a list of relative directory paths.".into()),
    };
    if items.len() > MAXIMUM_WORKTREE_RULES {
        return Err(format!("'worktree.share' exceeds maximum of {MAXIMUM_WORKTREE_RULES} entries."));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let raw = expect_scalar(item, "worktree.share")?;
        let norm = normalize_single_rel_path(&raw, "worktree.share")?;
        if seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    Ok(out)
}

fn decode_copy_list(value: &YamlValue) -> Result<Vec<CopyRule>, String> {
    let items = match value {
        YamlValue::Null => return Ok(Vec::new()),
        YamlValue::Seq(seq) => seq,
        _ => return Err("'worktree.copy' must be a list of copy rules.".into()),
    };
    if items.len() > MAXIMUM_WORKTREE_RULES {
        return Err(format!("'worktree.copy' exceeds maximum of {MAXIMUM_WORKTREE_RULES} entries."));
    }
    let mut out: Vec<CopyRule> = Vec::new();
    for item in items {
        let rule = match item {
            YamlValue::Scalar(text) => {
                if let Some((lhs, rhs)) = text.split_once("->") {
                    let from = normalize_single_rel_path(lhs, "worktree.copy.from")?;
                    let to = normalize_single_rel_path(rhs, "worktree.copy.to")?;
                    CopyRule { from, to }
                } else {
                    let path = normalize_single_rel_path(text, "worktree.copy")?;
                    CopyRule::same_path(path)
                }
            }
            YamlValue::Map(map) => {
                let mut from_raw = None;
                let mut to_raw = None;
                for (k, v) in map {
                    match k.as_str() {
                        "from" | "src" | "source" => {
                            if from_raw.is_some() {
                                return Err("Duplicate 'from' key in 'worktree.copy' rule.".into());
                            }
                            from_raw = Some(expect_scalar(v, "worktree.copy.from")?);
                        }
                        "to" | "dst" | "dest" | "target" => {
                            if to_raw.is_some() {
                                return Err("Duplicate 'to' key in 'worktree.copy' rule.".into());
                            }
                            to_raw = Some(expect_scalar(v, "worktree.copy.to")?);
                        }
                        other => return Err(format!("Unknown key '{other}' in 'worktree.copy' rule.")),
                    }
                }
                let Some(from_raw) = from_raw else {
                    return Err("Mapping entry in 'worktree.copy' requires a 'from' path.".into());
                };
                let from = normalize_single_rel_path(&from_raw, "worktree.copy.from")?;
                let to = match to_raw {
                    Some(raw) => normalize_single_rel_path(&raw, "worktree.copy.to")?,
                    None => from.clone(),
                };
                CopyRule { from, to }
            }
            _ => return Err("Each entry in 'worktree.copy' must be a string or '{ from, to }' mapping.".into()),
        };
        upsert_copy_rule(&mut out, rule);
    }
    Ok(out)
}

fn upsert_copy_rule(rules: &mut Vec<CopyRule>, next: CopyRule) {
    if let Some(existing) = rules.iter_mut().find(|r| r.to == next.to) {
        *existing = next;
    } else {
        rules.push(next);
    }
}

fn decode_command_value(value: &YamlValue, field: &str, allow_empty: bool) -> Result<Option<String>, String> {
    let raw_parts: Vec<String> = match value {
        YamlValue::Null => {
            return if allow_empty { Ok(None) } else { Err(format!("'{field}' cannot be empty.")) };
        }
        YamlValue::Scalar(s) => vec![s.clone()],
        YamlValue::Seq(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                list.push(expect_scalar(item, field)?);
            }
            list
        }
        _ => return Err(format!("'{field}' must be a command string or list of command strings.")),
    };

    let joined = normalize_shell_commands(&raw_parts);
    if has_invisible_format_chars(&joined) {
        return Err(format!("'{field}' contains an invisible formatting character (bidi control or zero-width space)."));
    }
    if joined.is_empty() {
        return if allow_empty { Ok(None) } else { Err(format!("'{field}' cannot be empty.")) };
    }
    if joined.chars().count() > MAXIMUM_COMMAND_CHARS {
        return Err(format!("'{field}' exceeds maximum length of {MAXIMUM_COMMAND_CHARS} characters."));
    }
    Ok(Some(joined))
}

/// Normalizes one or more command strings into one shell command. Single-line commands (a list's items, a folded scalar) are joined
/// with ` && `; a literal block scalar (`|`) of several lines stays a script with real newlines, so `if`/`for`/here-documents keep
/// their meaning (the runners hand it to `sh -ec`; [`flatten_script`] makes it one line for `cmd.exe`). Blank and comment lines go.
pub fn normalize_shell_commands(parts: &[String]) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut script = false;
    for part in parts {
        let before = segments.len();
        let mut current = String::new();
        for raw_line in part.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(continued) = trimmed.strip_suffix('\\') {
                let piece = continued.trim_end();
                if !piece.is_empty() {
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(piece);
                }
                continue;
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(trimmed);
            segments.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            segments.push(current);
        }
        if segments.len() - before > 1 {
            script = true;
        }
    }
    if script { segments.join("\n") } else { join_with_and(&segments) }
}

/// A script of several lines as the one line `cmd.exe /c` runs: the lines joined with ` && ` (not after an operator that already joins).
pub fn flatten_script(command: &str) -> String {
    let lines: Vec<String> = command.lines().map(|line| line.trim().to_string()).filter(|line| !line.is_empty()).collect();
    join_with_and(&lines)
}

fn join_with_and(segments: &[String]) -> String {
    let mut combined = String::new();
    for segment in segments {
        let piece = segment.trim();
        if piece.is_empty() {
            continue;
        }
        if combined.is_empty() {
            combined.push_str(piece);
            continue;
        }
        let prev = combined.trim_end();
        if prev.ends_with("&&") || prev.ends_with("||") || prev.ends_with(';') || prev.ends_with('|') {
            combined.push(' ');
            combined.push_str(piece);
        } else {
            combined.push_str(" && ");
            combined.push_str(piece);
        }
    }
    combined
}

/// Environment names a repository's `worktree.env` may not set: they change which program, library, shell startup file or git
/// configuration a later command runs (or where it looks), so the value would be code the person approved without reading it.
pub fn is_dangerous_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const EXACT: [&str; 36] = [
        "PATH",
        "PATHEXT",
        "HOME",
        "USERPROFILE",
        "SHELL",
        "COMSPEC",
        "ENV",
        "BASH_ENV",
        "PROMPT_COMMAND",
        "IFS",
        "CDPATH",
        "PS4",
        "NODE_OPTIONS",
        "NODE_PATH",
        "PYTHONSTARTUP",
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONUSERBASE",
        "RUBYOPT",
        "RUBYLIB",
        "PERL5OPT",
        "PERL5LIB",
        "JAVA_TOOL_OPTIONS",
        "_JAVA_OPTIONS",
        "JDK_JAVA_OPTIONS",
        "CLASSPATH",
        "RUSTC",
        "RUSTFLAGS",
        "RUSTDOC",
        "EDITOR",
        "VISUAL",
        "PAGER",
        "BROWSER",
        "SSH_ASKPASS",
        "GOFLAGS",
        "GOROOT",
    ];
    EXACT.contains(&upper.as_str())
        || ["LD_", "DYLD_", "GIT_", "NPM_CONFIG_", "CARGO_", "RUSTC_", "BUNDLE_", "GEM_"].iter().any(|prefix| upper.starts_with(prefix))
}

fn is_valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else { return false };
    if name.len() > 128 || !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn decode_env_map(value: &YamlValue) -> Result<IndexMap<String, String>, String> {
    let map = match value {
        YamlValue::Null => return Ok(IndexMap::new()),
        YamlValue::Map(map) => map,
        _ => return Err("'worktree.env' must be a mapping of environment variable names to values.".into()),
    };
    if map.len() > 128 {
        return Err("'worktree.env' exceeds maximum of 128 variables.".into());
    }
    let mut env = IndexMap::new();
    for (key, item) in map {
        if !is_valid_env_name(key) {
            return Err(format!("Invalid environment variable name '{key}' in 'worktree.env'."));
        }
        if is_dangerous_env_name(key) {
            return Err(format!(
                "Environment variable '{key}' in 'worktree.env' is not allowed: it changes which programs a command runs."
            ));
        }
        if RESERVED_ENV_NAMES.contains(&key.to_ascii_uppercase().as_str()) {
            return Err(format!("Environment variable '{key}' in 'worktree.env' is set by the app for each worktree."));
        }
        let val = match item {
            YamlValue::Null => String::new(),
            YamlValue::Scalar(s) => {
                if s.len() > 4096 {
                    return Err(format!("Environment variable '{key}' exceeds 4096 bytes."));
                }
                if has_invisible_format_chars(s) {
                    return Err(format!("Environment variable '{key}' contains an invisible formatting character."));
                }
                s.clone()
            }
            _ => return Err(format!("Environment variable '{key}' in 'worktree.env' must be a scalar value.")),
        };
        env.insert(key.clone(), val);
    }
    Ok(env)
}

fn is_valid_task_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed == name
        && trimmed.chars().count() <= 80
        && !trimmed.chars().any(|c| c.is_control() || is_invisible_format_char(c))
}

fn decode_tasks(value: &YamlValue) -> Result<IndexMap<String, RepoTaskSpec>, String> {
    let map = match value {
        YamlValue::Null => return Ok(IndexMap::new()),
        YamlValue::Map(map) => map,
        _ => return Err("'tasks' must be a mapping of task names to commands.".into()),
    };
    if map.len() > MAXIMUM_TASKS {
        return Err(format!("'tasks' exceeds maximum of {MAXIMUM_TASKS} entries."));
    }
    let mut tasks = IndexMap::new();
    for (name, spec_val) in map {
        if !is_valid_task_name(name) {
            return Err(format!("Invalid task name '{name}'."));
        }
        let field = format!("tasks.{name}");
        let spec = match spec_val {
            YamlValue::Scalar(_) | YamlValue::Seq(_) => {
                let command = decode_command_value(spec_val, &field, false)?.ok_or_else(|| format!("'{field}' cannot be empty."))?;
                RepoTaskSpec { command, description: None, cwd: None }
            }
            YamlValue::Map(spec_map) => {
                let mut command = None;
                let mut description = None;
                let mut cwd = None;
                for (k, v) in spec_map {
                    match k.as_str() {
                        "command" | "run" | "cmd" => {
                            if command.is_some() {
                                return Err(format!("Duplicate command key in '{field}'."));
                            }
                            command = Some(
                                decode_command_value(v, &format!("{field}.{k}"), false)?
                                    .ok_or_else(|| format!("'{field}.{k}' cannot be empty."))?,
                            );
                        }
                        "description" | "label" | "summary" => {
                            if description.is_some() {
                                return Err(format!("Duplicate description key in '{field}'."));
                            }
                            let text = expect_scalar(v, &format!("{field}.{k}"))?;
                            let cleaned: String =
                                text.trim().chars().filter(|c| !c.is_control() && !is_invisible_format_char(*c)).take(240).collect();
                            if !cleaned.is_empty() {
                                description = Some(cleaned);
                            }
                        }
                        "cwd" => {
                            let raw = expect_scalar(v, &format!("{field}.cwd"))?;
                            let trimmed = raw.trim();
                            if !trimmed.is_empty() && trimmed != "." {
                                cwd = Some(normalize_single_rel_path(trimmed, &format!("{field}.cwd"))?);
                            }
                        }
                        other => return Err(format!("Unknown key '{other}' in '{field}'.")),
                    }
                }
                let Some(command) = command else {
                    return Err(format!("Task '{name}' must define 'command' (or 'run')."));
                };
                RepoTaskSpec { command, description, cwd }
            }
            YamlValue::Null => return Err(format!("Task '{name}' cannot be empty.")),
        };
        tasks.insert(name.clone(), spec);
    }
    Ok(tasks)
}

fn expect_scalar(value: &YamlValue, field: &str) -> Result<String, String> {
    match value {
        YamlValue::Scalar(s) => Ok(s.clone()),
        _ => Err(format!("Expected a scalar value for '{field}'.")),
    }
}

// ── Overlay merging & file loading ──────────────────────────────────────────────────────────────────────────────────────

/// Merges a `.mighty.local.yaml` config over a base `.mighty.yaml` config.
pub fn merge_configs(base: &MightyRepoConfig, local: &MightyRepoConfig) -> MightyRepoConfig {
    merge_layers(base, local, local.worktree.setup.is_some(), local.worktree.teardown.is_some())
}

fn merge_layers(
    base: &MightyRepoConfig,
    local: &MightyRepoConfig,
    local_setup_specified: bool,
    local_teardown_specified: bool,
) -> MightyRepoConfig {
    let mut share = base.worktree.share.clone();
    for entry in &local.worktree.share {
        if !share.contains(entry) && share.len() < MAXIMUM_WORKTREE_RULES {
            share.push(entry.clone());
        }
    }

    let mut copy = base.worktree.copy.clone();
    for rule in &local.worktree.copy {
        if copy.iter().any(|r| r.to == rule.to) || copy.len() < MAXIMUM_WORKTREE_RULES {
            upsert_copy_rule(&mut copy, rule.clone());
        }
    }

    let setup = if local_setup_specified { local.worktree.setup.clone() } else { base.worktree.setup.clone() };

    let mut env = base.worktree.env.clone();
    for (k, v) in &local.worktree.env {
        env.insert(k.clone(), v.clone());
    }

    let mut tasks = base.tasks.clone();
    for (k, v) in &local.tasks {
        tasks.insert(k.clone(), v.clone());
    }

    let teardown = if local_teardown_specified { local.worktree.teardown.clone() } else { base.worktree.teardown.clone() };
    let teardown_timeout_seconds = local.worktree.teardown_timeout_seconds.or(base.worktree.teardown_timeout_seconds);
    let ports = local.worktree.ports.or(base.worktree.ports);

    let mut run = base.run.clone();
    for (k, v) in &local.run {
        if run.contains_key(k) || run.len() < MAXIMUM_RUN_ACTIONS {
            run.insert(k.clone(), v.clone());
        }
    }

    // A local schedule or trigger of the same name replaces the shared one.
    let mut schedules = base.schedules.clone();
    for (k, v) in &local.schedules {
        if schedules.contains_key(k) || schedules.len() < automation::MAX_SCHEDULES {
            schedules.insert(k.clone(), v.clone());
        }
    }
    let mut triggers = base.triggers.clone();
    for (k, v) in &local.triggers {
        if triggers.contains_key(k) || triggers.len() < automation::MAX_TRIGGERS {
            triggers.insert(k.clone(), v.clone());
        }
    }

    // A local verify command of the same name replaces the shared one; the other settings are the local file's when it names them.
    let mut verify_commands = base.verify.commands.clone();
    for (k, v) in &local.verify.commands {
        if verify_commands.contains_key(k) || verify_commands.len() < MAXIMUM_VERIFY_COMMANDS {
            verify_commands.insert(k.clone(), v.clone());
        }
    }
    let verify = VerifyConfig {
        commands: verify_commands,
        max_rounds: local.verify.max_rounds.or(base.verify.max_rounds),
        when: local.verify.when.or(base.verify.when),
        browser: local.verify.browser.clone().or_else(|| base.verify.browser.clone()),
    };

    MightyRepoConfig {
        version: local.version.max(base.version),
        worktree: WorktreeConfig { share, copy, setup, env, teardown, teardown_timeout_seconds, ports },
        tasks,
        run,
        // Personal only: the shared file's are refused as it loads.
        hooks: local.hooks.clone(),
        schedules,
        triggers,
        verify,
    }
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

fn read_bounded_config_file(folder: &Path, candidates: &[&str]) -> Result<Option<(String, Vec<u8>)>, String> {
    for &name in candidates {
        let path = folder.join(name);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("Cannot inspect {name}: {err}")),
        };
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(format!("{name} must be a regular file, not a symbolic link."));
        }
        if !meta.is_file() {
            return Err(format!("{name} must be a regular file."));
        }
        if meta.len() > MAXIMUM_CONFIG_BYTES as u64 {
            return Err(format!("{name} exceeds maximum size of {MAXIMUM_CONFIG_BYTES} bytes."));
        }
        let file = std::fs::File::open(&path).map_err(|err| format!("Cannot open {name}: {err}"))?;
        let mut bytes = Vec::with_capacity((meta.len() as usize).min(MAXIMUM_CONFIG_BYTES));
        file.take((MAXIMUM_CONFIG_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|err| format!("Cannot read {name}: {err}"))?;
        if bytes.len() > MAXIMUM_CONFIG_BYTES {
            return Err(format!("{name} exceeds maximum size of {MAXIMUM_CONFIG_BYTES} bytes."));
        }
        return Ok(Some((name.to_string(), bytes)));
    }
    Ok(None)
}

/// Computes the canonical 64-character hex SHA-256 hash over the raw file bytes and normalized executable config.
pub fn compute_content_hash(raw_files: &[(String, Vec<u8>)], merged: &MightyRepoConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mighty-repo-config-v1\0");
    for (name, bytes) in raw_files {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        hasher.update(b"\0");
    }
    let canonical = serde_json::to_vec(merged).unwrap_or_default();
    hasher.update((canonical.len() as u64).to_le_bytes());
    hasher.update(&canonical);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Loads `.mighty.yaml` (or `.mighty.yml`) and optional `.mighty.local.yaml` (or `.mighty.local.yml`) from `folder`.
pub fn load_from_folder(folder: &Path) -> Result<Option<LoadedRepoConfig>, String> {
    load_from_folder_with_local_fallback(folder, folder)
}

/// Loads `.mighty.yaml` from `target_folder` (falling back to `workspace_folder` if absent in `target_folder`) and
/// `.mighty.local.yaml` from `target_folder` (falling back to `workspace_folder` so gitignored local overlays in the main
/// workspace apply seamlessly inside linked worktrees while keeping identical `content_hash`).
pub fn load_from_folder_with_local_fallback(target_folder: &Path, workspace_folder: &Path) -> Result<Option<LoadedRepoConfig>, String> {
    let base_raw = match read_bounded_config_file(target_folder, &BASE_CONFIG_FILES)? {
        Some(found) => Some(found),
        None if target_folder != workspace_folder => read_bounded_config_file(workspace_folder, &BASE_CONFIG_FILES)?,
        None => None,
    };
    let local_raw = match read_bounded_config_file(target_folder, &LOCAL_CONFIG_FILES)? {
        Some(found) => Some(found),
        None if target_folder != workspace_folder => read_bounded_config_file(workspace_folder, &LOCAL_CONFIG_FILES)?,
        None => None,
    };

    load_from_bytes(base_raw, local_raw)
}

/// The config of the files already read: `base` (`.mighty.yaml`) and `local` (`.mighty.local.yaml`), each its name and bytes (MC-47:
/// an SSH workspace's, read on its machine with the same rules). None when there is neither.
pub fn load_from_bytes(
    base_raw: Option<(String, Vec<u8>)>,
    local_raw: Option<(String, Vec<u8>)>,
) -> Result<Option<LoadedRepoConfig>, String> {
    if base_raw.is_none() && local_raw.is_none() {
        return Ok(None);
    }

    let mut raw_files: Vec<(String, Vec<u8>)> = Vec::new();
    let base_layer = match base_raw {
        Some((name, bytes)) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| format!("{name} is not valid UTF-8."))?;
            let parsed = parse_layer(text).map_err(|err| format!("{name}: {err}"))?;
            if !parsed.config.hooks.is_empty() {
                return Err(format!("{name}: 'hooks' runs a command on this computer; put it in .mighty.local.yaml."));
            }
            raw_files.push((name, bytes));
            parsed
        }
        None => ParsedLayer { config: MightyRepoConfig::default(), setup_specified: false, teardown_specified: false },
    };

    let merged = match local_raw {
        Some((name, bytes)) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| format!("{name} is not valid UTF-8."))?;
            let local_layer = parse_layer(text).map_err(|err| format!("{name}: {err}"))?;
            raw_files.push((name, bytes));
            merge_layers(&base_layer.config, &local_layer.config, local_layer.setup_specified, local_layer.teardown_specified)
        }
        None => base_layer.config,
    };

    if let Some(action) = merged.verify.browser.as_ref().and_then(|b| b.run_action.as_deref())
        && !merged.run.contains_key(action)
    {
        return Err(format!("'verify.browser.runAction' names '{action}', which is not under 'run'."));
    }

    let content_hash = compute_content_hash(&raw_files, &merged);
    let files = raw_files.into_iter().map(|(name, _)| name).collect();
    Ok(Some(LoadedRepoConfig { config: merged, files, content_hash }))
}

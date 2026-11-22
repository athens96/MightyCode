//! Repository shared configuration (`.mighty.yaml` and `.mighty.local.yaml`, `MC-8`): pure-Rust strict YAML subset parser,
//! normalized schema (`MightyRepoConfig`), local overlay merging, and canonical SHA-256 content hashing for trust verification.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::Path;

use indexmap::IndexMap;
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

/// Worktree cache sharing, file copy, setup command, and environment rules from `.mighty.yaml`.
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
}

impl Default for MightyRepoConfig {
    fn default() -> Self {
        Self { version: 1, worktree: WorktreeConfig::default(), tasks: IndexMap::new() }
    }
}

impl MightyRepoConfig {
    /// Whether this configuration defines any worktree creation actions (`share`, `copy`, `setup`, or `env`).
    pub fn has_worktree_actions(&self) -> bool {
        !self.worktree.share.is_empty() || !self.worktree.copy.is_empty() || self.worktree.setup.is_some() || !self.worktree.env.is_empty()
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

    /// Whether this configuration defines any worktree creation actions (`share`, `copy`, `setup`, or `env`).
    pub fn has_worktree_actions(&self) -> bool {
        self.config.has_worktree_actions()
    }
}

#[derive(Debug, Clone)]
struct ParsedLayer {
    config: MightyRepoConfig,
    setup_specified: bool,
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
    let mut tasks = IndexMap::new();
    let mut saw_tasks = false;

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
                let (wt, has_setup) = decode_worktree(&value)?;
                worktree = wt;
                setup_specified = has_setup;
            }
            "tasks" | "scripts" => {
                if saw_tasks {
                    return Err("Specify only one of 'tasks' or 'scripts', not both.".into());
                }
                saw_tasks = true;
                tasks = decode_tasks(&value)?;
            }
            other => return Err(format!("Unknown top-level key '{other}' in .mighty.yaml.")),
        }
    }

    Ok(ParsedLayer { config: MightyRepoConfig { version, worktree, tasks }, setup_specified })
}

fn decode_worktree(value: &YamlValue) -> Result<(WorktreeConfig, bool), String> {
    let map = match value {
        YamlValue::Null => return Ok((WorktreeConfig::default(), false)),
        YamlValue::Map(map) => map,
        _ => return Err("'worktree' must be a mapping.".into()),
    };

    let mut share = Vec::new();
    let mut saw_share = false;
    let mut copy = Vec::new();
    let mut setup = None;
    let mut setup_specified = false;
    let mut env = IndexMap::new();

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
            other => return Err(format!("Unknown key 'worktree.{other}' in .mighty.yaml.")),
        }
    }

    Ok((WorktreeConfig { share, copy, setup, env }, setup_specified))
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
    merge_layers(base, local, local.worktree.setup.is_some())
}

fn merge_layers(base: &MightyRepoConfig, local: &MightyRepoConfig, local_setup_specified: bool) -> MightyRepoConfig {
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

    MightyRepoConfig { version: local.version.max(base.version), worktree: WorktreeConfig { share, copy, setup, env }, tasks }
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

    if base_raw.is_none() && local_raw.is_none() {
        return Ok(None);
    }

    let mut raw_files: Vec<(String, Vec<u8>)> = Vec::new();
    let base_layer = match base_raw {
        Some((name, bytes)) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| format!("{name} is not valid UTF-8."))?;
            let parsed = parse_layer(text).map_err(|err| format!("{name}: {err}"))?;
            raw_files.push((name, bytes));
            parsed
        }
        None => ParsedLayer { config: MightyRepoConfig::default(), setup_specified: false },
    };

    let merged = match local_raw {
        Some((name, bytes)) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| format!("{name} is not valid UTF-8."))?;
            let local_layer = parse_layer(text).map_err(|err| format!("{name}: {err}"))?;
            raw_files.push((name, bytes));
            merge_layers(&base_layer.config, &local_layer.config, local_layer.setup_specified)
        }
        None => base_layer.config,
    };

    let content_hash = compute_content_hash(&raw_files, &merged);
    let files = raw_files.into_iter().map(|(name, _)| name).collect();
    Ok(Some(LoadedRepoConfig { config: merged, files, content_hash }))
}

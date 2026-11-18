//! The manifest's records (C# `Core\Styles\StyleManifest.cs`: `StyleManifestException`, `StyleText`, `StyleAction`, `StylePhase`,
//! `StyleGroup`, `StyleManifest`, `StyleCapabilityID`; `StyleEmoji.cs`). The decoded manifest keeps its JSON root as the document
//! wrote it (serde_json's `preserve_order`: C#'s `JsonElement` keeps document order too) and reads the rules from it, as C# does.
//! Owner: W9-D.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub use mighty_graph::structure::StylePhase;

/// C# `StyleManifestException(code, detail)`: the code a manifest is refused with and its message, `code` then `": "` and the detail
/// made safe ([`safe`]) when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleManifestError {
    pub code: String,
    pub message: String,
}

impl StyleManifestError {
    pub fn new(code: &str, detail: &str) -> Self {
        let message = if detail.is_empty() { code.to_string() } else { format!("{code}: {}", safe(detail, 64)) };
        StyleManifestError { code: code.to_string(), message }
    }
}

impl fmt::Display for StyleManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StyleManifestError {}

/// `StyleText.Banned`: C0 and C1 controls, the soft hyphen, the Arabic letter mark, word joiner, BOM, zero-width and direction marks.
pub fn banned(c: u32) -> bool {
    c <= 31
        || (127..=159).contains(&c)
        || matches!(c, 0xad | 0x61c | 0x2060 | 0xfeff)
        || (0x200b..=0x200f).contains(&c)
        || (0x2028..=0x202e).contains(&c)
        || (0x2066..=0x2069).contains(&c)
}

/// `StyleText.Length`: text elements (`StringInfo.LengthInTextElements`, extended grapheme clusters).
pub fn length(value: &str) -> usize {
    value.graphemes(true).count()
}

/// `StyleText.Safe`: banned characters shown as U+FFFD, then cut to `limit - 1` text elements and an ellipsis when longer than `limit`.
pub fn safe(value: &str, limit: usize) -> String {
    let clean: String = value.chars().map(|c| if banned(c as u32) { '\u{fffd}' } else { c }).collect();
    if length(&clean) <= limit {
        return clean;
    }
    let mut cut: String = clean.graphemes(true).take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// `StyleText.Folded`: NFKC, lower case, every white space removed — how a reserved name is compared.
pub fn folded(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).collect()
}

/// .NET's `StringComparer.Ordinal`: by UTF-16 code unit.
pub fn ordinal(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// One action (C# `StyleAction`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleAction {
    pub id: String,
    pub title: String,
    pub help: String,
    pub prompt_template: String,
    pub takes_text: bool,
    pub requires_text: bool,
    pub fold_text: Option<String>,
    pub r#match: Option<String>,
    pub phase: Option<String>,
    pub glyph: Option<String>,
    pub icon: Option<String>,
    pub tint: Option<String>,
    pub request_title: Option<String>,
    pub scope: Option<String>,
    pub flags: Vec<String>,
}

impl StyleAction {
    /// `StyleAction.Prompt(text)`: the template with `{text}` filled (folded to one line for `oneLine`, trimmed otherwise); an empty
    /// text takes the placeholder out with the spaces before it.
    pub fn prompt(&self, text: &str) -> String {
        let folded = if self.fold_text.as_deref() == Some("oneLine") {
            text.split(['\r', '\n']).map(trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
        } else {
            trim(text).to_string()
        };
        let Some(at) = self.prompt_template.find("{text}") else {
            return self.prompt_template.clone();
        };
        if !folded.is_empty() {
            return self.prompt_template.replace("{text}", &folded);
        }
        let start = self.prompt_template[..at].trim_end_matches(' ').len();
        format!("{}{}", &self.prompt_template[..start], &self.prompt_template[at + 6..])
    }
}

/// .NET's `string.Trim()`: every white space at both ends.
pub fn trim(value: &str) -> &str {
    value.trim_matches(char::is_whitespace)
}

/// One group (C# `StyleGroup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleGroup {
    pub id: String,
    pub title: String,
    pub axis: Option<String>,
    pub question: Option<String>,
    pub actions: Vec<String>,
}

/// A decoded manifest (C# `StyleManifest`): the JSON root as read, and its actions, phases and groups.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleManifest {
    pub root: Value,
    pub actions: Vec<StyleAction>,
    pub phases: Vec<StylePhase>,
    pub groups: Vec<StyleGroup>,
}

static NULL: Value = Value::Null;

impl StyleManifest {
    /// Built from a root the decoder has checked (C#'s internal constructor).
    pub(crate) fn new(root: Value) -> Self {
        let actions = items(&root, "actions")
            .iter()
            .map(|a| StyleAction {
                id: text(a, "id").unwrap_or_default().to_string(),
                title: text(a, "title").unwrap_or_default().to_string(),
                help: text(a, "help").unwrap_or_default().to_string(),
                prompt_template: text(a, "prompt").unwrap_or_default().to_string(),
                takes_text: boolean(a, "takesText"),
                requires_text: boolean(a, "requiresText"),
                fold_text: owned(a, "foldText"),
                r#match: owned(a, "match"),
                phase: owned(a, "phase"),
                glyph: owned(a, "glyph"),
                icon: owned(a, "icon"),
                tint: owned(a, "tint"),
                request_title: owned(a, "requestTitle"),
                scope: owned(a, "scope"),
                flags: strings(a, "flags"),
            })
            .collect();
        let phases = items(&root, "phases")
            .iter()
            .map(|p| StylePhase {
                id: text(p, "id").unwrap_or_default().to_string(),
                title: text(p, "title").unwrap_or_default().to_string(),
                order: p.get("order").and_then(Value::as_f64).unwrap_or(0.0) as i32,
            })
            .collect();
        let groups = items(&root, "groups")
            .iter()
            .map(|g| StyleGroup {
                id: text(g, "id").unwrap_or_default().to_string(),
                title: text(g, "title").unwrap_or_default().to_string(),
                axis: owned(g, "axis"),
                question: owned(g, "question"),
                actions: strings(g, "actions"),
            })
            .collect();
        StyleManifest { root, actions, phases, groups }
    }

    pub fn id(&self) -> &str {
        text(&self.root, "id").unwrap_or_default()
    }
    pub fn name(&self) -> &str {
        text(&self.root, "name").unwrap_or_default()
    }
    pub fn summary(&self) -> &str {
        text(&self.root, "summary").unwrap_or_default()
    }
    pub fn subtitle(&self) -> &str {
        text(&self.root, "subtitle").unwrap_or_default()
    }
    /// `Rules`.
    pub fn rules(&self) -> &Value {
        self.get("rules")
    }
    /// One rule (`Rules.GetProperty(name)`).
    pub fn rule(&self, name: &str) -> &Value {
        self.rules().get(name).unwrap_or(&NULL)
    }
    /// A top-level field, or null when absent (`Root.TryGetProperty`).
    pub fn get(&self, name: &str) -> &Value {
        self.root.get(name).unwrap_or(&NULL)
    }
    /// `Root.TryGetProperty(name, out _)`.
    pub fn has(&self, name: &str) -> bool {
        self.root.get(name).is_some()
    }
    pub fn action(&self, id: &str) -> Option<&StyleAction> {
        self.actions.iter().find(|a| a.id == id)
    }
    pub fn phase(&self, id: &str) -> Option<&StylePhase> {
        self.phases.iter().find(|p| p.id == id)
    }
    /// Each `autoAllow` entry as its wire name: `mcp__<server>__<tool>`, or the bare tool.
    pub fn auto_allow(&self) -> Vec<String> {
        items(&self.root, "autoAllow")
            .iter()
            .map(|e| match text(e, "server") {
                Some(server) => format!("mcp__{server}__{}", text(e, "tool").unwrap_or_default()),
                None => text(e, "tool").unwrap_or_default().to_string(),
            })
            .collect()
    }
}

/// `StyleManifest.Text(e, name)`: a string field of an object, else None.
pub fn text<'a>(e: &'a Value, name: &str) -> Option<&'a str> {
    e.as_object().and_then(|o| o.get(name)).and_then(Value::as_str)
}

fn owned(e: &Value, name: &str) -> Option<String> {
    text(e, name).map(str::to_string)
}

/// `StyleManifest.Bool(e, name)`: the field is `true`.
pub fn boolean(e: &Value, name: &str) -> bool {
    e.as_object().and_then(|o| o.get(name)) == Some(&Value::Bool(true))
}

/// `StyleManifest.Items(e, name)`: an array field of an object, else none.
pub fn items<'a>(e: &'a Value, name: &str) -> &'a [Value] {
    e.as_object().and_then(|o| o.get(name)).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// `StyleManifest.Strings(e, name)`: the strings of an array field.
pub fn strings(e: &Value, name: &str) -> Vec<String> {
    items(e, name).iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect()
}

/// `StyleCapabilityID.Vocabulary`: the app's built-in features and their closed states — empty since v7 (§1.8), so a declared name is
/// refused as unknown; the tests decode against a vocabulary of their own.
pub type Vocabulary = BTreeMap<String, Vec<String>>;

/// The app's own vocabulary (empty).
pub fn app_vocabulary() -> Vocabulary {
    Vocabulary::new()
}

/// `StyleEmoji` (generated from the macOS reference): a glyph is one grapheme with Emoji on its first scalar and Presentation or VS16.
pub mod emoji {
    const EMOJI: &[(u32, u32)] = &[
        (0x23, 0x23),
        (0x2a, 0x2a),
        (0x30, 0x39),
        (0xa9, 0xa9),
        (0xae, 0xae),
        (0x203c, 0x203c),
        (0x2049, 0x2049),
        (0x2122, 0x2122),
        (0x2139, 0x2139),
        (0x2194, 0x2199),
        (0x21a9, 0x21aa),
        (0x231a, 0x231b),
        (0x2328, 0x2328),
        (0x23cf, 0x23cf),
        (0x23e9, 0x23f3),
        (0x23f8, 0x23fa),
        (0x24c2, 0x24c2),
        (0x25aa, 0x25ab),
        (0x25b6, 0x25b6),
        (0x25c0, 0x25c0),
        (0x25fb, 0x25fe),
        (0x2600, 0x2604),
        (0x260e, 0x260e),
        (0x2611, 0x2611),
        (0x2614, 0x2615),
        (0x2618, 0x2618),
        (0x261d, 0x261d),
        (0x2620, 0x2620),
        (0x2622, 0x2623),
        (0x2626, 0x2626),
        (0x262a, 0x262a),
        (0x262e, 0x262f),
        (0x2638, 0x263a),
        (0x2640, 0x2640),
        (0x2642, 0x2642),
        (0x2648, 0x2653),
        (0x265f, 0x2660),
        (0x2663, 0x2663),
        (0x2665, 0x2666),
        (0x2668, 0x2668),
        (0x267b, 0x267b),
        (0x267e, 0x267f),
        (0x2692, 0x2697),
        (0x2699, 0x2699),
        (0x269b, 0x269c),
        (0x26a0, 0x26a1),
        (0x26a7, 0x26a7),
        (0x26aa, 0x26ab),
        (0x26b0, 0x26b1),
        (0x26bd, 0x26be),
        (0x26c4, 0x26c5),
        (0x26c8, 0x26c8),
        (0x26ce, 0x26cf),
        (0x26d1, 0x26d1),
        (0x26d3, 0x26d4),
        (0x26e9, 0x26ea),
        (0x26f0, 0x26f5),
        (0x26f7, 0x26fa),
        (0x26fd, 0x26fd),
        (0x2702, 0x2702),
        (0x2705, 0x2705),
        (0x2708, 0x270d),
        (0x270f, 0x270f),
        (0x2712, 0x2712),
        (0x2714, 0x2714),
        (0x2716, 0x2716),
        (0x271d, 0x271d),
        (0x2721, 0x2721),
        (0x2728, 0x2728),
        (0x2733, 0x2734),
        (0x2744, 0x2744),
        (0x2747, 0x2747),
        (0x274c, 0x274c),
        (0x274e, 0x274e),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2763, 0x2764),
        (0x2795, 0x2797),
        (0x27a1, 0x27a1),
        (0x27b0, 0x27b0),
        (0x27bf, 0x27bf),
        (0x2934, 0x2935),
        (0x2b05, 0x2b07),
        (0x2b1b, 0x2b1c),
        (0x2b50, 0x2b50),
        (0x2b55, 0x2b55),
        (0x3030, 0x3030),
        (0x303d, 0x303d),
        (0x3297, 0x3297),
        (0x3299, 0x3299),
        (0x1f004, 0x1f004),
        (0x1f0cf, 0x1f0cf),
        (0x1f170, 0x1f171),
        (0x1f17e, 0x1f17f),
        (0x1f18e, 0x1f18e),
        (0x1f191, 0x1f19a),
        (0x1f1e6, 0x1f1ff),
        (0x1f201, 0x1f202),
        (0x1f21a, 0x1f21a),
        (0x1f22f, 0x1f22f),
        (0x1f232, 0x1f23a),
        (0x1f250, 0x1f251),
        (0x1f300, 0x1f321),
        (0x1f324, 0x1f393),
        (0x1f396, 0x1f397),
        (0x1f399, 0x1f39b),
        (0x1f39e, 0x1f3f0),
        (0x1f3f3, 0x1f3f5),
        (0x1f3f7, 0x1f4fd),
        (0x1f4ff, 0x1f53d),
        (0x1f549, 0x1f54e),
        (0x1f550, 0x1f567),
        (0x1f56f, 0x1f570),
        (0x1f573, 0x1f57a),
        (0x1f587, 0x1f587),
        (0x1f58a, 0x1f58d),
        (0x1f590, 0x1f590),
        (0x1f595, 0x1f596),
        (0x1f5a4, 0x1f5a5),
        (0x1f5a8, 0x1f5a8),
        (0x1f5b1, 0x1f5b2),
        (0x1f5bc, 0x1f5bc),
        (0x1f5c2, 0x1f5c4),
        (0x1f5d1, 0x1f5d3),
        (0x1f5dc, 0x1f5de),
        (0x1f5e1, 0x1f5e1),
        (0x1f5e3, 0x1f5e3),
        (0x1f5e8, 0x1f5e8),
        (0x1f5ef, 0x1f5ef),
        (0x1f5f3, 0x1f5f3),
        (0x1f5fa, 0x1f64f),
        (0x1f680, 0x1f6c5),
        (0x1f6cb, 0x1f6d2),
        (0x1f6d5, 0x1f6d8),
        (0x1f6dc, 0x1f6e5),
        (0x1f6e9, 0x1f6e9),
        (0x1f6eb, 0x1f6ec),
        (0x1f6f0, 0x1f6f0),
        (0x1f6f3, 0x1f6fc),
        (0x1f7e0, 0x1f7eb),
        (0x1f7f0, 0x1f7f0),
        (0x1f90c, 0x1f93a),
        (0x1f93c, 0x1f945),
        (0x1f947, 0x1f9ff),
        (0x1fa70, 0x1fa7c),
        (0x1fa80, 0x1fa8a),
        (0x1fa8e, 0x1fac6),
        (0x1fac8, 0x1fac8),
        (0x1facd, 0x1fadc),
        (0x1fadf, 0x1faea),
        (0x1faef, 0x1faf8),
    ];
    const PRESENTATION: &[(u32, u32)] = &[
        (0x231a, 0x231b),
        (0x23e9, 0x23ec),
        (0x23f0, 0x23f0),
        (0x23f3, 0x23f3),
        (0x25fd, 0x25fe),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267f, 0x267f),
        (0x2693, 0x2693),
        (0x26a1, 0x26a1),
        (0x26aa, 0x26ab),
        (0x26bd, 0x26be),
        (0x26c4, 0x26c5),
        (0x26ce, 0x26ce),
        (0x26d4, 0x26d4),
        (0x26ea, 0x26ea),
        (0x26f2, 0x26f3),
        (0x26f5, 0x26f5),
        (0x26fa, 0x26fa),
        (0x26fd, 0x26fd),
        (0x2705, 0x2705),
        (0x270a, 0x270b),
        (0x2728, 0x2728),
        (0x274c, 0x274c),
        (0x274e, 0x274e),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27b0, 0x27b0),
        (0x27bf, 0x27bf),
        (0x2b1b, 0x2b1c),
        (0x2b50, 0x2b50),
        (0x2b55, 0x2b55),
        (0x1f004, 0x1f004),
        (0x1f0cf, 0x1f0cf),
        (0x1f18e, 0x1f18e),
        (0x1f191, 0x1f19a),
        (0x1f1e6, 0x1f1ff),
        (0x1f201, 0x1f201),
        (0x1f21a, 0x1f21a),
        (0x1f22f, 0x1f22f),
        (0x1f232, 0x1f236),
        (0x1f238, 0x1f23a),
        (0x1f250, 0x1f251),
        (0x1f300, 0x1f320),
        (0x1f32d, 0x1f335),
        (0x1f337, 0x1f37c),
        (0x1f37e, 0x1f393),
        (0x1f3a0, 0x1f3ca),
        (0x1f3cf, 0x1f3d3),
        (0x1f3e0, 0x1f3f0),
        (0x1f3f4, 0x1f3f4),
        (0x1f3f8, 0x1f43e),
        (0x1f440, 0x1f440),
        (0x1f442, 0x1f4fc),
        (0x1f4ff, 0x1f53d),
        (0x1f54b, 0x1f54e),
        (0x1f550, 0x1f567),
        (0x1f57a, 0x1f57a),
        (0x1f595, 0x1f596),
        (0x1f5a4, 0x1f5a4),
        (0x1f5fb, 0x1f64f),
        (0x1f680, 0x1f6c5),
        (0x1f6cc, 0x1f6cc),
        (0x1f6d0, 0x1f6d2),
        (0x1f6d5, 0x1f6d8),
        (0x1f6dc, 0x1f6df),
        (0x1f6eb, 0x1f6ec),
        (0x1f6f4, 0x1f6fc),
        (0x1f7e0, 0x1f7eb),
        (0x1f7f0, 0x1f7f0),
        (0x1f90c, 0x1f93a),
        (0x1f93c, 0x1f945),
        (0x1f947, 0x1f9ff),
        (0x1fa70, 0x1fa7c),
        (0x1fa80, 0x1fa8a),
        (0x1fa8e, 0x1fac6),
        (0x1fac8, 0x1fac8),
        (0x1facd, 0x1fadc),
        (0x1fadf, 0x1faea),
        (0x1faef, 0x1faf8),
    ];

    fn within(ranges: &[(u32, u32)], c: u32) -> bool {
        ranges.iter().any(|&(first, last)| c >= first && c <= last)
    }

    /// `StyleEmoji.Valid`.
    pub fn valid(value: &str) -> bool {
        if super::length(value) != 1 {
            return false;
        }
        let Some(first) = value.chars().next().map(|c| c as u32) else {
            return false;
        };
        within(EMOJI, first) && (within(PRESENTATION, first) || value.chars().any(|c| c == '\u{fe0f}'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_cuts_by_text_element_and_marks_banned_characters() {
        assert_eq!(safe("a\u{202e}b", 64), "a\u{fffd}b");
        assert_eq!(safe("abcdef", 4), "abc…");
        assert_eq!(folded("ＯＵＲＯ BOROS"), "ouroboros");
    }

    #[test]
    fn a_prompt_folds_its_text() {
        let action = StyleAction {
            id: "x".into(),
            title: "x".into(),
            help: String::new(),
            prompt_template: "/x {text}".into(),
            takes_text: true,
            requires_text: false,
            fold_text: Some("oneLine".into()),
            r#match: None,
            phase: None,
            glyph: None,
            icon: None,
            tint: None,
            request_title: None,
            scope: None,
            flags: Vec::new(),
        };
        assert_eq!(action.prompt("  alpha  beta\n gamma "), "/x alpha  beta gamma");
        assert_eq!(action.prompt(""), "/x");
    }
}

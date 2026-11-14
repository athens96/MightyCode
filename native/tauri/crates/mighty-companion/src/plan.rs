//! The whole plan drawn as blocks (C# Core\CompanionBubble.cs `PlanBlocks`, `CompanionPlanBlock`, `CompanionPlanSpan`; the Mac's
//! AgentMarkdownView.swift compact; plan §1.7): headings, paragraphs, list items behind their bullets, numbers or check boxes, quotes,
//! code and rules, with bold, italic and code runs inside a line. A table keeps its lines as written. Nothing is fetched or opened: a
//! link is its words. A source past 128 KiB is one literal paragraph; nothing is ever dropped (an unclosed mark or fence stays as
//! written) and the plan is never cut short. Positions count characters (Unicode scalar values). Owner: W8-E.

use std::sync::LazyLock;

use regex::Regex;

/// Longer sources are shown as one literal paragraph, not parsed (C# `MaximumPlanBytes`).
pub const MAXIMUM_PLAN_BYTES: usize = 131_072;
/// How far ahead a closing mark is looked for, so a plan full of stray marks is still read in one pass (C# `MaximumRun`).
const MAXIMUM_RUN: usize = 2_000;

/// C# `CompanionPlanSpan(Start, Length, Style)`: a run of a block's text in `bold`, `italic` or `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSpan {
    pub start: usize,
    pub length: usize,
    pub style: &'static str,
}

/// C# `CompanionPlanBlock(Kind, Text, Level, Marker, Spans)`: a `heading` (level 1 to 6), a `paragraph`, a list `item` (level deep,
/// behind its marker), a `quote`, a `code` block (its lines as written) or a `rule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBlock {
    pub kind: &'static str,
    pub text: String,
    pub level: u32,
    pub marker: Option<String>,
    pub spans: Option<Vec<PlanSpan>>,
}

impl PlanBlock {
    fn new(kind: &'static str, text: String) -> PlanBlock {
        PlanBlock { kind, text, level: 0, marker: None, spans: None }
    }
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$").expect("heading"));
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([ \t]*)(?:([-*+])|(\d{1,9})[.)])[ \t]+(.*)$").expect("item"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}>[ \t]?(.*)$").expect("quote"));

/// C#'s `^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$`: three or more of one of `-*_`, spaces and tabs between, at most three spaces before.
fn rule(line: &str) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let Some(mark) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else { return false };
    rest.chars().all(|c| c == mark || c == ' ' || c == '\t') && rest.chars().filter(|c| *c == mark).count() >= 3
}

/// The plan's Markdown as the blocks the bubble draws.
pub fn plan_blocks(plan: &str) -> Vec<PlanBlock> {
    let mut blocks = Vec::new();
    if plan.len() > MAXIMUM_PLAN_BYTES {
        blocks.push(PlanBlock::new("paragraph", plan.to_string()));
        return blocks;
    }
    let mut paragraph = String::new();
    let mut kind = "paragraph";
    let mut level = 0u32;
    let mut marker: Option<String> = None;
    let flush =
        |blocks: &mut Vec<PlanBlock>, paragraph: &mut String, kind: &mut &'static str, level: &mut u32, marker: &mut Option<String>| {
            if paragraph.is_empty() {
                return;
            }
            let (text, spans) = inline(paragraph);
            if !text.is_empty() {
                blocks.push(PlanBlock { kind, text, level: *level, marker: marker.clone(), spans });
            }
            paragraph.clear();
            *kind = "paragraph";
            *level = 0;
            *marker = None;
        };
    let normalized = plan.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            // A fence runs to the next line that opens with the same three marks, or to the end.
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
            let fence = &trimmed[..3];
            let mut code = Vec::new();
            index += 1;
            while index < lines.len() && !lines[index].trim().starts_with(fence) {
                code.push(lines[index]);
                index += 1;
            }
            blocks.push(PlanBlock::new("code", code.join("\n")));
        } else if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
        } else if let Some(heading) = HEADING.captures(line) {
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
            let (text, spans) = inline(heading.get(2).map_or("", |m| m.as_str()));
            if !text.is_empty() {
                blocks.push(PlanBlock { kind: "heading", text, level: heading[1].len() as u32, marker: None, spans });
            }
        } else if rule(line) {
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
            blocks.push(PlanBlock::new("rule", String::new()));
        } else if trimmed.starts_with('|') {
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
            let mut rows = vec![trimmed.to_string()];
            while index + 1 < lines.len() && lines[index + 1].trim_start().starts_with('|') {
                index += 1;
                rows.push(lines[index].trim().to_string());
            }
            blocks.push(PlanBlock::new("code", rows.join("\n")));
        } else if let Some(item) = ITEM.captures(line) {
            flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
            kind = "item";
            let mut words = item.get(4).map_or("", |m| m.as_str());
            level = (item[1].replace('\t', "    ").chars().count() / 2).min(3) as u32;
            marker = Some(item.get(3).map_or_else(|| "\u{2022}".to_string(), |n| format!("{}.", n.as_str())));
            if let Some(rest) = words.strip_prefix("[ ] ") {
                marker = Some("\u{2610}".into());
                words = rest;
            } else if words.len() >= 4 && words.is_char_boundary(4) && words[..4].eq_ignore_ascii_case("[x] ") {
                marker = Some("\u{2611}".into());
                words = &words[4..];
            }
            paragraph.push_str(words);
        } else if let Some(quote) = QUOTE.captures(line) {
            if kind != "quote" {
                flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
                kind = "quote";
            }
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(quote[1].trim());
        } else {
            // A line under a paragraph, a list item or a quote goes on with it.
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
        index += 1;
    }
    flush(&mut blocks, &mut paragraph, &mut kind, &mut level, &mut marker);
    blocks
}

/// .NET's `char.IsLetterOrDigit`.
fn word(c: char) -> bool {
    c.is_alphanumeric()
}

/// .NET's `char.IsPunctuation(c) | char.IsSymbol(c)`.
fn punctuation_or_symbol(c: char) -> bool {
    if c.is_ascii() { c.is_ascii_punctuation() } else { !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control() }
}

/// A line's words without their marks, and where the bold, italic and code runs lie in them.
fn inline(source: &str) -> (String, Option<Vec<PlanSpan>>) {
    let chars: Vec<char> = source.chars().collect();
    let mut text: Vec<char> = Vec::with_capacity(chars.len());
    let mut spans = Vec::new();
    read(&chars, 0, chars.len(), &mut text, &mut spans, 0);
    // Trimmed at the end only: every run was placed before these last spaces.
    let mut end = text.len();
    while end > 0 && text[end - 1].is_whitespace() {
        end -= 1;
    }
    let words: String = text[..end].iter().collect();
    if spans.is_empty() {
        return (words, None);
    }
    let kept = spans
        .into_iter()
        .filter(|span: &PlanSpan| span.start < end)
        .map(|span| PlanSpan { length: span.length.min(end - span.start), ..span })
        .collect();
    (words, Some(kept))
}

/// `source.IndexOf(needle, start, count)`: where `needle` lies wholly inside `start..start + count`.
fn index_of(source: &[char], needle: &[char], start: usize, count: usize) -> Option<usize> {
    let end = (start + count).min(source.len());
    if needle.is_empty() || start >= end || end - start < needle.len() {
        return None;
    }
    (start..=end - needle.len()).find(|&at| source[at..at + needle.len()] == *needle)
}

fn read(source: &[char], from: usize, to: usize, text: &mut Vec<char>, spans: &mut Vec<PlanSpan>, depth: u32) {
    let mut at = from;
    while at < to {
        let c = source[at];
        if c == '\\' && at + 1 < to && punctuation_or_symbol(source[at + 1]) {
            at += 1;
            text.push(source[at]);
            at += 1;
            continue;
        }
        if c == '`' {
            let mut marks = 1;
            while at + marks < to && source[at + marks] == '`' {
                marks += 1;
            }
            let fence = vec!['`'; marks];
            if let Some(close) = index_of(source, &fence, at + marks, (to - at - marks).min(MAXIMUM_RUN)) {
                let code: String = source[at + marks..close].iter().collect();
                let code: Vec<char> = code.trim().chars().collect();
                spans.push(PlanSpan { start: text.len(), length: code.len(), style: "code" });
                text.extend(code);
                at = close + marks;
                continue;
            }
        }
        if matches!(c, '*' | '_' | '~') && depth < 4 {
            let twice = at + 1 < to && source[at + 1] == c;
            let marks = if twice { 2 } else { 1 };
            // An underscore inside a word is part of the word (snake_case); a lone tilde is a tilde.
            let opens = at + marks < to
                && !source[at + marks].is_whitespace()
                && (c != '_' || at == from || !word(source[at - 1]))
                && (c != '~' || twice);
            if opens && let Some(close) = closing(source, at + marks, to.min(at + marks + MAXIMUM_RUN), c, marks) {
                let start = text.len();
                read(source, at + marks, close, text, spans, depth + 1);
                if c != '~' {
                    spans.push(PlanSpan { start, length: text.len() - start, style: if twice { "bold" } else { "italic" } });
                }
                at = close + marks;
                continue;
            }
        }
        if matches!(c, '[' | '!') && depth < 4 {
            // A link or a picture is its words: nothing here opens or loads its target.
            let open = if c == '!' { at + 1 } else { at };
            if open < to
                && source[open] == '['
                && let Some(middle) = index_of(source, &[']', '('], open, (to - open).min(MAXIMUM_RUN))
                && let Some(end) = index_of(source, &[')'], middle, (to - middle).min(MAXIMUM_RUN))
            {
                read(source, open + 1, middle, text, spans, depth + 1);
                at = end + 1;
                continue;
            }
        }
        text.push(c);
        at += 1;
    }
}

/// `Closing`: where the run opened by `marks` of `mark` closes: the same marks after a character that is not a space, and for an
/// underscore not inside a word.
fn closing(source: &[char], from: usize, to: usize, mark: char, marks: usize) -> Option<usize> {
    let mut at = from;
    while at + marks <= to {
        if source[at] == '`'
            && let Some(end) = index_of(source, &['`'], at + 1, to - at - 1)
        {
            at = end + 1;
            continue;
        }
        if source[at] != mark || at == from || source[at - 1].is_whitespace() {
            at += 1;
            continue;
        }
        let mut run = 1;
        while at + run < to && source[at + run] == mark {
            run += 1;
        }
        // The marks nearest the end of a run close this one; those before them close what was opened inside it. A pair of marks is
        // never a single mark's close.
        if run >= marks && (marks == 2 || run % 2 == 1) {
            let close = at + run - marks;
            if mark != '_' || close + marks >= source.len() || !source[close + marks].is_alphanumeric() {
                return Some(close);
            }
        }
        at += run;
    }
    None
}

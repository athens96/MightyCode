//! MC-31: Jira's Atlassian Document Format (ADF) and Markdown. An issue's description reads as Markdown (paragraphs, headings,
//! lists, task lists, code, quotes, rules, tables, links, marks); a comment goes out as plain paragraphs, fenced code blocks and links.

use serde_json::{Value, json};

/// An ADF document (or any node) as Markdown.
pub(crate) fn to_markdown(doc: &Value) -> String {
    let mut out = Vec::new();
    blocks(doc.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]), 0, &mut out);
    if out.is_empty() && doc.get("type").and_then(Value::as_str).is_some_and(|t| t != "doc") {
        block(doc, 0, &mut out);
    }
    out.join("\n\n").trim().to_string()
}

fn content(node: &Value) -> &[Value] {
    node.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|a| a.get(name))
}

fn blocks(nodes: &[Value], depth: usize, out: &mut Vec<String>) {
    for node in nodes {
        block(node, depth, out);
    }
}

fn block(node: &Value, depth: usize, out: &mut Vec<String>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "paragraph" => {
            let text = inline(content(node));
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
        "heading" => {
            let level = attr(node, "level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6) as usize;
            out.push(format!("{} {}", "#".repeat(level), inline(content(node))));
        }
        "bulletList" | "orderedList" | "taskList" => {
            let mut lines = Vec::new();
            list(node, depth, &mut lines);
            if !lines.is_empty() {
                out.push(lines.join("\n"));
            }
        }
        "codeBlock" => {
            let lang = attr(node, "language").and_then(Value::as_str).unwrap_or("");
            let code: String = content(node).iter().filter_map(|n| n.get("text").and_then(Value::as_str)).collect();
            out.push(format!("```{lang}\n{code}\n```"));
        }
        "blockquote" => {
            let mut inner = Vec::new();
            blocks(content(node), depth, &mut inner);
            let quoted: Vec<String> =
                inner.join("\n\n").lines().map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect();
            out.push(quoted.join("\n"));
        }
        "rule" => out.push("---".into()),
        "table" => {
            let rows: Vec<Vec<String>> = content(node)
                .iter()
                .map(|row| {
                    content(row)
                        .iter()
                        .map(|cell| {
                            let mut inner = Vec::new();
                            blocks(content(cell), 0, &mut inner);
                            inner.join(" ").replace('\n', " ").replace('|', "\\|")
                        })
                        .collect()
                })
                .filter(|cells: &Vec<String>| !cells.is_empty())
                .collect();
            if let Some(first) = rows.first() {
                let mut lines = vec![format!("| {} |", first.join(" | ")), format!("|{}", " --- |".repeat(first.len()))];
                for row in rows.iter().skip(1) {
                    lines.push(format!("| {} |", row.join(" | ")));
                }
                out.push(lines.join("\n"));
            }
        }
        "mediaSingle" | "mediaGroup" | "media" => {}
        "text" | "hardBreak" | "mention" | "emoji" | "inlineCard" | "date" | "status" => {
            let text = inline(std::slice::from_ref(node));
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
        // panel, expand, layout sections and anything new: their content.
        _ => blocks(content(node), depth, out),
    }
}

fn list(node: &Value, depth: usize, lines: &mut Vec<String>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
    let start = attr(node, "order").and_then(Value::as_u64).unwrap_or(1);
    let indent = "  ".repeat(depth);
    for (i, item) in content(node).iter().enumerate() {
        let marker = match kind {
            "orderedList" => format!("{}.", start + i as u64),
            "taskList" => {
                let done = attr(item, "state").and_then(Value::as_str) == Some("DONE");
                format!("- [{}]", if done { "x" } else { " " })
            }
            _ => "-".to_string(),
        };
        // A task item holds inline nodes; a list item holds blocks (paragraphs, nested lists).
        if item.get("type").and_then(Value::as_str) == Some("taskItem") {
            lines.push(format!("{indent}{marker} {}", inline(content(item))));
            continue;
        }
        let mut first = true;
        for child in content(item) {
            match child.get("type").and_then(Value::as_str) {
                Some("bulletList" | "orderedList" | "taskList") => list(child, depth + 1, lines),
                _ => {
                    let mut inner = Vec::new();
                    block(child, depth + 1, &mut inner);
                    let text = inner.join(" ");
                    if first {
                        lines.push(format!("{indent}{marker} {text}"));
                        first = false;
                    } else {
                        lines.push(format!("{indent}  {text}"));
                    }
                }
            }
        }
        if first {
            lines.push(format!("{indent}{marker}"));
        }
    }
}

fn inline(nodes: &[Value]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                let mut text = node.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let marks = node.get("marks").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
                let mut link = None;
                for mark in marks {
                    match mark.get("type").and_then(Value::as_str).unwrap_or("") {
                        "code" => text = format!("`{text}`"),
                        "strong" => text = format!("**{text}**"),
                        "em" => text = format!("*{text}*"),
                        "strike" => text = format!("~~{text}~~"),
                        "link" => link = attr(mark, "href").and_then(Value::as_str).map(str::to_string),
                        _ => {}
                    }
                }
                match link {
                    Some(href) if href == text => out.push_str(&href),
                    Some(href) => out.push_str(&format!("[{text}]({href})")),
                    None => out.push_str(&text),
                }
            }
            "hardBreak" => out.push('\n'),
            "mention" => {
                let text = attr(node, "text").and_then(Value::as_str).unwrap_or("");
                out.push_str(if text.starts_with('@') { text.to_string() } else { format!("@{text}") }.as_str());
            }
            "emoji" => {
                let text = attr(node, "text").or_else(|| attr(node, "shortName")).and_then(Value::as_str).unwrap_or("");
                out.push_str(text);
            }
            "inlineCard" | "blockCard" => {
                if let Some(url) = attr(node, "url").and_then(Value::as_str) {
                    out.push_str(url);
                }
            }
            "status" => out.push_str(&format!("[{}]", attr(node, "text").and_then(Value::as_str).unwrap_or(""))),
            "date" => {
                if let Some(ts) = attr(node, "timestamp").and_then(Value::as_str) {
                    out.push_str(ts);
                }
            }
            _ => out.push_str(&inline(content(node))),
        }
    }
    out
}

/// Markdown as an ADF document: paragraphs (lines joined by hard breaks), fenced code blocks, and http(s) links — bare or
/// `[text](url)` — as link marks. Everything else stays as its text.
pub(crate) fn from_markdown(markdown: &str) -> Value {
    let mut nodes = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Option<(String, Vec<&str>)> = None;
    let flush = |paragraph: &mut Vec<&str>, nodes: &mut Vec<Value>| {
        if paragraph.is_empty() {
            return;
        }
        let mut content = Vec::new();
        for (i, line) in paragraph.iter().enumerate() {
            if i > 0 {
                content.push(json!({ "type": "hardBreak" }));
            }
            content.extend(inline_nodes(line));
        }
        nodes.push(json!({ "type": "paragraph", "content": content }));
        paragraph.clear();
    };
    let normalized = markdown.replace("\r\n", "\n");
    for line in normalized.split('\n') {
        if let Some((lang, lines)) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                let text = lines.join("\n");
                let mut block = json!({ "type": "codeBlock", "content": if text.is_empty() { json!([]) } else { json!([{ "type": "text", "text": text }]) } });
                if !lang.is_empty() {
                    block["attrs"] = json!({ "language": lang });
                }
                nodes.push(block);
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            flush(&mut paragraph, &mut nodes);
            code = Some((rest.trim().to_string(), Vec::new()));
        } else if line.trim().is_empty() {
            flush(&mut paragraph, &mut nodes);
        } else {
            paragraph.push(line);
        }
    }
    if let Some((lang, lines)) = code {
        // An unclosed fence: its lines as a code block all the same.
        let mut block = json!({ "type": "codeBlock", "content": [{ "type": "text", "text": lines.join("\n") }] });
        if !lang.is_empty() {
            block["attrs"] = json!({ "language": lang });
        }
        nodes.push(block);
    }
    flush(&mut paragraph, &mut nodes);
    json!({ "type": "doc", "version": 1, "content": nodes })
}

/// One line's text nodes, its links as link marks.
fn inline_nodes(line: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rest = line;
    let text = |s: &str| json!({ "type": "text", "text": s });
    let link = |label: &str, href: &str| json!({ "type": "text", "text": label, "marks": [{ "type": "link", "attrs": { "href": href } }] });
    while !rest.is_empty() {
        let markdown_link = rest.find('[').and_then(|open| {
            let close = rest[open..].find("](")? + open;
            let end = rest[close + 2..].find(')')? + close + 2;
            let href = &rest[close + 2..end];
            (href.starts_with("https://") || href.starts_with("http://")).then_some((open, &rest[open + 1..close], href, end + 1))
        });
        let bare = ["https://", "http://"].iter().filter_map(|scheme| rest.find(scheme)).min();
        match (markdown_link, bare) {
            (Some((open, label, href, after)), b) if b.is_none_or(|b| open <= b) => {
                if open > 0 {
                    out.push(text(&rest[..open]));
                }
                out.push(link(label, href));
                rest = &rest[after..];
            }
            (_, Some(start)) => {
                if start > 0 {
                    out.push(text(&rest[..start]));
                }
                let end = rest[start..].find(|c: char| c.is_whitespace()).map_or(rest.len(), |e| start + e);
                let mut url = &rest[start..end];
                // A sentence's closing punctuation is not part of the address.
                while url.ends_with(['.', ',', ')', ';', ':']) {
                    url = &url[..url.len() - 1];
                }
                let end = start + url.len();
                out.push(link(url, url));
                rest = &rest[end..];
            }
            _ => {
                out.push(text(rest));
                rest = "";
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adf_reads_as_markdown() {
        let doc = json!({
            "type": "doc", "version": 1,
            "content": [
                { "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Steps" }] },
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "Open " },
                    { "type": "text", "text": "login", "marks": [{ "type": "strong" }] },
                    { "type": "text", "text": " and see " },
                    { "type": "text", "text": "docs", "marks": [{ "type": "link", "attrs": { "href": "https://example.test/d" } }] },
                    { "type": "hardBreak" },
                    { "type": "text", "text": "run" , "marks": [{ "type": "code" }] },
                    { "type": "text", "text": " " },
                    { "type": "mention", "attrs": { "id": "1", "text": "@Kim" } }
                ]},
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": "one" }] },
                        { "type": "orderedList", "content": [
                            { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "nested" }] }] }
                        ]}
                    ]},
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "two" }] }] }
                ]},
                { "type": "taskList", "attrs": { "localId": "t" }, "content": [
                    { "type": "taskItem", "attrs": { "localId": "a", "state": "DONE" }, "content": [{ "type": "text", "text": "done item" }] },
                    { "type": "taskItem", "attrs": { "localId": "b", "state": "TODO" }, "content": [{ "type": "text", "text": "todo item" }] }
                ]},
                { "type": "codeBlock", "attrs": { "language": "rust" }, "content": [{ "type": "text", "text": "fn main() {}" }] },
                { "type": "blockquote", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "quoted" }] }] },
                { "type": "rule" },
                { "type": "panel", "attrs": { "panelType": "info" }, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "in a panel" }] }] },
                { "type": "table", "content": [
                    { "type": "tableRow", "content": [
                        { "type": "tableHeader", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "A" }] }] },
                        { "type": "tableHeader", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "B" }] }] }
                    ]},
                    { "type": "tableRow", "content": [
                        { "type": "tableCell", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "1" }] }] },
                        { "type": "tableCell", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "2" }] }] }
                    ]}
                ]}
            ]
        });
        let md = to_markdown(&doc);
        let expected = "## Steps\n\n\
            Open **login** and see [docs](https://example.test/d)\n`run` @Kim\n\n\
            - one\n  1. nested\n- two\n\n\
            - [x] done item\n- [ ] todo item\n\n\
            ```rust\nfn main() {}\n```\n\n\
            > quoted\n\n\
            ---\n\n\
            in a panel\n\n\
            | A | B |\n| --- | --- |\n| 1 | 2 |";
        assert_eq!(md, expected);
        assert_eq!(to_markdown(&Value::Null), "");
    }

    #[test]
    fn markdown_goes_out_as_paragraphs_code_and_links() {
        let adf = from_markdown(
            "Pull request: https://github.com/o/r/pull/7.\nsecond line\n\n```sh\nnpm test\n```\n\nSee [the PR](https://h.test/p) now",
        );
        assert_eq!(
            adf,
            json!({ "type": "doc", "version": 1, "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "Pull request: " },
                    { "type": "text", "text": "https://github.com/o/r/pull/7", "marks": [{ "type": "link", "attrs": { "href": "https://github.com/o/r/pull/7" } }] },
                    { "type": "text", "text": "." },
                    { "type": "hardBreak" },
                    { "type": "text", "text": "second line" }
                ]},
                { "type": "codeBlock", "attrs": { "language": "sh" }, "content": [{ "type": "text", "text": "npm test" }] },
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "See " },
                    { "type": "text", "text": "the PR", "marks": [{ "type": "link", "attrs": { "href": "https://h.test/p" } }] },
                    { "type": "text", "text": " now" }
                ]}
            ]})
        );
        // And back: the same text.
        assert_eq!(
            to_markdown(&adf),
            "Pull request: https://github.com/o/r/pull/7.\nsecond line\n\n```sh\nnpm test\n```\n\nSee [the PR](https://h.test/p) now"
        );
        assert_eq!(from_markdown(""), json!({ "type": "doc", "version": 1, "content": [] }));
    }
}

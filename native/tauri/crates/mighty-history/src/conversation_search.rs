//! 대화 검색 (Swift `ConversationSearch`, MightyCore/ConversationSearch.swift): finds the workspace's past sessions (the resume list's,
//! [`crate::resumable`]) whose messages hold every term of a query, in the CLIs' own session records. Only what a person or the agent
//! said counts — the requests and the answers — not tool calls or their output, so a hit is something to read again. Read only;
//! nothing is indexed or kept.
//!
//! The JSONL records are searched: Claude's (`message.content`), Codex's (`payload` of a `message`, or a `user_message` /
//! `agent_message` event), pi's (`message` entries), Antigravity's transcripts (`USER_INPUT` / `PLANNER_RESPONSE` steps) and other
//! records with a `role` and plain `text`. opencode's and Hermes's sessions live in databases and are not searched.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use crate::{Cancel, ResumableSession};

/// A record larger than this is read from its end only (the newest part).
pub const MAXIMUM_BYTES: u64 = 24 * 1_048_576;
/// Characters kept on each side of the first match in a snippet.
pub const SNIPPET_RADIUS: usize = 60;
/// The most sessions one search answers with.
pub const LIMIT: usize = 50;

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Agent,
}

/// One session that matched: the words around its first matching message, and how many messages matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub session: ResumableSession,
    pub snippet: String,
    pub matches: usize,
}

/// Lower-cased terms of a query, split on white space; empty for a blank query.
pub fn terms(query: &str) -> Vec<String> {
    query.to_lowercase().split_whitespace().map(str::to_string).collect()
}

/// Whether a session's record is one this search reads (a JSONL file; not a database).
pub fn searchable(session: &ResumableSession) -> bool {
    session.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
}

const ROLES: [&str; 2] = ["user", "assistant"];

/// The speaker and words of one JSON line of a session record, or None when the line is no message (a tool call or result, metadata,
/// bad JSON).
pub fn message(line: &str) -> Option<(Speaker, String)> {
    let object: Value = serde_json::from_str(line).ok()?;
    let object = object.as_object()?;
    let speaker = |role: &str| if role == "user" { Speaker::User } else { Speaker::Agent };
    if let Some(message) = object.get("message").and_then(Value::as_object)
        && let Some(role) = message.get("role").and_then(Value::as_str)
        && ROLES.contains(&role)
    {
        return texts(message.get("content")).map(|text| (speaker(role), text));
    }
    if let Some(payload) = object.get("payload").and_then(Value::as_object) {
        return match payload.get("type").and_then(Value::as_str) {
            Some("message") => {
                let role = payload.get("role").and_then(Value::as_str).filter(|r| ROLES.contains(r))?;
                texts(payload.get("content")).map(|text| (speaker(role), text))
            }
            Some("user_message") => payload.get("message").and_then(Value::as_str).map(|t| (Speaker::User, t.to_string())),
            Some("agent_message") => payload.get("message").and_then(Value::as_str).map(|t| (Speaker::Agent, t.to_string())),
            _ => None,
        };
    }
    // Antigravity's transcript steps: a request the person typed (its words inside <USER_REQUEST>) and the agent's answers.
    match object.get("type").and_then(Value::as_str) {
        Some("USER_INPUT") if object.get("source").and_then(Value::as_str) != Some("SYSTEM") => {
            return crate::antigravity::request(object.get("content").and_then(Value::as_str)).map(|t| (Speaker::User, t));
        }
        Some("PLANNER_RESPONSE") => {
            return object.get("content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).map(|t| (Speaker::Agent, t.to_string()));
        }
        _ => {}
    }
    let role = object.get("role").and_then(Value::as_str).filter(|r| ["user", "assistant", "model"].contains(r))?;
    let content = object.get("content").or_else(|| object.get("parts")).or_else(|| object.get("text"));
    texts(content).map(|text| (speaker(role), text))
}

/// Text of a message's content: a string, or the `text` of its text parts (tool parts left out).
fn texts(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let words: Vec<&str> = parts
                .iter()
                .filter_map(|part| match part {
                    Value::String(text) => Some(text.as_str()),
                    Value::Object(part) => {
                        let kind = part.get("type").and_then(Value::as_str).unwrap_or("text");
                        if !["text", "input_text", "output_text"].contains(&kind) {
                            return None;
                        }
                        part.get("text").and_then(Value::as_str)
                    }
                    _ => None,
                })
                .collect();
            (!words.is_empty()).then(|| words.join("\n"))
        }
        _ => None,
    }
}

fn newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The words around the first of `terms` in `text`, on one line: up to [`SNIPPET_RADIUS`] characters before it and twice that plus
/// the first term after the start, "…" where the text goes on. With no match, the text's start.
pub fn snippet(text: &str, terms: &[String]) -> String {
    let flat = text.split(newline).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    // The lowered text, each of its characters knowing the one of `flat` it came from (lowering may change the count).
    let mut lower = String::new();
    let mut origin: Vec<usize> = vec![];
    for (index, c) in chars.iter().enumerate() {
        for l in c.to_lowercase() {
            lower.push(l);
            origin.push(index);
        }
    }
    let first = terms.iter().filter_map(|term| lower.find(term.as_str())).min();
    let Some(byte) = first else { return chars.iter().take(SNIPPET_RADIUS * 2).collect() };
    let offset = origin[lower[..byte].chars().count()];
    let start = offset.saturating_sub(SNIPPET_RADIUS);
    let span = SNIPPET_RADIUS * 2 + terms.first().map_or(0, |t| t.chars().count());
    let end = (start + span).min(chars.len());
    let middle: String = chars[start..end].iter().collect();
    format!("{}{}{}", if start > 0 { "…" } else { "" }, middle.trim(), if end < chars.len() { "…" } else { "" })
}

/// A term that JSON writes as it is: one that may be looked for in a raw line before it is parsed.
fn raw(term: &str) -> bool {
    !term.chars().any(|c| c == '"' || c == '\\' || c.is_control())
}

/// Searches one record's lines: the snippet of its first matching message and how many matched; None when none did.
pub fn search_lines<'a>(lines: impl IntoIterator<Item = &'a str>, terms: &[String], cancel: &dyn Cancel) -> Option<(String, usize)> {
    if terms.is_empty() {
        return None;
    }
    let cheap: Vec<&String> = terms.iter().filter(|t| raw(t)).collect();
    let (mut first, mut count) = (None::<String>, 0);
    for (index, line) in lines.into_iter().enumerate() {
        if index % 4096 == 4095 && cancel.cancelled() {
            return None;
        }
        // Cheap test before the JSON parse: every term must be somewhere on the line.
        if !cheap.is_empty() {
            let lower = line.to_lowercase();
            if !cheap.iter().all(|t| lower.contains(t.as_str())) {
                continue;
            }
        }
        let Some((_, text)) = message(line) else { continue };
        let words = text.to_lowercase();
        if !terms.iter().all(|t| words.contains(t.as_str())) {
            continue;
        }
        count += 1;
        if first.is_none() {
            first = Some(snippet(&text, terms));
        }
    }
    first.map(|snippet| (snippet, count))
}

/// The sessions among `sessions` whose messages hold every term of `query`, newest first, at most `limit`. Stops (with what it found)
/// once `cancel` says so.
pub fn search(sessions: &[ResumableSession], query: &str, limit: usize, cancel: &dyn Cancel) -> Vec<Hit> {
    let terms = terms(query);
    if terms.is_empty() {
        return vec![];
    }
    let mut ordered: Vec<&ResumableSession> = sessions.iter().filter(|s| searchable(s)).collect();
    ordered.sort_by(|a, b| b.modified.utc_ticks().cmp(&a.modified.utc_ticks()).then_with(|| a.id().cmp(&b.id())));
    let mut hits = vec![];
    for session in ordered {
        if cancel.cancelled() || hits.len() >= limit {
            break;
        }
        let Some(text) = read(&session.path) else { continue };
        if let Some((snippet, matches)) = search_lines(text.split('\n'), &terms, cancel) {
            hits.push(Hit { session: session.clone(), snippet, matches });
        }
    }
    hits
}

/// The session as Markdown: a title, then each message under its speaker's heading. Codex writes a message twice (its response item
/// and its event); a message equal to the one before it is left out.
pub fn markdown<'a>(lines: impl IntoIterator<Item = &'a str>, title: &str, user: &str, agent: &str) -> String {
    let mut out = vec![format!("# {title}")];
    let mut last: Option<String> = None;
    for line in lines {
        let Some((speaker, text)) = message(line) else { continue };
        let body = text.trim();
        if body.is_empty() || last.as_deref() == Some(body) {
            continue;
        }
        last = Some(body.to_string());
        out.push(format!("## {}\n\n{body}", if speaker == Speaker::User { user } else { agent }));
    }
    out.join("\n\n") + "\n"
}

/// The session's record as Markdown ([`markdown`]); None when it cannot be read.
pub fn markdown_of(session: &ResumableSession, title: &str, user: &str, agent: &str) -> Option<String> {
    searchable(session).then_some(())?;
    read(&session.path).map(|text| markdown(text.split('\n'), title, user, agent))
}

/// A file name for a session's Markdown: its title's first 60 characters, without what a file name may not hold, and `.md`.
pub fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .take(60)
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c })
        .collect();
    let name = name.trim().trim_matches('.');
    format!("{}.md", if name.is_empty() { "conversation" } else { name })
}

/// The record's text, its last [`MAXIMUM_BYTES`] when larger (the first line, then cut, reads as no message).
fn read(path: &Path) -> Option<String> {
    let mut file = crate::files::open(path)?;
    let size = file.metadata().ok()?.len();
    let start = size.saturating_sub(MAXIMUM_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((size - start) as usize);
    file.take(MAXIMUM_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    use mighty_core::time::DateTimeOffset;

    use super::*;
    use crate::Never;

    fn text(line: &str) -> Option<String> {
        message(line).map(|(_, text)| text)
    }

    #[test]
    fn messages_of_each_record_shape_are_read_and_tool_traffic_is_not() {
        assert_eq!(
            text(r#"{"type":"user","message":{"role":"user","content":"마이그레이션 해 줘"}}"#).as_deref(),
            Some("마이그레이션 해 줘")
        );
        assert_eq!(
            text(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"},{"type":"tool_use","name":"Bash"}]}}"#)
                .as_deref(),
            Some("done")
        );
        assert_eq!(text(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"secret output"}]}}"#), None);
        assert_eq!(
            text(
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix ci"}]}}"#
            )
            .as_deref(),
            Some("fix ci")
        );
        assert_eq!(text(r#"{"type":"response_item","payload":{"type":"function_call_output","output":"x"}}"#), None);
        assert_eq!(
            message(r#"{"type":"event_msg","payload":{"type":"agent_message","message":"ok"}}"#),
            Some((Speaker::Agent, "ok".into()))
        );
        assert_eq!(message(r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#), Some((Speaker::User, "hi".into())));
        assert_eq!(message(r#"{"role":"model","parts":[{"text":"hello"}]}"#), Some((Speaker::Agent, "hello".into())));
        assert_eq!(text("not json"), None);
        // pi
        assert_eq!(
            message(r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"rename it"}]}}"#),
            Some((Speaker::User, "rename it".into()))
        );
        // Antigravity
        assert_eq!(
            message(
                r#"{"step_index":0,"source":"USER","type":"USER_INPUT","content":"<USER_REQUEST>\n add tests \n</USER_REQUEST><ADDITIONAL_METADATA>x</ADDITIONAL_METADATA>"}"#
            ),
            Some((Speaker::User, "add tests".into()))
        );
        assert_eq!(text(r#"{"step_index":1,"source":"SYSTEM","type":"USER_INPUT","content":"<USER_REQUEST>auto</USER_REQUEST>"}"#), None);
        assert_eq!(message(r#"{"step_index":2,"type":"PLANNER_RESPONSE","content":"Added."}"#), Some((Speaker::Agent, "Added.".into())));
        assert_eq!(text(r#"{"step_index":3,"type":"GENERIC","content":"tool output"}"#), None);
    }

    #[test]
    fn every_term_must_match_one_message() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Add a database migration for users"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The migration is in db/migrate."}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"migration users"}]}}"#,
        ];
        let found = search_lines(lines, &terms("Migration  USERS"), &Never).unwrap();
        assert_eq!(found, ("Add a database migration for users".to_string(), 1));
        assert_eq!(search_lines(lines, &["migration".into(), "nowhere".into()], &Never), None);
        assert_eq!(search_lines(lines, &[], &Never), None);
        assert!(terms("   ").is_empty());
        // A term JSON escapes is still found in the message's words.
        let quoted = [r#"{"message":{"role":"user","content":"say \"hello\" twice"}}"#];
        assert_eq!(search_lines(quoted, &terms("\"hello\""), &Never).map(|f| f.1), Some(1));
    }

    #[test]
    fn a_snippet_centres_on_the_match() {
        let text = "a ".repeat(100) + "needle\nhere " + &"b ".repeat(100);
        let snippet = snippet(&text, &["needle".into()]);
        assert!(snippet.starts_with('…') && snippet.ends_with('…') && snippet.contains("needle here"), "{snippet}");
        assert_eq!(snippet.chars().count(), SNIPPET_RADIUS * 2 + "needle".len() + 2 - 1, "{snippet}");
        assert_eq!(super::snippet("short\n\nline", &["line".into()]), "short line");
        assert_eq!(super::snippet("İİ needle", &["needle".into()]), "İİ needle", "lowering that grows the text keeps the place");
        assert_eq!(super::snippet(&"x".repeat(300), &["zzz".into()]).chars().count(), SNIPPET_RADIUS * 2);
    }

    fn folder() -> PathBuf {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let folder = std::env::temp_dir().join(format!("mighty-search-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn session(folder: &Path, id: &str, text: &str, age_seconds: i64) -> ResumableSession {
        let path = folder.join(format!("{id}.jsonl"));
        std::fs::write(&path, format!(r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#)).unwrap();
        ResumableSession {
            provider: "claude".into(),
            session_id: id.into(),
            title: Some(text.into()),
            modified: DateTimeOffset::from_utc_ticks(DateTimeOffset::now().utc_ticks() - age_seconds * 10_000_000),
            requests: Some(1),
            model: None,
            path,
            automated: false,
        }
    }

    #[test]
    fn sessions_are_searched_newest_first_up_to_the_limit() {
        let folder = folder();
        let mut database = session(&folder, "db", "relay deploy", 0);
        database.path = folder.join("state.db");
        let sessions = [
            session(&folder, "old", "deploy the relay", 100),
            session(&folder, "new", "relay deploy fails", 1),
            session(&folder, "other", "unrelated", 0),
            database,
        ];
        let ids = |hits: Vec<Hit>| hits.into_iter().map(|h| h.session.session_id).collect::<Vec<_>>();
        assert_eq!(ids(search(&sessions, "relay deploy", LIMIT, &Never)), ["new", "old"]);
        assert_eq!(ids(search(&sessions, "relay", 1, &Never)), ["new"]);
        assert!(search(&sessions, "   ", LIMIT, &Never).is_empty());
        assert!(search(&sessions, "relay", LIMIT, &AtomicBool::new(true)).is_empty(), "a superseded search stops");
        std::fs::remove_dir_all(folder).ok();
    }

    #[test]
    fn a_large_record_is_read_from_its_end() {
        let folder = folder();
        let path = folder.join("big.jsonl");
        let filler = format!("{}\n", r#"{"message":{"role":"user","content":"early needle"}}"#).repeat((MAXIMUM_BYTES as usize / 50) + 10);
        std::fs::write(&path, filler + r#"{"message":{"role":"user","content":"late"}}"#).unwrap();
        let text = read(&path).unwrap();
        assert!(text.len() as u64 <= MAXIMUM_BYTES);
        assert!(text.ends_with(r#""late"}}"#));
        std::fs::remove_dir_all(folder).ok();
    }

    #[test]
    fn a_session_becomes_markdown_without_tool_traffic_or_repeats() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"fix it"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"fixed"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"fixed"}}"#,
        ];
        assert_eq!(markdown(lines, "T", "Me", "Agent"), "# T\n\n## Me\n\nfix it\n\n## Agent\n\nfixed\n");
    }

    #[test]
    fn file_names_hold_no_separator() {
        assert_eq!(file_name("fix a/b: the \"thing\""), "fix a-b- the -thing-.md");
        assert_eq!(file_name("   "), "conversation.md");
        assert_eq!(file_name(&"가".repeat(80)).chars().count(), 63);
    }
}

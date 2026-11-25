//! MC-98: reading a JSON body into an event's envelope: a webhook's `map` (and its presets), and an MCP tool's items.
//!
//! An expression names where a value is: a JSON pointer (`/task/key`), a dotted path (`task.key`; a number picks a list's item,
//! `items.0.name`) or text with `{{path}}` in it (`PR#{{pull_request.number}}`; empty when every path it names is missing).
//! Alternatives are joined with `||`, the first that is not empty wins (`pull_request.title || issue.title`). A value found is
//! text as is, a number or boolean as written, or a list of them (or of objects with a `name`, `key`, `title` or `login`) joined
//! with `,`; an object or `null` is nothing. An event's `type` is a name: its plain text is kept as written and only `{{path}}`
//! reads the body (`mightywork.{{event}}`), then it is made a dotted lowercase name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::watch::MAX_FIELD_CHARS;

/// The longest expression.
pub const MAX_EXPR_CHARS: usize = 300;
/// The most fields a map names.
pub const MAX_MAP_FIELDS: usize = 32;
/// The longest event type.
pub const MAX_KIND_CHARS: usize = 64;
/// An event type when a map gives none.
pub const WEBHOOK_KIND: &str = "webhook.received";

/// The value at `path` in `body`: a JSON pointer (`/a/b`, `""` the body itself) or a dotted path (`a.b`, `items.0`).
pub fn lookup<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    if path.is_empty() || path.starts_with('/') {
        return body.pointer(path);
    }
    path.split('.').try_fold(body, |at, part| match at {
        Value::Object(map) => map.get(part),
        Value::Array(items) => part.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => None,
    })
}

/// A scalar's text: a string trimmed, a number or boolean as written; None for anything else or empty text.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|t| !t.is_empty()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

/// A list item's text: a scalar's, or an object's `name`, `key`, `title` or `login`.
pub fn item_text(item: &Value) -> Option<String> {
    match item {
        Value::Object(map) => ["name", "key", "title", "login"].iter().find_map(|k| map.get(*k).and_then(scalar_text)),
        other => scalar_text(other),
    }
}

/// A found value as text: a scalar's, or a list's items joined with `,` ([`item_text`]); None for `null`, an object, an empty text
/// or an empty list.
pub fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => Some(items.iter().filter_map(item_text).collect::<Vec<_>>().join(",")).filter(|t| !t.is_empty()),
        other => scalar_text(other),
    }
}

/// Where an expression reads: the body, and named values read before it (`webhook.event`, the sender's event header).
pub struct Source<'a> {
    pub body: &'a Value,
    pub extra: &'a BTreeMap<String, String>,
}

impl Source<'_> {
    fn get(&self, path: &str) -> Option<String> {
        let path = path.trim();
        match self.extra.get(path) {
            Some(value) => Some(value.trim().to_string()).filter(|v| !v.is_empty()),
            None => lookup(self.body, path).and_then(text_of),
        }
    }

    /// `text` with each `{{path}}` read; None when it names a path and every one is missing, or it comes to nothing.
    fn template(&self, text: &str) -> Option<String> {
        let (mut out, mut rest, mut named, mut found) = (String::new(), text, false, false);
        while let Some(open) = rest.find("{{") {
            out.push_str(&rest[..open]);
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else {
                out.push_str(&rest[open..]);
                rest = "";
                break;
            };
            named = true;
            if let Some(value) = self.get(&after[..close]) {
                found = true;
                out.push_str(&value);
            }
            rest = &after[close + 2..];
        }
        out.push_str(rest);
        if named && !found {
            return None;
        }
        Some(out.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// An expression's value: the first of its `||` alternatives that is not empty.
    pub fn read(&self, expr: &str) -> Option<String> {
        expr.split("||").map(str::trim).find_map(|alt| if alt.contains("{{") { self.template(alt) } else { self.get(alt) })
    }

    /// An event type's value: as [`Source::read`], but plain text is kept as written; made a dotted lowercase name.
    pub fn read_kind(&self, expr: &str) -> Option<String> {
        expr.split("||").map(str::trim).find_map(|alt| {
            let text = if alt.contains("{{") { self.template(alt) } else { Some(alt.to_string()) };
            text.and_then(|t| normalize_kind(&t))
        })
    }
}

/// `text` as an event type: lowercase, anything but letters, digits, `.`, `_` and `-` made `_`, dots never doubled nor at the ends;
/// None when it does not start with a letter. Cut to [`MAX_KIND_CHARS`].
pub fn normalize_kind(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.trim().to_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') { c } else { '_' };
        if c == '.' && (out.is_empty() || out.ends_with('.')) {
            continue;
        }
        out.push(c);
    }
    let out: String = out.trim_end_matches('.').chars().take(MAX_KIND_CHARS).collect();
    let out = out.trim_end_matches('.').to_string();
    out.starts_with(|c: char| c.is_ascii_lowercase()).then_some(out)
}

/// Why an expression cannot be read: empty, too long, a control character, an empty alternative, an unclosed `{{`.
pub fn validate_expr(expr: &str, what: &str) -> Result<(), String> {
    if expr.trim().is_empty() || expr.chars().count() > MAX_EXPR_CHARS || expr.chars().any(char::is_control) {
        return Err(format!("'{what}' is empty, longer than {MAX_EXPR_CHARS} characters or has a control character"));
    }
    for alt in expr.split("||") {
        if alt.trim().is_empty() {
            return Err(format!("'{what}' has an empty alternative (||)"));
        }
        if alt.matches("{{").count() != alt.matches("}}").count() {
            return Err(format!("'{what}' has a {{{{ without its }}}}"));
        }
    }
    Ok(())
}

/// A field's name a map writes: 1 to 64 letters, digits, `.`, `_` and `-`, not one of the envelope's `event.*`.
pub fn valid_field_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.starts_with("event.")
}

/// A safe link: `http://` or `https://`, no space or control character.
pub fn web_url(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    ((lower.starts_with("https://") || lower.starts_with("http://"))
        && text.len() <= 2_000
        && !text.chars().any(|c| c.is_control() || c.is_whitespace()))
    .then(|| text.to_string())
}

/// How a webhook's body becomes its event: each key an expression (see the module); `type` a name with `{{path}}` in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookMap {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// More named values for a `where` and a prompt's `{{name}}` (`task.to: task.stage`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

impl WebhookMap {
    pub fn is_empty(&self) -> bool {
        *self == WebhookMap::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        let keys = [("type", &self.kind), ("subject", &self.subject), ("title", &self.title), ("url", &self.url), ("actor", &self.actor)];
        for (key, expr) in keys {
            if let Some(expr) = expr {
                validate_expr(expr, &format!("map.{key}"))?;
            }
        }
        if self.fields.len() > MAX_MAP_FIELDS {
            return Err(format!("a map names at most {MAX_MAP_FIELDS} fields"));
        }
        for (name, expr) in &self.fields {
            if !valid_field_name(name) {
                return Err(format!("'{name}' is not a field name: letters, digits, . _ - (not event.…)"));
            }
            validate_expr(expr, &format!("map.fields.{name}"))?;
        }
        Ok(())
    }

    /// This map over `base`: each key it sets wins, its fields added to (or replacing) the base's.
    pub fn over(&self, base: &WebhookMap) -> WebhookMap {
        let pick = |mine: &Option<String>, theirs: &Option<String>| mine.clone().filter(|m| !m.trim().is_empty()).or(theirs.clone());
        let mut fields = base.fields.clone();
        fields.extend(self.fields.clone());
        WebhookMap {
            kind: pick(&self.kind, &base.kind),
            subject: pick(&self.subject, &base.subject),
            title: pick(&self.title, &base.title),
            url: pick(&self.url, &base.url),
            actor: pick(&self.actor, &base.actor),
            fields,
        }
    }
}

/// A ready map for a sender: `generic` (the body as `data.*` only, `webhook.received`), `github` (GitHub's webhook: its
/// `X-GitHub-Event` and `action` as `github.<event>.<action>`) and `mightywork` (a task's change: `mightywork.<event>`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookPreset {
    #[default]
    Generic,
    Github,
    Mightywork,
}

impl WebhookPreset {
    pub const ALL: [WebhookPreset; 3] = [WebhookPreset::Generic, WebhookPreset::Github, WebhookPreset::Mightywork];

    pub fn name(self) -> &'static str {
        match self {
            WebhookPreset::Generic => "generic",
            WebhookPreset::Github => "github",
            WebhookPreset::Mightywork => "mightywork",
        }
    }

    pub fn parse(text: &str) -> Option<WebhookPreset> {
        let text = text.trim().to_ascii_lowercase();
        WebhookPreset::ALL.into_iter().find(|p| p.name() == text)
    }

    /// How a picker shows it (the generic one's words are the app's own).
    pub fn label(self) -> &'static str {
        match self {
            WebhookPreset::Generic => "generic",
            WebhookPreset::Github => "GitHub",
            WebhookPreset::Mightywork => "MightyWork",
        }
    }

    pub fn is_generic(&self) -> bool {
        *self == WebhookPreset::Generic
    }

    /// The preset's map (data only: what its sender's body looks like).
    pub fn map(self) -> WebhookMap {
        let fields = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        match self {
            WebhookPreset::Generic => WebhookMap::default(),
            WebhookPreset::Github => WebhookMap {
                kind: Some("github.{{webhook.event}}.{{action}}".into()),
                subject: Some(
                    "PR#{{pull_request.number}} || issue#{{issue.number}} || release:{{release.tag_name}} || run-{{workflow_run.id}} || {{ref}}"
                        .into(),
                ),
                title: Some(
                    "pull_request.title || issue.title || release.name || workflow_run.display_title || head_commit.message".into(),
                ),
                url: Some(
                    "pull_request.html_url || issue.html_url || release.html_url || workflow_run.html_url || compare || repository.html_url"
                        .into(),
                ),
                actor: Some("sender.login".into()),
                fields: fields(&[
                    ("action", "action"),
                    ("repo", "repository.full_name"),
                    ("author", "pull_request.user.login || issue.user.login || release.author.login || sender.login"),
                    ("base", "pull_request.base.ref || workflow_run.head_branch"),
                    ("labels", "pull_request.labels || issue.labels"),
                    ("pr.number", "pull_request.number"),
                    ("pr.title", "pull_request.title"),
                    ("pr.url", "pull_request.html_url"),
                    ("issue.number", "issue.number"),
                    ("issue.title", "issue.title"),
                    ("release.tag", "release.tag_name"),
                ]),
            },
            WebhookPreset::Mightywork => WebhookMap {
                kind: Some("mightywork.{{event}}".into()),
                subject: Some("task.key".into()),
                title: Some("{{task.key}} {{task.title}}".into()),
                url: Some("task.url".into()),
                actor: Some("actor || actor.name || actor.login".into()),
                fields: fields(&[
                    ("task.key", "task.key"),
                    ("task.title", "task.title"),
                    ("task.stage", "task.stage"),
                    ("task.to", "task.stage"),
                    ("task.from", "task.previous_stage"),
                    ("task.labels", "task.labels"),
                    ("task.version", "task.version"),
                    ("task.url", "task.url"),
                ]),
            },
        }
    }
}

/// What a map read from a body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapped {
    /// The event type ([`WEBHOOK_KIND`] when the map gives none).
    pub kind: String,
    pub subject: Option<String>,
    pub title: Option<String>,
    /// An `http(s)` link only.
    pub url: Option<String>,
    pub actor: Option<String>,
    pub fields: BTreeMap<String, String>,
}

/// `map` (over `preset`'s) read from `body`, with the sender's event header as `webhook.event`.
pub fn apply(preset: WebhookPreset, map: &WebhookMap, body: &Value, event_header: Option<&str>) -> Mapped {
    let map = map.over(&preset.map());
    let mut extra = BTreeMap::new();
    if let Some(event) = event_header.map(str::trim).filter(|e| !e.is_empty()) {
        extra.insert("webhook.event".to_string(), event.to_string());
    }
    let source = Source { body, extra: &extra };
    let cut = |text: String| -> String { text.chars().take(MAX_FIELD_CHARS).collect() };
    let read = |expr: &Option<String>| expr.as_deref().and_then(|e| source.read(e)).map(cut);
    Mapped {
        kind: map.kind.as_deref().and_then(|e| source.read_kind(e)).unwrap_or_else(|| WEBHOOK_KIND.to_string()),
        subject: read(&map.subject).map(|s| s.chars().take(200).collect()),
        title: read(&map.title),
        url: read(&map.url).and_then(|u| web_url(&u)),
        actor: read(&map.actor).map(|a| a.chars().take(200).collect()),
        fields: map.fields.iter().filter_map(|(name, expr)| source.read(expr).map(|v| (name.clone(), cut(v)))).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(body: &Value, expr: &str) -> Option<String> {
        Source { body, extra: &BTreeMap::new() }.read(expr)
    }

    #[test]
    fn expressions_read_pointers_paths_templates_and_alternatives() {
        let body = json!({"task": {"key": "MC-12", "labels": ["bug", "v0.6.4"], "n": 3, "done": false, "empty": ""},
            "items": [{"name": "a"}, {"name": "b"}], "labels": [{"name": "x"}, {"name": "y"}], "obj": {"a": 1}});
        assert_eq!(read(&body, "/task/key").as_deref(), Some("MC-12"));
        assert_eq!(read(&body, "task.key").as_deref(), Some("MC-12"));
        assert_eq!(read(&body, "items.1.name").as_deref(), Some("b"));
        assert_eq!(read(&body, "task.labels").as_deref(), Some("bug,v0.6.4"), "a list joined");
        assert_eq!(read(&body, "labels").as_deref(), Some("x,y"), "objects by their name");
        assert_eq!((read(&body, "task.n").as_deref(), read(&body, "task.done").as_deref()), (Some("3"), Some("false")));
        assert_eq!(read(&body, "obj"), None, "an object is nothing");
        assert_eq!(read(&body, "task.empty"), None);
        assert_eq!(read(&body, "task.nope || task.empty || /task/key").as_deref(), Some("MC-12"), "the first not empty");
        assert_eq!(read(&body, "PR#{{task.n}}").as_deref(), Some("PR#3"));
        assert_eq!(read(&body, "PR#{{pr.number}} || issue#{{task.n}}").as_deref(), Some("issue#3"), "a template of nothing is empty");
        assert_eq!(read(&body, "{{task.key}} {{task.nope}}").as_deref(), Some("MC-12"), "some of it found");
        assert_eq!(read(&body, "{{/task/key}}").as_deref(), Some("MC-12"));
    }

    #[test]
    fn a_type_keeps_its_text_and_reads_only_its_placeholders() {
        let body = json!({"action": "Opened", "event": "task.stage_changed"});
        let extra: BTreeMap<String, String> = [("webhook.event".to_string(), "pull_request".to_string())].into();
        let source = Source { body: &body, extra: &extra };
        assert_eq!(source.read_kind("deploy.done").as_deref(), Some("deploy.done"), "plain text is a name");
        assert_eq!(source.read_kind("github.{{webhook.event}}.{{action}}").as_deref(), Some("github.pull_request.opened"));
        assert_eq!(source.read_kind("mightywork.{{event}}").as_deref(), Some("mightywork.task.stage_changed"));
        assert_eq!(source.read_kind("x.{{nope}} || fallback").as_deref(), Some("fallback"));
        assert_eq!(normalize_kind(" Deploy..Done. ").as_deref(), Some("deploy.done"));
        assert_eq!(normalize_kind("a b/c").as_deref(), Some("a_b_c"));
        assert_eq!(normalize_kind("1st"), None);
        assert_eq!(normalize_kind(&"a".repeat(100)).map(|k| k.len()), Some(MAX_KIND_CHARS));
    }

    #[test]
    fn a_map_checks_its_expressions_and_field_names() {
        assert!(WebhookPreset::ALL.iter().all(|p| p.map().validate().is_ok()), "the presets are valid maps");
        let bad = |map: WebhookMap| map.validate().is_err();
        assert!(bad(WebhookMap { subject: Some(" ".into()), ..WebhookMap::default() }));
        assert!(bad(WebhookMap { title: Some("a || ".into()), ..WebhookMap::default() }));
        assert!(bad(WebhookMap { kind: Some("x.{{a".into()), ..WebhookMap::default() }));
        assert!(bad(WebhookMap { url: Some("x".repeat(MAX_EXPR_CHARS + 1)), ..WebhookMap::default() }));
        assert!(bad(WebhookMap { fields: [("event.type".to_string(), "a".to_string())].into(), ..WebhookMap::default() }));
        assert!(bad(WebhookMap { fields: [("a b".to_string(), "a".to_string())].into(), ..WebhookMap::default() }));
        assert_eq!(WebhookPreset::parse(" GitHub "), Some(WebhookPreset::Github));
        let map: WebhookMap = serde_json::from_value(json!({"type": "deploy.{{env}}", "fields": {"env": "env"}})).unwrap();
        assert_eq!(map.kind.as_deref(), Some("deploy.{{env}}"));
        assert_eq!(serde_json::to_value(&map).unwrap(), json!({"type": "deploy.{{env}}", "fields": {"env": "env"}}));
        assert!(WebhookMap::default().is_empty() && serde_json::to_value(WebhookMap::default()).unwrap() == json!({}));
    }

    /// The generic preset is the old webhook: `webhook.received`, nothing read but what a map adds.
    #[test]
    fn a_generic_webhook_is_received_and_its_own_map_reads_the_body() {
        let body = json!({"env": "prod", "who": "kim", "link": "https://x.dev/1", "bad": "javascript:alert(1)"});
        let plain = apply(WebhookPreset::Generic, &WebhookMap::default(), &body, Some("deploy"));
        assert_eq!(plain, Mapped { kind: WEBHOOK_KIND.into(), ..Mapped::default() });
        let map = WebhookMap {
            kind: Some("deploy.{{env}}".into()),
            subject: Some("env".into()),
            url: Some("link".into()),
            actor: Some("who".into()),
            fields: [("env".to_string(), "env".to_string()), ("hook".to_string(), "{{webhook.event}}".to_string())].into(),
            ..WebhookMap::default()
        };
        let mapped = apply(WebhookPreset::Generic, &map, &body, Some("deploy"));
        assert_eq!(mapped.kind, "deploy.prod");
        assert_eq!(
            (mapped.subject.as_deref(), mapped.actor.as_deref(), mapped.url.as_deref()),
            (Some("prod"), Some("kim"), Some("https://x.dev/1"))
        );
        assert_eq!(mapped.fields.get("hook").map(String::as_str), Some("deploy"), "the event header as webhook.event");
        let unsafe_url = apply(WebhookPreset::Generic, &WebhookMap { url: Some("bad".into()), ..WebhookMap::default() }, &body, None);
        assert_eq!(unsafe_url.url, None, "an http(s) link only");
    }

    #[test]
    fn the_github_preset_reads_the_event_header_and_action() {
        let body = json!({"action": "opened", "number": 7, "sender": {"login": "kim"}, "repository": {"full_name": "a/b"},
            "pull_request": {"number": 7, "title": "Add login", "html_url": "https://github.com/a/b/pull/7",
                "user": {"login": "kim"}, "base": {"ref": "main"}, "labels": [{"name": "bot:dev"}]}});
        let mapped = apply(WebhookPreset::Github, &WebhookMap::default(), &body, Some("pull_request"));
        assert_eq!(mapped.kind, "github.pull_request.opened");
        assert_eq!((mapped.subject.as_deref(), mapped.title.as_deref()), (Some("PR#7"), Some("Add login")));
        assert_eq!(mapped.url.as_deref(), Some("https://github.com/a/b/pull/7"));
        assert_eq!(mapped.actor.as_deref(), Some("kim"));
        let field = |name: &str| mapped.fields.get(name).map(String::as_str);
        assert_eq!(
            (field("base"), field("labels"), field("author"), field("repo")),
            (Some("main"), Some("bot:dev"), Some("kim"), Some("a/b"))
        );
        let issue = json!({"action": "labeled", "issue": {"number": 9, "title": "Crash", "labels": ["bug"]}, "sender": {"login": "lee"}});
        let mapped = apply(WebhookPreset::Github, &WebhookMap::default(), &issue, Some("issues"));
        assert_eq!((mapped.kind.as_str(), mapped.subject.as_deref()), ("github.issues.labeled", Some("issue#9")));
        let push = json!({"ref": "refs/heads/main", "compare": "https://github.com/a/b/compare/x", "head_commit": {"message": "Fix"}});
        let mapped = apply(WebhookPreset::Github, &WebhookMap::default(), &push, Some("push"));
        assert_eq!((mapped.kind.as_str(), mapped.subject.as_deref()), ("github.push", Some("refs/heads/main")), "no action");
        // A map of its own goes over the preset's.
        let mine = WebhookMap { kind: Some("ci.{{action}}".into()), ..WebhookMap::default() };
        assert_eq!(apply(WebhookPreset::Github, &mine, &body, Some("pull_request")).kind, "ci.opened");
    }

    #[test]
    fn the_mightywork_preset_reads_a_task_change() {
        let body = json!({"event": "task.stage_changed", "actor": "kim", "task": {"key": "MC-12", "title": "Fix the cache",
            "stage": "review", "previous_stage": "in_progress", "labels": ["bug", "v0.6.4"], "url": "https://w.dev/t/MC-12",
            "version": "v0.6.4"}});
        let mapped = apply(WebhookPreset::Mightywork, &WebhookMap::default(), &body, None);
        assert_eq!(mapped.kind, "mightywork.task.stage_changed");
        assert_eq!((mapped.subject.as_deref(), mapped.title.as_deref()), (Some("MC-12"), Some("MC-12 Fix the cache")));
        assert_eq!((mapped.url.as_deref(), mapped.actor.as_deref()), (Some("https://w.dev/t/MC-12"), Some("kim")));
        let field = |name: &str| mapped.fields.get(name).map(String::as_str);
        assert_eq!((field("task.to"), field("task.from"), field("task.stage")), (Some("review"), Some("in_progress"), Some("review")));
        assert_eq!((field("task.labels"), field("task.version"), field("task.key")), (Some("bug,v0.6.4"), Some("v0.6.4"), Some("MC-12")));
        let created = json!({"event": "task.created", "actor": {"name": "lee"}, "task": {"key": "MC-13"}});
        let mapped = apply(WebhookPreset::Mightywork, &WebhookMap::default(), &created, None);
        assert_eq!((mapped.kind.as_str(), mapped.actor.as_deref()), ("mightywork.task.created", Some("lee")));
        assert!(!mapped.fields.contains_key("task.from"), "a value the body lacks is left out");
        assert_eq!(apply(WebhookPreset::Mightywork, &WebhookMap::default(), &json!({}), None).kind, WEBHOOK_KIND, "no event");
    }
}

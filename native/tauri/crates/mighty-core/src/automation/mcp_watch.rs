//! MC-98: an MCP polling source: every few minutes a tool of one of the person's MCP servers is called, the list of items in its
//! result (`item.list`, a JSON pointer such as `/tasks`) is read by each item's key (`item.key`), and compared with the last look:
//!
//! - a key not seen before: `<prefix>.created`;
//! - a watched field (`item.watch`) of a seen item that changed: `<prefix>.<field>_changed`, with `<name>.from` and `<name>.to`;
//! - a watched list field (labels, tags) that gained a value: `<prefix>.labeled`, one per value, the value as `label`; one that lost
//!   a value: `<prefix>.unlabeled`, the same way. MC-99: a list field's change is only these (no `_changed` beside them, so a rule on
//!   a label added fires once), `_changed` is a scalar field's.
//!
//! An item gone from the list is kept (so its return is not new) and says nothing; an item without its key is passed over. The first
//! look only learns what is there. `prefix` is `mcp.item` unless set; each event comes from `mcp:<server>`, about the item's key,
//! with the item's values as `<name>.*` fields (`task.key`, `task.stage`; `item.*` unless `item.name` says otherwise).
//!
//! The app knows no server: a preset ([`preset`]) is data a form or `.mighty.yaml` fills a source with.
//!
//! MC-99: polling only reads. A source calls a tool whose name says it reads (`list_`, `get_`, `search_`, `read_`, `query_`,
//! `find_`, `fetch_`) unless `allowWrite` says otherwise; a tool its server marks as changing things (`readOnlyHint: false`) is
//! refused at the look as well ([`read_only`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::mapping::{item_text, lookup, text_of, valid_field_name, web_url};
use super::watch::{MAX_FIELD_CHARS, TriggerEvent};

/// How often a tool is called: every 2 minutes at most, every 5 by default, once a day at least.
pub const MIN_MCP_MINUTES: u32 = 2;
pub const DEFAULT_MCP_MINUTES: u32 = 5;
pub const MAX_MCP_MINUTES: u32 = 1_440;
/// The type prefix when `item.prefix` is unset, and the field prefix when `item.name` is.
pub const DEFAULT_PREFIX: &str = "mcp.item";
pub const DEFAULT_NAME: &str = "item";
/// The most items a source remembers.
pub const MAX_ITEMS: usize = 2_000;
/// The most watched fields.
pub const MAX_WATCH: usize = 16;
/// The largest `args`, as JSON.
pub const MAX_ARGS_BYTES: usize = 4_096;
/// The most fields an item gives an event.
const MAX_ITEM_FIELDS: usize = 64;

fn default_key() -> String {
    "key".into()
}

/// Which items of a tool's result a source reads, and what it watches of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpItem {
    /// Where the list is in the result (a JSON pointer `/tasks` or a dotted path); empty: the result is the list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub list: String,
    /// The path of an item's key in it (`key`, `id`).
    #[serde(default = "default_key")]
    pub key: String,
    /// The prefix of its fields (`task`: `task.stage`); `item` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The fields whose change is an event (`stage`, `tags`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
    /// The events' type prefix (`mightywork.task`); `mcp.item` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// The paths of an item's title and link; `title` and `url` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Default for McpItem {
    fn default() -> McpItem {
        McpItem { list: String::new(), key: default_key(), name: None, watch: vec![], prefix: None, title: None, url: None }
    }
}

/// A path's text: 1 to 200 characters, none a control character or a space.
fn valid_path(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty() && path.chars().count() <= 200 && !path.chars().any(|c| c.is_control() || c.is_whitespace())
}

impl McpItem {
    pub fn name(&self) -> &str {
        self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(DEFAULT_NAME)
    }

    pub fn prefix(&self) -> &str {
        self.prefix.as_deref().map(str::trim).filter(|p| !p.is_empty()).unwrap_or(DEFAULT_PREFIX)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.list.trim().is_empty() && !valid_path(&self.list) {
            return Err(format!("'{}' is not a path to the list (item.list: /tasks)", self.list));
        }
        if !valid_path(&self.key) {
            return Err("item.key names the path of an item's key (key, id)".into());
        }
        if !valid_field_name(self.name()) {
            return Err(format!("'{}' is not a field prefix (item.name): letters, digits, . _ -", self.name()));
        }
        if !super::spec::valid_event_name(self.prefix(), false) {
            return Err(format!("'{}' is not an event type prefix (item.prefix): lowercase letters, digits, . _ -", self.prefix()));
        }
        if self.watch.len() > MAX_WATCH || self.watch.iter().any(|w| !valid_path(w)) {
            return Err(format!("item.watch names at most {MAX_WATCH} fields, each a path"));
        }
        for path in [&self.title, &self.url].into_iter().flatten() {
            if !valid_path(path) {
                return Err(format!("'{path}' is not a path in an item"));
            }
        }
        Ok(())
    }
}

/// MC-98: an MCP source's `server`, `tool` and `args` checked: a server's name as the MCP hub writes it, a tool's name, and `args` an
/// object (or nothing) of at most [`MAX_ARGS_BYTES`].
pub fn validate_call(server: &str, tool: &str, args: &Value) -> Result<(), String> {
    let name = |text: &str, max: usize| {
        let text = text.trim();
        !text.is_empty() && text.chars().count() <= max && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !name(server, 64) {
        return Err(format!("'{server}' is not an MCP server's name: letters, digits, - _ ."));
    }
    if !name(tool, 128) {
        return Err(format!("'{tool}' is not a tool's name: letters, digits, - _ ."));
    }
    if !(args.is_null() || args.is_object()) {
        return Err("args is an object of the tool's arguments".into());
    }
    if serde_json::to_string(args).map_or(true, |text| text.len() > MAX_ARGS_BYTES) {
        return Err(format!("args is longer than {MAX_ARGS_BYTES} bytes"));
    }
    Ok(())
}

/// The list of items in a tool's `result` at `list` (the result itself when empty); None when there is none there.
pub fn items_of<'a>(result: &'a Value, list: &str) -> Option<&'a Vec<Value>> {
    lookup(result, list)?.as_array()
}

/// A watched field's value as kept: a list as its items' texts, sorted and once each; a scalar as its text; `null` when missing.
fn watched(item: &Value, path: &str) -> Value {
    match lookup(item, path) {
        None | Some(Value::Null) => Value::Null,
        Some(Value::Array(items)) => {
            let mut texts: Vec<String> = items.iter().filter_map(item_text).collect();
            texts.sort();
            texts.dedup();
            Value::Array(texts.into_iter().map(Value::String).collect())
        }
        Some(other) => text_of(other).map_or(Value::Null, Value::String),
    }
}

/// MC-99: what a list field gained and lost, when it is a list now or was one (nothing counts as an empty list beside a list); None
/// when neither is a list (a scalar's change).
fn list_diff(was: &Value, now: &Value) -> Option<(Vec<String>, Vec<String>)> {
    let texts = |value: &Value| match value {
        Value::Array(items) => Some(items.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()),
        Value::Null => Some(vec![]),
        _ => None,
    };
    if !(was.is_array() || now.is_array()) {
        return None;
    }
    let (was, now) = (texts(was)?, texts(now)?);
    let added = now.iter().filter(|v| !was.contains(v)).cloned().collect();
    let removed = was.iter().filter(|v| !now.contains(v)).cloned().collect();
    Some((added, removed))
}

/// What changed of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Created,
    /// A watched field (its path), from and to (None: it had none).
    Changed {
        field: String,
        from: Option<String>,
        to: Option<String>,
    },
    /// A watched list field gained `label`.
    Labeled {
        field: String,
        label: String,
    },
    /// MC-99: a watched list field lost `label`.
    Unlabeled {
        field: String,
        label: String,
    },
}

/// One change a look found: the item's key, the item and what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub key: String,
    pub item: Value,
    pub change: Change,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SeenItem {
    /// Each watched field's value at the last look that had the item.
    #[serde(default)]
    values: BTreeMap<String, Value>,
    /// The look that last had it.
    #[serde(default)]
    last: u64,
}

/// What an MCP source keeps between looks: each item's watched values, by key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSnapshot {
    #[serde(default)]
    items: BTreeMap<String, SeenItem>,
    #[serde(default)]
    primed: bool,
    #[serde(default)]
    looks: u64,
}

impl McpSnapshot {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A look's `items` compared with the last: what changed, in the list's order. The first look learns and finds nothing; a field
    /// newly watched is learned the same way.
    pub fn look(&mut self, items: &[Value], config: &McpItem) -> Vec<Found> {
        let primed = self.primed;
        self.primed = true;
        self.looks += 1;
        let mut found = vec![];
        let mut this_look = std::collections::HashSet::new();
        for item in items {
            let Some(key) = lookup(item, &config.key).and_then(text_of).filter(|k| k.chars().count() <= 200) else { continue };
            if !this_look.insert(key.clone()) {
                continue;
            }
            let values: BTreeMap<String, Value> = config.watch.iter().map(|w| (w.trim().to_string(), watched(item, w))).collect();
            let before = self.items.insert(key.clone(), SeenItem { values: values.clone(), last: self.looks });
            if !primed {
                continue;
            }
            let Some(before) = before else {
                found.push(Found { key, item: item.clone(), change: Change::Created });
                continue;
            };
            for (field, now) in &values {
                let Some(was) = before.values.get(field) else { continue };
                if was == now {
                    continue;
                }
                // MC-99: a list (or a list beside nothing) changes by the values it gained and lost, one event each.
                if let Some((added, removed)) = list_diff(was, now) {
                    let change = |label: &String, gained: bool| Found {
                        key: key.clone(),
                        item: item.clone(),
                        change: if gained {
                            Change::Labeled { field: field.clone(), label: label.clone() }
                        } else {
                            Change::Unlabeled { field: field.clone(), label: label.clone() }
                        },
                    };
                    found.extend(added.iter().map(|label| change(label, true)));
                    found.extend(removed.iter().map(|label| change(label, false)));
                    continue;
                }
                found.push(Found {
                    key: key.clone(),
                    item: item.clone(),
                    change: Change::Changed { field: field.clone(), from: text_of(was), to: text_of(now) },
                });
            }
        }
        if self.items.len() > MAX_ITEMS {
            let mut ages: Vec<(u64, String)> = self.items.iter().map(|(k, s)| (s.last, k.clone())).collect();
            ages.sort();
            for (_, key) in ages.into_iter().take(self.items.len() - MAX_ITEMS) {
                self.items.remove(&key);
            }
        }
        found
    }
}

/// A field's path as part of a type: lowercase letters, digits and `_` (`status.name` → `status_name`).
fn type_part(field: &str) -> String {
    field.trim().to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' }).collect()
}

/// An item's values as `<name>.*`: its scalars and lists two levels deep (`task.stage`, `task.assignee.name`, `task.labels`
/// joined), at most 64.
pub fn item_fields(item: &Value, name: &str) -> BTreeMap<String, String> {
    fn walk(prefix: &str, value: &Value, depth: usize, out: &mut BTreeMap<String, String>) {
        if out.len() >= MAX_ITEM_FIELDS {
            return;
        }
        match value {
            Value::Object(map) if depth < 2 => {
                for (key, inner) in map {
                    if key.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-')) {
                        walk(&format!("{prefix}.{key}"), inner, depth + 1, out);
                    }
                }
            }
            Value::Object(_) | Value::Null => {}
            other => {
                if let Some(text) = text_of(other) {
                    out.insert(prefix.to_string(), text.chars().take(MAX_FIELD_CHARS).collect());
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(name, item, 0, &mut out);
    out
}

/// The event of one change an MCP source found on `server`.
pub fn event_of(server: &str, config: &McpItem, found: &Found) -> TriggerEvent {
    let name = config.name();
    let prefix = config.prefix();
    let mut fields = item_fields(&found.item, name);
    let (kind, suffix, id_tail) = match &found.change {
        Change::Created => (format!("{prefix}.created"), String::new(), String::new()),
        Change::Changed { field, from, to } => {
            let (from, to) = (from.clone().unwrap_or_default(), to.clone().unwrap_or_default());
            for (key, value) in [(format!("{name}.from"), &from), (format!("{name}.to"), &to)] {
                if value.is_empty() {
                    fields.remove(&key);
                } else {
                    fields.insert(key, value.chars().take(MAX_FIELD_CHARS).collect());
                }
            }
            fields.insert("changed".into(), field.clone());
            (format!("{prefix}.{}_changed", type_part(field)), format!(" ({field}: {} → {})", or_dash(&from), or_dash(&to)), to)
        }
        Change::Labeled { label, .. } => {
            fields.insert("label".into(), label.clone());
            (format!("{prefix}.labeled"), format!(" (+{label})"), label.clone())
        }
        Change::Unlabeled { label, .. } => {
            fields.insert("label".into(), label.clone());
            (format!("{prefix}.unlabeled"), format!(" (-{label})"), label.clone())
        }
    };
    let title_path = config.title.as_deref().unwrap_or("title");
    let url_path = config.url.as_deref().unwrap_or("url");
    let title = lookup(&found.item, title_path).and_then(text_of);
    let url = lookup(&found.item, url_path).and_then(text_of).and_then(|u| web_url(&u));
    let head = match title {
        Some(title) => format!("{}: {title}", found.key),
        None => found.key.clone(),
    };
    TriggerEvent {
        id: format!("mcp:{}:{kind}:{id_tail}", found.key),
        title: format!("{head}{suffix}").chars().take(500).collect(),
        url,
        fields,
        ..TriggerEvent::default()
    }
    .with_envelope(&format!("mcp:{server}"), &kind, &found.key, None)
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

/// MC-99: the starts of a tool's name that say it only reads.
pub const READ_ONLY_PREFIXES: [&str; 7] = ["list_", "get_", "search_", "read_", "query_", "find_", "fetch_"];

/// MC-99: whether a tool's name says it only reads: it (or its last `.`/`/` part) starts with one of [`READ_ONLY_PREFIXES`], any case.
pub fn looks_read_only(tool: &str) -> bool {
    let name = tool.trim().to_ascii_lowercase();
    let last = name.rsplit(['.', '/']).next().unwrap_or(&name).to_string();
    READ_ONLY_PREFIXES.iter().any(|p| name.starts_with(p) || last.starts_with(p))
}

/// MC-99: whether an MCP source may call `tool` on every look, or why not: `allow_write` lets any; else the server's `annotations`
/// (from `tools/list`, when known) decide when they say `readOnlyHint`, and the name ([`looks_read_only`]) when they do not.
pub fn read_only(tool: &str, allow_write: bool, annotations: Option<&Value>) -> Result<(), String> {
    if allow_write {
        return Ok(());
    }
    match annotations.and_then(|a| a.get("readOnlyHint")).and_then(Value::as_bool) {
        Some(true) => return Ok(()),
        Some(false) => {
            return Err(format!(
                "its server marks '{tool}' as a tool that changes things (readOnlyHint: false): an MCP polling trigger calls it on every look only with allowWrite: true"
            ));
        }
        None => {}
    }
    if looks_read_only(tool) {
        return Ok(());
    }
    Err(format!(
        "'{tool}' does not look read-only (its name starts with none of {}): an MCP polling trigger calls it on every look only with allowWrite: true",
        READ_ONLY_PREFIXES.join(", ")
    ))
}

/// A ready source: the server, tool, `args` and items a form or `.mighty.yaml` fills in when this preset is picked (data only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPreset {
    pub name: &'static str,
    /// How a picker shows it.
    pub label: &'static str,
    pub server: &'static str,
    pub tool: &'static str,
    pub args: Value,
    pub item: McpItem,
}

/// The presets: `mightywork` (a project's tasks from the MCP server named `mightywork`: `list_tasks`, `/tasks` by `key`, watching
/// `stage` and `tags`, as `mightywork.task.*`; `args.project` is the person's project key).
pub fn presets() -> Vec<McpPreset> {
    vec![McpPreset {
        name: "mightywork",
        label: "MightyWork",
        server: "mightywork",
        tool: "list_tasks",
        args: serde_json::json!({ "project": "", "limit": 100 }),
        item: McpItem {
            list: "/tasks".into(),
            key: "key".into(),
            name: Some("task".into()),
            watch: vec!["stage".into(), "tags".into()],
            prefix: Some("mightywork.task".into()),
            title: None,
            url: None,
        },
    }]
}

/// The preset named `name`, if any.
pub fn preset(name: &str) -> Option<McpPreset> {
    presets().into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tasks() -> McpItem {
        preset("mightywork").unwrap().item
    }

    fn task(key: &str, stage: &str, tags: &[&str]) -> Value {
        json!({"key": key, "title": format!("Task {key}"), "stage": stage, "tags": tags, "url": format!("https://w.dev/{key}")})
    }

    fn look(snapshot: &mut McpSnapshot, items: Vec<Value>) -> Vec<(String, Change)> {
        snapshot.look(&items, &tasks()).into_iter().map(|f| (f.key, f.change)).collect()
    }

    #[test]
    fn the_first_look_is_a_baseline_and_finds_nothing() {
        let mut snapshot = McpSnapshot::default();
        assert!(look(&mut snapshot, vec![task("MC-1", "todo", &[]), task("MC-2", "review", &["bug"])]).is_empty());
        assert_eq!(snapshot.len(), 2);
        assert!(look(&mut snapshot, vec![task("MC-1", "todo", &[]), task("MC-2", "review", &["bug"])]).is_empty(), "the same again");
    }

    #[test]
    fn a_new_key_is_created_and_a_watched_change_is_found() {
        let mut snapshot = McpSnapshot::default();
        look(&mut snapshot, vec![task("MC-1", "todo", &[])]);
        let found = look(&mut snapshot, vec![task("MC-1", "in_progress", &[]), task("MC-2", "backlog", &["bug"])]);
        assert_eq!(
            found,
            vec![
                ("MC-1".into(), Change::Changed { field: "stage".into(), from: Some("todo".into()), to: Some("in_progress".into()) }),
                ("MC-2".into(), Change::Created),
            ]
        );
        // A field not watched (the title) changes nothing.
        let mut renamed = task("MC-1", "in_progress", &[]);
        renamed["title"] = json!("Renamed");
        assert!(look(&mut snapshot, vec![renamed, task("MC-2", "backlog", &["bug"])]).is_empty());
    }

    /// MC-99: a list field's change is the values it gained (`labeled`) and lost (`unlabeled`), one each, never a `tags_changed`
    /// beside them; the same values in another order are no change; a scalar field's change stays `_changed`.
    #[test]
    fn a_label_added_is_labeled_once_and_a_label_removed_unlabeled() {
        let mut snapshot = McpSnapshot::default();
        look(&mut snapshot, vec![task("MC-1", "todo", &["bug"])]);
        let found = look(&mut snapshot, vec![task("MC-1", "todo", &["v0.6.4", "bug", "bot:dev"])]);
        assert_eq!(
            found,
            vec![
                ("MC-1".into(), Change::Labeled { field: "tags".into(), label: "bot:dev".into() }),
                ("MC-1".into(), Change::Labeled { field: "tags".into(), label: "v0.6.4".into() }),
            ]
        );
        let found = look(&mut snapshot, vec![task("MC-1", "todo", &["v0.6.4", "qa"])]);
        assert_eq!(
            found,
            vec![
                ("MC-1".into(), Change::Labeled { field: "tags".into(), label: "qa".into() }),
                ("MC-1".into(), Change::Unlabeled { field: "tags".into(), label: "bot:dev".into() }),
                ("MC-1".into(), Change::Unlabeled { field: "tags".into(), label: "bug".into() }),
            ]
        );
        assert!(look(&mut snapshot, vec![task("MC-1", "todo", &["qa", "v0.6.4"])]).is_empty(), "another order is no change");
        // A stage move and a label at once: the scalar's `_changed` and the label's own, nothing else.
        let found = look(&mut snapshot, vec![task("MC-1", "review", &["qa", "v0.6.4", "bot:dev"])]);
        assert_eq!(
            found,
            vec![
                ("MC-1".into(), Change::Changed { field: "stage".into(), from: Some("todo".into()), to: Some("review".into()) }),
                ("MC-1".into(), Change::Labeled { field: "tags".into(), label: "bot:dev".into() }),
            ]
        );
        // A list from nothing: each value is labeled.
        let mut none = task("MC-1", "review", &[]);
        none["tags"] = Value::Null;
        look(&mut snapshot, vec![none]);
        let found = look(&mut snapshot, vec![task("MC-1", "review", &["a"])]);
        assert_eq!(found, vec![("MC-1".into(), Change::Labeled { field: "tags".into(), label: "a".into() })]);
    }

    #[test]
    fn a_removed_item_says_nothing_and_its_return_is_not_new() {
        let mut snapshot = McpSnapshot::default();
        look(&mut snapshot, vec![task("MC-1", "todo", &[]), task("MC-2", "todo", &[])]);
        assert!(look(&mut snapshot, vec![task("MC-2", "todo", &[])]).is_empty(), "MC-1 left the list");
        assert!(look(&mut snapshot, vec![task("MC-1", "todo", &[]), task("MC-2", "todo", &[])]).is_empty(), "back: not created");
        let found = look(&mut snapshot, vec![task("MC-1", "done", &[])]);
        assert_eq!(found.len(), 1, "back and moved: its change since it was last seen");
    }

    #[test]
    fn an_item_without_its_key_is_passed_over_and_a_key_twice_counts_once() {
        let mut snapshot = McpSnapshot::default();
        look(&mut snapshot, vec![task("MC-1", "todo", &[])]);
        let keyless = json!({"title": "no key", "stage": "todo"});
        let found = look(&mut snapshot, vec![keyless.clone(), json!({"key": null}), task("MC-3", "todo", &[]), task("MC-3", "done", &[])]);
        assert_eq!(found, vec![("MC-3".into(), Change::Created)]);
        assert_eq!(snapshot.len(), 2, "nothing kept for the keyless");
        assert_eq!(snapshot.look(&[json!(7), json!("x")], &tasks()), vec![], "not objects");
    }

    #[test]
    fn a_field_newly_watched_is_learned_first() {
        let mut snapshot = McpSnapshot::default();
        let only_stage = McpItem { watch: vec!["stage".into()], ..tasks() };
        snapshot.look(&[task("MC-1", "todo", &["a"])], &only_stage);
        snapshot.look(&[task("MC-1", "todo", &["a"])], &only_stage);
        assert!(look(&mut snapshot, vec![task("MC-1", "todo", &["a", "b"])]).is_empty(), "tags not seen before: a baseline");
        assert_eq!(look(&mut snapshot, vec![task("MC-1", "todo", &["a", "b", "c"])]).len(), 1, "c labeled");
    }

    #[test]
    fn the_snapshot_forgets_the_items_seen_longest_ago() {
        let mut snapshot = McpSnapshot::default();
        let many: Vec<Value> = (0..MAX_ITEMS).map(|i| task(&format!("A-{i}"), "todo", &[])).collect();
        look(&mut snapshot, many);
        look(&mut snapshot, vec![task("B-1", "todo", &[])]);
        assert_eq!(snapshot.len(), MAX_ITEMS);
        let text = serde_json::to_string(&snapshot).unwrap();
        let mut back: McpSnapshot = serde_json::from_str(&text).unwrap();
        assert!(look(&mut back, vec![task("B-1", "todo", &[])]).is_empty(), "kept across a save");
    }

    #[test]
    fn a_result_s_list_is_found_at_its_pointer() {
        let result = json!({"tasks": [task("MC-1", "todo", &[])], "total": 1});
        assert_eq!(items_of(&result, "/tasks").map(Vec::len), Some(1));
        assert_eq!(items_of(&result, "tasks").map(Vec::len), Some(1));
        assert_eq!(items_of(&json!([1, 2]), "").map(Vec::len), Some(2), "the result itself");
        assert!(items_of(&result, "/total").is_none() && items_of(&result, "/nope").is_none());
    }

    #[test]
    fn each_change_makes_its_event() {
        let item = task("MC-12", "review", &["bug", "v0.6.4"]);
        let event = |change: Change| event_of("mightywork", &tasks(), &Found { key: "MC-12".into(), item: item.clone(), change });
        let created = event(Change::Created);
        assert_eq!(
            (created.source.as_str(), created.kind.as_str(), created.subject.as_str()),
            ("mcp:mightywork", "mightywork.task.created", "MC-12")
        );
        assert_eq!(created.title, "MC-12: Task MC-12");
        assert_eq!(created.url.as_deref(), Some("https://w.dev/MC-12"));
        let field = |e: &TriggerEvent, name: &str| e.fields.get(name).cloned();
        assert_eq!((field(&created, "task.key"), field(&created, "task.tags")), (Some("MC-12".into()), Some("bug,v0.6.4".into())));
        assert_eq!(field(&created, "event.type").as_deref(), Some("mightywork.task.created"));
        let moved = event(Change::Changed { field: "stage".into(), from: Some("in_progress".into()), to: Some("review".into()) });
        assert_eq!(moved.kind, "mightywork.task.stage_changed");
        assert_eq!((field(&moved, "task.from"), field(&moved, "task.to")), (Some("in_progress".into()), Some("review".into())));
        assert_eq!(moved.title, "MC-12: Task MC-12 (stage: in_progress → review)");
        assert_eq!(moved.id, "mcp:MC-12:mightywork.task.stage_changed:review");
        let labeled = event(Change::Labeled { field: "tags".into(), label: "v0.6.4".into() });
        assert_eq!((labeled.kind.as_str(), field(&labeled, "label").as_deref()), ("mightywork.task.labeled", Some("v0.6.4")));
        let unlabeled = event(Change::Unlabeled { field: "tags".into(), label: "bug".into() });
        assert_eq!((unlabeled.kind.as_str(), field(&unlabeled, "label").as_deref()), ("mightywork.task.unlabeled", Some("bug")));
        assert_eq!(unlabeled.title, "MC-12: Task MC-12 (-bug)");
        // Unset prefix and name: `mcp.item.*` and `item.*`; a field with a dot in it as `_`.
        let plain = McpItem { watch: vec!["status.name".into()], ..McpItem::default() };
        let found = Found {
            key: "7".into(),
            item: json!({"id": 7, "status": {"name": "Open"}, "url": "javascript:x"}),
            change: Change::Changed { field: "status.name".into(), from: None, to: Some("Open".into()) },
        };
        let e = event_of("tracker", &plain, &found);
        assert_eq!((e.kind.as_str(), e.url.as_deref()), ("mcp.item.status_name_changed", None));
        assert_eq!((field(&e, "item.status.name"), field(&e, "item.from")), (Some("Open".into()), None));
    }

    #[test]
    fn a_source_checks_its_call_and_items() {
        assert!(validate_call("mightywork", "list_tasks", &json!({"project": "MC"})).is_ok());
        assert!(validate_call("mighty work", "list_tasks", &Value::Null).is_err());
        assert!(validate_call("a", "", &Value::Null).is_err());
        assert!(validate_call("a", "t", &json!([1])).is_err());
        assert!(validate_call("a", "t", &json!({"x": "y".repeat(MAX_ARGS_BYTES)})).is_err());
        assert!(tasks().validate().is_ok() && McpItem::default().validate().is_ok());
        assert!(McpItem { prefix: Some("Bad Prefix".into()), ..McpItem::default() }.validate().is_err());
        assert!(McpItem { key: " ".into(), ..McpItem::default() }.validate().is_err());
        assert!(McpItem { watch: vec!["a b".into()], ..McpItem::default() }.validate().is_err());
        assert!(McpItem { name: Some("event.x".into()), ..McpItem::default() }.validate().is_err());
        assert_eq!(serde_json::to_value(McpItem::default()).unwrap(), json!({"key": "key"}));
        let read: McpItem = serde_json::from_value(json!({"list": "/tasks"})).unwrap();
        assert_eq!(read.key, "key");
    }

    /// MC-99: a polled tool reads: its name says so, or its server's annotations do; anything else only with allowWrite, and a tool
    /// the server marks as writing is refused whatever its name.
    #[test]
    fn a_polled_tool_must_read_unless_allow_write() {
        for good in ["list_tasks", "get_task", "search_issues", "read_file", "query_db", "find_x", "fetch_y", "GET_Task", "mw.list_tasks"] {
            assert!(looks_read_only(good), "{good}");
            assert!(read_only(good, false, None).is_ok(), "{good}");
        }
        for bad in ["update_task", "create_task", "delete", "add_comment", "listing", "tasks", "search"] {
            let why = read_only(bad, false, None).unwrap_err();
            assert!(why.contains("allowWrite") && why.contains(bad), "{why}");
            assert!(read_only(bad, true, None).is_ok(), "{bad} with allowWrite");
        }
        assert!(read_only("tasks", false, Some(&json!({"readOnlyHint": true}))).is_ok(), "its server says it reads");
        let marked = read_only("get_or_create", false, Some(&json!({"readOnlyHint": false}))).unwrap_err();
        assert!(marked.contains("readOnlyHint"), "{marked}");
        assert!(read_only("get_or_create", true, Some(&json!({"readOnlyHint": false}))).is_ok());
        assert!(read_only("list_tasks", false, Some(&json!({"title": "x"}))).is_ok(), "no hint: the name decides");
    }
}

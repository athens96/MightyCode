//! MC-97: a trigger's `where`: conditions over the event that reached it, every one of which must hold for the trigger to fire.
//!
//! A condition names a field (one of the event's `fields`, or the envelope's `type`, `subject`, `source` and `actor`), an op and a
//! value. The ops are `eq` (the default), `in` (a list), `has` (a comma- or space-separated field such as `labels` holds the value)
//! and `matches` (a regular expression), each with its `not_` form; every comparison ignores case. A field the event does not carry
//! is seen as nothing: `eq`, `in`, `has` and `matches` fail on it and their `not_` forms hold. MC-98: a condition marked `ifPresent`
//! holds when the event does not carry its field at all (a GitHub trigger's old `author` and `base` filter took such an event).
//!
//! The same shape comes from settings (JSON) and from bot specs: `{ "field": "labels", "op": "has", "value": "bot:dev" }`, or the
//! compact `{ "labels": { "has": "bot:dev" } }` and `{ "base": "main" }` (MC-98: `{ "base": { "eq": "main", "if_present": true } }`,
//! `ifPresent` in the long form). A failed check says which condition failed and the value
//! it saw ([`Miss`]), for the event inbox.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::watch::TriggerEvent;

/// The most conditions one trigger has.
pub const MAX_CONDITIONS: usize = 16;
/// The longest value (and regular expression) a condition compares with.
pub const MAX_VALUE_CHARS: usize = 200;
/// The most values an `in` list has.
pub const MAX_LIST: usize = 32;
/// The largest compiled regular expression, in bytes.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// How a condition compares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Op {
    #[default]
    Eq,
    In,
    Has,
    Matches,
    NotEq,
    NotIn,
    NotHas,
    NotMatches,
}

impl Op {
    pub const ALL: [Op; 8] = [Op::Eq, Op::In, Op::Has, Op::Matches, Op::NotEq, Op::NotIn, Op::NotHas, Op::NotMatches];

    pub fn name(self) -> &'static str {
        match self {
            Op::Eq => "eq",
            Op::In => "in",
            Op::Has => "has",
            Op::Matches => "matches",
            Op::NotEq => "not_eq",
            Op::NotIn => "not_in",
            Op::NotHas => "not_has",
            Op::NotMatches => "not_matches",
        }
    }

    pub fn parse(text: &str) -> Option<Op> {
        let text = text.trim().to_ascii_lowercase();
        Op::ALL.into_iter().find(|op| op.name() == text)
    }

    /// Whether it is a `not_` form.
    pub fn negated(self) -> bool {
        matches!(self, Op::NotEq | Op::NotIn | Op::NotHas | Op::NotMatches)
    }

    /// Its form without `not_`.
    pub fn positive(self) -> Op {
        match self {
            Op::NotEq => Op::Eq,
            Op::NotIn => Op::In,
            Op::NotHas => Op::Has,
            Op::NotMatches => Op::Matches,
            other => other,
        }
    }

    /// Whether it compares with a list (`in`, `not_in`); the others take one value.
    pub fn takes_list(self) -> bool {
        self.positive() == Op::In
    }
}

/// One condition: `field op value`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Condition {
    pub field: String,
    pub op: Op,
    /// One value, or the list of `in` / `not_in`.
    pub values: Vec<String>,
    /// MC-98: an event without the field passes (`ifPresent`); otherwise a missing field is seen as nothing.
    pub if_present: bool,
}

/// Why an event did not pass a trigger's `where`: the condition that failed and the value the event had for its field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Miss {
    pub field: String,
    pub op: String,
    /// The value (an `in` list joined with `, `).
    pub expected: String,
    /// None when the event did not carry the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A list field's items: split at commas when it has any (`bug,good first issue`), else at spaces.
fn items(text: &str) -> Vec<&str> {
    let parts: Vec<&str> = if text.contains(',') { text.split(',').collect() } else { text.split_whitespace().collect() };
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

fn regex_of(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern).case_insensitive(true).size_limit(REGEX_SIZE_LIMIT).build()
}

/// A field's name: 1 to 64 letters, digits, `.`, `_` and `-`.
fn valid_field(field: &str) -> bool {
    !field.is_empty() && field.chars().count() <= 64 && field.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Condition {
    /// `field op value` with one value.
    pub fn new(field: &str, op: Op, value: &str) -> Condition {
        Condition { field: field.trim().to_string(), op, values: vec![value.trim().to_string()], if_present: false }
    }

    /// MC-98: the condition holding as well for an event that does not carry its field.
    pub fn if_present(mut self) -> Condition {
        self.if_present = true;
        self
    }

    /// Why it cannot be checked: an odd field name, no value, a list where one value goes (or the other way), a value too long, a
    /// regular expression that does not compile.
    pub fn validate(&self) -> Result<(), String> {
        let field = self.field.trim();
        if !valid_field(field) {
            return Err(format!("'{}' is not a field name: letters, digits, . _ -", self.field));
        }
        let op = self.op.name();
        if self.values.is_empty() || self.values.iter().any(|v| v.trim().is_empty()) {
            return Err(format!("the condition on '{field}' ({op}) has no value"));
        }
        if self.op.takes_list() && self.values.len() > MAX_LIST {
            return Err(format!("the condition on '{field}' lists more than {MAX_LIST} values"));
        }
        if !self.op.takes_list() && self.values.len() > 1 {
            return Err(format!("the condition on '{field}' ({op}) takes one value: use in for a list"));
        }
        if self.values.iter().any(|v| v.chars().count() > MAX_VALUE_CHARS || v.chars().any(char::is_control)) {
            return Err(format!(
                "the condition on '{field}' has a value longer than {MAX_VALUE_CHARS} characters or with control characters"
            ));
        }
        if self.op.positive() == Op::Matches
            && let Err(error) = regex_of(&self.values[0])
        {
            let first = error.to_string().lines().last().unwrap_or("").trim().to_string();
            return Err(format!("the condition on '{field}' is not a regular expression: {first}"));
        }
        Ok(())
    }

    /// Whether `event` passes; when not, the miss.
    pub fn check(&self, event: &TriggerEvent) -> Result<(), Miss> {
        let seen = event.value(self.field.trim());
        if seen.is_none() && self.if_present {
            return Ok(());
        }
        let held = match (self.op.positive(), seen) {
            (_, None) => false,
            (Op::Matches, Some(seen)) => regex_of(&self.values[0]).is_ok_and(|r| r.is_match(seen)),
            (Op::Has, Some(seen)) => items(seen).into_iter().any(|item| self.values.iter().any(|v| same(item, v))),
            (_, Some(seen)) => self.values.iter().any(|v| same(seen, v)),
        };
        if held != self.op.negated() {
            return Ok(());
        }
        Err(Miss {
            field: self.field.trim().to_string(),
            op: self.op.name().to_string(),
            expected: self.values.join(", "),
            seen: seen.map(|s| s.chars().take(MAX_VALUE_CHARS).collect()),
        })
    }

    /// The condition in a few words: `base=main`, `author!=dependabot[bot]`, `labels has bot:dev`, `type in [a, b]`; MC-98: `?` after
    /// the field of an `ifPresent` one (`base?=main`).
    pub fn words(&self) -> String {
        let value = if self.op.takes_list() { format!("[{}]", self.values.join(", ")) } else { self.values.join(", ") };
        let field = if self.if_present { format!("{}?", self.field) } else { self.field.clone() };
        match self.op {
            Op::Eq => format!("{field}={value}"),
            Op::NotEq => format!("{field}!={value}"),
            op => format!("{field} {} {value}", op.name()),
        }
    }
}

/// Why a `where` cannot be checked: too many conditions, or one of them.
pub fn validate_all(conditions: &[Condition]) -> Result<(), String> {
    if conditions.len() > MAX_CONDITIONS {
        return Err(format!("at most {MAX_CONDITIONS} conditions (where)"));
    }
    conditions.iter().try_for_each(Condition::validate)
}

/// The first condition `event` does not pass, if any.
pub fn first_miss(conditions: &[Condition], event: &TriggerEvent) -> Option<Miss> {
    conditions.iter().find_map(|c| c.check(event).err())
}

/// A `where` in a few words, its conditions joined with ` · `.
pub fn words(conditions: &[Condition]) -> String {
    conditions.iter().map(Condition::words).collect::<Vec<_>>().join(" · ")
}

impl Serialize for Condition {
    /// Always the long form: `{field, op, value}`, `value` a list for `in` and `not_in`; MC-98: `ifPresent: true` when set.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = if self.op.takes_list() {
            Value::from(self.values.clone())
        } else {
            Value::from(self.values.first().cloned().unwrap_or_default())
        };
        let mut long = serde_json::json!({ "field": self.field, "op": self.op.name(), "value": value });
        if self.if_present {
            long["ifPresent"] = Value::Bool(true);
        }
        long.serialize(serializer)
    }
}

/// A scalar or a list as condition values (numbers and booleans as written).
fn values_of(value: &Value) -> Result<Vec<String>, String> {
    let one = |v: &Value| match v {
        Value::String(text) => Ok(text.trim().to_string()),
        Value::Number(_) | Value::Bool(_) => Ok(v.to_string()),
        _ => Err("a condition's value is a string, a number or a list of them".to_string()),
    };
    match value {
        Value::Array(items) => items.iter().map(one).collect(),
        other => Ok(vec![one(other)?]),
    }
}

/// MC-98: the key that marks a condition `ifPresent` (`if_present` too, as YAML writes it).
fn is_if_present_key(key: &str) -> bool {
    matches!(key, "ifPresent" | "if_present")
}

/// MC-98: an `ifPresent` flag's value: a boolean, or `true` / `false` / `yes` / `no` as text (YAML's scalars).
fn flag_of(value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(on) => Ok(*on),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Ok(true),
            "false" | "no" | "off" | "" => Ok(false),
            _ => Err(format!("ifPresent is true or false, not '{text}'")),
        },
        Value::Null => Ok(false),
        _ => Err("ifPresent is true or false".into()),
    }
}

/// A condition from JSON: the long form `{field, op?, value, ifPresent?}` or the compact `{field: value}` / `{field: {op: value,
/// if_present?}}`. A bare list is `in`.
pub fn condition_from(value: &Value) -> Result<Condition, String> {
    let Value::Object(map) = value else { return Err("a condition is an object: {field, op, value}".into()) };
    let mut if_present = false;
    let (field, op, raw) = if let Some(field) = map.get("field") {
        if let Some(unknown) = map.keys().find(|k| !matches!(k.as_str(), "field" | "op" | "value") && !is_if_present_key(k)) {
            return Err(format!("a condition has no '{unknown}': use field, op, value, ifPresent"));
        }
        if let Some(flag) = map.iter().find(|(k, _)| is_if_present_key(k)).map(|(_, v)| v) {
            if_present = flag_of(flag)?;
        }
        let field = field.as_str().ok_or("a condition's field is a string")?.trim().to_string();
        let op = match map.get("op") {
            None | Some(Value::Null) => None,
            Some(Value::String(op)) => Some(Op::parse(op).ok_or_else(|| unknown_op(op))?),
            Some(_) => return Err("a condition's op is a string".into()),
        };
        (field, op, map.get("value").cloned().unwrap_or(Value::Null))
    } else {
        let mut entries = map.iter();
        let (Some((field, inner)), None) = (entries.next(), entries.next()) else {
            return Err("a compact condition names one field: {labels: {has: bot:dev}}".into());
        };
        match inner {
            Value::Object(ops) => {
                if let Some(flag) = ops.iter().find(|(k, _)| is_if_present_key(k)).map(|(_, v)| v) {
                    if_present = flag_of(flag)?;
                }
                let mut ops = ops.iter().filter(|(k, _)| !is_if_present_key(k));
                let (Some((op, raw)), None) = (ops.next(), ops.next()) else {
                    return Err(format!("the condition on '{field}' names one op: {{{field}: {{has: value}}}}"));
                };
                (field.trim().to_string(), Some(Op::parse(op).ok_or_else(|| unknown_op(op))?), raw.clone())
            }
            other => (field.trim().to_string(), None, other.clone()),
        }
    };
    if raw.is_null() {
        return Err(format!("the condition on '{field}' has no value"));
    }
    let values = values_of(&raw)?;
    let op = op.unwrap_or(if raw.is_array() { Op::In } else { Op::Eq });
    Ok(Condition { field, op, values, if_present })
}

fn unknown_op(op: &str) -> String {
    let all = Op::ALL.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ");
    format!("unknown op '{op}': use {all}")
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Condition, D::Error> {
        let value = Value::deserialize(deserializer)?;
        condition_from(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(fields: &[(&str, &str)]) -> TriggerEvent {
        TriggerEvent {
            id: "pr:7".into(),
            title: "PR #7".into(),
            fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..TriggerEvent::default()
        }
        .with_envelope("github:a/b", "github.pr.opened", "PR#7", Some("kim"))
    }

    fn parse(json: &str) -> Result<Condition, String> {
        condition_from(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn each_op_and_its_not_form_compare_ignoring_case() {
        let e = event(&[("base", "Main"), ("labels", "bug,Bot:Dev,good first issue"), ("pr.title", "Fix the cache")]);
        let holds = |field: &str, op: Op, values: &[&str]| {
            Condition { field: field.into(), op, values: values.iter().map(|v| v.to_string()).collect(), if_present: false }
                .check(&e)
                .is_ok()
        };
        assert!(holds("base", Op::Eq, &["main"]) && !holds("base", Op::NotEq, &["MAIN"]));
        assert!(holds("base", Op::In, &["dev", "main"]) && !holds("base", Op::NotIn, &["dev", "main"]));
        assert!(holds("labels", Op::Has, &["bot:dev"]) && holds("labels", Op::Has, &["good first issue"]));
        assert!(!holds("labels", Op::Has, &["good"]), "a whole item, not a part");
        assert!(holds("labels", Op::NotHas, &["wontfix"]));
        assert!(holds("pr.title", Op::Matches, &["^fix\\b"]) && !holds("pr.title", Op::NotMatches, &["CACHE"]));
        // The envelope's keys.
        assert!(holds("type", Op::Eq, &["github.pr.opened"]) && holds("subject", Op::Eq, &["pr#7"]));
        assert!(holds("source", Op::Matches, &["^github:"]) && holds("actor", Op::Eq, &["Kim"]));
        // A space-separated list.
        let spaced = event(&[("labels", "a b")]);
        assert!(Condition::new("labels", Op::Has, "b").check(&spaced).is_ok());
    }

    #[test]
    fn a_missing_field_fails_and_its_not_form_holds_and_a_miss_says_what_was_seen() {
        let e = event(&[("author", "dependabot[bot]")]);
        let miss = Condition::new("base", Op::Eq, "main").check(&e).unwrap_err();
        assert_eq!(miss, Miss { field: "base".into(), op: "eq".into(), expected: "main".into(), seen: None });
        assert!(Condition::new("base", Op::NotEq, "main").check(&e).is_ok());
        let conditions = vec![Condition::new("base", Op::NotEq, "dev"), Condition::new("author", Op::NotEq, "dependabot[bot]")];
        let miss = first_miss(&conditions, &e).unwrap();
        assert_eq!((miss.field.as_str(), miss.op.as_str(), miss.seen.as_deref()), ("author", "not_eq", Some("dependabot[bot]")));
        let list = Condition { field: "type".into(), op: Op::In, values: vec!["a".into(), "b".into()], if_present: false };
        assert_eq!(list.check(&e).unwrap_err().expected, "a, b");
        assert!(first_miss(&[], &e).is_none(), "no condition, every event");
    }

    #[test]
    fn conditions_read_long_and_compact_and_write_long() {
        let long = parse(r#"{"field":"labels","op":"has","value":"bot:dev"}"#).unwrap();
        assert_eq!(long, Condition::new("labels", Op::Has, "bot:dev"));
        assert_eq!(parse(r#"{"labels":{"has":"bot:dev"}}"#).unwrap(), long);
        assert_eq!(parse(r#"{"base":"main"}"#).unwrap(), Condition::new("base", Op::Eq, "main"));
        assert_eq!(parse(r#"{"field":"base","value":"main"}"#).unwrap().op, Op::Eq, "eq by default");
        let listed = parse(r#"{"author":["kim","lee"]}"#).unwrap();
        assert_eq!((listed.op, listed.values.len()), (Op::In, 2), "a bare list is in");
        assert_eq!(parse(r#"{"pr.number":{"not_eq":5}}"#).unwrap().values, vec!["5".to_string()]);
        for bad in [
            r#"{"field":"a","op":"near","value":"x"}"#,
            r#"{"a":1,"b":2}"#,
            r#"{"field":"a"}"#,
            r#"{"field":"a","value":"x","x":1}"#,
            "[1]",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_value(&long).unwrap(), serde_json::json!({"field": "labels", "op": "has", "value": "bot:dev"}));
        assert_eq!(serde_json::to_value(&listed).unwrap()["value"], serde_json::json!(["kim", "lee"]));
        let back: Vec<Condition> = serde_json::from_value(serde_json::to_value(vec![&long, &listed]).unwrap()).unwrap();
        assert_eq!(back, vec![long, listed]);
    }

    #[test]
    fn a_where_checks_its_ops_values_regexes_and_count() {
        assert!(validate_all(&[Condition::new("labels", Op::Has, "bot")]).is_ok());
        assert!(Condition::new("a b", Op::Eq, "x").validate().unwrap_err().contains("field name"));
        assert!(Condition::new("a", Op::Eq, " ").validate().unwrap_err().contains("no value"));
        assert!(Condition::new("a", Op::Matches, "(").validate().unwrap_err().contains("regular expression"));
        assert!(Condition::new("a", Op::Eq, &"x".repeat(MAX_VALUE_CHARS + 1)).validate().is_err());
        let two = Condition { field: "a".into(), op: Op::Eq, values: vec!["x".into(), "y".into()], if_present: false };
        assert!(two.validate().unwrap_err().contains("use in"));
        let many = vec![Condition::new("a", Op::Eq, "x"); MAX_CONDITIONS + 1];
        assert!(validate_all(&many).unwrap_err().contains("at most"));
        assert_eq!(
            words(&[
                Condition::new("base", Op::Eq, "main"),
                Condition::new("author", Op::NotEq, "bot"),
                Condition::new("labels", Op::Has, "x")
            ]),
            "base=main · author!=bot · labels has x"
        );
    }

    /// MC-98: an `ifPresent` condition holds for an event without its field, and checks one that has it; it reads in the long and
    /// compact forms (`if_present` as YAML writes it) and writes `ifPresent` only when set.
    #[test]
    fn an_if_present_condition_passes_a_missing_field() {
        let without = event(&[("author", "kim")]);
        let with = event(&[("base", "dev")]);
        let base = Condition::new("base", Op::Eq, "main").if_present();
        assert!(base.check(&without).is_ok(), "no base: it holds");
        let miss = base.check(&with).unwrap_err();
        assert_eq!((miss.field.as_str(), miss.seen.as_deref()), ("base", Some("dev")), "a base it has is still checked");
        assert!(Condition::new("base", Op::Eq, "main").check(&without).is_err(), "unmarked, a missing field fails as before");
        assert!(Condition::new("base", Op::NotEq, "main").if_present().check(&without).is_ok());
        assert_eq!(base.words(), "base?=main");
        assert_eq!(parse(r#"{"base":{"eq":"main","if_present":true}}"#).unwrap(), base);
        assert_eq!(parse(r#"{"base":{"if_present":"yes","eq":"main"}}"#).unwrap(), base, "YAML's text, in any order");
        assert_eq!(parse(r#"{"field":"base","value":"main","ifPresent":true}"#).unwrap(), base);
        assert_eq!(parse(r#"{"field":"base","value":"main","ifPresent":false}"#).unwrap(), Condition::new("base", Op::Eq, "main"));
        assert!(parse(r#"{"base":{"if_present":true}}"#).is_err(), "a flag is not an op");
        assert!(parse(r#"{"field":"base","value":"main","ifPresent":"maybe"}"#).is_err());
        let json = serde_json::to_value(&base).unwrap();
        assert_eq!(json, serde_json::json!({"field": "base", "op": "eq", "value": "main", "ifPresent": true}));
        assert!(serde_json::to_value(Condition::new("a", Op::Eq, "x")).unwrap().get("ifPresent").is_none());
        assert_eq!(serde_json::from_value::<Condition>(json).unwrap(), base);
    }
}

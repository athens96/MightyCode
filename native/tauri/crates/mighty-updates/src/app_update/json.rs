//! JSON read as C#'s `JsonDocument.Parse` with its defaults reads it, for the manifest (AppUpdateManifest.cs): strict RFC 8259 (no
//! comments, no trailing commas), at most 64 levels deep, every member of an object kept in order so a duplicate name is settled as
//! `JsonElement.TryGetProperty` settles it (the last one wins; `update-manifests` parity corpus), numbers kept as written so
//! `TryGetInt32`/`TryGetInt64` read them as .NET does (an integer literal that fits; `-0` is 0), and the exceptions .NET throws when
//! an element of the wrong kind is asked for kept word for word.
//!
//! serde_json validates the text (and its UTF-8) and hands back each value's raw text; a string with a lone surrogate escape, which
//! .NET keeps in its UTF-16 string, cannot be a Rust string and refuses the whole document.

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

/// `JsonReaderOptions.MaxDepth`'s default.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The literal as written.
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// Every member, in order, duplicates included.
    Object(Vec<(String, Json)>),
}

/// `JsonValueKind`'s name, as .NET's messages say it.
fn kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "Null",
        Json::Bool(true) => "True",
        Json::Bool(false) => "False",
        Json::Number(_) => "Number",
        Json::String(_) => "String",
        Json::Array(_) => "Array",
        Json::Object(_) => "Object",
    }
}

/// .NET's `InvalidOperationException` text for an element of the wrong kind (`JsonElement.GetString`, `TryGetInt32`, …).
pub fn wrong_kind(expected: &str, actual: &Json) -> String {
    format!("The requested operation requires an element of type '{expected}', but the target element has type '{}'.", kind(actual))
}

impl Json {
    /// `TryGetProperty(name, out value)`: the last member with that name.
    pub fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().rev().find(|(key, _)| key == name).map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Object(_))
    }

    /// `GetString()`: the text, None for a JSON null, .NET's exception text for anything else.
    pub fn get_string(&self) -> Result<Option<&str>, String> {
        match self {
            Json::String(text) => Ok(Some(text)),
            Json::Null => Ok(None),
            other => Err(wrong_kind("String", other)),
        }
    }

    /// C#'s `TryGetString(el, name)`: the member's text when it is a string, else None.
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Json::String(text)) => Some(text),
            _ => None,
        }
    }

    /// `TryGetInt64(out value)`: Some for an integer literal that fits, None for another number, .NET's exception text for an
    /// element that is not a number.
    pub fn try_i64(&self) -> Result<Option<i64>, String> {
        match self {
            Json::Number(text) => Ok(integer(text).and_then(|value| i64::try_from(value).ok())),
            other => Err(wrong_kind("Number", other)),
        }
    }

    /// `TryGetInt32(out value)`.
    pub fn try_i32(&self) -> Result<Option<i32>, String> {
        match self {
            Json::Number(text) => Ok(integer(text).and_then(|value| i32::try_from(value).ok())),
            other => Err(wrong_kind("Number", other)),
        }
    }
}

/// `Utf8Parser.TryParse(literal, out int, out consumed) && consumed == literal.Length`: an optional `-` then digits, nothing else.
fn integer(literal: &str) -> Option<i128> {
    let digits = literal.strip_prefix('-').unwrap_or(literal);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    super::dotnet::parse_integer(literal)
}

/// The deepest nesting of arrays and objects, strings skipped (the text is valid JSON by the time the answer matters).
fn depth(bytes: &[u8]) -> usize {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// `JsonDocument.Parse(bytes)`: the document, or None where .NET throws a `JsonException`.
pub fn parse(bytes: &[u8]) -> Option<Json> {
    if depth(bytes) > MAX_DEPTH {
        return None;
    }
    let raw: &RawValue = serde_json::from_slice(bytes).ok()?;
    convert(raw.get())
}

struct Members<'a>(Vec<(String, &'a RawValue)>);

impl<'de: 'a, 'a> Deserialize<'de> for Members<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Each<'a>(std::marker::PhantomData<&'a ()>);
        impl<'de: 'a, 'a> Visitor<'de> for Each<'a> {
            type Value = Members<'a>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Members<'a>, M::Error> {
                let mut members = Vec::new();
                while let Some((name, value)) = map.next_entry::<String, &'de RawValue>()? {
                    members.push((name, value));
                }
                Ok(Members(members))
            }
        }
        deserializer.deserialize_map(Each(std::marker::PhantomData))
    }
}

fn convert(text: &str) -> Option<Json> {
    let text = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    Some(match text.as_bytes().first()? {
        b'{' => {
            let members: Members = serde_json::from_str(text).ok()?;
            Json::Object(members.0.into_iter().map(|(name, value)| Some((name, convert(value.get())?))).collect::<Option<_>>()?)
        }
        b'[' => {
            let items: Vec<&RawValue> = serde_json::from_str(text).ok()?;
            Json::Array(items.into_iter().map(|item| convert(item.get())).collect::<Option<_>>()?)
        }
        b'"' => Json::String(serde_json::from_str(text).ok()?),
        b't' => Json::Bool(true),
        b'f' => Json::Bool(false),
        b'n' => Json::Null,
        _ => Json::Number(text.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_keep_their_order_and_duplicates() {
        let doc = parse(br#"{"a":1,"b":"x","a":"two","n":-0,"f":1.0,"big":2147483648}"#).unwrap();
        assert_eq!(doc.get("a"), Some(&Json::String("two".into())), "the last duplicate wins");
        assert_eq!(doc.get("n").unwrap().try_i32(), Ok(Some(0)));
        assert_eq!(doc.get("f").unwrap().try_i32(), Ok(None));
        assert_eq!(doc.get("big").unwrap().try_i32(), Ok(None));
        assert_eq!(doc.get("big").unwrap().try_i64(), Ok(Some(2_147_483_648)));
        assert!(doc.get("b").unwrap().try_i64().unwrap_err().contains("'Number', but the target element has type 'String'"));
    }

    #[test]
    fn the_document_is_read_as_strictly_as_dotnet_reads_it() {
        assert!(parse(b"{\"a\":1,}").is_none(), "a trailing comma");
        assert!(parse(b"{\"a\":1 // c\n}").is_none(), "a comment");
        assert!(parse(b"{} x").is_none(), "text after the value");
        let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(nested(64).as_bytes()).is_some(), "64 levels");
        assert!(parse(nested(65).as_bytes()).is_none(), "65 levels");
        assert_eq!(
            parse(b" [1, \"a\\u00e9\", null, true] ").unwrap(),
            Json::Array(vec![Json::Number("1".into()), Json::String("a\u{e9}".into()), Json::Null, Json::Bool(true)])
        );
    }
}

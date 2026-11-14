//! `DelegationJson` (C# Core/DelegationFileStore.cs:215-268; Mac-identical): the delegation file's own writer, not `Wire.Json` and not
//! serde's: camelCase, nulls omitted, keys sorted ordinal at every level, no whitespace, `\/` escaped, other code points below 0x20 as
//! lowercase `\u00xx`, non-ASCII raw; `EscapedBytes` (the 64 KiB copy and 48 KiB follow-up limits are measured in it). Owner: W7-A.
//!
//! A record goes through serde's derive to a [`Value`] (camelCase names, absent values left out, as C#'s `SerializeToNode` with the
//! app's wire options gives them) and is then written here, as C# writes its node.

use serde::Serialize;
use serde_json::Value;

/// `DelegationJson.Encode`: the exact bytes a save writes. A part of the file encodes to the same bytes it takes inside the file.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut text = String::new();
    // Every record here serializes to JSON (strings, integers, booleans, objects and arrays only): nothing can fail.
    write(&serde_json::to_value(value).unwrap_or(Value::Null), &mut text);
    text.into_bytes()
}

fn write(node: &Value, text: &mut String) {
    match node {
        Value::Object(fields) => {
            text.push('{');
            // Ordinal: C# compares UTF-16 code units; every key here is ASCII, where that is byte order.
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                escape(key, text);
                text.push(':');
                write(&fields[key], text);
            }
            text.push('}');
        }
        Value::Array(items) => {
            text.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                write(item, text);
            }
            text.push(']');
        }
        Value::String(value) => escape(value, text),
        Value::Null => text.push_str("null"),
        Value::Bool(value) => text.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => text.push_str(&number.to_string()),
    }
}

/// `DelegationJson.Escaped`: the escape of one character, or None when it is written as it is.
fn escaped(character: char) -> Option<&'static str> {
    const CONTROL: [&str; 32] = [
        "\\u0000", "\\u0001", "\\u0002", "\\u0003", "\\u0004", "\\u0005", "\\u0006", "\\u0007", "\\b", "\\t", "\\n", "\\u000b", "\\f",
        "\\r", "\\u000e", "\\u000f", "\\u0010", "\\u0011", "\\u0012", "\\u0013", "\\u0014", "\\u0015", "\\u0016", "\\u0017", "\\u0018",
        "\\u0019", "\\u001a", "\\u001b", "\\u001c", "\\u001d", "\\u001e", "\\u001f",
    ];
    match character {
        '"' => Some("\\\""),
        '\\' => Some("\\\\"),
        '/' => Some("\\/"),
        c if (c as u32) < 0x20 => Some(CONTROL[c as usize]),
        _ => None,
    }
}

fn escape(value: &str, text: &mut String) {
    text.push('"');
    for character in value.chars() {
        match escaped(character) {
            Some(escape) => text.push_str(escape),
            None => text.push(character),
        }
    }
    text.push('"');
}

/// `DelegationJson.EscapedBytes`: the bytes `value` takes between its quotes.
pub fn escaped_bytes(value: &str) -> usize {
    value.len() + value.chars().filter_map(escaped).map(|escape| escape.len() - 1).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Keys sorted at every level, slashes and control characters escaped, text outside ASCII raw, no whitespace.
    #[test]
    fn writes_as_the_mac_encoder_writes() {
        let value = json!({"b": [1, {"z": true, "a": null}], "a": "x/\"\\\u{8}\u{c}\n\r\t\u{1}\u{1b}한"});
        assert_eq!(
            String::from_utf8(encode(&value)).unwrap(),
            "{\"a\":\"x\\/\\\"\\\\\\b\\f\\n\\r\\t\\u0001\\u001b한\",\"b\":[1,{\"a\":null,\"z\":true}]}"
        );
        assert_eq!(escaped_bytes("a\"b\\c/d\ne\u{1b}f한"), 1 + 2 + 1 + 2 + 1 + 2 + 1 + 2 + 1 + 6 + 1 + 3);
        assert_eq!(String::from_utf8(encode(&i64::MAX)).unwrap(), "9223372036854775807");
    }
}

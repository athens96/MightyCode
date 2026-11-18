//! The `screen-control` data channel's typed input (C# `ScreenControlWire`, R\ScreenSharePolicy.cs) and the JSON the hub writes for
//! the E2EE routes (`state`, `control-key`, `sessions`) and the `screen-*` signalling frames (R\ScreenShareHub.cs). Every text here is
//! written with `JsonOut` in C#'s property order, nulls left out, as `Wire.Json` writes it. Owner: W8-F.
//!
//! One frame is bounded and explicitly typed: no OS virtual key codes, arbitrary clipboard encodings or non-finite coordinates cross
//! it.

use mighty_core::json::{self, JsonOut};
use mighty_core::wire::utf16_len;
use serde_json::Value;

use super::policy::{ScreenDisplay, ScreenRegion, id_chars, number};

/// `ScreenControlWire.MaximumBytes`: one frame's UTF-8 size.
pub const MAXIMUM_BYTES: usize = 65536;

/// `ScreenControlWire.Parse`: the frame when it is one of `tap|drag|scroll|text|key|display|zoom|clipboard-request|clipboard` with
/// its members in bounds; none otherwise (too large, not JSON, deeper than 64, an unknown type or a member out of bounds).
pub fn parse(text: &str) -> Option<Value> {
    if text.len() > MAXIMUM_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(text).ok()?;
    if json::depth(&value) > json::MAXIMUM_DEPTH {
        return None;
    }
    let point = || display_id(&value).is_some() && number(&value, "x").is_some() && number(&value, "y").is_some();
    let valid = match json::text(&value, "t") {
        Some("tap") => {
            point()
                && matches!(json::text(&value, "button"), Some("left" | "right"))
                && (!has(&value, "marker") || json::text(&value, "marker").is_some_and(|marker| id_chars(marker, 1, 32)))
        }
        Some("drag") => point() && matches!(json::text(&value, "phase"), Some("begin" | "move" | "end")),
        Some("scroll") => point() && number(&value, "dx").is_some() && number(&value, "dy").is_some(),
        Some("text") => json::text(&value, "text").is_some_and(|input| !input.is_empty() && !input.contains('\0') && input.len() <= 4096),
        Some("key") => json::text(&value, "combo").is_some_and(key_combo),
        Some("display") => display_id(&value).is_some(),
        Some("zoom") => display_id(&value).is_some() && value.get("region").and_then(ScreenRegion::parse).is_some(),
        Some("clipboard-request") => true,
        Some("clipboard") => {
            json::text(&value, "dir") == Some("to-mac")
                && json::text(&value, "id").is_some_and(|id| (1..=64).contains(&utf16_len(id)))
                && matches!(json::text(&value, "enc"), Some("raw" | "zstd"))
                && integer(&value, "bytes", 1, 1_048_576)
                && integer(&value, "seq", 0, 63)
                && integer(&value, "total", 1, 64)
                && json::text(&value, "data").is_some()
        }
        _ => false,
    };
    valid.then_some(value)
}

fn has(value: &Value, key: &str) -> bool {
    value.get(key).is_some()
}

/// `TryGetProperty(key) && ValueKind == Number && TryGetInt32`.
pub(crate) fn int32(value: &Value, key: &str) -> Option<i32> {
    value.get(key).and_then(json::as_integer).and_then(|n| i32::try_from(n).ok())
}

fn display_id(value: &Value) -> Option<i32> {
    int32(value, "displayId")
}

fn integer(value: &Value, key: &str, min: i32, max: i32) -> bool {
    int32(value, key).is_some_and(|n| (min..=max).contains(&n))
}

/// `ScreenControlWire.KeyCombo`: up to four distinct modifiers (`ctrl`, `opt`, `shift`, `cmd`) then one key: a lower-case letter, a
/// digit, or a named key.
pub fn key_combo(combo: &str) -> bool {
    let keys: Vec<&str> = combo.split('+').collect();
    if !(1..=5).contains(&keys.len()) {
        return false;
    }
    let (key, modifiers) = keys.split_last().expect("one key at least");
    for (at, modifier) in modifiers.iter().enumerate() {
        if modifiers[..at].contains(modifier) || !matches!(*modifier, "ctrl" | "opt" | "shift" | "cmd") {
            return false;
        }
    }
    key.len() == 1 && matches!(key.as_bytes()[0], b'a'..=b'z' | b'0'..=b'9')
        || matches!(
            *key,
            "return"
                | "tab"
                | "space"
                | "backspace"
                | "delete"
                | "escape"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
        )
}

/// C# `ScreenIceServer(Urls, Username, Credential)`: a STUN or TURN server for the peer. The credential never reaches `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenIceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

impl std::fmt::Debug for ScreenIceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenIceServer").field("urls", &self.urls).field("username", &self.username).finish_non_exhaustive()
    }
}

/// `iceServers`: `[{urls, username?, credential?}]`.
pub(crate) fn write_ice(out: &mut JsonOut, servers: &[ScreenIceServer]) {
    out.begin_array();
    for server in servers {
        out.begin_object();
        out.prop_strings("urls", Some(&server.urls));
        out.prop_str("username", server.username.as_deref());
        out.prop_str("credential", server.credential.as_deref());
        out.end_object();
    }
    out.end_array();
}

/// `DisplaysWire`: `[{displayId, width, height, main}]`.
pub(crate) fn write_displays(out: &mut JsonOut, displays: &[ScreenDisplay]) {
    out.begin_array();
    for display in displays {
        out.begin_object();
        out.prop_int("displayId", display.display_id.into());
        out.prop_int("width", display.width.into());
        out.prop_int("height", display.height.into());
        out.prop_bool("main", display.main);
        out.end_object();
    }
    out.end_array();
}

/// `{error:{reason}}`: a route's refusal.
pub(crate) fn error(reason: &str) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.name("error");
    out.begin_object();
    out.prop_str("reason", Some(reason));
    out.end_object();
    out.end_object();
    out.finish()
}

/// `{type:"screen-grant", allowed, grant, displays, iceServers}`: what a phone may do and see now.
pub(crate) fn grant(allowed: bool, grant: &str, displays: &[ScreenDisplay], ice: &[ScreenIceServer]) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("type", Some("screen-grant"));
    out.prop_bool("allowed", allowed);
    out.prop_str("grant", Some(grant));
    out.name("displays");
    write_displays(&mut out, displays);
    out.name("iceServers");
    write_ice(&mut out, ice);
    out.end_object();
    out.finish()
}

/// `{type:"screen-kill"|"screen-session-end", sessionId, reason}`.
pub(crate) fn session_end(killed: bool, session_id: &str, reason: &str) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("type", Some(if killed { "screen-kill" } else { "screen-session-end" }));
    out.prop_str("sessionId", Some(session_id));
    out.prop_str("reason", Some(reason));
    out.end_object();
    out.finish()
}

/// `{t:"marker", id, shown}`: whether the tap marker was drawn.
pub(crate) fn marker(id: &str, shown: bool) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("t", Some("marker"));
    out.prop_str("id", Some(id));
    out.prop_bool("shown", shown);
    out.end_object();
    out.finish()
}

/// `{fingerprint}`: the enrolled control key's.
pub(crate) fn fingerprint(fingerprint: &str) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("fingerprint", Some(fingerprint));
    out.end_object();
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_typed_and_bounded() {
        assert!(parse(r#"{"t":"tap","displayId":1,"x":0.5,"y":0.5,"button":"left","marker":"m-1"}"#).is_some());
        assert!(parse(r#"{"t":"tap","displayId":1,"x":0.5,"y":0.5,"button":"left","marker":"m 1"}"#).is_none());
        assert!(parse(r#"{"t":"tap","displayId":1,"x":0.5,"y":0.5,"button":"left","marker":7}"#).is_none());
        assert!(parse(r#"{"t":"tap","displayId":1.5,"x":0.5,"y":0.5,"button":"left"}"#).is_none());
        assert!(parse(r#"{"t":"drag","displayId":1,"x":0,"y":0,"phase":"end"}"#).is_some());
        assert!(parse(r#"{"t":"zoom","displayId":1,"region":{"x":0,"y":0,"width":0.01,"height":1}}"#).is_none());
        assert!(parse(r#"{"t":"clipboard","dir":"to-mac","id":"a","enc":"raw","bytes":1,"seq":0,"total":1,"data":""}"#).is_some());
        assert!(parse(r#"{"t":"clipboard","dir":"to-phone","id":"a","enc":"raw","bytes":1,"seq":0,"total":1,"data":""}"#).is_none());
        assert!(parse(r#"{"t":"clipboard-request"}"#).is_some() && parse("[1]").is_none() && parse("{").is_none());
        let deep = format!(r#"{{"t":"clipboard-request","x":{}{}}}"#, "[".repeat(64), "]".repeat(64));
        assert!(parse(&deep).is_none(), "deeper than JsonDocument reads");
        assert!(key_combo("ctrl+shift+a") && key_combo("pagedown") && !key_combo("ctrl+A") && !key_combo("a+b") && !key_combo(""));
    }

    #[test]
    fn replies_are_wire_json() {
        assert_eq!(error("bad-request"), r#"{"error":{"reason":"bad-request"}}"#);
        assert_eq!(session_end(true, "s", "revoked"), r#"{"type":"screen-kill","sessionId":"s","reason":"revoked"}"#);
        let ice = [ScreenIceServer { urls: vec!["turn:x".into()], username: Some("u".into()), credential: Some("SECRET".into()) }];
        assert_eq!(
            grant(true, "view", &[ScreenDisplay::new(1, 1920, 1080, true)], &ice),
            r#"{"type":"screen-grant","allowed":true,"grant":"view","displays":[{"displayId":1,"width":1920,"height":1080,"main":true}],"iceServers":[{"urls":["turn:x"],"username":"u","credential":"SECRET"}]}"#
        );
        assert!(!format!("{:?}", ice[0]).contains("SECRET"));
    }
}

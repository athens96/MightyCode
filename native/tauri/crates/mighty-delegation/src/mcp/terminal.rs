//! `mighty-terminal`'s server (C# Core/AgentIO/AgentIOMcp.cs; AgentIOBinding.cs:120-139 `AgentIOTools`): `run_in_terminal`,
//! `read_latest_output`, `stop`, `open_url`, their verbatim descriptions and `Guidance`. Owner: W7-C.
//!
//! Its answers are `Wire.Json` (`JavaScriptEncoder.Default`: non-ASCII and `"&'+<>` and the backtick written `\uXXXX`); an error keeps
//! `"id":null`. Its words differ from the delegation server's on purpose (C#'s): "Parse error.", "Invalid request.", "Invalid request
//! identifier.", "Method not found.", "Unknown tool.", "Message exceeds 1 MiB.".

use std::sync::Arc;

use mighty_core::json::JsonOut;
use mighty_platform::BoxFuture;
use serde_json::Value;

use super::serve::{self, Handle};
use crate::agent_io::client::CallFailed;
use crate::agent_io::tools::{self, ALL, GUIDANCE};
use crate::agent_io::wire::{Lenient, MAXIMUM_BYTES, Unreadable, raw_member};

/// `AgentIOMcp`'s transport: one tool call (the tool and its validated arguments) forwarded to the app on the pane's token; the app's
/// answer packet as its JSON text, or Err when the call could not be made or answered.
pub type TerminalTransport = Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<String, CallFailed>> + Send + Sync>;

/// The answer when the call threw (the transport failed, the app's answer was not what it should be).
pub const REFUSED_TEXT: &str = "The tool request was refused or its pane is unavailable.";

/// `AgentIOMcp`.
#[derive(Clone)]
pub struct TerminalServer {
    transport: TerminalTransport,
}

/// An answer that never reaches stdout: C# throws (a string it cannot decode, read outside a `try`), and the line is not answered.
struct Thrown;

impl From<Unreadable> for Thrown {
    fn from(_: Unreadable) -> Self {
        Thrown
    }
}

impl TerminalServer {
    pub fn new(transport: TerminalTransport) -> TerminalServer {
        TerminalServer { transport }
    }

    /// The loop's handler (`Serve(input, output, Handle, …)`).
    pub fn handler(&self) -> Handle {
        let server = self.clone();
        Arc::new(move |line: String| {
            let server = server.clone();
            Box::pin(async move { server.handle(&line).await })
        })
    }

    /// `Handle(line)`: the answer line, or None (a notification, a blank line, a line C# throws on).
    pub async fn handle(&self, line: &str) -> Option<String> {
        if line.chars().all(char::is_whitespace) {
            return None;
        }
        self.answer(line).await.unwrap_or_default()
    }

    async fn answer(&self, line: &str) -> Result<Option<String>, Thrown> {
        if line.len() > MAXIMUM_BYTES {
            return Ok(Some(error("null", -32700, "Message exceeds 1 MiB.")));
        }
        let Some(document) = Lenient::parse(line) else { return Ok(Some(error("null", -32700, "Parse error."))) };
        let request = &document.value;
        let Value::Object(map) = request else { return Ok(Some(error("null", -32600, "Invalid request."))) };
        // Read in C#'s order: the method only once the version is right.
        if document.text(map.get("jsonrpc"))? != Some("2.0") {
            return Ok(Some(error("null", -32600, "Invalid request.")));
        }
        let Some(method) = document.text(map.get("method"))? else { return Ok(Some(error("null", -32600, "Invalid request."))) };
        let Some(request_id) = map.get("id") else { return Ok(None) };
        let id = match request_id {
            Value::String(text) => {
                let mut out = JsonOut::new();
                out.str(document.read(text)?);
                out.finish()
            }
            Value::Number(number) => raw_member(line, "id").unwrap_or_else(|| number.to_string()),
            _ => return Ok(Some(error("null", -32600, "Invalid request identifier."))),
        };
        let empty = Value::Object(Default::default());
        let args = match map.get("params") {
            Some(parameters @ Value::Object(_)) => parameters,
            _ => &empty,
        };
        Ok(Some(match method {
            "initialize" => {
                let version = serve::protocol_version(document.text(args.get("protocolVersion"))?);
                result(&id, |out| {
                    out.begin_object();
                    out.prop_str("protocolVersion", Some(version));
                    out.name("capabilities");
                    out.begin_object();
                    out.name("tools");
                    out.begin_object();
                    out.prop_bool("listChanged", false);
                    out.end_object();
                    out.end_object();
                    out.name("serverInfo");
                    out.begin_object();
                    out.prop_str("name", Some(crate::agent_io::binding::SERVER_NAME));
                    out.prop_str("title", Some("Mighty Code terminal and web"));
                    out.prop_str("version", Some("1.0.0"));
                    out.end_object();
                    out.prop_str("instructions", Some(GUIDANCE));
                    out.end_object();
                })
            }
            "ping" => result(&id, |out| {
                out.begin_object();
                out.end_object();
            }),
            "tools/list" => result(&id, |out| {
                out.begin_object();
                out.name("tools");
                out.begin_array();
                for tool in ALL {
                    out.begin_object();
                    out.prop_str("name", Some(tool.name));
                    out.prop_str("description", Some(tool.description));
                    out.name("inputSchema");
                    out.begin_object();
                    out.prop_str("type", Some("object"));
                    out.name("properties");
                    out.begin_object();
                    if tool.name == tools::BROWSER_CHECK {
                        // MC-48: both optional.
                        out.name("url");
                        out.begin_object();
                        out.prop_str("type", Some("string"));
                        out.end_object();
                        out.name("paths");
                        out.begin_object();
                        out.prop_str("type", Some("array"));
                        out.name("items");
                        out.begin_object();
                        out.prop_str("type", Some("string"));
                        out.end_object();
                        out.end_object();
                        out.end_object();
                    } else if tool.name == tools::CREATE_BOT || tool.name == tools::UPDATE_BOT {
                        // MC-65: the bot's spec, an object.
                        out.name("spec");
                        out.begin_object();
                        out.prop_str("type", Some("object"));
                        out.end_object();
                        out.end_object();
                        out.name("required");
                        out.strings(&["spec".to_string()]);
                    } else if tool.name == tools::LIST_BOTS {
                        // MC-58: no arguments.
                        out.end_object();
                    } else if tool.name == tools::ASK_BOT || tool.name == tools::BOT_ANSWER {
                        // MC-58: the bot's @handle and its request, or whether to wait for its answer.
                        let second = if tool.name == tools::ASK_BOT { ("prompt", "string") } else { ("wait", "boolean") };
                        for (name, kind) in [("bot", "string"), second] {
                            out.name(name);
                            out.begin_object();
                            out.prop_str("type", Some(kind));
                            out.end_object();
                        }
                        out.end_object();
                        out.name("required");
                        let required: &[String] =
                            if tool.name == tools::ASK_BOT { &["bot".to_string(), "prompt".to_string()] } else { &["bot".to_string()] };
                        out.strings(required);
                    } else {
                        out.name(tool.argument);
                        out.begin_object();
                        out.prop_str("type", Some("string"));
                        out.end_object();
                        out.end_object();
                        out.name("required");
                        out.strings(&[tool.argument.to_string()]);
                    }
                    out.prop_bool("additionalProperties", false);
                    out.end_object();
                    out.end_object();
                }
                out.end_array();
                out.end_object();
            }),
            "tools/call" => {
                let name = document.text(args.get("name"))?.unwrap_or("");
                if tools::named(name).is_none() {
                    return Ok(Some(error(&id, -32602, "Unknown tool.")));
                }
                self.call(&document, name, args, &id).await
            }
            _ => error(&id, -32601, "Method not found."),
        }))
    }

    /// The `tools/call` of a known tool: validated, forwarded, answered; anything that throws is the one refusal text.
    async fn call(&self, document: &Lenient, name: &str, args: &Value, id: &str) -> String {
        let refused = || {
            result(id, |out| {
                out.begin_object();
                out.name("content");
                out.begin_array();
                out.begin_object();
                out.prop_str("type", Some("text"));
                out.prop_str("text", Some(REFUSED_TEXT));
                out.end_object();
                out.end_array();
                out.prop_bool("isError", true);
                out.end_object();
            })
        };
        let Some(input) = args.get("arguments") else { return refused() };
        if tools::validate_in(document, name, input).is_err() {
            return refused();
        }
        let Ok(reply) = (self.transport)(name.to_string(), input.clone()).await else { return refused() };
        let Some(answer) = Lenient::parse(&reply) else { return refused() };
        let Value::Object(reply_map) = &answer.value else { return refused() };
        let success = reply_map.get("ok") == Some(&Value::Bool(true));
        // The result as the app wrote it (`GetRawText`), and as Wire.Json writes it again (`structuredContent`).
        let (raw, value) = if success {
            let Some(value) = reply_map.get("result") else { return refused() };
            if answer.writable(value).is_err() {
                return refused();
            }
            (raw_member(&reply, "result").unwrap_or_else(|| wire(value)), value.clone())
        } else {
            let Ok(text) = answer.text(reply_map.get("error")) else { return refused() };
            let mut object = serde_json::Map::new();
            object.insert("error".into(), Value::String(text.unwrap_or("Request failed.").to_string()));
            let value = Value::Object(object);
            (wire(&value), value)
        };
        result(id, |out| {
            out.begin_object();
            out.name("content");
            out.begin_array();
            out.begin_object();
            out.prop_str("type", Some("text"));
            out.prop_str("text", Some(&raw));
            out.end_object();
            out.end_array();
            out.name("structuredContent");
            out.value(&value);
            out.prop_bool("isError", !success);
            out.end_object();
        })
    }
}

fn wire(value: &Value) -> String {
    let mut out = JsonOut::new();
    out.value(value);
    out.finish()
}

/// `{"jsonrpc":"2.0","id":…,"result":…}`.
fn result(id: &str, write: impl FnOnce(&mut JsonOut)) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("jsonrpc", Some("2.0"));
    out.name("id");
    out.raw(id);
    out.name("result");
    write(&mut out);
    out.end_object();
    out.finish()
}

/// `{"jsonrpc":"2.0","id":…,"error":{"code":…,"message":…}}`, the id written even when null.
fn error(id: &str, code: i64, message: &str) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("jsonrpc", Some("2.0"));
    out.name("id");
    out.raw(id);
    out.name("error");
    out.begin_object();
    out.prop_int("code", code);
    out.prop_str("message", Some(message));
    out.end_object();
    out.end_object();
    out.finish()
}

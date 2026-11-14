//! `mighty-delegation`'s server (C# Core/DelegationMcp.cs; the tools' list `DelegationToolManifest`, DelegationCoordinator.cs:10-43):
//! the six tools, `tools/call` forwarded over the pipe, the Refused and result texts, `UnsafeRelaxedJsonEscaping`. Owner: W7-C.
//!
//! The server lists exactly the six tools and forwards every well-formed call: the app decides who may call them. Its JSON is
//! `JsonNode` written with the relaxed encoder (non-ASCII kept; a null member written as `null`); every word is the Mac's, byte for
//! byte, since the agent reads them. A string with a lone surrogate escape fails only where C# reads it, and that answers
//! `-32700 "Parse error"` (C#'s late `InvalidOperationException`).

use std::sync::Arc;

use mighty_platform::BoxFuture;
use mighty_run::text_json::{escape_relaxed, relaxed};
use serde_json::{Map, Value, json};

use super::serve::{self, Handle};
use crate::agent_io::wire::{Lenient, MAXIMUM_BYTES, Unreadable, raw_member};
use crate::request::{DelegationName, DelegationReasonCode, DelegationRequest, DelegationResponse, prefix};

/// `DelegationMcp.ServerName`.
pub const SERVER_NAME: &str = "mighty-delegation";
/// `DelegationMcp.HeadlessArgument`.
pub const HEADLESS_ARGUMENT: &str = crate::modes::DELEGATION_MCP_ARGUMENT;

/// `DelegationToolManifest.Argument(Name, Description, Required = true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolArgument {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// `DelegationToolManifest.Tool(Name, Description, Arguments)`: every argument a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [ToolArgument],
}

const CHILD: ToolArgument =
    ToolArgument { name: "child", description: "The child's id, as delegate or list_children gave it.", required: true };

/// `DelegationToolManifest.All`: exactly these six, in this order. The words are the Mac's (M/DelegationCoordinator.swift:3-81).
pub const MANIFEST: [Tool; 6] = [
    Tool {
        name: "delegate",
        description: "Start a child pane that works on a task in its own git worktree, on the flat branch mighty/<child id> made from your current branch head. It runs your own agent CLI unless agent names another: claude, codex or antigravity, each only if installed and signed in on this PC. Use it only when the user asks for parallel or delegated work, or when the task clearly splits into independent parts that can run at the same time; never fan out speculatively, and do the work yourself otherwise. The child starts in the permission mode you ask for, which must be one of the child agent's modes and one in which it can change files. For your own agent it must be no wider than your own mode (Claude: plan < manual < acceptEdits < auto < fullAccess; Codex: manual < acceptEdits < onRequest < fullAccess). For another agent, the child's mode may do nothing yours may not: edit files without asking only if yours does (Claude acceptEdits and auto, Codex onRequest and acceptEdits, Antigravity acceptEdits, any fullAccess), edit after asking only if yours edits at all, run shell commands without asking only if yours does (Claude auto, Codex manual and acceptEdits, any fullAccess), and have full access only from your fullAccess. So from Claude acceptEdits a Codex child gets onRequest, not acceptEdits (which runs commands without asking), and from Claude plan or manual no Codex or Antigravity child can start. Answers as soon as the child exists; its run goes on in the background. The child writes its result to REPORT.md, and you get one notice per report. At most 3 open children per pane, of all agents together, and a child cannot delegate. The same call twice in one of your runs returns the same child.",
        arguments: &[
            ToolArgument { name: "task", description: "What the child should do. It is saved as the child's TASK.md.", required: true },
            ToolArgument {
                name: "mode",
                description: "The child's starting permission mode, one of the child agent's in which it can change files, no wider than yours: plan, manual, acceptEdits, auto or fullAccess for a Claude child; acceptEdits, onRequest or fullAccess for a Codex child; acceptEdits or fullAccess for an Antigravity child.",
                required: true,
            },
            ToolArgument {
                name: "agent",
                description: "Optional: the agent CLI the child runs, claude, codex or antigravity. Leave it out for your own agent.",
                required: false,
            },
        ],
    },
    Tool {
        name: "list_children",
        description: "List your children with their id, agent, state and current report revision.",
        arguments: &[],
    },
    Tool {
        name: "child_status",
        description: "Read one of your children: its agent, its state, its head and the body of its latest REPORT.md.",
        arguments: &[CHILD],
    },
    Tool {
        name: "merge",
        description: "Fast-forward your recorded branch to a reported child's head. Refused, changing nothing, unless the child has reported at exactly expected_head with no tracked changes, your recorded branch is checked out with no tracked changes and it can fast-forward. Follows your permission mode; a Codex pane may merge only when its run started in fullAccess and is otherwise refused with merge_human_only, so the human merges from the child's card.",
        arguments: &[
            CHILD,
            ToolArgument {
                name: "expected_head",
                description: "The child's reported head commit, as child_status gave it.",
                required: true,
            },
        ],
    },
    Tool {
        name: "follow_up",
        description: "Send one of your children a follow-up instruction, at most 2 per child. It reaches the child exactly once, as its next run, and clears its reported state.",
        arguments: &[CHILD, ToolArgument { name: "text", description: "The instruction for the child.", required: true }],
    },
    Tool {
        name: "discard",
        description: "Discarding a child is for the human only, from the child's card in the app. This tool always refuses with discard_human_only and changes nothing.",
        arguments: &[CHILD],
    },
];

/// `DelegationToolManifest.Named`.
pub fn named(name: &str) -> Option<&'static Tool> {
    MANIFEST.iter().find(|tool| tool.name == name)
}

/// The server's transport: one well-formed call forwarded to the app on the pane's token (`AgentIOPipe.CallDelegation`, which never
/// throws: a call it could not make is answered as a failed one).
pub type DelegationTransport = Arc<dyn Fn(DelegationRequest) -> BoxFuture<'static, DelegationResponse> + Send + Sync>;

/// The ToolError of a server started without the pane's pipe or token.
pub const NO_CONNECTION: &str = "This MCP server was started without a Mighty Code pane connection, so it cannot reach the app.";

/// `DelegationMcp`: None as transport is a server started without a pane connection.
#[derive(Clone)]
pub struct DelegationServer {
    transport: Option<DelegationTransport>,
}

/// C#'s late `InvalidOperationException`: answered `-32700 "Parse error"`.
struct Late;

impl From<Unreadable> for Late {
    fn from(_: Unreadable) -> Self {
        Late
    }
}

impl DelegationServer {
    pub fn new(transport: Option<DelegationTransport>) -> DelegationServer {
        DelegationServer { transport }
    }

    /// The loop's handler.
    pub fn handler(&self) -> Handle {
        let server = self.clone();
        Arc::new(move |line: String| {
            let server = server.clone();
            Box::pin(async move { server.handle(&line).await })
        })
    }

    /// `Handle(line)`: the answer line, or None for notifications and blank lines.
    pub async fn handle(&self, line: &str) -> Option<String> {
        let line = line.trim_end_matches('\r');
        if line.chars().all(|c| c == ' ' || c == '\t') {
            return None;
        }
        if line.len() > MAXIMUM_BYTES {
            return Some(error("null", -32700, "Message exceeds 1 MB."));
        }
        let Some(document) = Lenient::parse(line) else { return Some(error("null", -32700, "Parse error")) };
        match self.answer(&document, line).await {
            Ok(answer) => answer,
            Err(Late) => Some(error("null", -32700, "Parse error")),
        }
    }

    async fn answer(&self, document: &Lenient, line: &str) -> Result<Option<String>, Late> {
        let json = &document.value;
        // `ValidId`: a string or a number, written again as it was read (a string re-escaped, a number's literal).
        let valid_id = |value: Option<&Value>| -> Result<Option<String>, Late> {
            Ok(match value {
                Some(Value::String(text)) => Some(escape_relaxed(document.read(text)?)),
                Some(Value::Number(number)) => Some(raw_member(line, "id").unwrap_or_else(|| number.to_string())),
                _ => None,
            })
        };
        let Value::Object(request) = json else { return Ok(Some(error("null", -32600, "Invalid Request"))) };
        // Indexing a JsonObject decodes all its names.
        document.keys(request)?;
        let version_ok = match request.get("jsonrpc") {
            Some(Value::String(version)) => document.read(version)? == "2.0",
            _ => false,
        };
        let Some(Value::String(named)) = request.get("method").filter(|_| version_ok) else {
            let id = valid_id(request.get("id"))?;
            return Ok(Some(error(id.as_deref().unwrap_or("null"), -32600, "Invalid Request")));
        };
        // Without an id it is a notification: never answered, whatever the method.
        if !request.contains_key("id") {
            return Ok(None);
        }
        let Some(id) = valid_id(request.get("id"))? else { return Ok(Some(error("null", -32600, "Invalid Request"))) };
        let method = document.read(named)?;
        let empty = Map::new();
        let parameters = match request.get("params") {
            Some(Value::Object(parameters)) => parameters,
            _ => &empty,
        };
        Ok(Some(match method {
            "initialize" => {
                document.keys(parameters)?;
                let asked = document.text(parameters.get("protocolVersion"))?;
                reply(
                    &id,
                    "result",
                    &json!({
                        "protocolVersion": serve::protocol_version(asked),
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": SERVER_NAME, "title": "Mighty Code delegation", "version": "1.0.0"},
                    }),
                )
            }
            "ping" => reply(&id, "result", &json!({})),
            "tools/list" => reply(&id, "result", &json!({"tools": MANIFEST.iter().map(definition).collect::<Vec<_>>()})),
            "tools/call" => {
                document.keys(parameters)?;
                let name = match document.text(parameters.get("name"))? {
                    Some(name) if DelegationRequest::is_tool_name(name) => name,
                    text => {
                        let shown = match (text, parameters.get("name")) {
                            (Some(text), _) => text.to_string(),
                            (None, Some(value)) if !value.is_null() => {
                                document.writable(value)?;
                                relaxed(value, false)
                            }
                            _ => "none".to_string(),
                        };
                        return Ok(Some(error(&id, -32602, &format!("Unknown tool: {}", prefix(&shown, 80)))));
                    }
                };
                let arguments = match parameters.get("arguments") {
                    None | Some(Value::Null) if !parameters.contains_key("arguments") => Map::new(),
                    Some(Value::Object(arguments)) => arguments.clone(),
                    _ => return Ok(Some(reply(&id, "result", &tool_error("arguments must be an object.")))),
                };
                let answer = self.call(document, name, &arguments).await?;
                reply(&id, "result", &answer)
            }
            _ => error(&id, -32601, &format!("Method not found: {}", prefix(method, 80))),
        }))
    }

    /// `Call`: every well-formed call forwarded to the app, which decides whether this pane may use the tools.
    async fn call(&self, document: &Lenient, name: &str, arguments: &Map<String, Value>) -> Result<Value, Late> {
        document.keys(arguments)?;
        let mut values = indexmap::IndexMap::new();
        for (key, value) in arguments {
            let Value::String(text) = value else {
                return Ok(tool_error(&format!("Arguments of {name} must be strings with lowercase names.")));
            };
            if !DelegationRequest::is_tool_name(key) {
                return Ok(tool_error(&format!("Arguments of {name} must be strings with lowercase names.")));
            }
            values.insert(key.clone(), document.read(text)?.to_string());
        }
        let Some(transport) = &self.transport else { return Ok(tool_error(NO_CONNECTION)) };
        Ok(result(&transport(DelegationRequest { tool: name.to_string(), arguments: values }).await))
    }
}

/// `{"jsonrpc":"2.0","id":…,"<key>":…}` written with the relaxed encoder.
fn reply(id: &str, key: &str, value: &Value) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"{key}\":{}}}", relaxed(value, false))
}

fn error(id: &str, code: i64, message: &str) -> String {
    reply(id, "error", &json!({"code": code, "message": message}))
}

/// `Definition(tool)`: the `tools/list` entry.
pub fn definition(tool: &Tool) -> Value {
    let mut properties = Map::new();
    for argument in tool.arguments {
        properties.insert(argument.name.into(), json!({"type": "string", "description": argument.description}));
    }
    let required: Vec<&str> = tool.arguments.iter().filter(|a| a.required).map(|a| a.name).collect();
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
    })
}

fn content(text: &str) -> Value {
    json!([{"type": "text", "text": text}])
}

/// `ToolError(message)`: a failed tool call, shown to the model as the call's result, never as a protocol error.
pub fn tool_error(message: &str) -> Value {
    json!({"content": content(message), "isError": true})
}

/// `Hint(reason)`: what a refusal's code means when its name does not say it.
pub fn hint(reason: DelegationReasonCode) -> &'static str {
    match reason {
        DelegationReasonCode::ClaudeOnly => " (this pane's agent cannot use the delegation tools)",
        DelegationReasonCode::MergeHumanOnly => {
            " (in this permission mode only the human merges, from the child's card in the app; tell the user the child is ready)"
        }
        DelegationReasonCode::AgentUnknown => " (agent must be claude, codex or antigravity, or left out for your own)",
        _ => "",
    }
}

fn answer(text: &str, key: &str, data: Value) -> Value {
    let mut structured = Map::new();
    structured.insert(key.into(), data);
    json!({"content": content(text), "structuredContent": structured})
}

/// `Result(response)`: the `tools/call` result for the app's answer.
pub fn result(response: &DelegationResponse) -> Value {
    if let Some(reason) = response.refused {
        let code = reason.name();
        let about = match &response.error {
            Some(detail) => format!(" ({detail})"),
            None => hint(reason).to_string(),
        };
        return json!({
            "content": content(&format!("Refused: {code}{about}. Nothing was changed.")),
            "structuredContent": {"refused": code},
            "isError": true,
        });
    }
    if let Some(child) = &response.child {
        let state = child.state.name();
        return answer(
            &format!(
                "Child {} is {state}, runs {}, on the branch {}, starting in {} mode.",
                child.id, child.agent, child.branch, child.starting_mode
            ),
            "child",
            json!({"id": child.id, "state": state, "agent": child.agent, "branch": child.branch, "mode": child.starting_mode}),
        );
    }
    if let Some(children) = &response.children {
        let text = if children.is_empty() {
            "You have no children.".to_string()
        } else {
            let mut lines = vec!["Your children:".to_string()];
            lines.extend(children.iter().map(|c| format!("- {}: {}, {}, report revision {}", c.id, c.agent, c.state.name(), c.revision)));
            lines.join("\n")
        };
        let listed: Vec<Value> =
            children.iter().map(|c| json!({"id": c.id, "agent": c.agent, "state": c.state.name(), "revision": c.revision})).collect();
        return answer(&text, "children", Value::Array(listed));
    }
    if let Some(status) = &response.status {
        let mut text = format!("Child {} runs {} and is {}.", status.id, status.agent, status.state.name());
        if status.revision == 0 {
            text.push_str(" It has not reported yet.");
        } else {
            text.push_str(&format!(
                " Its report revision {} was made at head {}.",
                status.revision,
                status.head.as_deref().unwrap_or("unknown")
            ));
            match &status.report {
                Some(report) => text.push_str(&format!("\n\nREPORT.md:\n{report}")),
                None => text.push_str(" Its REPORT.md copy is no longer kept."),
            }
        }
        return answer(
            &text,
            "status",
            json!({
                "id": status.id, "agent": status.agent, "state": status.state.name(), "revision": status.revision,
                "head": status.head, "report": status.report, "reportTruncated": status.report_truncated,
            }),
        );
    }
    if let Some(merged) = &response.merged {
        return answer(
            &format!(
                "Merged child {}: {} fast-forwarded from {} to {}. The merge is recorded; only a human can undo it, from the child's card.",
                merged.child_id, merged.parent_branch, merged.pre_merge_commit, merged.merged_commit
            ),
            "merged",
            json!({
                "child": merged.child_id, "kind": merged.kind.name(), "parentBranch": merged.parent_branch,
                "preMergeCommit": merged.pre_merge_commit, "mergedCommit": merged.merged_commit, "childHead": merged.child_head,
            }),
        );
    }
    if let Some(follow_up) = &response.follow_up {
        let left = if follow_up.remaining == 0 {
            "It was this child's last follow-up.".to_string()
        } else {
            format!("This child may get {} more.", follow_up.remaining)
        };
        return answer(
            &format!(
                "Follow-up {} for child {} is recorded; Mighty Code hands it to the child exactly once. {left}",
                follow_up.id, follow_up.child
            ),
            "followUp",
            json!({"id": follow_up.id, "child": follow_up.child, "lane": follow_up.lane.name(), "remaining": follow_up.remaining}),
        );
    }
    tool_error(response.error.as_deref().unwrap_or("Mighty Code did not answer the request."))
}

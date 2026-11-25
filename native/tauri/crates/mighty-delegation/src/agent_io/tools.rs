//! `AgentIOTools` (C# Core/AgentIO/AgentIOBinding.cs:120-139): mighty-terminal's four tools, their descriptions and `Guidance` (C#'s
//! verbatim but where they name `open_url`'s browser: 0.5 opens the page in the default browser, where C# let the user choose the
//! app's own), and `Validate`: exactly one non-empty string argument with no NUL, the byte limits (65,536, 128, 128, 8192),
//! `open_url`'s absolute http(s) URL with a host and no userinfo or control characters. Owner: W7-C.

use serde_json::Value;

use super::wire::{Lenient, Unreadable};

/// `AgentIOTool(Name, Argument, Description)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentIoTool {
    pub name: &'static str,
    pub argument: &'static str,
    pub description: &'static str,
}

/// `AgentIOTools.Guidance`: mighty-terminal's `instructions`, and the start of `run_in_terminal`'s description.
pub const GUIDANCE: &str = "Use run_in_terminal for user-visible or long-running commands, including sign-ins and interactive prompts so the user can answer in their terminal. Keep short internal file reads and checks in built-in tools. Use open_url for web pages; it opens them in the user's default browser. Do not drive sign-in prompts by sending keystrokes.";

/// MC-48: the browser check of the pane's own dev server.
pub const BROWSER_CHECK: &str = "mighty_browser_check";

/// MC-58: the bots another agent may ask, their request, and a bot's answer.
pub const LIST_BOTS: &str = "list_bots";
pub const ASK_BOT: &str = "ask_bot";
pub const BOT_ANSWER: &str = "bot_answer";

/// MC-65: a bot made or changed from a spec, once the person allows it.
pub const CREATE_BOT: &str = "create_bot";
pub const UPDATE_BOT: &str = "update_bot";
/// MC-65: the largest spec, in bytes.
pub const BOT_SPEC_BYTES: usize = 65_536;

/// MC-73: a bot's report of the work it just finished (its status, a one-line summary and up to three items).
pub const BOT_REPORT: &str = "bot_report";
/// MC-73: the largest report, in bytes, and the most items kept.
pub const BOT_REPORT_BYTES: usize = 16_384;
pub const BOT_REPORT_ITEMS: usize = 3;
/// MC-73: a report's status.
pub const BOT_REPORT_STATUSES: [&str; 4] = ["ok", "report", "needs", "failed"];

/// MC-58: the longest prompt `ask_bot` sends, in bytes.
pub const BOT_PROMPT_BYTES: usize = 65_536;

/// `AgentIOTools.All`, in this order (MC-48's browser check, then MC-58's bot tools).
pub const ALL: [AgentIoTool; 11] = [
    AgentIoTool {
        name: "run_in_terminal",
        argument: "command",
        description: "Use run_in_terminal for user-visible or long-running commands, including sign-ins and interactive prompts so the user can answer in their terminal. Keep short internal file reads and checks in built-in tools. Use open_url for web pages; it opens them in the user's default browser. Do not drive sign-in prompts by sending keystrokes. Run a PowerShell command in the workspace. Returns output and exit code within 12 seconds, otherwise a running handle.",
    },
    AgentIoTool {
        name: "read_latest_output",
        argument: "handle",
        description: "Read up to 64 KiB of new output from this pane's terminal command, including user input. Returns running/done, exit code, and dropped/more flags.",
    },
    AgentIoTool {
        name: "stop",
        argument: "handle",
        description: "Stop this pane's terminal command: Ctrl+C first, then terminate its Windows job after three seconds. Returns the final status.",
    },
    AgentIoTool {
        name: "open_url",
        argument: "url",
        description: "Open an http/https page (including localhost) in the user's default browser. Other schemes are refused.",
    },
    AgentIoTool {
        name: BROWSER_CHECK,
        argument: "url",
        description: "Check this worktree's running dev server in a headless local Chrome/Edge/Chromium/Brave: visits each path (default /) and returns console errors, uncaught exceptions, failed requests (4xx/5xx, network errors), an accessibility-tree summary and a saved screenshot's name. Arguments are optional: url (http://localhost or 127.0.0.1 within this worktree's port block or its run action's address; default: the running run action's address) and paths (a list, or one string of paths separated by spaces). Other addresses are refused. Says so plainly when no browser is installed.",
    },
    AgentIoTool {
        name: LIST_BOTS,
        argument: "",
        description: "List the bots the person set up in Mighty Code: other agent panes, each with a name (@handle), what it is for, its workspace and whether it is busy. Ask one with ask_bot.",
    },
    AgentIoTool {
        name: ASK_BOT,
        argument: "prompt",
        description: "Send a request to a bot (an agent pane the person named, see list_bots): bot is its @handle, prompt the request. The bot works on it with its own settings and permissions, in its own pane; when it is busy the request waits in its queue. Returns at once; read the answer with bot_answer. A refusal comes back as `refused` with its reason. While working on a request another bot sent, a pane cannot ask a bot.",
    },
    AgentIoTool {
        name: BOT_ANSWER,
        argument: "bot",
        description: "Read a bot's state and the answer to its last request: status (running, blocked while it waits for the person, or done) and its answer, secrets redacted. With wait: true it waits up to 20 seconds for the bot to finish first; call it again while it is still running.",
    },
    AgentIoTool {
        name: CREATE_BOT,
        argument: "spec",
        description: "Propose a new bot to the person: spec is a JSON object, every field optional — name (no @; a friendly one when left out), agent (claude, codex, gemini, pi, opencode, hermes), model, permission (one of the agent's modes), workspace (a workspace's id or name, for a bot of that workspace; else an independent bot), folder (an independent bot's folder), instructions, budgetPerDay (USD), schedules ([{name, cron or rrule, prompt, catchUp}]) and triggers ([{name, on, where, prompt}], on being {type: github, events: [pr_opened|review_requested|ci_failed|release_published|pr_merged|issue_opened], repo: owner/name}, {type: event, name: release.done}, {type: bot, bot: @name, on: [done|failed]}, {type: standing, command, everyMinutes, dailyCap} or {type: webhook}; where an optional list of conditions every event must pass, each {field, op, value} with op eq, in (a list), has (a comma-separated field such as labels holds it) or matches (a regex), or their not_ forms, e.g. [{field: labels, op: has, value: bot:dev}, {field: author, op: not_eq, value: dependabot[bot]}, {field: base, op: eq, value: main}]; a field is one of the event's values or type, subject, source, actor). A prompt may use {{pr.title}}, {{pr.url}}, {{data.version}}, {{bot.answer}}, {{event.type}}, {{event.subject}}. The app shows the person an approval sheet and makes the bot only if they allow it: this returns pending at once (see list_bots later), or refused with the reason.",
    },
    AgentIoTool {
        name: UPDATE_BOT,
        argument: "spec",
        description: "Propose a change to an existing bot: spec as create_bot's, its name naming the bot. Given instructions, permission and budgetPerDay replace the bot's; given schedules or triggers replace the bot's own. The person allows it on an approval sheet; returns pending at once, or refused with the reason.",
    },
    AgentIoTool {
        name: BOT_REPORT,
        argument: "summary",
        description: "As a bot, report the work you just finished, once, at its end: status is ok (nothing to report), report (something the person should know), needs (the person must decide or check something) or failed; summary is one short sentence; items are up to three {text, url} lines, each a finding with its link (a pull request, an issue, a file). The person sees it in Mighty Code's report box; needs and failed also notify them. With nothing to report, use ok and keep the summary short.",
    },
];

/// The most paths one browser check visits, and the longest address or path.
pub const BROWSER_CHECK_PATHS: usize = 3;
const BROWSER_CHECK_TEXT: usize = 2048;

/// The tool of this name.
pub fn named(name: &str) -> Option<&'static AgentIoTool> {
    ALL.iter().find(|tool| tool.name == name)
}

/// Why `Validate` refused (C#'s `ArgumentException` messages; the agent never sees them: the server answers its one refusal text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    UnknownTool,
    NotOneString,
    TooLong,
    NotWebAddress,
    /// The argument held a lone surrogate escape (C# throws while reading it).
    Unreadable,
}

impl From<Unreadable> for Invalid {
    fn from(_: Unreadable) -> Self {
        Invalid::Unreadable
    }
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Invalid::UnknownTool => "Unknown tool.",
            Invalid::NotOneString => "Exactly one non-empty string argument is required.",
            Invalid::TooLong => "Tool argument exceeds its limit.",
            Invalid::NotWebAddress => "Only http/https URLs without credentials are accepted.",
            Invalid::Unreadable => "Cannot read invalid UTF-16 JSON text as string.",
        })
    }
}

/// `AgentIOTools.Validate(tool, arguments)` over an argument object read strictly: the one argument's text.
pub fn validate(tool: &str, arguments: &Value) -> Result<String, Invalid> {
    validate_in(&Lenient::parse("null").expect("null parses"), tool, arguments)
}

/// [`validate`] over an object of a document read lazily ([`Lenient`]).
pub(crate) fn validate_in(document: &Lenient, tool: &str, arguments: &Value) -> Result<String, Invalid> {
    let spec = named(tool).ok_or(Invalid::UnknownTool)?;
    let Value::Object(map) = arguments else { return Err(Invalid::NotOneString) };
    if tool == BROWSER_CHECK {
        return browser_check_arguments(document, map);
    }
    if matches!(tool, LIST_BOTS | ASK_BOT | BOT_ANSWER) {
        return bot_arguments(document, tool, map);
    }
    if matches!(tool, CREATE_BOT | UPDATE_BOT) {
        return spec_argument(document, map);
    }
    if tool == BOT_REPORT {
        return report_arguments(document, map);
    }
    if map.len() != 1 {
        return Err(Invalid::NotOneString);
    }
    let value = document.text(map.get(spec.argument))?.ok_or(Invalid::NotOneString)?;
    if value.chars().all(char::is_whitespace) || value.contains('\0') {
        return Err(Invalid::NotOneString);
    }
    let max = match tool {
        "run_in_terminal" => 65_536,
        "open_url" => 8192,
        _ => 128,
    };
    if value.len() > max {
        return Err(Invalid::TooLong);
    }
    if tool == "open_url" && !web_address(value) {
        return Err(Invalid::NotWebAddress);
    }
    Ok(value.to_string())
}

/// MC-48: `mighty_browser_check`'s optional `url` and `paths` (a list of strings or one string of paths separated by spaces or
/// commas), as compact JSON `{"url": …, "paths": [...]}`: nothing else, no NUL or control characters, at most
/// [`BROWSER_CHECK_PATHS`] paths each starting with `/`. The app checks the address against the pane's ports.
fn browser_check_arguments(document: &Lenient, map: &serde_json::Map<String, Value>) -> Result<String, Invalid> {
    if map.keys().any(|key| key != "url" && key != "paths") {
        return Err(Invalid::NotOneString);
    }
    let clean = |text: &str| -> Result<String, Invalid> {
        if text.chars().any(char::is_control) {
            return Err(Invalid::NotOneString);
        }
        let text = text.trim();
        if text.len() > BROWSER_CHECK_TEXT {
            return Err(Invalid::TooLong);
        }
        Ok(text.to_string())
    };
    let url = match map.get("url") {
        None | Some(Value::Null) => None,
        Some(value) => Some(clean(document.text(Some(value))?.ok_or(Invalid::NotOneString)?)?).filter(|u| !u.is_empty()),
    };
    if url.as_deref().is_some_and(|u| !web_address(u)) {
        return Err(Invalid::NotWebAddress);
    }
    let mut paths: Vec<String> = Vec::new();
    match map.get("paths") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                paths.push(clean(document.text(Some(item))?.ok_or(Invalid::NotOneString)?)?);
            }
        }
        Some(value) => {
            let text = clean(document.text(Some(value))?.ok_or(Invalid::NotOneString)?)?;
            paths.extend(text.split([' ', ',']).filter(|p| !p.is_empty()).map(str::to_string));
        }
    }
    paths.retain(|p| !p.is_empty());
    if paths.len() > BROWSER_CHECK_PATHS {
        return Err(Invalid::TooLong);
    }
    if paths.iter().any(|p| !p.starts_with('/') || p.starts_with("//") || p.contains(char::is_whitespace)) {
        return Err(Invalid::NotOneString);
    }
    Ok(serde_json::json!({ "url": url, "paths": paths }).to_string())
}

/// MC-58: the bot tools' arguments as compact JSON: `list_bots` takes none; `ask_bot` a `bot` (at most 40 characters) and a
/// `prompt` (at most [`BOT_PROMPT_BYTES`], no NUL); `bot_answer` a `bot` and an optional boolean `wait`. The app finds the bot.
fn bot_arguments(document: &Lenient, tool: &str, map: &serde_json::Map<String, Value>) -> Result<String, Invalid> {
    let allowed: &[&str] = match tool {
        LIST_BOTS => &[],
        ASK_BOT => &["bot", "prompt"],
        _ => &["bot", "wait"],
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Invalid::NotOneString);
    }
    if tool == LIST_BOTS {
        return Ok("{}".into());
    }
    let bot = document.text(map.get("bot"))?.map(str::trim).filter(|b| !b.is_empty()).ok_or(Invalid::NotOneString)?;
    if bot.chars().count() > 40 || bot.chars().any(char::is_control) {
        return Err(Invalid::TooLong);
    }
    if tool == ASK_BOT {
        let prompt = document.text(map.get("prompt"))?.ok_or(Invalid::NotOneString)?;
        if prompt.trim().is_empty() || prompt.contains('\0') {
            return Err(Invalid::NotOneString);
        }
        if prompt.len() > BOT_PROMPT_BYTES {
            return Err(Invalid::TooLong);
        }
        return Ok(serde_json::json!({ "bot": bot, "prompt": prompt }).to_string());
    }
    let wait = match map.get("wait") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(wait)) => *wait,
        Some(_) => return Err(Invalid::NotOneString),
    };
    Ok(serde_json::json!({ "bot": bot, "wait": wait }).to_string())
}

/// MC-65: `create_bot` / `update_bot`'s one `spec`, an object (or its JSON text), as compact JSON at most [`BOT_SPEC_BYTES`]; the app
/// reads and checks the spec itself.
fn spec_argument(document: &Lenient, map: &serde_json::Map<String, Value>) -> Result<String, Invalid> {
    if map.len() != 1 {
        return Err(Invalid::NotOneString);
    }
    let text = match map.get("spec") {
        Some(Value::Object(_)) => map.get("spec").map(Value::to_string).unwrap_or_default(),
        Some(Value::String(_)) => {
            let text = document.text(map.get("spec"))?.ok_or(Invalid::NotOneString)?;
            serde_json::from_str::<Value>(text).ok().filter(Value::is_object).ok_or(Invalid::NotOneString)?.to_string()
        }
        _ => return Err(Invalid::NotOneString),
    };
    if text.len() > BOT_SPEC_BYTES || text.contains('\0') {
        return Err(Invalid::TooLong);
    }
    Ok(text)
}

/// MC-73: `bot_report`'s status, summary and items as compact JSON `{"status", "summary", "items": [{"text", "url"}]}`: a known
/// status, a non-empty summary, at most [`BOT_REPORT_ITEMS`] items (each a text, a link only when it is a web address), no NUL,
/// at most [`BOT_REPORT_BYTES`].
fn report_arguments(document: &Lenient, map: &serde_json::Map<String, Value>) -> Result<String, Invalid> {
    if map.keys().any(|key| !matches!(key.as_str(), "status" | "summary" | "items")) {
        return Err(Invalid::NotOneString);
    }
    let text = |value: Option<&Value>| -> Result<Option<String>, Invalid> {
        Ok(document.text(value)?.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
    };
    let status = text(map.get("status"))?.unwrap_or_else(|| "report".into());
    if !BOT_REPORT_STATUSES.contains(&status.as_str()) {
        return Err(Invalid::NotOneString);
    }
    let summary = text(map.get("summary"))?.ok_or(Invalid::NotOneString)?;
    let mut items = Vec::new();
    match map.get("items") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for item in list.iter().take(BOT_REPORT_ITEMS) {
                let (line, url) = match item {
                    Value::Object(fields) => (text(fields.get("text"))?, text(fields.get("url"))?),
                    other => (text(Some(other))?, None),
                };
                let Some(line) = line else { continue };
                items.push(serde_json::json!({ "text": line, "url": url.filter(|u| web_address(u)) }));
            }
        }
        Some(_) => return Err(Invalid::NotOneString),
    }
    let out = serde_json::json!({ "status": status, "summary": summary, "items": items }).to_string();
    if out.contains('\0') {
        return Err(Invalid::NotOneString);
    }
    if out.len() > BOT_REPORT_BYTES {
        return Err(Invalid::TooLong);
    }
    Ok(out)
}

/// `Uri.TryCreate(value, UriKind.Absolute, out var uri) && uri.Scheme is "http" or "https" && uri.UserInfo.Length == 0 &&
/// uri.Host != "" && !value.Any(char.IsControl)`.
pub fn web_address(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(value) else { return false };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return false;
    }
    // The authority after exactly two slashes (.NET reads "http:///x" as no host, where the WHATWG parser skips the third), and
    // .NET's UserInfo is everything before its '@', even ":" (which the WHATWG parser drops as empty).
    let after = value.trim_start().get(parsed.scheme().len() + 1..).unwrap_or("");
    let separator = |c: char| c == '/' || c == '\\';
    let mut chars = after.chars();
    if !(chars.next().is_some_and(separator) && chars.next().is_some_and(separator)) {
        return false;
    }
    let rest = &after[2..];
    if rest.starts_with(separator) {
        return false;
    }
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    !matches!(authority.rfind('@'), Some(at) if at > 0)
}

/// `Uri.AbsoluteUri` of an address [`web_address`] took: its normalized text (what the browser is given).
pub fn absolute_uri(value: &str) -> String {
    url::Url::parse(value).map(String::from).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-73: bot_report takes a known status and a summary, keeps three items, drops a link that is not a web address.
    #[test]
    fn a_report_is_a_status_a_summary_and_at_most_three_items() {
        let read = |v: serde_json::Value| validate(BOT_REPORT, &v);
        let out: serde_json::Value = serde_json::from_str(
            &read(serde_json::json!({ "status": "needs", "summary": " Pick one ", "items": [
                { "text": "#412 risky", "url": "https://github.com/o/r/pull/412" },
                { "text": "file", "url": "file:///etc/passwd" },
                "plain line", { "text": "dropped, the fourth" }
            ]}))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(out["status"], "needs");
        assert_eq!(out["summary"], "Pick one");
        assert_eq!(out["items"].as_array().unwrap().len(), 3);
        assert_eq!(out["items"][0]["url"], "https://github.com/o/r/pull/412");
        assert!(out["items"][1]["url"].is_null(), "only web addresses are kept");
        assert_eq!(out["items"][2]["text"], "plain line");
        assert_eq!(read(serde_json::json!({ "status": "great", "summary": "x" })), Err(Invalid::NotOneString));
        assert_eq!(read(serde_json::json!({ "status": "ok" })), Err(Invalid::NotOneString), "a summary is needed");
        assert_eq!(read(serde_json::json!({ "summary": "x", "extra": 1 })), Err(Invalid::NotOneString));
        assert!(read(serde_json::json!({ "summary": "x" })).unwrap().contains("\"report\""), "report is the default status");
        assert_eq!(read(serde_json::json!({ "summary": "x".repeat(BOT_REPORT_BYTES) })), Err(Invalid::TooLong));
    }
    use serde_json::json;

    /// MC-48: the browser check takes no argument, a url, paths as a list or one string; nothing else.
    #[test]
    fn the_browser_check_takes_optional_url_and_paths() {
        let none: Value = serde_json::from_str(&validate(BROWSER_CHECK, &json!({})).unwrap()).unwrap();
        assert_eq!(none, json!({"url": null, "paths": []}));
        let both = validate(BROWSER_CHECK, &json!({"url": "http://localhost:3000", "paths": ["/", "/settings"]})).unwrap();
        let read: Value = serde_json::from_str(&both).unwrap();
        assert_eq!(read, json!({"url": "http://localhost:3000", "paths": ["/", "/settings"]}));
        let split: Value = serde_json::from_str(&validate(BROWSER_CHECK, &json!({"paths": "/a, /b"})).unwrap()).unwrap();
        assert_eq!(split["paths"], json!(["/a", "/b"]));
        for refused in [
            json!({"command": "x"}),
            json!({"url": "file:///etc/passwd"}),
            json!({"paths": ["relative"]}),
            json!({"paths": ["//evil.example/x"]}),
            json!({"paths": ["/a", "/b", "/c", "/d"]}),
            json!({"url": "http://localhost:3000\n"}),
            json!("x"),
        ] {
            assert!(validate(BROWSER_CHECK, &refused).is_err(), "{refused}");
        }
    }

    /// The URL half of C# "Agent IO BindingSecretsAndArgumentBoundaries" and the argument rules around it.
    #[test]
    fn only_one_bounded_string_and_web_addresses_pass() {
        for url in
            ["file:///tmp/private", "javascript:alert(1)", "https://user:password@example.com", "data:text/html,x", "https://example.com\n"]
        {
            assert!(validate("open_url", &json!({ "url": url })).is_err(), "{url}");
        }
        assert_eq!(validate("open_url", &json!({"url": "http://localhost:3000/path"})).unwrap(), "http://localhost:3000/path");
        assert!(validate("run_in_terminal", &json!({"command": "echo safe", "workspace": "other"})).is_err());
        assert_eq!(validate("run_in_terminal", &json!({"command": "가".repeat(30_000)})), Err(Invalid::TooLong));
        assert_eq!(validate("run_in_terminal", &json!({"command": "가".repeat(21_845)})).unwrap().len(), 65_535);
        assert_eq!(validate("stop", &json!({"handle": "h".repeat(129)})), Err(Invalid::TooLong));
        for refused in [
            json!({}),
            json!({"command": ""}),
            json!({"command": " \t\u{3000}"}),
            json!({"command": 5}),
            json!("x"),
            json!({"command": "a\0b"}),
        ] {
            assert_eq!(validate("run_in_terminal", &refused), Err(Invalid::NotOneString), "{refused}");
        }
        assert_eq!(validate("nope", &json!({"x": "y"})), Err(Invalid::UnknownTool));
        assert!(!web_address("https://:@example.com") && web_address("https://@example.com/") && web_address("HTTPS://Example.com"));
        assert!(!web_address("http:///path") && !web_address("ftp://example.com") && !web_address("https://exa mple.com"));
        assert_eq!(absolute_uri("http://localhost:3000"), "http://localhost:3000/");
        assert_eq!(
            ALL.map(|tool| tool.name),
            [
                "run_in_terminal",
                "read_latest_output",
                "stop",
                "open_url",
                "mighty_browser_check",
                "list_bots",
                "ask_bot",
                "bot_answer",
                "create_bot",
                "update_bot",
                "bot_report"
            ]
        );
        assert!(ALL[0].description.starts_with(GUIDANCE));
    }

    /// MC-65: create_bot and update_bot take one spec, an object or its JSON text, as compact JSON.
    #[test]
    fn the_bot_spec_tools_take_one_object() {
        let sent: Value = serde_json::from_str(&validate(CREATE_BOT, &json!({"spec": {"name": "a", "agent": "codex"}})).unwrap()).unwrap();
        assert_eq!(sent, json!({"name": "a", "agent": "codex"}));
        assert_eq!(validate(UPDATE_BOT, &json!({"spec": "{\"name\":\"a\"}"})).unwrap(), r#"{"name":"a"}"#);
        assert_eq!(validate(CREATE_BOT, &json!({"spec": "not json"})), Err(Invalid::NotOneString));
        assert_eq!(validate(CREATE_BOT, &json!({"spec": [1]})), Err(Invalid::NotOneString));
        assert_eq!(validate(CREATE_BOT, &json!({"spec": {}, "extra": 1})), Err(Invalid::NotOneString));
        assert_eq!(validate(CREATE_BOT, &json!({"spec": {"instructions": "x".repeat(BOT_SPEC_BYTES)}})), Err(Invalid::TooLong));
    }

    /// MC-58: the bot tools' arguments come out as compact JSON, with anything else refused.
    #[test]
    fn the_bot_tools_take_their_own_arguments_only() {
        assert_eq!(validate(LIST_BOTS, &json!({})).unwrap(), "{}");
        assert_eq!(validate(LIST_BOTS, &json!({"bot": "x"})), Err(Invalid::NotOneString));
        let sent: Value = serde_json::from_str(&validate(ASK_BOT, &json!({"bot": " @reviewer ", "prompt": "look"})).unwrap()).unwrap();
        assert_eq!(sent, json!({"bot": "@reviewer", "prompt": "look"}));
        assert_eq!(validate(ASK_BOT, &json!({"bot": "@r"})), Err(Invalid::NotOneString), "no prompt");
        assert_eq!(validate(ASK_BOT, &json!({"bot": "@r", "prompt": "  "})), Err(Invalid::NotOneString));
        assert_eq!(validate(ASK_BOT, &json!({"bot": "@r", "prompt": "x", "mode": "fullAccess"})), Err(Invalid::NotOneString));
        assert_eq!(validate(ASK_BOT, &json!({"bot": "@r", "prompt": "x".repeat(BOT_PROMPT_BYTES + 1)})), Err(Invalid::TooLong));
        assert_eq!(validate(ASK_BOT, &json!({"bot": "x".repeat(41), "prompt": "x"})), Err(Invalid::TooLong));
        let read: Value = serde_json::from_str(&validate(BOT_ANSWER, &json!({"bot": "@r", "wait": true})).unwrap()).unwrap();
        assert_eq!(read, json!({"bot": "@r", "wait": true}));
        let read: Value = serde_json::from_str(&validate(BOT_ANSWER, &json!({"bot": "@r"})).unwrap()).unwrap();
        assert_eq!(read["wait"], false);
        assert_eq!(validate(BOT_ANSWER, &json!({"bot": "@r", "wait": "yes"})), Err(Invalid::NotOneString));
    }
}

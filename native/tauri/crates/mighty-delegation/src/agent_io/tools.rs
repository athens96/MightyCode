//! `AgentIOTools` (C# Core/AgentIO/AgentIOBinding.cs:120-139): mighty-terminal's four tools, their verbatim descriptions and
//! `Guidance`, and `Validate`: exactly one non-empty string argument with no NUL, the byte limits (65,536, 128, 128, 8192),
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
pub const GUIDANCE: &str = "Use run_in_terminal for user-visible or long-running commands, including sign-ins and interactive prompts so the user can answer in their terminal. Keep short internal file reads and checks in built-in tools. Use open_url for web pages; the user chooses the browser. Do not drive sign-in prompts by sending keystrokes.";

/// `AgentIOTools.All`, in this order.
pub const ALL: [AgentIoTool; 4] = [
    AgentIoTool {
        name: "run_in_terminal",
        argument: "command",
        description: "Use run_in_terminal for user-visible or long-running commands, including sign-ins and interactive prompts so the user can answer in their terminal. Keep short internal file reads and checks in built-in tools. Use open_url for web pages; the user chooses the browser. Do not drive sign-in prompts by sending keystrokes. Run a PowerShell command in the workspace. Returns output and exit code within 12 seconds, otherwise a running handle.",
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
        description: "Open an http/https page (including localhost) in the app or system browser as chosen by the user for this workspace. Other schemes are refused.",
    },
];

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

/// `Uri.AbsoluteUri` of an address [`web_address`] took: its normalized text (what the card shows and the browser is given).
pub fn absolute_uri(value: &str) -> String {
    url::Url::parse(value).map(String::from).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert_eq!(ALL.map(|tool| tool.name), ["run_in_terminal", "read_latest_output", "stop", "open_url"]);
        assert!(ALL[0].description.starts_with(GUIDANCE));
    }
}

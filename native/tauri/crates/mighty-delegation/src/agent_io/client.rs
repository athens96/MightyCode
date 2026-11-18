//! The servers' side of the pipe (C# Core/AgentIO/AgentIOPipe.cs:138-184): connect, one request, one answer, and the literal failure
//! strings ("Mighty Code's agent socket path is invalid.", "The request is too large.", ...). Owner: W7-C.

use std::time::Duration;

use mighty_core::json::JsonOut;
use mighty_platform::{AgentIoConnector, McpTransport};
use serde_json::Value;

use super::wire::{self, Lenient, MAXIMUM_BYTES};
use crate::request::{DelegationRequest, DelegationResponse};

/// `AgentIOPipe.DelegationSeconds`: a delegation call's limit at both ends (it covers the coordinator's 45 s).
pub const DELEGATION_SECONDS: u64 = 60;
/// A terminal or web tool call's limit. (C# gave `open_url` 60 s, the user's 30 s to choose where it opened; 0.5 opens it in the
/// default browser at once.)
pub const TERMINAL_SECONDS: u64 = 30;

/// A terminal or web call that could not be made or answered (C#'s exceptions from `AgentIOPipe.Call`; the server answers them with its
/// one refusal text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallFailed;

impl std::fmt::Display for CallFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the tool call could not be made or answered")
    }
}

impl std::error::Error for CallFailed {}

const PREFIX: &str = "mighty-agent-io-";

/// `AgentIOPipe.IsPipeName`: `mighty-agent-io-` and exactly 32 hex digits on Windows; the Mac's socket is a full path.
pub fn is_pipe_name(name: &str, transport: McpTransport) -> bool {
    match transport {
        McpTransport::NamedPipe => {
            name.strip_prefix(PREFIX).is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        }
        McpTransport::UnixSocket => name.starts_with('/') && !name.contains('\0'),
    }
}

/// `AgentIOPipe.Call`: one terminal or web tool call; the app's answer packet as its JSON text (a document it could read), or Err when
/// the name or token is not one, the app is not there, or it did not answer in time (30 s).
pub async fn call(connector: &dyn AgentIoConnector, pipe: &str, token: &str, tool: &str, arguments: &Value) -> Result<String, CallFailed> {
    let transport = connector.transport();
    if !is_pipe_name(pipe, transport) || token.len() != 64 {
        return Err(CallFailed);
    }
    let deadline = Duration::from_secs(TERMINAL_SECONDS);
    let work = async {
        let mut stream = connector.connect(pipe, deadline).await.map_err(|_| CallFailed)?;
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("token", Some(token));
        out.prop_str("tool", Some(tool));
        out.name("arguments");
        out.value(arguments);
        out.end_object();
        wire::write_packet(&mut stream, out.as_str().as_bytes(), transport).await.map_err(|_| CallFailed)?;
        let answer = wire::read_packet(&mut stream, transport).await.map_err(|_| CallFailed)?;
        let text = String::from_utf8(answer).map_err(|_| CallFailed)?;
        Lenient::parse(&text).ok_or(CallFailed)?;
        Ok(text)
    };
    tokio::time::timeout(deadline, work).await.unwrap_or(Err(CallFailed))
}

/// `AgentIOPipe.CallDelegation`: one delegation call on the same pipe and token, within 60 s. It never fails: a call that could not be
/// made is answered as a failed one.
pub async fn call_delegation(connector: &dyn AgentIoConnector, pipe: &str, token: &str, request: &DelegationRequest) -> DelegationResponse {
    let transport = connector.transport();
    if !is_pipe_name(pipe, transport) {
        return DelegationResponse::failure("Mighty Code's agent socket path is invalid.");
    }
    let bytes = delegation_packet(token, request);
    if bytes.len() >= MAXIMUM_BYTES {
        return DelegationResponse::failure("The request is too large.");
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(DELEGATION_SECONDS);
    let not_running = || DelegationResponse::failure("Mighty Code is not running or not accepting agent tool calls.");
    let no_answer = || DelegationResponse::failure("Mighty Code did not answer the request.");
    let mut stream = match tokio::time::timeout_at(deadline, connector.connect(pipe, Duration::from_secs(DELEGATION_SECONDS))).await {
        Ok(Ok(stream)) => stream,
        _ => return not_running(),
    };
    match tokio::time::timeout_at(deadline, wire::write_packet(&mut stream, &bytes, transport)).await {
        Ok(Ok(())) => {}
        _ => return DelegationResponse::failure("Could not send the request to Mighty Code."),
    }
    match tokio::time::timeout_at(deadline, wire::read_packet(&mut stream, transport)).await {
        Ok(Ok(answer)) => read_response(&answer).unwrap_or_else(no_answer),
        _ => no_answer(),
    }
}

/// `{"token":…,"delegation":{"tool":…,"arguments":{…}}}` as Wire.Json writes it.
pub fn delegation_packet(token: &str, request: &DelegationRequest) -> Vec<u8> {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("token", Some(token));
    out.name("delegation");
    out.begin_object();
    out.prop_str("tool", Some(&request.tool));
    out.name("arguments");
    out.begin_object();
    for (key, value) in &request.arguments {
        out.prop_str(key, Some(value));
    }
    out.end_object();
    out.end_object();
    out.end_object();
    out.finish().into_bytes()
}

/// `JsonSerializer.Deserialize<DelegationResponse>(bytes, Wire.Json)`: None when it does not read (or reads as null).
pub fn read_response(bytes: &[u8]) -> Option<DelegationResponse> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    if value.is_null() {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// `JsonSerializer.SerializeToUtf8Bytes(response, Wire.Json)`: the answer as the app writes it on the pipe.
pub fn response_bytes(response: &DelegationResponse) -> Vec<u8> {
    let value = serde_json::to_value(response).unwrap_or(Value::Null);
    let mut out = JsonOut::new();
    out.value(&value);
    out.finish().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names_are_the_prefix_and_32_hex_digits() {
        assert!(is_pipe_name(&format!("{PREFIX}{}", "aF09".repeat(8)), McpTransport::NamedPipe));
        for bad in ["", "not-a-pipe", "mighty-agent-io-", &format!("{PREFIX}{}", "a".repeat(31)), &format!("{PREFIX}{}g", "a".repeat(31))] {
            assert!(!is_pipe_name(bad, McpTransport::NamedPipe), "{bad}");
        }
        assert!(is_pipe_name("/tmp/mc-501/io.sock", McpTransport::UnixSocket) && !is_pipe_name("io.sock", McpTransport::UnixSocket));
    }

    #[test]
    fn a_delegation_packet_is_written_as_the_wire_writes_it() {
        let mut request = DelegationRequest::new("follow_up");
        request.arguments.insert("text".into(), "한글 \"q\"".into());
        // Wire.Json's escapes: every non-ASCII character and the quote as `\u` and four upper-case hex digits.
        let esc = |hex: &str| format!("\\u{hex}");
        assert_eq!(
            String::from_utf8(delegation_packet("t", &request)).unwrap(),
            format!(
                r#"{{"token":"t","delegation":{{"tool":"follow_up","arguments":{{"text":"{}{} {}q{}"}}}}}}"#,
                esc("D55C"),
                esc("AE00"),
                esc("0022"),
                esc("0022")
            )
        );
        let failure = DelegationResponse::failure("한");
        assert_eq!(String::from_utf8(response_bytes(&failure)).unwrap(), format!(r#"{{"error":"{}"}}"#, esc("D55C")));
        assert_eq!(read_response(&response_bytes(&failure)), Some(failure));
        assert_eq!(read_response(b"null"), None);
    }
}

//! The stdio JSON-RPC loop both servers share (C# AgentIOMcp.cs:20-92, DelegationMcp.cs:38-97): newline-delimited, `\r` trimmed, a
//! line over 1,048,576 bytes gives `-32700`, up to 16 lines at once, each answer written whole on its line; `initialize` (the four
//! protocol versions), `ping`, the error codes of W7 plan section 1.3. Owner: W7-C.
//!
//! The loop ([`serve`]) splits the input at `\n` (C#'s `StreamReader` decodes it as strict UTF-8, a leading byte order mark skipped):
//! a line longer than 1,048,576 UTF-16 units is handed on as that many `x` plus one, which each server answers as too large; the
//! text after the last `\n` is a line too. Each line is handled on its own task, at most 16 at once, and an answer is written whole
//! with the OS's line end (`WriteLineAsync`: CRLF on Windows) and flushed. Answers may come back out of order.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use mighty_platform::BoxFuture;
use tokio::sync::{Semaphore, mpsc};

use crate::agent_io::wire::MAXIMUM_BYTES;

/// How many lines are handled at once (`SemaphoreSlim(16)`).
pub const CONCURRENT_LINES: usize = 16;

/// One server's handling of a line: its answer line, or None (a notification, a blank line).
pub type Handle = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// Where answers go (stdout, or a check's buffer).
pub type Output = Arc<Mutex<dyn Write + Send>>;

/// The input ended in the middle of bytes that are not UTF-8 (C#'s strict decoder throws, and the server dies with it).
#[derive(Debug)]
pub struct InvalidInput;

/// The line handed on for one longer than 1 MiB (`new string('x', MaximumBytes + 1)`).
pub fn overflow_line() -> String {
    "x".repeat(MAXIMUM_BYTES + 1)
}

/// Feeds `input`'s chunks (stdin's bytes as they come; an `Err` or the channel's end ends it) to `handle` until it ends, then waits for
/// every answer. Err when the input was not UTF-8 (the answers already started are still written).
pub async fn serve(mut input: mpsc::Receiver<io::Result<Vec<u8>>>, output: Output, handle: Handle) -> Result<(), InvalidInput> {
    let limit = Arc::new(Semaphore::new(CONCURRENT_LINES));
    let mut pending = vec![];
    let mut line: Vec<u8> = vec![];
    let mut overflow = false;
    // The byte order mark is looked for in the stream's first three bytes, which may come in more than one chunk.
    let mut start: Option<Vec<u8>> = Some(vec![]);
    let mut result = Ok(());
    let dispatch = |text: String, permit: tokio::sync::OwnedSemaphorePermit| {
        let (handle, output) = (handle.clone(), output.clone());
        tokio::spawn(async move {
            let _permit = permit;
            if let Some(answer) = handle(text).await {
                let mut out = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = out.write_all(answer.as_bytes());
                let _ = out.write_all(mighty_run::os::new_line().as_bytes());
                let _ = out.flush();
            }
        })
    };
    'reading: loop {
        let chunk = match input.recv().await {
            Some(Ok(chunk)) if !chunk.is_empty() => chunk,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break,
        };
        let chunk = match start.take() {
            Some(mut head) => {
                head.extend_from_slice(&chunk);
                if head.len() < 3 && b"\xEF\xBB\xBF".starts_with(&head) {
                    start = Some(head);
                    continue;
                }
                if head.starts_with(b"\xEF\xBB\xBF") { head.split_off(3) } else { head }
            }
            None => chunk,
        };
        for byte in chunk {
            if byte != b'\n' {
                if !overflow {
                    line.push(byte);
                    // Past three bytes a unit, a line is certainly longer than 1 MiB of UTF-16.
                    if line.len() > 3 * MAXIMUM_BYTES + 3 {
                        line.clear();
                        overflow = true;
                    }
                }
                continue;
            }
            let text = match finish(std::mem::take(&mut line), overflow) {
                Ok(text) => text,
                Err(error) => {
                    result = Err(error);
                    break 'reading;
                }
            };
            overflow = false;
            let permit = limit.clone().acquire_owned().await.expect("the semaphore is never closed");
            pending.retain(|task: &tokio::task::JoinHandle<()>| !task.is_finished());
            pending.push(dispatch(text, permit));
        }
    }
    if result.is_ok() {
        if let Some(head) = start.take() {
            line = head;
        }
        // The text after the last line end, unless it was too long (C# has dropped it by then).
        if !line.is_empty() {
            match finish(line, false) {
                Ok(text) if text.len() > MAXIMUM_BYTES && text == overflow_line() => {}
                Ok(text) => {
                    let permit = limit.clone().acquire_owned().await.expect("the semaphore is never closed");
                    pending.push(dispatch(text, permit));
                }
                Err(error) => result = Err(error),
            }
        }
    }
    for task in pending {
        let _ = task.await;
    }
    result
}

/// One line's text: the overflow line when it was longer than 1 MiB of UTF-16 units.
fn finish(bytes: Vec<u8>, overflow: bool) -> Result<String, InvalidInput> {
    if overflow {
        return Ok(overflow_line());
    }
    let text = String::from_utf8(bytes).map_err(|_| InvalidInput)?;
    if text.encode_utf16().count() > MAXIMUM_BYTES { Ok(overflow_line()) } else { Ok(text) }
}

/// The protocol revisions `initialize` echoes; any other is answered with `2025-06-18`.
pub fn protocol_version(asked: Option<&str>) -> &'static str {
    match asked {
        Some("2024-11-05") => "2024-11-05",
        Some("2025-03-26") => "2025-03-26",
        Some("2025-11-25") => "2025-11-25",
        _ => "2025-06-18",
    }
}

/// Stdin's bytes on a thread of their own (the exe modes': tokio's stdin is not used, so nothing but this thread reads it), as
/// chunks for [`serve`].
pub fn stdin_chunks() -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel(16);
    std::thread::Builder::new()
        .name("mcp-stdin".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match io::Read::read(&mut stdin, &mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if sender.blocking_send(Ok(buffer[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = sender.blocking_send(Err(error));
                        break;
                    }
                }
            }
        })
        .expect("the stdin reader");
    receiver
}

/// Stdout, for answers.
pub fn stdout() -> Output {
    Arc::new(Mutex::new(io::stdout()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo() -> Handle {
        Arc::new(|line: String| Box::pin(async move { (!line.trim().is_empty()).then(|| format!("<{}>", line.len())) }))
    }

    async fn run(chunks: Vec<&[u8]>) -> (String, bool) {
        let (sender, receiver) = mpsc::channel(64);
        for chunk in chunks {
            sender.send(Ok(chunk.to_vec())).await.unwrap();
        }
        drop(sender);
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let output: Output = buffer.clone();
        let ended = serve(receiver, output, echo()).await.is_ok();
        let text = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        (text, ended)
    }

    /// Lines split at `\n` across chunks, a leading byte order mark skipped, the last unterminated line handled, answers on their
    /// own lines with the OS's line end.
    #[tokio::test]
    async fn lines_are_split_and_answered_whole() {
        let (text, ended) = run(vec![b"\xEF\xBB", b"\xBFab\nc", b"d\n\n", b"tail"]).await;
        assert!(ended);
        let nl = mighty_run::os::new_line();
        let mut lines: Vec<&str> = text.split(nl).filter(|l| !l.is_empty()).collect();
        lines.sort();
        assert_eq!(lines, ["<2>", "<2>", "<4>"]);
    }

    /// A line longer than 1 MiB of UTF-16 is handed on as the overflow line; one that is not UTF-8 ends the loop.
    #[tokio::test]
    async fn long_and_broken_lines() {
        let long = vec![b'a'; MAXIMUM_BYTES + 1];
        let (text, _) = run(vec![&long, b"\n"]).await;
        assert!(text.starts_with(&format!("<{}>", MAXIMUM_BYTES + 1)));
        // 1 MiB of three-byte characters is 1 MiB of UTF-16 units: not too long.
        let wide = "가".repeat(MAXIMUM_BYTES / 2);
        let (text, _) = run(vec![wide.as_bytes(), b"\n"]).await;
        assert!(text.starts_with(&format!("<{}>", wide.len())));
        let (_, ended) = run(vec![b"\xff\xfe\n"]).await;
        assert!(!ended);
    }

    #[test]
    fn the_protocol_versions_echoed() {
        for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
            assert_eq!(protocol_version(Some(version)), version);
        }
        assert_eq!(protocol_version(Some("2099-01-01")), "2025-06-18");
        assert_eq!(protocol_version(None), "2025-06-18");
    }
}

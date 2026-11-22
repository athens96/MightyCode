//! The script lifecycle of a worktree (`MC-25`): which run actions a non-concurrent start stops, the dev-server address read from a
//! run action's output, and a script (the worktree's `teardown`) run with a time limit. The host (src-tauri `repo_tasks.rs`,
//! `worktree_lifecycle.rs`) owns the processes, the trust check and the panel.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_platform::{ProcessHost, SpawnRequest};

// ── the non-concurrent rule ─────────────────────────────────────────────────────────────────────────────────────────────

/// A run action running now: its name, the repository (normalized main checkout) and the worktree folder it runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningAction<'a> {
    pub action: &'a str,
    pub repo: &'a str,
    pub folder: &'a str,
}

/// The indexes in `running` a start of `action` in `folder` of `repo` stops first: with `concurrent: false`, the same action in the
/// repository's other worktrees. A concurrent action stops nothing; other repositories and other actions are never touched.
pub fn runs_to_stop(running: &[RunningAction<'_>], action: &str, repo: &str, folder: &str, concurrent: bool) -> Vec<usize> {
    if concurrent {
        return Vec::new();
    }
    running
        .iter()
        .enumerate()
        .filter(|(_, run)| run.action == action && run.repo == repo && run.folder != folder)
        .map(|(index, _)| index)
        .collect()
}

// ── the dev-server address ──────────────────────────────────────────────────────────────────────────────────────────────

/// A dev server's address found in a run action's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevUrl {
    /// The address to open: as printed, with `0.0.0.0` (every interface, which no browser opens) read as `localhost`.
    pub url: String,
    pub port: u16,
}

const LOCAL_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"];

/// The first `http(s)://(localhost|127.0.0.1|0.0.0.0|[::1]):<port>` address in `text` (ANSI codes already stripped), with the path
/// that follows it up to a space or a quote; trailing punctuation (`.`, `,`, `)`) is left out.
pub fn detect_dev_url(text: &str) -> Option<DevUrl> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("http") {
        let start = from + found;
        from = start + 4;
        let rest = &lower[start..];
        let after_scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            continue;
        };
        // A scheme glued to a word before it (`xhttp://`) is no address.
        if start > 0 && lower.as_bytes()[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let host_start = start + after_scheme;
        let Some(host) = LOCAL_HOSTS.iter().find(|host| lower[host_start..].starts_with(*host)) else { continue };
        let colon = host_start + host.len();
        if lower.as_bytes().get(colon) != Some(&b':') {
            continue;
        }
        let digits: String = lower[colon + 1..].chars().take_while(char::is_ascii_digit).take(6).collect();
        let Ok(port) = digits.parse::<u16>() else { continue };
        if port == 0 || digits.is_empty() {
            continue;
        }
        let mut end = colon + 1 + digits.len();
        // The path: up to a space, a quote, a bracket or a control character.
        if matches!(text.as_bytes().get(end), Some(b'/')) {
            let path_len = text[end..]
                .char_indices()
                .find(|(_, c)| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '[' | ']'))
                .map_or(text.len() - end, |(at, _)| at);
            end += path_len;
            while end > colon + 1 + digits.len() && matches!(text.as_bytes()[end - 1], b'.' | b',' | b')' | b';' | b':' | b'!' | b'?') {
                end -= 1;
            }
        }
        let printed = &text[start..end];
        let scheme = &printed[..after_scheme];
        let tail = &printed[after_scheme + host.len()..];
        let shown_host = if *host == "0.0.0.0" { "localhost" } else { host };
        return Some(DevUrl { url: format!("{}{shown_host}{tail}", scheme.to_ascii_lowercase()), port });
    }
    None
}

/// The port a fixed `run.<name>.url` names (`http://localhost:3000/app` -> 3000), None without one.
pub fn url_port(url: &str) -> Option<u16> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.port_or_known_default()
}

// ── a script with a time limit ──────────────────────────────────────────────────────────────────────────────────────────

/// How a script run with [`run_bounded`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOutcome {
    /// The exit code; None when it could not start or was stopped at the limit.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The end of its output and errors (at most [`ScriptOutcome::KEEP`] bytes), as written.
    pub output: String,
    /// Why it could not start.
    pub error: Option<String>,
}

impl ScriptOutcome {
    pub const KEEP: usize = 64 * 1024;

    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// How long the pipes may keep delivering once the script's shell has exited (a grandchild that inherited them must not hold the run).
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Runs `request` (stdin closed), reading its output, for at most `timeout`; past it the script and everything it started are ended.
pub async fn run_bounded(processes: &dyn ProcessHost, request: SpawnRequest, timeout: Duration) -> ScriptOutcome {
    let mut child = match processes.spawn(request).await {
        Ok(child) => child,
        Err(error) => return ScriptOutcome { exit_code: None, timed_out: false, output: String::new(), error: Some(error.to_string()) },
    };
    let control = child.control();
    drop(child.stdin.take());
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let kept: Arc<Mutex<Vec<u8>>> = Arc::default();

    let pump = |reader: Option<mighty_platform::process::ChildReader>, kept: Arc<Mutex<Vec<u8>>>| async move {
        use tokio::io::AsyncReadExt as _;
        let Some(mut reader) = reader else { return };
        let mut buffer = [0u8; 8192];
        while let Ok(read) = reader.read(&mut buffer).await {
            if read == 0 {
                return;
            }
            let mut kept = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            kept.extend_from_slice(&buffer[..read]);
            let over = kept.len().saturating_sub(ScriptOutcome::KEEP);
            if over > 0 {
                kept.drain(..over);
            }
        }
    };
    let reading = async {
        tokio::join!(pump(stdout, kept.clone()), pump(stderr, kept.clone()));
    };
    tokio::pin!(reading);
    let mut read_all = false;
    let waited = tokio::time::timeout(timeout, async {
        let wait = control.wait();
        tokio::pin!(wait);
        loop {
            tokio::select! {
                code = &mut wait => break code,
                () = &mut reading, if !read_all => read_all = true,
            }
        }
    })
    .await;
    let (exit_code, timed_out) = match waited {
        Ok(Ok(code)) => (Some(code), false),
        Ok(Err(_)) => (None, false),
        Err(_) => {
            control.kill();
            (None, true)
        }
    };
    if !read_all {
        let _ = tokio::time::timeout(PIPE_DRAIN_GRACE, &mut reading).await;
    }
    let _ = control.close().await;
    let bytes = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    ScriptOutcome { exit_code, timed_out, output: String::from_utf8_lossy(&bytes).into_owned(), error: None }
}

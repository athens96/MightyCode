//! The scripted Claude (C# Core/DelegationFakeClaude.cs; M/AppStore+DelegationSmoke.swift:7-12, the Mac's Tests/FakeClaude): the app's
//! own exe in a second mode, `--fake-claude <absolute root> <the CLI's arguments>`, which the delegation smoke and the scenarios start in
//! the CLI's place. Kept in the shipping exe because docs/tauri-conversion.md section 1 lists the mode. Owner: W7-C.
//!
//! What it must do (W7 plan, W7-C): stream-json in and out, `initialize` answered; `--version` gives `2.1.271 (Claude Code)` and
//! `--no-session-persistence` exits 0 (:53-55); the steps of `scripts/<ClaudeProjectFolder(cwd)>.json` (`{turns:[[step]]}`) and
//! `child-scripts.json` (`[{phrase, steps}]`): say, call, permission, run, write, sleep, exit; the logs `logs/<key>.jsonl` and
//! `claude-config/projects/<key>/<session>.jsonl`; `--mcp-config`'s servers started with the inherited environment. Every child goes
//! through the given `ProcessHost` (it ships in the exe: no `Command::new`). Until W7-C fills it, it ends at once with 0.

use std::path::Path;
use std::sync::Arc;

use mighty_platform::ProcessHost;

/// `DelegationFakeClaude.Argument`.
pub const ARGUMENT: &str = "--fake-claude";

/// `DelegationFakeClaude.RunAsync(root, args)`: `root` is the fully qualified folder after [`ARGUMENT`], `args` the CLI's arguments
/// after it; the process's exit code.
pub fn run(root: &Path, args: &[String], processes: Arc<dyn ProcessHost>) -> i32 {
    let _ = (root, args, processes);
    0
}

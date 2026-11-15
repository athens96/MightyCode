//! The help capture's canned CLI (C# `WinUI\HelpDemoCli.cs`; plan §1.8 "Help capture", W9-H): the app's own binary in another mode,
//! `--help-demo-cli <absolute root> …`, installed as `claude` / `codex` in the help capture's demo profile, answering each request with
//! the canned stream the pictures need. Served by main.rs's `exe_mode` before anything else, beside [`crate::fake_claude`]. Owner: W9-H.

/// The mode's first argument (C# `HelpDemoCli.Argument`).
pub const ARGUMENT: &str = "--help-demo-cli";

/// Serves the mode when `args` start with it (the root absolute), with the exit code; None otherwise. Step 0 stub (W9-H writes it):
/// None, so the arguments reach the normal parser, which refuses them, as before W9.
pub fn try_run(_args: &[String]) -> Option<i32> {
    None
}

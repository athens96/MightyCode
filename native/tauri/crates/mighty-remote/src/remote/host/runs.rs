//! The sharing host's runs (C# `RemoteHostService.Runs`, R\RemoteHostService.Runs.cs; plan §1.6): `StartRunAsync`'s order (415, 400
//! empty, 400 decode, 413 over 512 KiB, 400 models, 400 `Validate`, 403 `notShared`, 403 full access, 400 shell too long, 429 at 16, 503
//! admission, Claude, the Antigravity grant, under the lock 503/429, 202), the jobs and their buffers (4096 events, 4 MiB; a poll at most
//! 256 events and 1.5 MiB; finished runs kept 2 minutes, 32 at most), stop, steer, permission, questions, plan; the host's runner is a
//! `RunManager` with PowerShell commands, no pane MCP, no delegation and no picture cache. Owner: W8-D.
//!
//! At rest (W8 Step 0): empty.

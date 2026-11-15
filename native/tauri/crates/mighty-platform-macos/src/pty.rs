//! M-A: the PTY host, which also feeds the Ghostty view (plan D2: Rust owns every terminal's PTY). Swift
//! `MightyCore/PTYAgentTerminalPane.swift` (352 lines), `AgentTerminalRunner.swift` (203), `AgentProcessRegistry.swift` (92),
//! `ShellProcessProbe.swift` (98), `ShellActivity.swift` (21); contract `mighty_platform::pty` (`PtyHost`, `PtyControl`).
//!
//! To port (plan §1.3): `forkpty` with every signal blocked across the fork, the child setting IUTF8, resetting signals and closing
//! fds 3…65536, a 30 s ready-pipe handshake, the master non-blocking, writes retried for 2 s, `TIOCSWINSZ`, exit code 128+signal,
//! `kill(-pid, …)`; `busy` = `tcgetpgrp(master) != shell pgid`. The default shell `$SHELL` else `/bin/zsh`, `-il` (`-f` in a smoke);
//! `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=ghostty` and Ghostty's shell integration (zsh `ZDOTDIR`), checked
//! with M-D. The Step 0 spike's `native/tauri/spikes/mac/ghostty/src/pty.rs` is the least of it, proven on CI.

use std::sync::Arc;

use mighty_platform::PtyHost;

/// The Mac's `PtyHost`: until M-A's port lands, M-D's minimal `forkpty` host (`terminal_surface::pty`), so the Ghostty view has a
/// shell to draw. M-A's `MacPtyHost` replaces this line and that module.
pub fn pty_host() -> Arc<dyn PtyHost> {
    crate::terminal_surface::pty::pty_host()
}

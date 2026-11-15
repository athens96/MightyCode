//! The data an app from before the rename left under its old names (C# `Core\RenameMigration.cs:83-182`, `WinUI\Program.cs:163-205`;
//! docs/rename-identifiers.md; plan §1.8 "RenameMigration", W9-F). Owner: W9-F.
//!
//! For the default profile only, outside a smoke, where the app may change the machine, before anything reads the data: move
//! `%APPDATA%\MightyClaudeNative` to `MightyCode` when the target is absent; move `~\.mightyclaude` to `~\.mightycode` and, if moved,
//! rewrite the paths in `delegation-state.json` and `workspace-state.json` (plain and JSON-escaped, only before a separator), move
//! `~/.claude/projects/<old worktrees prefix>-*` to the new prefix and list the worktrees; copy HKCU `Software\MightyClaudeNative` to
//! `Software\MightyCode` once (values and subkeys; the old key stays); after start, `git worktree repair` in each moved worktree. Pure
//! over a file-system and registry seam (`move_once`, `rewritten`, `move_claude_projects`, `copy_registry`); src-tauri's `startup.rs`
//! runs it. It replaces mighty-platform-windows' `LEGACY_FOLDERS` copy.
//!
//! Step 0: empty; W9-F writes it.

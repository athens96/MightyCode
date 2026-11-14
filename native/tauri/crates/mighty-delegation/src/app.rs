//! The desktop service's delegation (C# Core/DesktopService.Delegation.cs; DesktopService.cs:129-155, 261-273, 346-351):
//! `DelegationApp::{start, restart, second_send, worktree_child_choices, start_human_child, send, run_next, close_session,
//! workspace_removal_refusal, merge_child, undo_child_merge, ask_to_discard, discard_child, card_action, sidebar_rows, window_inputs}`
//! over an `Arc<mighty_run::RunService>`, the coordinator, a `StateAccess` and a `Notes` sink; no Tauri, so the end-to-end scenarios
//! run headless in `cargo test`. It implements [`crate::host::DelegationHost`] and mighty-run's `service::DelegationRunHooks`; the
//! merge queue retries every 10 s on its clock. src-tauri delegation.rs drives it. Owner: W7-D.

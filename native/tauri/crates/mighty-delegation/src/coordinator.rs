//! The coordinator (C# Core/DelegationCoordinator.cs; M/DelegationCoordinator.swift): the manifest, the gates in order
//! (:253-261), `ArgumentProblem` (:280-288), caps (3 agent-made + 3 human, depth 1, 2 follow-ups, 48 KiB), `LateMessage` after 45 s,
//! the FIFO turns for merges and run events, and the seams W7-D and W7-C call: `Coordinator::{new, handle, start_human_child,
//! close_child, discard_child, merge_from_card, undo_merge_from_card, run_started, run_ended, deliver_pending, release,
//! held_items_by_pane, rows, interrupt_runs_killed_by_quit}` (it implements [`crate::request::DelegationHandler`]). Concurrency (W7 plan
//! section 1.9): state behind a `std::sync::Mutex` held only across synchronous mutate-and-save sections, never across an await.
//! Owner: W7-A.

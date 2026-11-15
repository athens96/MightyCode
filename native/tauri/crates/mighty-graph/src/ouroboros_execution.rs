//! Ouroboros executions (C# `Core\OuroborosExecution.cs`, 730 lines; plan §1.8 "The Ouroboros dashboard", W9-E). Owner: W9-E.
//!
//! The pure half: `OuroborosExecutionLinks.Extract` (the executions a run's stream names), the daemon's two loopback GET endpoints'
//! answers read and bounded, the polling and back-off rules and the dashboard's address. The loopback client (through mighty-run's
//! loopback HTTP) and starting the daemon sparingly without a shell (through `ProcessHost`) are src-tauri's `ouroboros.rs`;
//! `DEFERRED.md` lines 76–77 go once it lands.
//!
//! Step 0: empty; W9-E writes it.

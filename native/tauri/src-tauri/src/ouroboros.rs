//! The Ouroboros dashboard (C# `WinUI\MainWindow.GraphOuroboros.cs` over `Core\OuroborosExecution.cs`, ported pure in mighty-graph's
//! `ouroboros_execution`; plan §1.8, W9-E). Owner: W9-E.
//!
//! The daemon's executions read through its two loopback GET endpoints (mighty-run's loopback HTTP), polled with back-off; the daemon
//! started sparingly without a shell (through `ProcessHost`); `mightyOpenDashboard` opening the execution's dashboard.

use std::sync::Arc;

use crate::host::Host;

/// After the run engine (main.rs). Step 0 stub (W9-E writes it): nothing polls.
pub fn start(_host: &Arc<Host>) {}

/// At quit (main.rs), before the runs are disposed: the polling stopped. Step 0 stub (W9-E writes it).
pub fn shutdown(_host: &Arc<Host>) {}

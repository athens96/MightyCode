//! Styles in the host (C# `WinUI\MainWindow.Styles.cs`, `MainWindow.StyleApproval.cs`, `MainWindow.WebShell.Guided.cs`,
//! `MainWindow.Terminal.cs:20` `RunTerminalCommandAsync`, the style parts of `MainWindow.MobileRemote.cs`; plan §1.7, W9-D). Owner: W9-D.
//!
//! mighty-styles' registry, trust store and projection for each pane: the composer's `style` row and `guided` panel, the style menu
//! rows, `guidedAction{paneId, what, id}`, Enter rewritten to the style's slash command, the approval, remove and install sheets
//! (D11–D13), the install through a command typed into a new shell pane (`terminal::Pending` removed) where the host may change the
//! machine, the plan-mode launch, the Mighty views' `StyleSource`, the change watcher (off in a smoke) and the phone's style parts.
//! Writes (`approvals.json`, a registered manifest copied to `<data>\styles`, a removal) only where `Host::writes` allows.

use std::sync::Arc;

use crate::host::{Host, Registry};

/// The page method this part answers: `guidedAction`. Step 0: none registered, so it stays on the unimplemented list (README) until W9-D.
pub fn register(_methods: &mut Registry) {}

/// After the run engine (main.rs): the registry's first scan and the change watcher. Step 0 stub (W9-D writes it): nothing starts.
pub fn start(_host: &Arc<Host>, _platform: &mighty_platform::Platform) {}

/// At quit (main.rs), before the runs are disposed: the watcher stopped. Step 0 stub (W9-D writes it).
pub fn shutdown(_host: &Arc<Host>) {}

//! Background output tails (C# `WinUI\MainWindow.BackgroundOutput.cs`, `Core\BackgroundOutputTail.cs` ported pure in mighty-run's
//! `background_output`; plan §1.8, W9-E). Owner: W9-E.
//!
//! Once a second, off the dispatcher, the end of every running task block's output file is read (one more read after a block ends; a
//! 10 s timeout skips a hung file); the phone's revision moves at most every 3 s; nothing is saved; a remote pane has none. Feeds
//! mighty-graph's `Sources.tails` and the phone's `RunsInputs.output_tails`.

use std::sync::Arc;

use crate::host::Host;

/// After the run engine (main.rs): the poller. Step 0 stub (W9-E writes it): nothing polls.
pub fn start(_host: &Arc<Host>) {}

/// At quit (main.rs), before the runs are disposed: the poller stopped. Step 0 stub (W9-E writes it).
pub fn shutdown(_host: &Arc<Host>) {}

//! The app update in the host (C# `WinUI\MainWindow.AppUpdate.cs` whole, `MainWindow.WebShell.Shell.cs:130-131,478-500`,
//! `Core\PhoneAppUpdate.cs`; the Mac's `AppStore+AppUpdate.swift` for the idle facts; plan §1.4, W9-C). Owner: W9-C.
//!
//! mighty-updates' coordinator over its `Service` with `build_info`'s key and address, this machine's architecture and the exe's folder
//! as the install; the startup housekeeping (restore, prune, `SyncDisplayVersion`, the renamed shortcuts) where the host may change the
//! machine, never in a smoke; the 10 min / 1 s loop as a task off the bridge, started after the first render, never in a smoke; the idle
//! facts; the countdown banner and the status bar's update word through the shell model; the page methods `postponeUpdate` and
//! `restartForUpdate`; the helper's launch through `ProcessHost::start_detached` (hidden, in `%TEMP%`, only `SystemRoot`); the install on
//! quit from the quit path (skipped while Windows ends the session); `appUpdateLastCheckedAt` and `appUpdateAutoCheck` through the store.

use std::sync::Arc;

use crate::host::{Host, Registry};

/// The page methods this part answers: `postponeUpdate` (C#: snooze 4 h) and `restartForUpdate` (C#: install now and relaunch). Step 0:
/// none registered, so both stay on the unimplemented list (README) until W9-C.
pub fn register(_methods: &mut Registry) {}

/// After the window exists (main.rs): the housekeeping and the update loop. Step 0 stub (W9-C writes it): nothing starts.
pub fn start(_host: &Arc<Host>, _platform: &mighty_platform::Platform) {}

/// The quit path's first step (main.rs, C#'s `InstallOnQuitAsync` before the shutdown): a ready update installs with `--relaunch 0`
/// unless `powering_off`. Step 0 stub (W9-C writes it): nothing happens.
pub fn quit_hook(_host: &Arc<Host>, _powering_off: bool) {}

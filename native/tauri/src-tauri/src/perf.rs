//! The perf scenario (C# `WinUI\MainWindow.PerfScenario.cs`; `scripts\perf-scenario.ps1 -Engine tauri`; plan §1.8 "Perf scenario",
//! W9-H). Owner: W9-H.
//!
//! `--smoke-test --perf-scenario [--smoke-exit] --profile <empty>`: the scenario's seed and steps in the smoke's place, writing
//! `perf-result.json` (C#'s keys), `perf.log` and `startup.log`.

use crate::host::Host;

/// Called by the smoke once the page is up (smoke.rs) in a `--perf-scenario` run: true when the scenario took over from the smoke's
/// probe. Step 0 stub (W9-H writes it): false, so the run is a plain smoke.
pub fn begin(_host: &Host) -> bool {
    false
}

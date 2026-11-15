//! The help site's pictures (C# `WinUI\MainWindow.HelpCapture.cs`, `MainWindow.HelpCaptureDelegation.cs`; plan §1.8 "Help capture",
//! D16, W9-H). Owner: W9-H.
//!
//! `--help-capture --profile <empty> [-language ko|en|zh|ja] [--smoke-exit]` runs in the smoke's place (args.rs makes it a smoke): the
//! demo profile seeded (mighty-core's `demo`), `--help-demo-cli` installed as the CLI, `docs/help/shots.json` walked (`MIGHTY_HELP_SHOTS`
//! filters), the language and theme set, each shot captured through WebView2 (`CapturePreview`, or CDP with `--remote-debugging-port`)
//! into `<profile>\help-shots\<lang>\<id>-<theme>.png` with `manifest.json` and `smoke-result.json` in C#'s shape. The help site keeps the
//! Mac's pictures.

use crate::host::Host;

/// Called by the smoke once the page is up (smoke.rs) in a `--help-capture` run: true when the capture took over from the smoke's probe.
/// Step 0 stub (W9-H writes it): false, so the run is a plain smoke.
pub fn begin(_host: &Host, _language: &str) -> bool {
    false
}

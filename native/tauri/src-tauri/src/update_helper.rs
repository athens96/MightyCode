//! The exe's `--update-helper` mode (C# `WinUI\Program.cs:158,215-271` `RunUpdateHelper`; plan §1.1, W9-B). Owner: W9-B.
//!
//! The 0.4.x app (and later this one) unpacks the package to `updates\<version>\staged` and starts the staged `MightyCode.exe` with the
//! helper's arguments, detached, hidden, in `%TEMP%`, with only `SystemRoot` set. main.rs serves it right after the exe's other modes,
//! before the arguments are parsed, before Tauri, WebView2, the profile, the log or the platform. Here: the plan parsed
//! (`mighty_updates::app_update::plan::try_parse`), the mutex taken through the platform's `UpdateInstaller` (another helper holding it:
//! skipped, exit 0), `replacement::run` with the Windows seams (the start through `ProcessHost::start_detached` with the fresh
//! environment), `SyncDisplayVersion` on success with this exe's `major.minor.patch`, one `install.log` line, the mutex released, exit 0
//! when replaced or skipped, else 1; all inside `catch_unwind`, the sentences in the OS's language, no Tauri, no profile, no other log.

/// The helper when `args` are its arguments, with the exit code; None otherwise (the arguments then go to the parser, which refuses
/// `--update-helper`, as C#'s does). Step 0 stub (W9-B writes it): None until W9-A's `try_parse` reads a plan.
pub fn try_run(args: &[String]) -> Option<i32> {
    mighty_updates::app_update::plan::try_parse(args).map(|_plan| 1)
}

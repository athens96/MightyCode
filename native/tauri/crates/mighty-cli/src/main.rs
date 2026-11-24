//! `mighty.exe` (MC-43): runs `MightyCode.exe --cli <arguments>` on this console and ends with its exit code. See the library.

use std::ffi::OsString;
use std::process::ExitCode;

use mighty_cli::{APP_VARIABLE, OPEN_UNSUPPORTED, Route, app_path, exit_code, route};

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if route(&args) == Route::Open {
        eprintln!("{OPEN_UNSUPPORTED}");
        return ExitCode::FAILURE;
    }
    let own = std::env::current_exe().unwrap_or_default();
    let app = app_path(&own, std::env::var_os(APP_VARIABLE));
    if !app.is_file() {
        eprintln!("mighty: {} is not there; install Mighty Code again (or set {APP_VARIABLE})", app.display());
        return ExitCode::FAILURE;
    }
    console::wait_through_ctrl_c();
    // One child, started once, with this program's standard handles; nothing else runs here, so no other child can inherit them.
    #[allow(clippy::disallowed_methods, reason = "this program's only child, started before anything else could spawn")]
    let status = std::process::Command::new(&app).arg("--cli").args(&args).status();
    match status {
        // The app's own exit code, all 32 bits (Windows keeps them; ExitCode takes only a byte).
        Ok(status) => std::process::exit(exit_code(status.code())),
        Err(error) => {
            eprintln!("mighty: {} did not start: {error}", app.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
mod console {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
    use windows_sys::core::BOOL;

    /// Ctrl+C and Ctrl+Break are the app's (it is on this console and ends, `mighty events` included): this program keeps waiting
    /// for it and ends with its exit code. A handler, not `SetConsoleCtrlHandler(NULL, TRUE)`, whose "ignore" the app would inherit.
    unsafe extern "system" fn keep_waiting(event: u32) -> BOOL {
        BOOL::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }

    pub fn wait_through_ctrl_c() {
        // SAFETY: a handler with the documented signature that touches nothing.
        unsafe { SetConsoleCtrlHandler(Some(keep_waiting), 1) };
    }
}

#[cfg(not(windows))]
mod console {
    /// Elsewhere the terminal's signal reaches the app and this program alike (the Mac uses its shell script, not this program).
    pub fn wait_through_ctrl_c() {}
}

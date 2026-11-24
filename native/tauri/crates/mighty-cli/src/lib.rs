//! MC-43: `mighty.exe`, the Windows `mighty` command (docs/windows-cli.md). The Mac's command is a shell script the app writes
//! (src-tauri `shell_command.rs`); Windows gets a small console program in the package's `bin` folder instead, because:
//! - a `mighty.cmd` would hand the words to cmd.exe a second time: `%*` is parsed again, so `&`, `|`, `<`, `>` and `^` in a request
//!   that PowerShell passed unquoted (`mighty agent send p1 a&b`) run as commands, and Ctrl+C asks "Terminate batch job (Y/N)?";
//! - a console program gets its arguments as Windows split them, waits for the app, passes its exit code (0, 1, 2) and standard
//!   input and output (pipes and files included) through untouched, and is the same in cmd, PowerShell, Windows Terminal and Git Bash.
//!
//! It runs the windowed app's exe as `MightyCode.exe --cli <its arguments>`, which attaches to this console (src-tauri
//! `platform::attach_parent_console`). Only the control commands go there: opening a folder from the command line (the Mac's
//! `mighty <folder> [request]`) has no route on Windows yet, and says so.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The words that make `mighty` a control command (the Mac script's `case`): the app's subcommands and the options before them.
pub const CONTROL_WORDS: [&str; 9] = ["status", "workspace", "pane", "agent", "events", "server", "--profile", "--help", "-h"];

/// The app's exe name, in the install folder.
pub const APP_EXE: &str = "MightyCode.exe";

/// The variable that names another app exe (as the Mac script's `MIGHTY_CODE_APP`).
pub const APP_VARIABLE: &str = "MIGHTY_CODE_APP";

/// What the arguments are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The app's CLI (`MightyCode.exe --cli …`); no arguments at all show its usage.
    Cli,
    /// A folder or a request to open: not on Windows yet.
    Open,
}

/// Where the arguments go: a control word first (or nothing) is the CLI's, anything else would open a folder.
pub fn route(args: &[OsString]) -> Route {
    match args.first().and_then(|first| first.to_str()) {
        None if args.is_empty() => Route::Cli,
        Some(first) if CONTROL_WORDS.contains(&first) => Route::Cli,
        _ => Route::Open,
    }
}

/// What `mighty <folder>` says on Windows.
pub const OPEN_UNSUPPORTED: &str = "mighty: opening a folder from the command line (mighty [folder] [request]) is not on Windows yet; \
    add the folder in Mighty Code. mighty --help lists the commands.";

/// The app this command runs: `MIGHTY_CODE_APP` when set, else `MightyCode.exe` in the install folder, which is the parent of this
/// program's `bin` folder (or this program's own folder when it is not in one called `bin`).
pub fn app_path(own_exe: &Path, variable: Option<OsString>) -> PathBuf {
    if let Some(app) = variable.filter(|value| !value.is_empty()) {
        return PathBuf::from(app);
    }
    let folder = own_exe.parent().unwrap_or(Path::new(""));
    let in_bin = folder.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.eq_ignore_ascii_case("bin"));
    let install = if in_bin { folder.parent().unwrap_or(folder) } else { folder };
    install.join(APP_EXE)
}

/// The exit code the command ends with for the app's exit `code` (None when it had none: ended by a signal, never on Windows).
pub fn exit_code(code: Option<i32>) -> i32 {
    code.unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// The control words are the app CLI's subcommands, and the options that may come before them, as the Mac script reads them.
    #[test]
    fn the_control_words_are_the_apps_subcommands() {
        for subcommand in mighty_core::control::cli::SUBCOMMANDS {
            assert!(CONTROL_WORDS.contains(&subcommand), "{subcommand}");
        }
        assert_eq!(CONTROL_WORDS.len(), mighty_core::control::cli::SUBCOMMANDS.len() + 3);
    }

    #[test]
    fn control_commands_go_to_the_cli_and_folders_do_not() {
        assert_eq!(route(&[]), Route::Cli, "no arguments: the usage");
        assert_eq!(route(&words(&["status", "--json"])), Route::Cli);
        assert_eq!(route(&words(&["agent", "send", "p1", "a&b | c"])), Route::Cli);
        assert_eq!(route(&words(&["--profile", r"D:\work\", "pane", "list"])), Route::Cli);
        assert_eq!(route(&words(&["server", "start"])), Route::Cli);
        assert_eq!(route(&words(&["-h"])), Route::Cli);
        assert_eq!(route(&words(&["."])), Route::Open);
        assert_eq!(route(&words(&[r"C:\code\app", "fix the tests"])), Route::Open);
        assert_eq!(route(&words(&["fix the tests"])), Route::Open);
        assert_eq!(route(&words(&["Status"])), Route::Open, "the words are exact, as the app's CLI reads them");
    }

    /// The app is the install folder's exe (the parent of `bin`), or the one `MIGHTY_CODE_APP` names.
    #[test]
    fn the_app_is_beside_the_bin_folder_unless_named() {
        // Forward slashes: a separator on both OSes.
        let own = Path::new("C:/Users/me/AppData/Local/Programs/MightyCode/bin/mighty.exe");
        let install = Path::new("C:/Users/me/AppData/Local/Programs/MightyCode");
        assert_eq!(app_path(own, None), install.join(APP_EXE));
        assert_eq!(app_path(Path::new("C:/Users/me/AppData/Local/Programs/MightyCode/BIN/mighty.exe"), None), install.join(APP_EXE));
        assert_eq!(app_path(Path::new("D:/tools/mighty.exe"), None), Path::new("D:/tools").join(APP_EXE), "not in bin: beside it");
        assert_eq!(app_path(own, Some(OsString::from("D:/dev/MightyCode.exe"))), PathBuf::from("D:/dev/MightyCode.exe"));
        assert_eq!(app_path(own, Some(OsString::new())), install.join(APP_EXE), "an empty variable is no variable");
    }

    #[test]
    fn the_apps_exit_code_is_passed_on() {
        assert_eq!(exit_code(Some(0)), 0);
        assert_eq!(exit_code(Some(2)), 2);
        assert_eq!(exit_code(Some(0xC000_013Au32 as i32)), 0xC000_013Au32 as i32, "Ctrl+C's STATUS_CONTROL_C_EXIT");
        assert_eq!(exit_code(None), 1);
    }
}

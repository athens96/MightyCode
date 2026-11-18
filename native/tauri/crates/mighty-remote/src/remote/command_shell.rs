//! The sharing host's command line (C# `RemoteCommandShell`, R\RemoteCommandShell.cs; plan §1.6): Windows PowerShell from the system
//! folder Windows reports (never a PATH or current-folder lookup), `-NoProfile -NonInteractive -Command <text>` (a command that prompts
//! fails at once instead of waiting on the host's desktop), one process per command in the shared folder; mighty-run's runner adds
//! `NoDefaultCurrentDirectoryInExePath=1`. The Mac host runs the same text in its shell. Owner: W8-D.

/// `NoDefaultCurrentDirectoryInExePath`: nothing in the shared folder may stand in for a program the command names.
pub const NO_CURRENT_DIRECTORY_SEARCH: &str = "NoDefaultCurrentDirectoryInExePath";

/// CreateProcess's limit with its terminating NUL.
pub const MAXIMUM_COMMAND_LINE: usize = 32767;

/// `RemoteCommandShell.Launch(command, systemFolder)`: the program and its arguments; the text is the one argument after `-Command`,
/// so it can never add another. Err when `system_folder` is not a full drive path (an empty, relative or drive-relative one would
/// resolve against the current folder).
pub fn launch(command: &str, system_folder: &str) -> Result<(String, Vec<String>), String> {
    let bytes = system_folder.as_bytes();
    let full = system_folder.encode_utf16().count() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && !system_folder.contains(['"', '\0'])
        && !system_folder.split(['\\', '/']).any(|part| part == "." || part == "..");
    if !full {
        return Err("The system folder must be a full path.".into());
    }
    let binary = format!("{}\\WindowsPowerShell\\v1.0\\powershell.exe", system_folder.trim_end_matches(['\\', '/']));
    Ok((binary, vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), command.into()]))
}

/// `RemoteCommandShell.CommandLine`: the command line CreateProcess receives (each argument quoted as C# `ChildProcess.QuoteWindows`).
pub fn command_line(command: &str, system_folder: &str) -> Result<String, String> {
    let (binary, arguments) = launch(command, system_folder)?;
    Ok(std::iter::once(binary.as_str()).chain(arguments.iter().map(String::as_str)).map(quote_windows).collect::<Vec<_>>().join(" "))
}

/// `RemoteCommandShell.Fits`: the command line fits CreateProcess's limit (counted in UTF-16 units, as C#'s `string.Length`).
pub fn fits(command: &str, system_folder: &str) -> Result<bool, String> {
    Ok(command_line(command, system_folder)?.encode_utf16().count() < MAXIMUM_COMMAND_LINE)
}

/// C# `ChildProcess.QuoteWindows`: as it is when it is not empty and holds no white space or quote; else in quotes, a quote escaped,
/// and a backslash run doubled before a quote or the closing quote (mighty-platform-windows `command::quote_windows`).
fn quote_windows(argument: &str) -> String {
    if !argument.is_empty() && !argument.chars().any(|c| c.is_whitespace() || c == '"') {
        return argument.to_string();
    }
    let mut result = String::from("\"");
    let mut slashes = 0usize;
    for c in argument.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        let run = if c == '"' { slashes * 2 + 1 } else { slashes };
        result.extend(std::iter::repeat_n('\\', run));
        slashes = 0;
        result.push(c);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

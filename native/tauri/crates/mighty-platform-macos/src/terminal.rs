//! The Mac's terminal window (contract `mighty_platform::console`): a script the app wrote opened in Terminal.app, a window of its own
//! that outlives the app, as Swift's setup screen ran its install in a terminal window (`SetupTerminalWindow`). The setup screen falls
//! back to it when this Mac has no local workspace to hold its terminal pane (src-tauri setup.rs `start_install`).
//!
//! No command line is made from text and no process is started by the app: the one script file is handed to Terminal.app through
//! LaunchServices (`NSWorkspace openURLs:withApplicationAtURL:configuration:completionHandler:`, what `open -a Terminal <file>` does),
//! and Terminal runs it in a new window. The script must be a regular file of this user's (a link is refused, never followed); it is
//! made runnable by its owner alone (0700), which Terminal needs to run it.

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::{BoxFuture, PlatformError, Result, TerminalLaunch, TerminalLauncher, TerminalWindow};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSString, NSURL};

/// Terminal.app's bundle identifier.
pub const TERMINAL_BUNDLE: &str = "com.apple.Terminal";

/// The Mac's `TerminalLauncher`.
pub fn terminal_launcher() -> Arc<dyn TerminalLauncher> {
    Arc::new(TerminalApp)
}

/// Opens a script in a Terminal.app window.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalApp;

/// Terminal took the script into a window of its own: the app holds no process of it (its end is seen by the script's own markers).
struct Opened;

impl TerminalWindow for Opened {
    fn holds_command(&self) -> bool {
        false
    }
    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        Box::pin(async { Ok(0) })
    }
}

/// The one script a launch may name: `argv` is exactly one absolute path (`resolved`, when given, must be the same file), to a
/// regular file this user owns. Anything else (a program with arguments, a link, another user's file) is refused before anything
/// opens.
pub fn script_of(launch: &TerminalLaunch) -> Result<PathBuf> {
    let [path] = launch.argv.as_slice() else {
        return Err(PlatformError::Invalid("the Mac's terminal window runs one script file the app wrote".into()));
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() || launch.resolved.as_deref().is_some_and(|resolved| Path::new(resolved) != path) {
        return Err(PlatformError::Invalid("the script needs its absolute path".into()));
    }
    let meta = std::fs::symlink_metadata(&path)?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::Refused(format!("not a plain file: {}", path.display())));
    }
    // SAFETY: getuid has no preconditions.
    if meta.uid() != unsafe { libc::getuid() } {
        return Err(PlatformError::Refused(format!("the script belongs to another user: {}", path.display())));
    }
    Ok(path)
}

/// Makes the script runnable by its owner alone, through the file itself (opened without following a link).
fn make_runnable(path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(PlatformError::Refused(format!("not a plain file: {}", path.display())));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

impl TerminalLauncher for TerminalApp {
    fn open(&self, launch: TerminalLaunch) -> Result<Box<dyn TerminalWindow>> {
        let script = script_of(&launch)?;
        make_runnable(&script)?;
        let opened = autoreleasepool(|_| {
            let workspace = NSWorkspace::sharedWorkspace();
            let Some(terminal) = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(TERMINAL_BUNDLE)) else {
                return false;
            };
            let file = NSURL::fileURLWithPath(&NSString::from_str(&String::from_utf8_lossy(script.as_os_str().as_bytes())));
            let configuration = NSWorkspaceOpenConfiguration::configuration();
            configuration.setActivates(true);
            workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&[file]),
                &terminal,
                &configuration,
                None,
            );
            true
        });
        if !opened {
            return Err(PlatformError::Refused("Terminal.app is not on this Mac".into()));
        }
        Ok(Box::new(Opened))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only one absolute script path of this user's goes to Terminal: a program with arguments, a relative path, a link and a folder
    /// are refused before anything opens; the script is made runnable by its owner alone.
    #[test]
    fn only_one_script_file_of_this_users_is_handed_to_terminal() {
        let root = std::env::temp_dir().join(format!("terminal-app-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("install-1.command");
        std::fs::write(&script, "#!/bin/bash\necho one\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
        let launch = |argv: &[&str]| TerminalLaunch { argv: argv.iter().map(|a| a.to_string()).collect(), ..TerminalLaunch::default() };
        let text = script.to_string_lossy().into_owned();
        assert_eq!(script_of(&launch(&[&text])).unwrap(), script);
        assert!(script_of(&launch(&["/bin/bash", &text])).is_err(), "a program with arguments");
        assert!(script_of(&launch(&[])).is_err(), "nothing");
        assert!(script_of(&launch(&["install-1.command"])).is_err(), "a relative path");
        assert!(script_of(&TerminalLaunch { resolved: Some("/bin/sh".into()), ..launch(&[&text]) }).is_err(), "another file resolved");
        let link = root.join("linked.command");
        std::os::unix::fs::symlink(&script, &link).unwrap();
        assert!(script_of(&launch(&[&link.to_string_lossy()])).is_err(), "a link");
        assert!(make_runnable(&link).is_err(), "a link is not followed");
        assert!(script_of(&launch(&[&root.to_string_lossy()])).is_err(), "a folder");
        assert!(script_of(&launch(&[&root.join("gone").to_string_lossy()])).is_err(), "nothing there");
        make_runnable(&script).unwrap();
        assert_eq!(std::fs::metadata(&script).unwrap().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

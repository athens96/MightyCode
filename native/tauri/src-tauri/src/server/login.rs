//! MC-3: 로그인할 때 창 없이 시작 (off unless the person turns it on): the OS starts the app as `--server` when the user logs in.
//!
//! - The Mac: a LaunchAgent, `~/Library/LaunchAgents/<label>.plist` (`<identifier>.server`, a profile's own label beside it),
//!   `ProgramArguments` the app's exe and `--server` (and `--profile`), `RunAtLoad`, no `KeepAlive` (a server that was stopped stays
//!   stopped until the next login). Written to a temporary file and renamed over the old one, mode 0644; removed by deleting it. It is
//!   not loaded now (`launchctl` is never run): it takes effect at the next login.
//! - Windows: a value of `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`Mighty Code (server)`, a profile's own name beside
//!   it), the exe's quoted path and `--server` (and `--profile "<folder>"`); one registry write, removed by deleting the value.
//!
//! On means the item is there; what Settings shows is read from the disk or the registry each time. Tests use a temporary folder and
//! a fake registry.

use std::io;
use std::path::{Path, PathBuf};

use crate::host::Host;

/// The Run key, below HKEY_CURRENT_USER.
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// What the OS starts at login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItem {
    /// The app's identifier (`dev.mightycode.native`).
    pub identifier: String,
    /// The app's exe.
    pub program: PathBuf,
    /// The app's `--profile`, if any.
    pub profile: Option<PathBuf>,
}

/// A profile's mark in names: 16 hex digits of its path (FNV-1a), so two profiles never share an item.
fn profile_mark(profile: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in profile.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl LoginItem {
    /// The LaunchAgent's label (also its file's name).
    #[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
    pub fn label(&self) -> String {
        match &self.profile {
            None => format!("{}.server", self.identifier),
            Some(profile) => format!("{}.server.{}", self.identifier, profile_mark(profile)),
        }
    }

    /// The Run key's value name.
    #[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
    pub fn run_name(&self) -> String {
        match &self.profile {
            None => "Mighty Code (server)".into(),
            Some(profile) => format!("Mighty Code (server {})", profile_mark(profile)),
        }
    }

    /// The command line the Run key holds: each path in double quotes (a Windows path never holds one), its trailing backslashes
    /// doubled so the closing quote is not read as an escaped one (`--profile "D:\work\"` would swallow the quote).
    #[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
    pub fn run_value(&self) -> String {
        let quoted = |path: &Path| {
            let text = path.to_string_lossy();
            let trailing = text.len() - text.trim_end_matches('\\').len();
            format!("\"{text}{}\"", "\\".repeat(trailing))
        };
        let mut line = format!("{} {}", quoted(&self.program), super::ARGUMENT);
        if let Some(profile) = &self.profile {
            line.push_str(&format!(" --profile {}", quoted(profile)));
        }
        line
    }

    /// The LaunchAgent's property list.
    #[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
    pub fn plist(&self) -> String {
        let mut arguments = vec![self.program.to_string_lossy().into_owned()];
        arguments.extend(super::arguments(self.profile.as_deref()));
        let strings: String = arguments.iter().map(|argument| format!("\t\t<string>{}</string>\n", xml(argument))).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             {strings}\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n",
            xml(&self.label())
        )
    }
}

/// Text inside an XML element.
#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Where the login item lives.
pub trait LoginStore {
    fn installed(&self, item: &LoginItem) -> bool;
    fn install(&self, item: &LoginItem) -> io::Result<()>;
    fn remove(&self, item: &LoginItem) -> io::Result<()>;
}

/// The Mac's LaunchAgents folder (`~/Library/LaunchAgents`; a test's temporary one).
#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
pub struct LaunchAgents {
    pub folder: PathBuf,
}

#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
impl LaunchAgents {
    pub fn file(&self, item: &LoginItem) -> PathBuf {
        self.folder.join(format!("{}.plist", item.label()))
    }
}

impl LoginStore for LaunchAgents {
    fn installed(&self, item: &LoginItem) -> bool {
        std::fs::symlink_metadata(self.file(item)).is_ok_and(|meta| meta.is_file())
    }

    fn install(&self, item: &LoginItem) -> io::Result<()> {
        std::fs::create_dir_all(&self.folder)?;
        let file = self.file(item);
        let temporary = self.folder.join(format!(".{}.plist.{}.tmp", item.label(), std::process::id()));
        let written = write_readable(&temporary, item.plist().as_bytes()).and_then(|()| std::fs::rename(&temporary, &file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written
    }

    fn remove(&self, item: &LoginItem) -> io::Result<()> {
        match std::fs::remove_file(self.file(item)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// A file launchd reads: the owner writes it, nobody else (0644).
#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
fn write_readable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

/// The Run key's values as the login item reads and writes them (Windows' registry; a test's fake).
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
pub trait RunValues {
    fn read(&self, name: &str) -> Option<String>;
    fn write(&self, name: &str, value: &str) -> io::Result<()>;
    fn delete(&self, name: &str) -> io::Result<()>;
}

/// The Run key as a login store.
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
pub struct RunKey<V: RunValues>(pub V);

impl<V: RunValues> LoginStore for RunKey<V> {
    fn installed(&self, item: &LoginItem) -> bool {
        self.0.read(&item.run_name()).is_some_and(|value| !value.trim().is_empty())
    }

    fn install(&self, item: &LoginItem) -> io::Result<()> {
        self.0.write(&item.run_name(), &item.run_value())
    }

    fn remove(&self, item: &LoginItem) -> io::Result<()> {
        self.0.delete(&item.run_name())
    }
}

/// HKEY_CURRENT_USER's Run key.
#[cfg(windows)]
pub struct CurrentUserRun(pub mighty_platform_windows::registry::CurrentUserRegistry);

#[cfg(windows)]
impl RunValues for CurrentUserRun {
    fn read(&self, name: &str) -> Option<String> {
        let values = self.0.values(RUN_KEY).ok()?;
        let value = values.into_iter().find(|value| value.name.eq_ignore_ascii_case(name))?;
        let units: Vec<u16> = value.data.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
    }

    fn write(&self, name: &str, value: &str) -> io::Result<()> {
        self.0.set_text(RUN_KEY, name, value)
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        self.0.delete_value(RUN_KEY, name)
    }
}

/// This app's login item.
pub fn item(host: &Host) -> Option<LoginItem> {
    let program = std::env::current_exe().ok()?;
    Some(LoginItem { identifier: host.app.config().identifier.clone(), program, profile: super::profile() })
}

/// This OS's login store for the person.
pub fn store() -> Option<Box<dyn LoginStore>> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute())?;
        Some(Box::new(LaunchAgents { folder: home.join("Library").join("LaunchAgents") }))
    }
    #[cfg(windows)]
    {
        Some(Box::new(RunKey(CurrentUserRun(mighty_platform_windows::registry::CurrentUserRegistry::new()))))
    }
}

/// Whether the app starts without a window at login.
pub fn installed(host: &Host) -> bool {
    match (item(host), store()) {
        (Some(item), Some(store)) => store.installed(&item),
        _ => false,
    }
}

/// Settings' switch: the item written or removed (only where the app may change the machine).
pub fn set(host: &Host, on: bool) -> Result<(), String> {
    if !host.may_change_machine() {
        return Ok(());
    }
    let (Some(item), Some(store)) = (item(host), store()) else { return Err("the login item's place is not known".into()) };
    let done = if on { store.install(&item) } else { store.remove(&item) };
    match done {
        Ok(()) => {
            host.log.line(&format!("headless: start at login {}", if on { "on" } else { "off" }));
            Ok(())
        }
        Err(error) => {
            host.log.line(&format!("headless: the login item was not changed: {error}"));
            Err(host.locale.get_with("server.settings.loginFailed", &[("error", &error.to_string())]))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    fn mac_item(profile: Option<&str>) -> LoginItem {
        LoginItem {
            identifier: "dev.mightycode.native".into(),
            program: PathBuf::from("/Applications/Mighty Code.app/Contents/MacOS/MightyCode"),
            profile: profile.map(PathBuf::from),
        }
    }

    #[test]
    fn the_launch_agent_starts_the_app_without_a_window_at_login() {
        let item = mac_item(None);
        assert_eq!(item.label(), "dev.mightycode.native.server");
        assert_eq!(
            item.plist(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>dev.mightycode.native.server</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>/Applications/Mighty Code.app/Contents/MacOS/MightyCode</string>\n\
             \t\t<string>--server</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n"
        );
        assert!(!item.plist().contains("KeepAlive"), "a stopped server stays stopped until the next login");
    }

    #[test]
    fn a_profile_has_its_own_item_and_its_path_is_escaped() {
        let one = mac_item(Some("/Users/me/A & <B>"));
        let other = mac_item(Some("/Users/me/other"));
        assert!(
            one.label().starts_with("dev.mightycode.native.server.") && one.label().len() == "dev.mightycode.native.server.".len() + 16
        );
        assert_ne!(one.label(), other.label());
        assert_ne!(one.label(), mac_item(None).label());
        assert_eq!(one.label(), mac_item(Some("/Users/me/A & <B>")).label(), "the same profile, the same item");
        let plist = one.plist();
        assert!(
            plist.contains(
                "\t\t<string>--server</string>\n\t\t<string>--profile</string>\n\t\t<string>/Users/me/A &amp; &lt;B&gt;</string>\n"
            )
        );
        assert!(!plist.contains("A & <B>"));
    }

    #[test]
    fn the_run_key_value_quotes_the_paths() {
        let item = LoginItem {
            identifier: "dev.mightycode.native".into(),
            program: PathBuf::from(r"C:\Users\me\AppData\Local\Programs\MightyCode\MightyCode.exe"),
            profile: None,
        };
        assert_eq!(item.run_name(), "Mighty Code (server)");
        assert_eq!(item.run_value(), r#""C:\Users\me\AppData\Local\Programs\MightyCode\MightyCode.exe" --server"#);
        let with_profile = LoginItem { profile: Some(PathBuf::from(r"D:\work\my profile")), ..item.clone() };
        assert_eq!(
            with_profile.run_value(),
            r#""C:\Users\me\AppData\Local\Programs\MightyCode\MightyCode.exe" --server --profile "D:\work\my profile""#
        );
        assert!(with_profile.run_name().starts_with("Mighty Code (server ") && with_profile.run_name() != item.run_name());
        // A folder typed with a trailing backslash keeps its closing quote (CommandLineToArgvW reads `\"` as a quote).
        let trailing = LoginItem { profile: Some(PathBuf::from(r"D:\work\")), ..item.clone() };
        assert_eq!(
            trailing.run_value(),
            r#""C:\Users\me\AppData\Local\Programs\MightyCode\MightyCode.exe" --server --profile "D:\work\\""#
        );
    }

    #[derive(Default)]
    struct FakeRun(RefCell<HashMap<String, String>>);

    impl RunValues for &FakeRun {
        fn read(&self, name: &str) -> Option<String> {
            self.0.borrow().get(name).cloned()
        }
        fn write(&self, name: &str, value: &str) -> io::Result<()> {
            self.0.borrow_mut().insert(name.into(), value.into());
            Ok(())
        }
        fn delete(&self, name: &str) -> io::Result<()> {
            self.0.borrow_mut().remove(name);
            Ok(())
        }
    }

    #[test]
    fn the_run_key_is_written_and_removed_as_one_value() {
        let fake = FakeRun::default();
        fake.0.borrow_mut().insert("OneDrive".into(), "\"C:\\OneDrive.exe\" /background".into());
        let store = RunKey(&fake);
        let item =
            LoginItem { identifier: "dev.mightycode.native".into(), program: PathBuf::from(r"C:\Apps\MightyCode.exe"), profile: None };
        assert!(!store.installed(&item));
        store.install(&item).unwrap();
        assert!(store.installed(&item));
        assert_eq!(fake.0.borrow().get("Mighty Code (server)").map(String::as_str), Some(r#""C:\Apps\MightyCode.exe" --server"#));
        store.remove(&item).unwrap();
        assert!(!store.installed(&item));
        store.remove(&item).unwrap();
        assert_eq!(fake.0.borrow().len(), 1, "another program's value is left alone");
    }

    #[test]
    fn the_launch_agent_is_written_whole_and_removed() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-login-{}-{nanos}", std::process::id()));
        let store = LaunchAgents { folder: root.join("Library").join("LaunchAgents") };
        let item = mac_item(None);
        assert!(!store.installed(&item));
        store.remove(&item).unwrap();
        store.install(&item).unwrap();
        assert!(store.installed(&item));
        let file = store.file(&item);
        assert_eq!(file.file_name().unwrap(), "dev.mightycode.native.server.plist");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), item.plist());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        }
        // Written again over the old one (an app that moved): the new program, no temporary file left.
        let moved =
            LoginItem { program: PathBuf::from("/Users/me/Applications/Mighty Code.app/Contents/MacOS/MightyCode"), ..item.clone() };
        store.install(&moved).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("/Users/me/Applications/"));
        let names: Vec<_> = std::fs::read_dir(&store.folder).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        store.remove(&item).unwrap();
        assert!(!store.installed(&item));
        let _ = std::fs::remove_dir_all(&root);
    }
}

//! MC-33: a Mighty Code server on an SSH machine — the far side's command lines and what their output says. Only a remote **Mac**
//! gets one (there is no Linux build); every other machine keeps the minimal mode (PTY and pipes, nothing installed).
//!
//! Each remote command is one POSIX script handed to `/bin/sh -c` ([`sh`]), so a login shell that is not POSIX (fish, csh) only ever
//! sees one single-quoted word. Every script sets its own `PATH` (the system's folders: no tool is taken from the person's `PATH`) and
//! `umask 077`. Nothing secret is ever on a command line: the package goes through ssh's standard input, and the pairing code comes
//! back on standard output only.
//!
//! - [`detect_script`] / [`parse_detection`]: `uname -s -m`, the macOS version, who is logged in at the Mac's screen (`/dev/console`'s
//!   owner) and who we are, the installed app (`~/Applications/MightyCode.app`, else `/Applications/MightyCode.app`) with its
//!   `CFBundleShortVersionString`, and the running server's `mighty server status`.
//! - The install, one script a step: [`upload_script`] (the package read from standard input into a fresh folder beside the target,
//!   its SHA-256 printed), [`unpack_script`] (`ditto`, exactly one `.app`, the quarantine attribute taken off what we unpacked, the
//!   bundle identifier, the version, `codesign --verify --deep --strict` against [`requirement`]), [`stop_script`] (the server stopped
//!   with its own CLI), [`swap_script`] (two renames in the same folder, the old app put back when the second fails), [`start_script`]
//!   (`mighty server start`: the app started detached without a window, its control API waited for), [`cleanup_script`].
//! - [`pair_script`]: `mighty server ssh-pair`, the one-time code for the server's loopback listener.
//! - [`remove_script`]: the server stopped and `~/Applications/MightyCode.app` deleted (never `/Applications`: not ours).

use serde_json::Value;

use super::command::quote;

/// The app the install puts on the far side, under the remote home.
pub const HOME_APP: &str = "Applications/MightyCode.app";
/// An app someone else installed for every user; read, used for the CLI, never replaced or removed.
pub const SYSTEM_APP: &str = "/Applications/MightyCode.app";
/// The executable inside the bundle (`tauri.conf.json`'s `mainBinaryName`).
pub const EXECUTABLE: &str = "Contents/MacOS/MightyCode";
/// The bundle identifier an installed app must have.
pub const BUNDLE_IDENTIFIER: &str = "dev.mightycode.native";
/// The package's name in the install folder.
pub const PACKAGE: &str = "package.zip";
/// The install folder's name starts so (in `~/Applications`, beside the target, so the last rename stays in one file system).
pub const INSTALL_FOLDER_PREFIX: &str = ".mighty-install-";

/// Every script's first line: the system's tools only, nothing readable by other users.
const PRELUDE: &str = "PATH=/usr/bin:/bin:/usr/sbin:/sbin; export PATH; umask 077";

/// `script` as the far side's login shell runs it: `/bin/sh -c '<script>'`, whatever that shell is.
pub fn sh(script: &str) -> String {
    format!("/bin/sh -c {}", quote(script))
}

/// The two places an app is looked for, home first, as a shell list.
const PLACES: &str = "\"$HOME/Applications/MightyCode.app\" /Applications/MightyCode.app";

/// The detection's script (under [`sh`]).
pub fn detect_script() -> String {
    format!(
        "{PRELUDE}\n\
         printf 'os=%s\\n' \"$(uname -s)\"\n\
         printf 'arch=%s\\n' \"$(uname -m)\"\n\
         if [ \"$(uname -s)\" = Darwin ]; then\n\
         printf 'macos=%s\\n' \"$(sw_vers -productVersion 2>/dev/null)\"\n\
         printf 'console=%s\\n' \"$(stat -f %Su /dev/console 2>/dev/null)\"\n\
         printf 'user=%s\\n' \"$(id -un)\"\n\
         for place in home system; do\n\
         if [ \"$place\" = home ]; then app=\"$HOME/{HOME_APP}\"; else app={SYSTEM_APP}; fi\n\
         if [ -x \"$app/{EXECUTABLE}\" ]; then\n\
         printf 'app=%s\\n' \"$place\"\n\
         printf 'version=%s\\n' \"$(defaults read \"$app/Contents/Info\" CFBundleShortVersionString 2>/dev/null)\"\n\
         printf 'status-begin\\n'\n\
         \"$app/{EXECUTABLE}\" --cli server status 2>/dev/null\n\
         printf '\\nstatus-end\\n'\n\
         break\n\
         fi\n\
         done\n\
         fi\n\
         printf 'done=1\\n'"
    )
}

/// Where the app was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppPlace {
    /// `~/Applications/MightyCode.app`: ours to update and remove.
    Home,
    /// `/Applications/MightyCode.app`: used, never replaced or removed.
    System,
}

impl AppPlace {
    /// The app's executable as a remote shell word: under the home with `$HOME` outside the quotes.
    pub fn executable(self) -> String {
        match self {
            AppPlace::Home => format!("\"$HOME\"/{}", quote(&format!("{HOME_APP}/{EXECUTABLE}"))),
            AppPlace::System => quote(&format!("{SYSTEM_APP}/{EXECUTABLE}")),
        }
    }
}

/// The running app's answer to `mighty server status`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerStatus {
    pub running: bool,
    /// `headless` or `windowed`.
    pub mode: String,
    pub version: String,
    pub pid: Option<u32>,
}

/// What the detection read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Detection {
    /// `uname -s`: `Darwin`, `Linux`, …
    pub os: String,
    /// `uname -m`: `arm64`, `x86_64`, …
    pub arch: String,
    /// `sw_vers -productVersion` on a Mac.
    pub macos: String,
    /// Who is logged in at the Mac's screen (`root` at the login window), and who the ssh login is.
    pub console: String,
    pub user: String,
    pub app: Option<AppPlace>,
    /// The installed app's `CFBundleShortVersionString`.
    pub version: Option<String>,
    pub server: Option<ServerStatus>,
}

impl Detection {
    pub fn is_mac(&self) -> bool {
        self.os == "Darwin"
    }

    /// The Mac builds' architectures (a universal or per-architecture bundle covers both).
    pub fn supported_arch(&self) -> bool {
        matches!(self.arch.as_str(), "arm64" | "x86_64")
    }

    /// Whether the ssh login's user is the one logged in at the Mac's screen: the app needs that window-server session to start.
    /// None when it could not be told.
    pub fn gui_session(&self) -> Option<bool> {
        (!self.console.is_empty() && !self.user.is_empty()).then(|| self.console == self.user)
    }

    pub fn running(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.running)
    }
}

/// A field as it may be shown: control characters out, at most 120 characters.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string()
}

/// The detection's output read; None when it did not finish (`done=1` is its last line). A login banner before it is skipped.
pub fn parse_detection(stdout: &str) -> Option<Detection> {
    let mut detection = Detection::default();
    let mut status: Option<String> = None;
    let mut done = false;
    for line in stdout.lines() {
        if let Some(text) = status.as_mut() {
            if line.trim() == "status-end" {
                detection.server = Some(parse_status(text));
                status = None;
            } else if text.len() < 64 * 1024 {
                text.push_str(line);
                text.push('\n');
            }
            continue;
        }
        let line = line.trim();
        if line == "status-begin" {
            status = Some(String::new());
            continue;
        }
        let Some((name, value)) = line.split_once('=') else { continue };
        let value = clean(value);
        match name {
            "os" => detection.os = value,
            "arch" => detection.arch = value,
            "macos" => detection.macos = value,
            "console" => detection.console = value,
            "user" => detection.user = value,
            "app" => {
                detection.app = match value.as_str() {
                    "home" => Some(AppPlace::Home),
                    "system" => Some(AppPlace::System),
                    _ => None,
                }
            }
            "version" => detection.version = (!value.is_empty()).then_some(value),
            "done" => done = value == "1",
            _ => {}
        }
    }
    done.then_some(detection)
}

/// `mighty server status`'s JSON (pretty-printed over several lines); not running when it does not read.
pub fn parse_status(text: &str) -> ServerStatus {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else { return ServerStatus::default() };
    let word = |name: &str| value.get(name).and_then(Value::as_str).map(clean).unwrap_or_default();
    ServerStatus {
        running: value.get("running").and_then(Value::as_bool).unwrap_or(false),
        mode: word("mode"),
        version: word("version"),
        pid: value.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()),
    }
}

/// A new install folder's name: [`INSTALL_FOLDER_PREFIX`] and 16 to 64 lower-case hex digits (the caller's random id).
pub fn valid_install_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The install folder as a shell word: `"$HOME"/'Applications/.mighty-install-<id>'`.
fn install_folder(id: &str) -> String {
    format!("\"$HOME\"/{}", quote(&format!("Applications/{INSTALL_FOLDER_PREFIX}{id}")))
}

/// Step 1: a fresh install folder beside the target (refused when it is there already), the package read from standard input into
/// it, and its SHA-256 printed (`<hex>  <path>`).
pub fn upload_script(id: &str) -> String {
    let folder = install_folder(id);
    format!(
        "{PRELUDE}\n\
         mkdir -p \"$HOME/Applications\" || exit 30\n\
         d={folder}\n\
         mkdir \"$d\" || exit 31\n\
         cat > \"$d/{PACKAGE}\" || exit 32\n\
         shasum -a 256 \"$d/{PACKAGE}\" || exit 33"
    )
}

/// The SHA-256 [`upload_script`] printed (64 lower-case hex digits), if it did.
pub fn parse_sha256(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let hex = line.split_whitespace().next()?.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// The code signature a new app must meet: Mighty Code's identifier and, when this computer's own app is signed by a team, that team
/// (an ad hoc or unsigned local build asks for the identifier and the strict validity alone).
pub fn requirement(team: Option<&str>) -> String {
    let identifier = format!("identifier \"{BUNDLE_IDENTIFIER}\"");
    match team.map(|t| t.replace(['"', '\\'], "")).filter(|t| !t.is_empty()) {
        Some(team) => format!("={identifier} and anchor apple generic and certificate leaf[subject.OU] = \"{team}\""),
        None => format!("={identifier}"),
    }
}

/// Step 2: the package unpacked (`ditto`) beside it, exactly one `.app` taken (no link), the quarantine attribute taken off it, its
/// bundle identifier and version checked, its code signature verified strictly with [`requirement`]. Exit codes: [`UnpackRefusal`].
pub fn unpack_script(id: &str, version: &str, requirement: &str) -> String {
    let folder = install_folder(id);
    let (bundle, version, requirement) = (quote(BUNDLE_IDENTIFIER), quote(version), quote(requirement));
    format!(
        "{PRELUDE}\n\
         d={folder}\n\
         ditto -x -k \"$d/{PACKAGE}\" \"$d/unpacked\" || exit 40\n\
         set -- \"$d\"/unpacked/*.app\n\
         [ \"$#\" -eq 1 ] && [ -d \"$1\" ] && [ ! -L \"$1\" ] || exit 41\n\
         mv \"$1\" \"$d/MightyCode.app\" || exit 41\n\
         xattr -dr com.apple.quarantine \"$d/MightyCode.app\" 2>/dev/null\n\
         [ \"$(defaults read \"$d/MightyCode.app/Contents/Info\" CFBundleIdentifier 2>/dev/null)\" = {bundle} ] || exit 42\n\
         [ \"$(defaults read \"$d/MightyCode.app/Contents/Info\" CFBundleShortVersionString 2>/dev/null)\" = {version} ] || exit 43\n\
         [ -x \"$d/MightyCode.app/{EXECUTABLE}\" ] || exit 42\n\
         codesign --verify --deep --strict -R {requirement} \"$d/MightyCode.app\" || exit 44"
    )
}

/// Why [`unpack_script`] refused, by its exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackRefusal {
    Unpack,
    NotOneApp,
    NotMightyCode,
    Version,
    Signature,
}

impl UnpackRefusal {
    pub fn from_code(code: i32) -> Option<UnpackRefusal> {
        Some(match code {
            40 => UnpackRefusal::Unpack,
            41 => UnpackRefusal::NotOneApp,
            42 => UnpackRefusal::NotMightyCode,
            43 => UnpackRefusal::Version,
            44 => UnpackRefusal::Signature,
            _ => return None,
        })
    }
}

/// The running server stopped with an installed app's own CLI (`mighty server stop`, which waits until its process has ended); nothing
/// to do when no app is installed.
pub fn stop_script() -> String {
    format!(
        "{PRELUDE}\n\
         for app in {PLACES}; do\n\
         if [ -x \"$app/{EXECUTABLE}\" ]; then exec \"$app/{EXECUTABLE}\" --cli server stop; fi\n\
         done\n\
         exit 0"
    )
}

/// Step 4: the new app put in place: the old one renamed into the install folder, the new one renamed to the target, the old one put
/// back when that fails (both renames in `~/Applications`, one file system); the install folder (the old app, the package) removed
/// after. Exit 50: the old app could not be moved, 51: the new one could not (the old one is back).
pub fn swap_script(id: &str) -> String {
    let folder = install_folder(id);
    format!(
        "{PRELUDE}\n\
         d={folder}\n\
         t=\"$HOME/{HOME_APP}\"\n\
         if [ -e \"$t\" ] || [ -L \"$t\" ]; then mv \"$t\" \"$d/previous.app\" || exit 50; fi\n\
         if mv \"$d/MightyCode.app\" \"$t\"; then rm -rf \"$d\"; exit 0; fi\n\
         if [ -e \"$d/previous.app\" ]; then mv \"$d/previous.app\" \"$t\"; fi\n\
         exit 51"
    )
}

/// The install folder removed (after a failed step).
pub fn cleanup_script(id: &str) -> String {
    format!("{PRELUDE}\nrm -rf {}", install_folder(id))
}

/// The installed server started: `mighty server start` (the app detached without a window, its control API waited for; exit 2 when it
/// started but never answered).
pub fn start_script(app: AppPlace) -> String {
    format!("{PRELUDE}\nexec {} --cli server start", app.executable())
}

/// `mighty server ssh-pair`: the server's loopback listener started when it is not, and a one-time pairing code for it.
pub fn pair_script(app: AppPlace) -> String {
    format!("{PRELUDE}\nexec {} --cli server ssh-pair", app.executable())
}

/// The server stopped, then `~/Applications/MightyCode.app` deleted (exit 60 when the stop failed: nothing is deleted).
pub fn remove_script() -> String {
    format!(
        "{PRELUDE}\n\
         a=\"$HOME/{HOME_APP}\"\n\
         if [ -x \"$a/{EXECUTABLE}\" ]; then \"$a/{EXECUTABLE}\" --cli server stop >/dev/null || exit 60; fi\n\
         rm -rf \"$a\""
    )
}

/// What `mighty server ssh-pair` answered: the port the server's loopback listener has on the far side, and the code.
#[derive(Clone, PartialEq, Eq)]
pub struct Pairing {
    pub port: u16,
    pub code: String,
    pub version: String,
}

/// The code is left out.
impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing").field("port", &self.port).field("version", &self.version).finish_non_exhaustive()
    }
}

/// The pairing's JSON (the CLI's answer, after any banner); None when it is not one: a port of 1024 or more and a code of 43 to 128
/// URL-safe characters.
pub fn parse_pairing(stdout: &str) -> Option<Pairing> {
    let start = stdout.find('{')?;
    let value: Value = serde_json::from_str(stdout[start..].trim()).ok()?;
    let port = value.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p >= 1024)?;
    let code = value.get("code").and_then(Value::as_str)?;
    if !(43..=128).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let version = value.get("version").and_then(Value::as_str).map(clean).unwrap_or_default();
    Some(Pairing { port, code: code.to_string(), version })
}

/// The version text of `mighty server status` or a bundle: the leading `x.y.z` of `0.6.0 (abc123)` and the like.
pub fn version_core(text: &str) -> String {
    let text = text.trim().trim_start_matches(['v', 'V']);
    text.split(|c: char| c.is_whitespace() || c == '(' || c == '+').next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_with_an_installed_running_server_is_read() {
        let out = "Last login: today\nos=Darwin\narch=arm64\nmacos=15.1\nconsole=me\nuser=me\napp=home\n\
                   version=0.6.0\nstatus-begin\n{\n  \"running\": true,\n  \"mode\": \"headless\",\n  \"version\": \"0.6.0\",\n  \"pid\": 412\n}\n\nstatus-end\ndone=1\n";
        let detection = parse_detection(out).unwrap();
        assert!(detection.is_mac() && detection.supported_arch());
        assert_eq!(detection.macos, "15.1");
        assert_eq!(detection.app, Some(AppPlace::Home));
        assert_eq!(detection.version.as_deref(), Some("0.6.0"));
        assert_eq!(
            detection.server,
            Some(ServerStatus { running: true, mode: "headless".into(), version: "0.6.0".into(), pid: Some(412) })
        );
        assert_eq!(detection.gui_session(), Some(true));
    }

    #[test]
    fn a_linux_box_and_a_mac_without_the_app_are_read() {
        let linux = parse_detection("os=Linux\narch=x86_64\ndone=1\n").unwrap();
        assert!(!linux.is_mac() && linux.app.is_none() && linux.server.is_none());
        let mac = parse_detection("os=Darwin\narch=x86_64\nmacos=14.6\nconsole=root\nuser=me\ndone=1\n").unwrap();
        assert!(mac.is_mac() && mac.app.is_none() && !mac.running());
        assert_eq!(mac.gui_session(), Some(false), "the login window holds the screen: nobody is logged in there");
        let stopped = "os=Darwin\narch=arm64\napp=system\nversion=0.5.9\nstatus-begin\n{\"running\": false, \"dataDir\": \"/x\"}\n\nstatus-end\ndone=1\n";
        let stopped = parse_detection(stopped).unwrap();
        assert_eq!(stopped.app, Some(AppPlace::System));
        assert!(!stopped.running());
        assert_eq!(parse_detection("os=Darwin\narch=arm64\n"), None, "a detection cut short is no answer");
        let odd = parse_detection("os=Darwin\napp=/tmp/elsewhere/MightyCode.app\nversion=\u{1b}[31m1\ndone=1\n").unwrap();
        assert_eq!(odd.app, None, "an app anywhere else is not one we know");
        assert_eq!(odd.version.as_deref(), Some("[31m1"), "control characters are dropped");
    }

    #[test]
    fn the_pairing_is_read_and_its_code_never_shows_in_debug() {
        let code = "A".repeat(43);
        let pairing =
            parse_pairing(&format!("banner\n{{\n  \"port\": 50123,\n  \"code\": \"{code}\",\n  \"version\": \"0.6.0\"\n}}\n")).unwrap();
        assert_eq!(pairing.port, 50123);
        assert_eq!(pairing.code, code);
        assert!(!format!("{pairing:?}").contains(&code));
        assert_eq!(parse_pairing(&format!("{{\"port\": 80, \"code\": \"{code}\"}}")), None, "a system port is refused");
        assert_eq!(parse_pairing("{\"port\": 50123, \"code\": \"short\"}"), None);
        assert_eq!(parse_pairing(&format!("{{\"port\": 50123, \"code\": \"{}'\"}}", "a".repeat(43))), None);
        assert_eq!(parse_pairing("mighty: Mighty Code is not running"), None);
    }

    #[test]
    fn the_sha256_line_and_versions_are_read() {
        let hex = "ab".repeat(32);
        assert_eq!(parse_sha256(&format!("{hex}  /Users/me/Applications/.mighty-install-x/package.zip\n")), Some(hex));
        assert_eq!(parse_sha256("nope\n"), None);
        assert_eq!(version_core("0.6.0 (abc123)"), "0.6.0");
        assert_eq!(version_core("v0.6.1"), "0.6.1");
    }

    #[test]
    fn the_requirement_names_the_app_and_the_team() {
        assert_eq!(requirement(None), "=identifier \"dev.mightycode.native\"");
        assert_eq!(requirement(Some("")), "=identifier \"dev.mightycode.native\"");
        assert_eq!(
            requirement(Some("AB12\"CD")),
            "=identifier \"dev.mightycode.native\" and anchor apple generic and certificate leaf[subject.OU] = \"AB12CD\""
        );
    }

    #[test]
    fn install_ids_are_hex_only() {
        assert!(valid_install_id("0123456789abcdef"));
        assert!(!valid_install_id("0123456789abcdeg") && !valid_install_id("../x") && !valid_install_id("abc"));
    }

    /// The scripts through a real `/bin/sh` (with a home whose name needs quoting): every path lands where it should, the folder name
    /// and the requirement reach the tools as one word each.
    #[cfg(unix)]
    #[test]
    fn the_scripts_parse_in_a_real_shell() {
        #[allow(clippy::disallowed_methods, reason = "a test reads how /bin/sh parses the scripts")]
        let run = |script: &str, home: &std::path::Path| {
            std::process::Command::new("/bin/sh").arg("-c").arg(sh(script)).env("HOME", home).output().unwrap()
        };
        let root = std::env::temp_dir().join(format!("mc-ssh-server-{}", std::process::id()));
        let home = root.join("it's my \"home\"");
        std::fs::create_dir_all(&home).unwrap();
        // `sh -n`: every script reads as a shell program.
        for script in [
            detect_script(),
            upload_script("0123456789abcdef"),
            unpack_script("0123456789abcdef", "0.6.0", &requirement(Some("TEAM"))),
            stop_script(),
            swap_script("0123456789abcdef"),
            cleanup_script("0123456789abcdef"),
            start_script(AppPlace::Home),
            pair_script(AppPlace::System),
            remove_script(),
        ] {
            #[allow(clippy::disallowed_methods, reason = "a test reads how /bin/sh parses the scripts")]
            let checked = std::process::Command::new("/bin/sh").arg("-n").arg("-c").arg(&script).output().unwrap();
            assert!(checked.status.success(), "{script}: {}", String::from_utf8_lossy(&checked.stderr));
        }
        // The swap's renames in a home with quotes in its name; the old app comes back when the new one is missing.
        let target = home.join(HOME_APP);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old"), "").unwrap();
        let folder = home.join("Applications").join(format!("{INSTALL_FOLDER_PREFIX}0123456789abcdef"));
        std::fs::create_dir_all(&folder).unwrap();
        let missing = run(&swap_script("0123456789abcdef"), &home);
        assert_eq!(missing.status.code(), Some(51));
        assert!(target.join("old").is_file(), "the old app is back");
        std::fs::create_dir_all(folder.join("MightyCode.app")).unwrap();
        std::fs::write(folder.join("MightyCode.app").join("new"), "").unwrap();
        let swapped = run(&swap_script("0123456789abcdef"), &home);
        assert!(swapped.status.success(), "{}", String::from_utf8_lossy(&swapped.stderr));
        assert!(target.join("new").is_file() && !target.join("old").exists());
        assert!(!folder.exists(), "the install folder (with the old app) is gone");
        // The executable's word expands the home and keeps the rest one argument.
        #[allow(clippy::disallowed_methods, reason = "a test reads how /bin/sh parses the quoting")]
        let printed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", AppPlace::Home.executable()))
            .env("HOME", &home)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(printed.stdout).unwrap(), format!("{}/{HOME_APP}/{EXECUTABLE}", home.display()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}

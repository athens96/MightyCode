//! 편집기에서 열기: which external editors this computer has and how one opens a file at a line, or a folder (Settings › 일반 ›
//! 에이전트 창's 외부 편집기). No OS code: the disk is asked through [`Probe`] and paths are text, so the Windows layouts are checked
//! on any OS. The host (src-tauri `editor.rs`) resolves the path inside its workspace first and starts the program detached through
//! the process host, its arguments one by one (never a shell string).
//!
//! Editors are looked for along the login shell's PATH first, then where their installers put them: on the Mac the app bundles in
//! `/Applications` and `~/Applications` (their own CLI inside the bundle), on Windows the usual install folders. Windows starts only
//! a program (`.exe`): a batch shim found on the PATH (VS Code's `bin\code.cmd`) leads to the editor's own `.exe` beside it, else
//! the editor counts as not found — a batch file's arguments go through cmd.exe, which a path with `%` or `!` could change.

use std::collections::BTreeMap;

use mighty_platform::Os;

/// How an editor takes a file and a line on its command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// VS Code and Cursor: `-g <file>:<line>:<column>`.
    VsCode,
    /// Zed: `<file>:<line>:<column>`.
    Zed,
    /// Sublime Text: `<file>:<line>:<column>`.
    Sublime,
    /// The JetBrains IDEs: `--line <line> <file>`.
    JetBrains,
}

/// An app bundle on the Mac and the CLI inside it.
#[derive(Debug, Clone, Copy)]
struct MacApp {
    /// The bundle's name (`Visual Studio Code.app`), or with `prefix` the start of it (`IntelliJ IDEA` matches `IntelliJ IDEA CE.app`).
    bundle: &'static str,
    prefix: bool,
    /// Inside the bundle.
    cli: &'static str,
}

/// The folder a Windows install place starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    LocalAppData,
    ProgramFiles,
}

/// A Windows install place: `<base>\<parent>\<folder>\<program>`; with `prefix` every folder under `<base>\<parent>` whose name starts
/// with `folder` (a JetBrains IDE's version folder), the last in name order first.
#[derive(Debug, Clone, Copy)]
struct Place {
    base: Base,
    parent: &'static str,
    folder: &'static str,
    prefix: bool,
    program: &'static str,
}

/// One editor the setting can name.
#[derive(Debug, Clone, Copy)]
pub struct Editor {
    /// The setting's value (`vscode`, `idea` …).
    pub id: &'static str,
    /// Its name as the editor writes it.
    pub name: &'static str,
    pub family: Family,
    /// Looked for along the PATH (Windows adds PATHEXT).
    mac_commands: &'static [&'static str],
    windows_commands: &'static [&'static str],
    mac_apps: &'static [MacApp],
    windows_places: &'static [Place],
    /// The editor's own program on Windows, which a batch shim on the PATH leads to (looked for up to three folders above it).
    windows_program: &'static str,
}

const fn app(bundle: &'static str, cli: &'static str) -> MacApp {
    MacApp { bundle, prefix: false, cli }
}

const fn place(base: Base, parent: &'static str, folder: &'static str, program: &'static str) -> Place {
    Place { base, parent, folder, prefix: false, program }
}

macro_rules! jetbrains {
    ($id:literal, $name:literal, $mac:literal, $windows:literal, $folder:literal) => {
        Editor {
            id: $id,
            name: $name,
            family: Family::JetBrains,
            mac_commands: &[$mac],
            windows_commands: &[$windows],
            mac_apps: &[MacApp { bundle: $name, prefix: true, cli: concat!("Contents/MacOS/", $mac) }],
            windows_places: &[
                Place {
                    base: Base::ProgramFiles,
                    parent: "JetBrains",
                    folder: $folder,
                    prefix: true,
                    program: concat!("bin\\", $windows, ".exe"),
                },
                Place {
                    base: Base::LocalAppData,
                    parent: "Programs",
                    folder: $folder,
                    prefix: true,
                    program: concat!("bin\\", $windows, ".exe"),
                },
            ],
            windows_program: concat!($windows, ".exe"),
        }
    };
}

/// Every editor, in the order 자동 tries them.
pub const EDITORS: [Editor; 13] = [
    Editor {
        id: "vscode",
        name: "Visual Studio Code",
        family: Family::VsCode,
        mac_commands: &["code"],
        windows_commands: &["code"],
        mac_apps: &[app("Visual Studio Code.app", "Contents/Resources/app/bin/code")],
        windows_places: &[
            place(Base::LocalAppData, "Programs", "Microsoft VS Code", "Code.exe"),
            place(Base::ProgramFiles, "", "Microsoft VS Code", "Code.exe"),
        ],
        windows_program: "Code.exe",
    },
    Editor {
        id: "cursor",
        name: "Cursor",
        family: Family::VsCode,
        mac_commands: &["cursor"],
        windows_commands: &["cursor"],
        mac_apps: &[app("Cursor.app", "Contents/Resources/app/bin/cursor"), app("Cursor.app", "Contents/Resources/app/bin/code")],
        windows_places: &[place(Base::LocalAppData, "Programs", "cursor", "Cursor.exe")],
        windows_program: "Cursor.exe",
    },
    Editor {
        id: "zed",
        name: "Zed",
        family: Family::Zed,
        mac_commands: &["zed"],
        windows_commands: &["zed"],
        mac_apps: &[app("Zed.app", "Contents/MacOS/cli")],
        windows_places: &[
            place(Base::LocalAppData, "Programs", "Zed", "bin\\zed.exe"),
            place(Base::LocalAppData, "Programs", "Zed", "Zed.exe"),
        ],
        windows_program: "Zed.exe",
    },
    jetbrains!("idea", "IntelliJ IDEA", "idea", "idea64", "IntelliJ IDEA"),
    jetbrains!("webstorm", "WebStorm", "webstorm", "webstorm64", "WebStorm"),
    jetbrains!("pycharm", "PyCharm", "pycharm", "pycharm64", "PyCharm"),
    jetbrains!("goland", "GoLand", "goland", "goland64", "GoLand"),
    jetbrains!("clion", "CLion", "clion", "clion64", "CLion"),
    jetbrains!("rustrover", "RustRover", "rustrover", "rustrover64", "RustRover"),
    jetbrains!("rider", "Rider", "rider", "rider64", "JetBrains Rider"),
    jetbrains!("phpstorm", "PhpStorm", "phpstorm", "phpstorm64", "PhpStorm"),
    jetbrains!("rubymine", "RubyMine", "rubymine", "rubymine64", "RubyMine"),
    Editor {
        id: "sublime",
        name: "Sublime Text",
        family: Family::Sublime,
        mac_commands: &["subl"],
        windows_commands: &["subl"],
        mac_apps: &[app("Sublime Text.app", "Contents/SharedSupport/bin/subl")],
        windows_places: &[
            place(Base::ProgramFiles, "", "Sublime Text", "subl.exe"),
            place(Base::ProgramFiles, "", "Sublime Text 3", "subl.exe"),
        ],
        windows_program: "subl.exe",
    },
];

/// The editor the setting names by `id`.
pub fn editor(id: &str) -> Option<&'static Editor> {
    EDITORS.iter().find(|editor| editor.id == id)
}

/// What detection asks the disk. Paths are text in the OS's own form.
pub trait Probe {
    /// A file is there (a link to one counts).
    fn is_file(&self, path: &str) -> bool;
    /// The names in a folder (none when it cannot be read).
    fn names(&self, folder: &str) -> Vec<String>;
    /// The full path `name` runs as along the environment's PATH (the app's own lookup: Windows adds PATHEXT; the Mac wants an
    /// executable file).
    fn find_on_path(&self, name: &str, environment: &BTreeMap<String, String>) -> Option<String>;
}

/// An editor found on this computer and the program that opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub id: String,
    pub name: String,
    pub family: Family,
    pub program: String,
}

/// The value of an environment variable, its name compared ignoring case (Windows' rule; the Mac's names are upper case anyway).
fn variable<'a>(environment: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    environment.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str()).next_back()
}

/// `folder` and `name` joined with the OS's separator.
pub fn join(os: Os, folder: &str, name: &str) -> String {
    let separator = if os == Os::Windows { '\\' } else { '/' };
    let name = if os == Os::Windows { name.replace('/', "\\") } else { name.to_string() };
    if folder.is_empty() {
        return name;
    }
    if folder.ends_with(['/', '\\']) { format!("{folder}{name}") } else { format!("{folder}{separator}{name}") }
}

/// The folder above `path` (None at the top).
pub fn parent(os: Os, path: &str) -> Option<String> {
    let separators: &[char] = if os == Os::Windows { &['\\', '/'] } else { &['/'] };
    let trimmed = path.trim_end_matches(separators);
    let at = trimmed.rfind(separators)?;
    let above = &trimmed[..at];
    (!above.is_empty() && !above.ends_with(':')).then(|| above.to_string())
}

fn file_name(os: Os, path: &str) -> &str {
    let separators: &[char] = if os == Os::Windows { &['\\', '/'] } else { &['/'] };
    path.rsplit(separators).next().unwrap_or(path)
}

/// Whether Windows starts `program` itself: a `.exe` (a batch file needs cmd.exe; anything else has no program of its own).
fn windows_program(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".exe")
}

/// What a command found on the PATH starts as: as it is on the Mac; on Windows a `.exe` as it is, a batch shim the editor's own
/// program up to three folders above it (VS Code's `bin\code.cmd` → `Code.exe`, Cursor's `resources\app\bin\cursor.cmd` →
/// `Cursor.exe`), else nothing.
fn startable(os: Os, editor: &Editor, found: String, probe: &dyn Probe) -> Option<String> {
    if os == Os::Macos || windows_program(&found) {
        return Some(found);
    }
    let lower = found.to_ascii_lowercase();
    if !(lower.ends_with(".cmd") || lower.ends_with(".bat")) {
        return None;
    }
    let mut folder = parent(os, &found);
    for _ in 0..4 {
        let here = folder?;
        let program = join(os, &here, editor.windows_program);
        if probe.is_file(&program) {
            return Some(program);
        }
        folder = parent(os, &here);
    }
    None
}

/// The Mac's app folders: `/Applications`, `~/Applications` and the JetBrains Toolbox's older folder in it.
fn mac_app_folders(home: &str) -> Vec<String> {
    let mut folders = vec!["/Applications".to_string()];
    if home.starts_with('/') {
        folders.push(join(Os::Macos, home, "Applications"));
        folders.push(join(Os::Macos, home, "Applications/JetBrains Toolbox"));
    }
    folders
}

fn in_mac_apps(editor: &Editor, home: &str, probe: &dyn Probe) -> Option<String> {
    for folder in mac_app_folders(home) {
        for app in editor.mac_apps {
            let bundles: Vec<String> = if app.prefix {
                let mut names: Vec<String> =
                    probe.names(&folder).into_iter().filter(|name| name.starts_with(app.bundle) && name.ends_with(".app")).collect();
                names.sort();
                names.reverse();
                names
            } else {
                vec![app.bundle.to_string()]
            };
            for bundle in bundles {
                let cli = join(Os::Macos, &join(Os::Macos, &folder, &bundle), app.cli);
                if probe.is_file(&cli) {
                    return Some(cli);
                }
            }
        }
    }
    None
}

fn in_windows_places(editor: &Editor, environment: &BTreeMap<String, String>, probe: &dyn Probe) -> Option<String> {
    let os = Os::Windows;
    for place in editor.windows_places {
        let base = match place.base {
            Base::LocalAppData => variable(environment, "LOCALAPPDATA").map(str::to_string),
            Base::ProgramFiles => variable(environment, "ProgramFiles").map(str::to_string),
        };
        let Some(base) = base.filter(|b| !b.trim().is_empty()) else { continue };
        let above = join(os, &base, place.parent);
        let folders: Vec<String> = if place.prefix {
            let mut names: Vec<String> = probe
                .names(&above)
                .into_iter()
                .filter(|name| name.to_ascii_lowercase().starts_with(&place.folder.to_ascii_lowercase()))
                .collect();
            names.sort();
            names.reverse();
            names
        } else {
            vec![place.folder.to_string()]
        };
        for folder in folders {
            let program = join(os, &join(os, &above, &folder), place.program);
            if probe.is_file(&program) {
                return Some(program);
            }
        }
    }
    None
}

/// Every editor this computer has, in [`EDITORS`]' order: along `environment`'s PATH first, then in the installers' places.
pub fn detect(os: Os, environment: &BTreeMap<String, String>, home: &str, probe: &dyn Probe) -> Vec<Detected> {
    EDITORS
        .iter()
        .filter_map(|editor| {
            let commands = if os == Os::Windows { editor.windows_commands } else { editor.mac_commands };
            let on_path = commands
                .iter()
                .filter_map(|command| probe.find_on_path(command, environment))
                .find_map(|found| startable(os, editor, found, probe));
            let program = on_path.or_else(|| match os {
                Os::Macos => in_mac_apps(editor, home, probe),
                Os::Windows => in_windows_places(editor, environment, probe),
            })?;
            Some(Detected { id: editor.id.to_string(), name: editor.name.to_string(), family: editor.family, program })
        })
        .collect()
}

/// The 외부 편집기 setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// 자동: the first editor found, else the system's default.
    Auto,
    /// 시스템 기본: the file's default app (no line), a folder in the file manager.
    System,
    /// One editor by its id.
    Editor(String),
}

impl Choice {
    /// The stored value read back: `auto` (and anything empty or unknown), `system`, or an editor's id.
    pub fn parse(text: Option<&str>) -> Choice {
        match text.map(str::trim) {
            Some("system") => Choice::System,
            Some(id) if editor(id).is_some() => Choice::Editor(id.to_string()),
            _ => Choice::Auto,
        }
    }

    /// The value stored.
    pub fn value(&self) -> &str {
        match self {
            Choice::Auto => "auto",
            Choice::System => "system",
            Choice::Editor(id) => id,
        }
    }
}

/// What is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file (a full path), at a line when one is known.
    File { path: String, line: Option<u32> },
    /// A folder (a full path): a worktree.
    Folder { path: String },
}

/// The largest line asked for (beyond any file the editors open).
pub const MAXIMUM_LINE: u32 = 10_000_000;

/// A line the page sent: a whole number from 1 to [`MAXIMUM_LINE`].
pub fn line(value: Option<f64>) -> Option<u32> {
    value.filter(|n| n.fract() == 0.0 && *n >= 1.0 && *n <= f64::from(MAXIMUM_LINE)).map(|n| n as u32)
}

/// The editor's arguments for `target`, each one argument (the process host passes them as they are; Windows quotes each as the C
/// runtime splits it back). A file always gets its line (1 when none is known) and column 1 where the family writes them after the
/// path: the editor reads the last two numbers as line and column, so a file whose own name ends in `:<number>` still opens as
/// itself. A folder is the path alone.
pub fn arguments(family: Family, target: &Target) -> Vec<String> {
    match target {
        Target::Folder { path } => vec![path.clone()],
        Target::File { path, line } => {
            let line = line.unwrap_or(1);
            match family {
                Family::VsCode => vec!["-g".into(), format!("{path}:{line}:1")],
                Family::Zed | Family::Sublime => vec![format!("{path}:{line}:1")],
                Family::JetBrains => vec!["--line".into(), line.to_string(), path.clone()],
            }
        }
    }
}

/// How the target opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// The editor's program with its arguments.
    Program { editor: Detected, args: Vec<String> },
    /// The file's default app (no line).
    OpenFile(String),
    /// The folder in the OS's file manager.
    ShowFolder(String),
}

/// Why nothing opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The editor the setting names is not on this computer (any more).
    Missing { id: String, name: String },
}

/// The editor `choice` picks among `detected`.
pub fn chosen<'a>(choice: &Choice, detected: &'a [Detected]) -> Result<Option<&'a Detected>, PlanError> {
    match choice {
        Choice::System => Ok(None),
        Choice::Auto => Ok(detected.first()),
        Choice::Editor(id) => match detected.iter().find(|found| found.id == *id) {
            Some(found) => Ok(Some(found)),
            None => Err(PlanError::Missing { id: id.clone(), name: editor(id).map_or_else(|| id.clone(), |e| e.name.to_string()) }),
        },
    }
}

/// How `target` opens with the setting `choice` on a computer that has `detected`.
pub fn plan(choice: &Choice, detected: &[Detected], target: &Target) -> Result<Launch, PlanError> {
    Ok(match chosen(choice, detected)? {
        Some(editor) => Launch::Program { editor: editor.clone(), args: arguments(editor.family, target) },
        None => match target {
            Target::File { path, .. } => Launch::OpenFile(path.clone()),
            Target::Folder { path } => Launch::ShowFolder(path.clone()),
        },
    })
}

/// The folder the editor's program starts in: the target's own folder (a file's folder).
pub fn working_folder(os: Os, target: &Target) -> Option<String> {
    match target {
        Target::Folder { path } => Some(path.clone()),
        Target::File { path, .. } => parent(os, path),
    }
}

/// The environment the editor starts with: the login shell's, without what would make an Electron editor run as Node
/// (`ELECTRON_RUN_AS_NODE`, set when the app itself runs under one). None when the shell's environment has no PATH (the app's own is
/// used then).
pub fn environment(values: &BTreeMap<String, String>) -> Option<BTreeMap<String, String>> {
    variable(values, "PATH")?;
    Some(
        values.iter().filter(|(name, _)| !name.eq_ignore_ascii_case("ELECTRON_RUN_AS_NODE")).map(|(n, v)| (n.clone(), v.clone())).collect(),
    )
}

/// The editor's program as the settings show it: its file name.
pub fn program_name(os: Os, detected: &Detected) -> String {
    file_name(os, &detected.program).to_string()
}

// ── a folder on an SSH machine (MC-52) ──────────────────────────────────────────────────────────────────────────────────────

/// MC-52: the editor that opens an SSH machine's folder, and the scheme of its address: VS Code and Cursor take
/// `<scheme>://vscode-remote/<authority><path>` with their Remote-SSH extension. None of the others is given a remote folder
/// on its command line or by an address this app can rely on.
pub fn remote_scheme(id: &str) -> Option<&'static str> {
    match id {
        "vscode" => Some("vscode"),
        "cursor" => Some("cursor"),
        _ => None,
    }
}

/// MC-52: the editor the setting `choice` opens an SSH machine's folder with among `detected`: the chosen editor when it can, under
/// 자동 the first editor found that can; None when the choice is 시스템 기본 or an editor that cannot (the caller says why).
pub fn remote_editor<'a>(choice: &Choice, detected: &'a [Detected]) -> Option<&'a Detected> {
    match choice {
        Choice::System => None,
        Choice::Auto => detected.iter().find(|found| remote_scheme(&found.id).is_some()),
        Choice::Editor(id) => detected.iter().find(|found| found.id == *id && remote_scheme(id).is_some()),
    }
}

/// MC-52: the address `scheme` opens `path` (a POSIX path on the machine `authority` names, mighty-core `Target::vscode_authority`)
/// with, a file at `line` (`:<line>:1`, as its command line takes it). Every byte that is not an unreserved URI character or `/` is
/// percent-encoded, so a space or `#` in a folder name reaches the editor as itself. None for a path that is not absolute.
pub fn remote_uri(scheme: &str, authority: &str, path: &str, line: Option<u32>) -> Option<String> {
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return None;
    }
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    let at = line.map(|line| format!(":{line}:1")).unwrap_or_default();
    Some(format!("{scheme}://vscode-remote/{authority}{encoded}{at}"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// A disk of the given files (folders are what they sit in) and a PATH lookup over the given commands.
    struct Disk {
        files: BTreeSet<String>,
        path: BTreeMap<String, String>,
        os: Os,
    }

    impl Disk {
        fn new(os: Os, files: &[&str], path: &[(&str, &str)]) -> Disk {
            Disk {
                files: files.iter().map(|f| f.to_string()).collect(),
                path: path.iter().map(|(n, p)| (n.to_string(), p.to_string())).collect(),
                os,
            }
        }
    }

    impl Probe for Disk {
        fn is_file(&self, path: &str) -> bool {
            self.files.contains(path)
        }
        fn names(&self, folder: &str) -> Vec<String> {
            let names: BTreeSet<String> = self
                .files
                .iter()
                .filter_map(|file| {
                    let mut at = file.clone();
                    while let Some(up) = parent(self.os, &at) {
                        if up == folder {
                            return Some(file_name(self.os, &at).to_string());
                        }
                        at = up;
                    }
                    None
                })
                .collect();
            names.into_iter().collect()
        }
        fn find_on_path(&self, name: &str, _environment: &BTreeMap<String, String>) -> Option<String> {
            self.path.get(name).cloned()
        }
    }

    /// MC-52: an SSH machine's folder opens in VS Code or Cursor by their Remote-SSH address; the others are not offered one.
    #[test]
    fn an_ssh_folder_opens_by_the_remote_address_of_an_editor_that_takes_one() {
        let found = |id: &str, family: Family| Detected { id: id.into(), name: id.into(), family, program: format!("/bin/{id}") };
        let detected = vec![found("zed", Family::Zed), found("cursor", Family::VsCode), found("vscode", Family::VsCode)];
        assert_eq!(remote_editor(&Choice::Auto, &detected).map(|d| d.id.as_str()), Some("cursor"), "the first that can");
        assert_eq!(remote_editor(&Choice::Editor("vscode".into()), &detected).map(|d| d.id.as_str()), Some("vscode"));
        assert!(remote_editor(&Choice::Editor("zed".into()), &detected).is_none(), "Zed is given no remote address");
        assert!(remote_editor(&Choice::System, &detected).is_none());
        assert!(remote_editor(&Choice::Auto, &detected[..1]).is_none());
        assert_eq!(
            remote_uri("vscode", "ssh-remote+devbox", "/home/me/.mightycode/worktrees/a b#1", None).unwrap(),
            "vscode://vscode-remote/ssh-remote+devbox/home/me/.mightycode/worktrees/a%20b%231"
        );
        assert_eq!(
            remote_uri("cursor", "ssh-remote+me@box", "/srv/app/src/main.rs", Some(12)).unwrap(),
            "cursor://vscode-remote/ssh-remote+me@box/srv/app/src/main.rs:12:1"
        );
        assert_eq!(remote_uri("vscode", "ssh-remote+x", "/한", None).unwrap(), "vscode://vscode-remote/ssh-remote+x/%ED%95%9C");
        assert!(remote_uri("vscode", "ssh-remote+x", "relative", None).is_none());
        assert!(remote_uri("vscode", "ssh-remote+x", "/a\nb", None).is_none());
    }

    fn windows_environment() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("LOCALAPPDATA".to_string(), r"C:\Users\영희\AppData\Local".to_string()),
            ("ProgramFiles".to_string(), r"C:\Program Files".to_string()),
            ("Path".to_string(), r"C:\Windows".to_string()),
        ])
    }

    fn ids(found: &[Detected]) -> Vec<&str> {
        found.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn the_mac_finds_editors_on_the_path_then_in_their_app_bundles() {
        let disk = Disk::new(
            Os::Macos,
            &[
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
                "/Applications/Cursor.app/Contents/Resources/app/bin/cursor",
                "/Users/me/Applications/IntelliJ IDEA Ultimate.app/Contents/MacOS/idea",
                "/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl",
                "/Applications/Zed.app/Contents/MacOS/cli",
            ],
            &[("code", "/usr/local/bin/code")],
        );
        let found = detect(Os::Macos, &BTreeMap::new(), "/Users/me", &disk);
        assert_eq!(ids(&found), ["vscode", "cursor", "zed", "idea", "sublime"], "in the order 자동 tries them");
        assert_eq!(found[0].program, "/usr/local/bin/code", "the PATH's copy first");
        assert_eq!(found[1].program, "/Applications/Cursor.app/Contents/Resources/app/bin/cursor");
        assert_eq!(found[2].program, "/Applications/Zed.app/Contents/MacOS/cli");
        assert_eq!(
            found[3].program, "/Users/me/Applications/IntelliJ IDEA Ultimate.app/Contents/MacOS/idea",
            "a bundle named by its prefix"
        );
        assert_eq!(found[3].family, Family::JetBrains);
        assert!(detect(Os::Macos, &BTreeMap::new(), "/Users/me", &Disk::new(Os::Macos, &[], &[])).is_empty());
    }

    #[test]
    fn windows_starts_only_programs_and_follows_a_batch_shim_to_the_editors_own() {
        let local = r"C:\Users\영희\AppData\Local";
        let (code, cursor) =
            (format!(r"{local}\Programs\Microsoft VS Code\bin\code.cmd"), format!(r"{local}\Programs\cursor\resources\app\bin\cursor.cmd"));
        let files = [
            format!(r"{local}\Programs\Microsoft VS Code\Code.exe"),
            code.clone(),
            format!(r"{local}\Programs\cursor\Cursor.exe"),
            cursor.clone(),
            r"C:\Program Files\JetBrains\IntelliJ IDEA 2023.3\bin\idea64.exe".to_string(),
            r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.2\bin\idea64.exe".to_string(),
            r"C:\Program Files\Sublime Text\subl.exe".to_string(),
            r"C:\tools\zed.bat".to_string(),
        ];
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let disk = Disk::new(Os::Windows, &files, &[("code", &code), ("cursor", &cursor), ("zed", r"C:\tools\zed.bat")]);
        let found = detect(Os::Windows, &windows_environment(), "", &disk);
        assert_eq!(ids(&found), ["vscode", "cursor", "idea", "sublime"], "a batch file with no program above it is not an editor");
        assert_eq!(found[0].program, format!(r"{local}\Programs\Microsoft VS Code\Code.exe"));
        assert_eq!(found[1].program, format!(r"{local}\Programs\cursor\Cursor.exe"));
        assert_eq!(found[2].program, r"C:\Program Files\JetBrains\IntelliJ IDEA 2024.2\bin\idea64.exe", "the newest version folder");
        assert!(found.iter().all(|d| d.program.to_ascii_lowercase().ends_with(".exe")));
    }

    #[test]
    fn windows_finds_an_install_without_the_path_and_ignores_an_empty_base() {
        let disk = Disk::new(Os::Windows, &[r"C:\Program Files\Microsoft VS Code\Code.exe"], &[]);
        let found = detect(Os::Windows, &windows_environment(), "", &disk);
        assert_eq!(ids(&found), ["vscode"]);
        let mut empty = windows_environment();
        empty.insert("ProgramFiles".into(), " ".into());
        assert!(detect(Os::Windows, &empty, "", &disk).is_empty());
    }

    #[test]
    fn each_family_takes_the_file_and_line_as_one_argument_each() {
        let file = |path: &str, line| Target::File { path: path.into(), line };
        let spaced = "/Users/me/My Project/src/main.rs";
        assert_eq!(arguments(Family::VsCode, &file(spaced, Some(12))), ["-g", "/Users/me/My Project/src/main.rs:12:1"]);
        assert_eq!(arguments(Family::Zed, &file(spaced, Some(12))), ["/Users/me/My Project/src/main.rs:12:1"]);
        assert_eq!(arguments(Family::Sublime, &file(spaced, None)), ["/Users/me/My Project/src/main.rs:1:1"]);
        assert_eq!(arguments(Family::JetBrains, &file(spaced, Some(7))), ["--line", "7", "/Users/me/My Project/src/main.rs"]);
        // A quote, a dollar, a semicolon and Korean stay inside their one argument: nothing is ever handed to a shell.
        let odd = "/tmp/it's \"q\" $HOME; rm -rf/한글 파일.ts";
        assert_eq!(arguments(Family::VsCode, &file(odd, Some(3))), ["-g".to_string(), format!("{odd}:3:1")]);
        assert_eq!(arguments(Family::JetBrains, &file(odd, None)), ["--line".to_string(), "1".into(), odd.to_string()]);
        // A Windows path with spaces and non-ASCII: the process host quotes it, the argument is the path as it is.
        let windows = r"C:\Users\영희\내 프로젝트\app.py";
        assert_eq!(arguments(Family::VsCode, &file(windows, Some(40))), ["-g".to_string(), format!("{windows}:40:1")]);
        // A file whose own name ends in `:<number>` keeps it: the editor reads the last two numbers.
        assert_eq!(arguments(Family::Zed, &file("/tmp/notes:3", None)), ["/tmp/notes:3:1:1"]);
        // A folder is the path alone, whatever the family.
        for family in [Family::VsCode, Family::Zed, Family::Sublime, Family::JetBrains] {
            assert_eq!(arguments(family, &Target::Folder { path: "/w/tree one".into() }), ["/w/tree one"]);
        }
    }

    #[test]
    fn the_setting_picks_the_editor_or_the_systems_default() {
        let code = Detected { id: "vscode".into(), name: "Visual Studio Code".into(), family: Family::VsCode, program: "/c".into() };
        let zed = Detected { id: "zed".into(), name: "Zed".into(), family: Family::Zed, program: "/z".into() };
        let detected = [code.clone(), zed.clone()];
        let file = Target::File { path: "/w/a.rs".into(), line: Some(5) };
        let folder = Target::Folder { path: "/w".into() };
        assert_eq!(
            plan(&Choice::Auto, &detected, &file),
            Ok(Launch::Program { editor: code, args: vec!["-g".into(), "/w/a.rs:5:1".into()] })
        );
        assert_eq!(
            plan(&Choice::Editor("zed".into()), &detected, &file),
            Ok(Launch::Program { editor: zed, args: vec!["/w/a.rs:5:1".into()] })
        );
        assert_eq!(plan(&Choice::System, &detected, &file), Ok(Launch::OpenFile("/w/a.rs".into())));
        assert_eq!(plan(&Choice::System, &detected, &folder), Ok(Launch::ShowFolder("/w".into())));
        assert_eq!(plan(&Choice::Auto, &[], &file), Ok(Launch::OpenFile("/w/a.rs".into())), "자동 with no editor: the default app");
        assert_eq!(
            plan(&Choice::Editor("idea".into()), &detected, &file),
            Err(PlanError::Missing { id: "idea".into(), name: "IntelliJ IDEA".into() })
        );
    }

    #[test]
    fn the_stored_value_reads_back() {
        assert_eq!(Choice::parse(None), Choice::Auto);
        assert_eq!(Choice::parse(Some("system")), Choice::System);
        assert_eq!(Choice::parse(Some(" cursor ")), Choice::Editor("cursor".into()));
        assert_eq!(Choice::parse(Some("notepad.exe")), Choice::Auto, "an unknown value is 자동");
        for value in ["auto", "system", "vscode", "rider"] {
            assert_eq!(Choice::parse(Some(value)).value(), value);
        }
    }

    #[test]
    fn a_line_is_a_whole_number_in_range() {
        assert_eq!(line(Some(1.0)), Some(1));
        assert_eq!(line(Some(42.0)), Some(42));
        for bad in [0.0, -3.0, 1.5, f64::NAN, f64::INFINITY, 1e12] {
            assert_eq!(line(Some(bad)), None, "{bad}");
        }
        assert_eq!(line(None), None);
    }

    #[test]
    fn the_editor_starts_in_the_targets_folder_with_the_shells_environment() {
        assert_eq!(working_folder(Os::Macos, &Target::File { path: "/w/src/a.rs".into(), line: None }).as_deref(), Some("/w/src"));
        assert_eq!(working_folder(Os::Windows, &Target::File { path: r"C:\w\a.rs".into(), line: None }).as_deref(), Some(r"C:\w"));
        assert_eq!(working_folder(Os::Windows, &Target::File { path: r"C:\a.rs".into(), line: None }), None, "never a bare drive");
        assert_eq!(working_folder(Os::Macos, &Target::Folder { path: "/w".into() }).as_deref(), Some("/w"));
        let shell = BTreeMap::from([
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("ELECTRON_RUN_AS_NODE".to_string(), "1".to_string()),
            ("HOME".to_string(), "/Users/me".to_string()),
        ]);
        let started = environment(&shell).unwrap();
        assert!(!started.contains_key("ELECTRON_RUN_AS_NODE") && started.contains_key("HOME"));
        assert_eq!(environment(&BTreeMap::from([("HOME".to_string(), "/".to_string())])), None, "no PATH: the app's own");
    }
}

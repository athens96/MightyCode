//! The data an app from before the rename left under its old names (C# `Core\RenameMigration.cs`, `WinUI\Program.cs:163-205`;
//! docs/rename-identifiers.md; plan §1.8 "RenameMigration", W9-F). Owner: W9-F.
//!
//! For the default profile only, outside a smoke, where the write policy writes, before anything reads the data (src-tauri's
//! `startup::before_ui` decides): move `%APPDATA%\MightyClaudeNative` to `MightyCode` when the target is absent; move `~\.mightyclaude`
//! to `~\.mightycode` and, if moved, rewrite the paths in `delegation-state.json` and `workspace-state.json` (plain and JSON-escaped,
//! only before a separator), move `~/.claude/projects/<old worktrees prefix>-*` to the new prefix and list the worktrees; copy HKCU
//! `Software\MightyClaudeNative` to `Software\MightyCode` once (values and subkeys; the old key stays). The caller runs
//! `git worktree repair` in each listed worktree after start, off the UI ([`Report::worktrees`]).
//!
//! The file steps are std::fs over the folders given (tests use temporary ones); the registry is a seam ([`RegistryStore`]: the
//! Windows HKCU tree in the app, a fake in tests, nothing elsewhere). Every step is done once and never throws, as C#'s.
//!
//! The pre-rename state copy C#'s `MainWindow` makes from `%APPDATA%\MightyClaude` or `mighty-claude` (a default profile without a
//! state file takes theirs, `MainWindow.cs:64`) is a separate rule the store keeps (`store::Store`'s legacy folder, the platform's
//! `legacy_data_dirs`); this migration does not replace it, as C# runs both.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// C# `RenameMigration.LegacyDataFolder`, under `%APPDATA%`.
pub const LEGACY_DATA_FOLDER: &str = "MightyClaudeNative";
/// C# `RenameMigration.DataFolder`.
pub const DATA_FOLDER: &str = "MightyCode";
/// C# `RenameMigration.LegacyHomeFolder`, under the user's home.
pub const LEGACY_HOME_FOLDER: &str = ".mightyclaude";
/// C# `RenameMigration.HomeFolder`.
pub const HOME_FOLDER: &str = ".mightycode";
/// C# `RenameMigration.LegacyRegistryKey`, under HKEY_CURRENT_USER.
pub const LEGACY_REGISTRY_KEY: &str = r"Software\MightyClaudeNative";
/// C# `RenameMigration.RegistryKey`.
pub const REGISTRY_KEY: &str = r"Software\MightyCode";
/// The records that name each child's worktree by its full path, in the data folder (C#'s order).
pub const RECORDS: [&str; 2] = ["delegation-state.json", "workspace-state.json"];

/// One registry value as read and written again: its name (`""` the key's default), its Windows type (`REG_SZ` 1, `REG_EXPAND_SZ` 2,
/// `REG_BINARY` 3, `REG_DWORD` 4, `REG_MULTI_SZ` 7, `REG_QWORD` 11; C#'s `RegistryDataKind` numbers them the same) and its bytes as
/// stored (an expandable string unexpanded, as C# reads it with `DoNotExpandEnvironmentNames`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValue {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

/// C# `IRegistryStore`: the registry under HKEY_CURRENT_USER as the migration reads and writes it. Paths are `\`-separated key names
/// below HKCU (`Software\MightyCode`). A missing key reads as no subkeys and no values; a key that cannot be read is an error, so a copy
/// never stops part way and looks whole.
pub trait RegistryStore {
    fn key_exists(&self, path: &str) -> bool;
    fn sub_key_names(&self, path: &str) -> io::Result<Vec<String>>;
    fn values(&self, path: &str) -> io::Result<Vec<RegistryValue>>;
    fn create_key(&self, path: &str) -> io::Result<()>;
    fn set_value(&self, path: &str, value: &RegistryValue) -> io::Result<()>;
    /// The key and everything under it removed (a copy that failed part way takes back what it made, so the next start tries again).
    fn delete_tree(&self, path: &str) -> io::Result<()>;
}

/// A registry with no keys (C#'s `CurrentUserRegistry` off Windows): nothing to copy, nothing written.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRegistry;

impl RegistryStore for NoRegistry {
    fn key_exists(&self, _path: &str) -> bool {
        false
    }
    fn sub_key_names(&self, _path: &str) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn values(&self, _path: &str) -> io::Result<Vec<RegistryValue>> {
        Ok(Vec::new())
    }
    fn create_key(&self, _path: &str) -> io::Result<()> {
        Ok(())
    }
    fn set_value(&self, _path: &str, _value: &RegistryValue) -> io::Result<()> {
        Ok(())
    }
    fn delete_tree(&self, _path: &str) -> io::Result<()> {
        Ok(())
    }
}

/// C# `RenameMigration.Report`: what moved, and the child worktrees now under the new folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub moved_data_folder: bool,
    pub moved_worktrees: bool,
    pub copied_registry: bool,
    /// The child worktrees now under the new folder, whose git links are repaired after start (`git worktree repair` in each).
    pub worktrees: Vec<PathBuf>,
}

/// C# `RenameMigration.Run`: every step. `claude_config` is Claude's config folder (`~/.claude`, or `CLAUDE_CONFIG_DIR`). Never fails.
pub fn run(app_data: &Path, home: &Path, claude_config: &Path, registry: &dyn RegistryStore) -> Report {
    let mut report = Report::default();
    let data = app_data.join(DATA_FOLDER);
    report.moved_data_folder = move_once(&app_data.join(LEGACY_DATA_FOLDER), &data);

    let old_home = home.join(LEGACY_HOME_FOLDER);
    let new_home = home.join(HOME_FOLDER);
    if move_once(&old_home, &new_home) {
        report.moved_worktrees = true;
        // The records name each child's worktree by its full path.
        for name in RECORDS {
            rewrite_paths(&data.join(name), &old_home.to_string_lossy(), &new_home.to_string_lossy());
        }
        if let Ok(entries) = fs::read_dir(new_home.join("worktrees")) {
            report.worktrees.extend(entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()));
        }
        move_claude_projects(claude_config, &old_home, &new_home);
    }

    // C# catches the registry's security and I/O failures: a copy that fails part way reports nothing copied.
    report.copied_registry = copy_registry(registry, LEGACY_REGISTRY_KEY, REGISTRY_KEY).unwrap_or(false);
    report
}

/// C# `RenameMigration.Move`: moves `old` to `target` when it is a folder and the target is not there yet (neither a folder nor a
/// file). Whether it moved; a failure to move (another volume, a file in use, no right) is no move.
pub fn move_once(old: &Path, target: &Path) -> bool {
    if !old.is_dir() || target.exists() {
        return false;
    }
    if let Some(parent) = target.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    fs::rename(old, target).is_ok()
}

/// C# `RenameMigration.CopyRegistry`: copies a key, its values and subkeys, when `old` exists and `target` does not. The old key stays.
/// Whether it copied; an error part way is the caller's (C# lets the exception reach `Run`, which reports nothing copied). Beyond C#
/// (W9 review L4): what a failed copy made is taken away again, so a half-copied key never passes for the whole one and the next start
/// copies it again.
pub fn copy_registry(registry: &dyn RegistryStore, old: &str, target: &str) -> io::Result<bool> {
    if !registry.key_exists(old) || registry.key_exists(target) {
        return Ok(false);
    }
    if let Err(error) = copy(registry, old, target) {
        let _ = registry.delete_tree(target);
        return Err(error);
    }
    Ok(true)
}

fn copy(registry: &dyn RegistryStore, from: &str, to: &str) -> io::Result<()> {
    registry.create_key(to)?;
    for value in registry.values(from)? {
        registry.set_value(to, &value)?;
    }
    for child in registry.sub_key_names(from)? {
        copy(registry, &format!(r"{from}\{child}"), &format!(r"{to}\{child}"))?;
    }
    Ok(())
}

/// The user's home folder for the migration (C# `SpecialFolder.UserProfile`): the OS's own record of it (on Windows the Profile known
/// folder, never an environment variable a bare start may lack or a caller may have set), None unless it is an absolute path.
pub fn home() -> Option<PathBuf> {
    dirs::home_dir().filter(|home| home.is_absolute())
}

/// C# `RenameMigration.RewritePaths`: replaces the folder path `old` with `target` in a JSON file, written plainly or JSON-escaped
/// ([`rewritten`]). A file that does not read is left alone; one that does not change is not written. Read as C#'s `ReadAllText` reads
/// it (a UTF-8 byte order mark dropped, bytes that are not UTF-8 replaced) and written as `WriteAllText` writes (UTF-8, no mark).
pub fn rewrite_paths(file: &Path, old: &str, target: &str) {
    let Ok(bytes) = fs::read(file) else { return };
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes);
    let updated = rewritten(&text, old, target);
    if updated != text {
        let _ = fs::write(file, updated);
    }
}

/// C# `RenameMigration.Rewritten`: `old` replaced with `target` where a separator follows (so a folder that merely starts with the old
/// name is left alone): plainly before `\` or `/`, then with backslashes doubled before `\\`, `/` or `\/` (JSON text's escapes).
pub fn rewritten(text: &str, old: &str, target: &str) -> String {
    let mut text = text.to_string();
    for separator in ["\\", "/"] {
        text = text.replace(&format!("{old}{separator}"), &format!("{target}{separator}"));
    }
    let escaped = |path: &str| path.replace('\\', "\\\\");
    let (old, target) = (escaped(old), escaped(target));
    for separator in ["\\\\", "/", "\\/"] {
        text = text.replace(&format!("{old}{separator}"), &format!("{target}{separator}"));
    }
    text
}

/// C# `SessionHistory.ClaudeProjectFolder` (mighty-history's `claude_project_folder`, which this crate cannot reach): Claude's folder
/// name for a working folder, every character other than an ASCII letter or digit a `-` (one per Unicode scalar).
pub fn claude_project_folder(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// C# `RenameMigration.MoveClaudeProjects`: Claude keeps a child pane's conversations in a folder named after the pane's working
/// folder (`~/.claude/projects/C--Users-me--mightyclaude-worktrees-<id>`); it follows the worktree so the child resumes its
/// conversation. Folders of other projects stay.
pub fn move_claude_projects(claude_config: &Path, old_home: &Path, new_home: &Path) {
    let projects = claude_config.join("projects");
    let prefix = claude_project_folder(&old_home.join("worktrees").to_string_lossy()) + "-";
    let replacement = claude_project_folder(&new_home.join("worktrees").to_string_lossy()) + "-";
    if !projects.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(&projects) else { return };
    let folders: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    for folder in folders {
        let Some(name) = folder.file_name().map(|name| name.to_string_lossy().into_owned()) else { continue };
        if let Some(rest) = name.strip_prefix(&prefix) {
            move_once(&folder, &projects.join(format!("{replacement}{rest}")));
        }
    }
}

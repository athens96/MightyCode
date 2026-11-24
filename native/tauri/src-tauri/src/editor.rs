//! 편집기에서 열기: a file (at a line when known) or a worktree's folder opened in the user's external editor (mighty-tools'
//! `editor`: detection and arguments). The page's `openInEditor` names the place the way the view it came from knows it — the files
//! pane's workspace, a pane's run folder (reference bubble, result files, 요청 변경 검토), a worktree pane's or delegated child's
//! worktree — and the path under it; the path is resolved inside that folder (`WorkspaceFiles.Resolve`: its real path must stay in it)
//! and anything else is refused. Settings › 일반 › 에이전트 창's 외부 편집기 picks the editor: 자동 (the first one found), one of the
//! editors found, or 시스템 기본 (the file's default app, without the line; a folder in the file manager).
//!
//! MC-52, an SSH workspace's folder (its files pane, an SSH pane's run folder or worktree): no editor here can open a path on another
//! machine, so it opens by the editor's Remote-SSH address (`vscode://vscode-remote/ssh-remote+<host><path>`, Cursor's `cursor://…`;
//! mighty-tools `editor::remote_editor`, mighty-core `Target::vscode_authority`) when the setting names (or 자동 finds) such an
//! editor; otherwise the folder opens in the app's own files pane over ssh (MC-40's editor, rooted at that folder) and the banner says
//! why. The path stays inside the folder it was named in.
//!
//! The editor starts detached (`ProcessHost::start_detached_or_in_job`, named in the CI guard with this file): it outlives the app and
//! never sits in the job that ends the app's children. Its program and each argument are passed as they are (no shell string; on
//! Windows only a `.exe`, quoted per argument by the process host). It starts with the login shell's environment, so an editor the
//! app starts finds the same tools a terminal does. Nothing starts in a smoke.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_platform::{DetachedEnvironment, DetachedRequest, Os, PrefValue, PrefsStore};
use mighty_tools::editor::{self as core, Choice, Detected, Launch, PlanError, Target};
use serde_json::Value;
use tauri::Manager;

use crate::host::{Host, Registry, lock};
use crate::methods::{number, text};

/// The setting's name in the app's preferences (the registry on Windows, the defaults on the Mac).
pub const PREF: &str = "externalEditor";

/// How long a detection stands before a launch looks again (an editor installed meanwhile is found).
const DETECTION_STANDS: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    /// The choice made in a run that may not write the preferences (a smoke's, or one without `--allow-state-write`).
    held: Option<String>,
    /// The last detection and when it ran.
    detected: Option<(Vec<Detected>, Instant)>,
}

static STATE: Mutex<State> = Mutex::new(State { held: None, detected: None });

/// The setting: this run's choice, else the stored one (자동 when none).
pub fn choice(host: &Host) -> Choice {
    let held = lock(&STATE).held.clone();
    if let Some(held) = held {
        return Choice::parse(Some(&held));
    }
    let stored = host.app.try_state::<Arc<dyn PrefsStore>>().and_then(|prefs| prefs.get(PREF).ok().flatten());
    Choice::parse(stored.as_ref().and_then(PrefValue::text))
}

/// Saves the setting (held for this run when the preferences may not be written, as the delegation switch is).
pub fn set_choice(host: &Host, value: &str) {
    let value = Choice::parse(Some(value)).value().to_string();
    match host.app.try_state::<Arc<dyn PrefsStore>>().filter(|_| host.may_change_machine()) {
        Some(prefs) => {
            if let Err(error) = prefs.set(PREF, PrefValue::Text(value.clone())) {
                host.log.line(&format!("external editor not saved: {error}"));
                lock(&STATE).held = Some(value);
            }
        }
        None => lock(&STATE).held = Some(value),
    }
}

/// The editors the last detection found, without looking again (Settings' builder; none before the first detection).
pub fn detected() -> Vec<Detected> {
    lock(&STATE).detected.as_ref().map(|(found, _)| found.clone()).unwrap_or_default()
}

/// The disk as detection asks it.
struct Disk;

impl core::Probe for Disk {
    fn is_file(&self, path: &str) -> bool {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }
    fn names(&self, folder: &str) -> Vec<String> {
        std::fs::read_dir(folder)
            .map(|entries| entries.flatten().take(500).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }
    fn find_on_path(&self, name: &str, environment: &BTreeMap<String, String>) -> Option<String> {
        crate::platform::find_on_path(name, environment)
    }
}

/// The login shell's environment as the run engine keeps it (the app's own before the engine is up).
async fn shell_environment(host: &Host) -> BTreeMap<String, String> {
    match host.runs() {
        Some(runs) => runs.catalog.environment().resolve(None, false).await.values,
        None => std::env::vars().collect(),
    }
}

/// Looks for the editors again (off the page's thread) and keeps what it found.
pub async fn refresh(host: &Host) -> Vec<Detected> {
    let environment = shell_environment(host).await;
    let os = crate::platform::profile().os;
    let home = crate::accounts::home();
    let found = tauri::async_runtime::spawn_blocking(move || core::detect(os, &environment, &home, &Disk)).await.unwrap_or_default();
    lock(&STATE).detected = Some((found.clone(), Instant::now()));
    found
}

/// Settings opened: the editors looked for once more in the background (its builder shows them on its next pass).
pub fn refresh_in_background(host: &Arc<Host>) {
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        refresh(&host).await;
    });
}

/// The editors as a launch wants them: the last detection while it stands, else a new one.
async fn current(host: &Host) -> Vec<Detected> {
    let standing = lock(&STATE).detected.as_ref().filter(|(_, at)| at.elapsed() < DETECTION_STANDS).map(|(found, _)| found.clone());
    match standing {
        Some(found) => found,
        None => refresh(host).await,
    }
}

/// Opens `target` (a path already resolved inside its folder) as the setting says. The error is the banner's words.
pub async fn open(host: &Host, target: Target) -> Result<(), String> {
    let locale = host.locale;
    if host.smoke.is_some() {
        host.log.line("편집기에서 열기: not started in a smoke");
        return Ok(());
    }
    let detected = current(host).await;
    let launch = core::plan(&choice(host), &detected, &target).map_err(|error| match error {
        PlanError::Missing { name, .. } => locale.get_with("editor.error.missing", &[("editor", &name)]),
    })?;
    match launch {
        Launch::Program { editor, args } => {
            let os = crate::platform::profile().os;
            let environment = core::environment(&shell_environment(host).await);
            let cwd = core::working_folder(os, &target).filter(|folder| Path::new(folder).is_dir()).map(PathBuf::from);
            let Some(cwd) = cwd else { return Err(locale.get("editor.error.refused")) };
            let request = DetachedRequest {
                program: PathBuf::from(&editor.program),
                args,
                cwd,
                environment: environment.map_or(DetachedEnvironment::Inherit, DetachedEnvironment::Exactly),
                hidden: false,
            };
            let processes = host.processes.clone();
            let started = tauri::async_runtime::spawn_blocking(move || processes.start_detached_or_in_job(request))
                .await
                .map_err(|error| error.to_string())?;
            started
                .map(|_| ())
                .map_err(|error| locale.get_with("editor.error.failed", &[("editor", &editor.name), ("reason", &error.to_string())]))
        }
        Launch::OpenFile(path) => {
            let opened = tauri::async_runtime::spawn_blocking(move || crate::platform::file_ports().desktop.open(Path::new(&path)))
                .await
                .map_err(|error| error.to_string())?;
            match opened {
                Ok(true) => Ok(()),
                Ok(false) => Err(locale.get("editor.error.noDefaultApp")),
                Err(error) => Err(error.to_string()),
            }
        }
        Launch::ShowFolder(path) => tauri::async_runtime::spawn_blocking(move || crate::platform::show_in_explorer(Path::new(&path)))
            .await
            .map_err(|e| e.to_string())?,
    }
}

/// `path` (relative, "" for the folder itself) resolved inside `root` and opened: a folder as a folder, a file at `line`. A path that
/// is missing or leaves the root is refused.
pub async fn open_under(host: &Host, root: &str, path: &str, line: Option<u32>) -> Result<(), String> {
    let (relative, folder) = (path.to_string(), PathBuf::from(root));
    let resolved = tauri::async_runtime::spawn_blocking(move || mighty_core::workspace_files::resolve(&relative, &folder))
        .await
        .ok()
        .flatten()
        .ok_or_else(|| host.locale.get("editor.error.refused"))?;
    let path = resolved.to_string_lossy().into_owned();
    let target = if resolved.is_dir() { Target::Folder { path } } else { Target::File { path, line } };
    open(host, target).await
}

/// The folder `openInEditor` names, by its `source`: `files` (a files pane's workspace), `reference` (a pane's run folder: the
/// reference bubble, result files, 요청 변경 검토, the review queue's requests), `worktree` (a worktree pane's or delegated child's
/// worktree, `id`). None for a remote or unknown pane.
fn root(host: &Host, args: &Value) -> Result<Option<String>, String> {
    match text(args, "source") {
        Some("files") => crate::files::editor_root(host, args, false),
        Some("reference") => crate::files::editor_root(host, args, true),
        Some("worktree") => {
            Ok(text(args, "id").and_then(|id| crate::pane_worktree::review_record(host, id)).map(|r| r.worktree_path().to_string()))
        }
        _ => Ok(None),
    }
}

// ── an SSH machine's folder (MC-52) ─────────────────────────────────────────────────────────────────────────────────────────

/// MC-52: the editor that opens an SSH machine's folder now (VS Code or Cursor by their Remote-SSH address, as the setting says), its
/// name; None when the setting names none that can.
pub async fn remote_editor_name(host: &Host) -> Option<String> {
    let detected = current(host).await;
    core::remote_editor(&choice(host), &detected).map(|found| found.name.clone())
}

/// MC-52: `path` (a POSIX path on `workspace`'s SSH machine: a folder, or a file at `line`) opened in the editor by its Remote-SSH
/// address. Err with the banner's words when the setting names no editor that can.
pub async fn open_remote(host: &Host, workspace: &mighty_core::model::Workspace, path: &str, line: Option<u32>) -> Result<(), String> {
    let locale = host.locale;
    let detected = current(host).await;
    let Some(editor) = core::remote_editor(&choice(host), &detected) else { return Err(locale.get("ssh.editor.error.none")) };
    let scheme = core::remote_scheme(&editor.id).ok_or_else(|| locale.get("ssh.editor.error.none"))?;
    let launch = crate::ssh::launch_for(host, workspace)?;
    let uri = core::remote_uri(scheme, &launch.target.vscode_authority(), path, line).ok_or_else(|| locale.get("editor.error.refused"))?;
    if host.smoke.is_some() {
        host.log.line("open in editor (SSH): not opened in a smoke");
        return Ok(());
    }
    let name = editor.name.clone();
    match tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&uri)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(locale.get_with("editor.error.failed", &[("editor", &name), ("reason", &error)])),
        Err(error) => Err(error.to_string()),
    }
}

/// MC-52: an SSH workspace's place `openInEditor` names: the workspace and the folder on its machine the path is under (the files
/// pane's folder, an SSH pane's run folder, an SSH worktree pane's worktree). None for anything else.
fn ssh_root(host: &Host, args: &Value) -> Option<(mighty_core::model::Workspace, String)> {
    let view = host.view();
    let workspace_of = |id: &str| view.snapshot.workspace(id).filter(|w| w.is_ssh()).cloned();
    match text(args, "source")? {
        "files" => {
            let pane = view.snapshot.session(text(args, "paneId")?)?;
            let workspace = workspace_of(pane.workspace_id())?;
            let root = crate::ssh::files::root(workspace.id())
                .unwrap_or_else(|| workspace.ssh.as_ref().map(|r| r.dir.clone()).unwrap_or_default());
            Some((workspace, root))
        }
        "reference" => {
            let pane = view.snapshot.session(text(args, "paneId")?)?;
            let workspace = workspace_of(pane.workspace_id())?;
            let root = pane
                .working_folder
                .clone()
                .filter(|f| mighty_core::ssh::valid_remote_dir(f))
                .or_else(|| workspace.ssh.as_ref().map(|r| r.dir.clone()))?;
            Some((workspace, root))
        }
        "worktree" => {
            let pane = view.snapshot.session(text(args, "id")?)?;
            let workspace = workspace_of(pane.workspace_id())?;
            Some((workspace, pane.pane_worktree.as_ref()?.worktree_path().to_string()))
        }
        _ => None,
    }
    .filter(|(_, root)| mighty_core::ssh::valid_remote_dir(root))
}

/// MC-52: the path a page named under an SSH folder `root`, as a relative path inside it: relative already, or absolute inside it.
/// None for one that leaves it.
fn ssh_relative(root: &str, path: &str) -> Option<String> {
    use mighty_core::ssh::worktree::posix;
    let path = path.strip_prefix("file://").unwrap_or(path);
    let relative = if posix::absolute(path) {
        let (path, root) = (posix::standardized(path), posix::standardized(root));
        if path == root { String::new() } else { path.strip_prefix(&format!("{}/", root.trim_end_matches('/')))?.to_string() }
    } else {
        path.trim_start_matches("./").to_string()
    };
    mighty_core::ssh::files::valid_relative(&relative).then_some(relative)
}

/// MC-52: an SSH folder's path opened: in the editor by its Remote-SSH address, or (no editor that can) in the app's files pane rooted
/// at `root` with the path shown, the banner saying why. True when something opened.
pub async fn open_ssh(host: &Arc<Host>, workspace: &mighty_core::model::Workspace, root: &str, relative: &str, line: Option<u32>) -> bool {
    let full = if relative.is_empty() { root.to_string() } else { format!("{}/{relative}", root.trim_end_matches('/')) };
    match open_remote(host, workspace, &full, line).await {
        Ok(()) => true,
        Err(why) => {
            let shown = crate::files::open_ssh_folder(host, workspace.id(), root, relative);
            banner(host, if shown { host.locale.get_with("ssh.editor.fellBack", &[("reason", &why)]) } else { why });
            shown
        }
    }
}

/// The banner (C# `error.Text`).
fn banner(host: &Arc<Host>, message: String) {
    host.view().error = Some(message);
    host.schedule();
}

/// The page's `openInEditor{source, paneId | id, path?, line?}`: answers true when something was started.
pub fn register(methods: &mut Registry) {
    methods.add("openInEditor", |host: Arc<Host>, args| async move {
        // MC-52: a folder on an SSH machine opens by the editor's Remote-SSH address, or in the app's files pane.
        if let Some((workspace, root)) = ssh_root(&host, &args) {
            let line = core::line(number(&args, "line"));
            let Some(relative) = ssh_relative(&root, text(&args, "path").unwrap_or("")) else {
                banner(&host, host.locale.get("editor.error.refused"));
                return Ok(Value::Bool(false));
            };
            return Ok(Value::Bool(open_ssh(&host, &workspace, &root, &relative, line).await));
        }
        let Some(root) = root(&host, &args)? else {
            banner(&host, host.locale.get("editor.error.refused"));
            return Ok(Value::Bool(false));
        };
        let path = text(&args, "path").unwrap_or("").to_string();
        if path.encode_utf16().count() > 4096 {
            return Ok(Value::Bool(false));
        }
        let line = core::line(number(&args, "line"));
        // A path a pane named (absolute inside its folder, a `file:` destination, a `:line` suffix) is read as its reference bubble
        // reads it (`ReferencePreview.Resolve`): an existing file inside the run folder, else refused.
        let (path, line) = if text(&args, "source") == Some("reference") && !path.is_empty() {
            let (named, folder, asked) = (path.clone(), root.clone(), line.and_then(|l| i32::try_from(l).ok()));
            let target =
                tauri::async_runtime::spawn_blocking(move || mighty_files::reference::resolve(&named, &folder, asked)).await.ok().flatten();
            let Some(target) = target else {
                banner(&host, host.locale.get("editor.error.refused"));
                return Ok(Value::Bool(false));
            };
            (target.relative_path, target.line.and_then(|l| u32::try_from(l).ok()).or(line))
        } else {
            (path, line)
        };
        match open_under(&host, &root, &path, line).await {
            Ok(()) => Ok(Value::Bool(true)),
            Err(error) => {
                banner(&host, error);
                Ok(Value::Bool(false))
            }
        }
    });
}

/// The settings picker's rows: (value, words) — 자동 naming what it opens now, each editor found, 시스템 기본, and the chosen editor
/// when it is no longer found — and which row is chosen.
pub fn picker_rows(locale: &mighty_core::locale::Locale, choice: &Choice, detected: &[Detected], os: Os) -> (Vec<(String, String)>, usize) {
    let automatic = match detected.first() {
        Some(first) => locale.get_with("settings.editor.autoNamed", &[("editor", &first.name)]),
        None => locale.get("settings.editor.autoNone"),
    };
    let mut rows = vec![("auto".to_string(), automatic)];
    rows.extend(detected.iter().map(|found| (found.id.clone(), format!("{} — {}", found.name, core::program_name(os, found)))));
    if let Choice::Editor(id) = choice
        && !detected.iter().any(|found| found.id == *id)
    {
        let name = core::editor(id).map_or(id.as_str(), |editor| editor.name);
        rows.push((id.clone(), locale.get_with("settings.editor.missing", &[("editor", name)])));
    }
    rows.push(("system".to_string(), locale.get("settings.editor.system")));
    let chosen = rows.iter().position(|(value, _)| value == choice.value()).unwrap_or(0);
    (rows, chosen)
}

#[cfg(test)]
mod tests {
    use mighty_core::locale::Locale;

    /// MC-52: a path named under an SSH folder stays inside it.
    #[test]
    fn an_ssh_path_stays_inside_its_folder() {
        let root = "/home/me/.mightycode/worktrees/abc";
        assert_eq!(super::ssh_relative(root, "src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(super::ssh_relative(root, "./src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(super::ssh_relative(root, "/home/me/.mightycode/worktrees/abc/src/x.rs").as_deref(), Some("src/x.rs"));
        assert_eq!(super::ssh_relative(root, root).as_deref(), Some(""));
        assert_eq!(super::ssh_relative(root, "").as_deref(), Some(""));
        assert_eq!(super::ssh_relative(root, "../other/x"), None);
        assert_eq!(super::ssh_relative(root, "/home/me/.mightycode/worktrees/abcd/x"), None);
        assert_eq!(super::ssh_relative(root, "/etc/passwd"), None);
        assert_eq!(super::ssh_relative(root, "/home/me/.mightycode/worktrees/abc/../x"), None);
    }
    use mighty_tools::editor::Family;

    use super::*;

    #[test]
    fn the_picker_lists_auto_each_editor_found_and_the_systems_default() {
        let locale = Locale::new("en");
        let code = Detected {
            id: "vscode".into(),
            name: "Visual Studio Code".into(),
            family: Family::VsCode,
            program: "/usr/local/bin/code".into(),
        };
        let (rows, chosen) = picker_rows(&locale, &Choice::Auto, std::slice::from_ref(&code), Os::Macos);
        let values: Vec<&str> = rows.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(values, ["auto", "vscode", "system"]);
        assert_eq!(chosen, 0);
        assert!(rows[0].1.contains("Visual Studio Code"), "자동 names what it opens: {}", rows[0].1);
        assert!(rows[1].1.ends_with("code"));
        let (rows, chosen) = picker_rows(&locale, &Choice::Editor("zed".into()), &[code], Os::Macos);
        assert_eq!(
            rows.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>(),
            ["auto", "vscode", "zed", "system"],
            "a chosen editor not found stays listed"
        );
        assert_eq!(chosen, 2);
        let (rows, chosen) = picker_rows(&locale, &Choice::System, &[], Os::Windows);
        assert_eq!(rows.len(), 2);
        assert_eq!(chosen, 1);
    }
}

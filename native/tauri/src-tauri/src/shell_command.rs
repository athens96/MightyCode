//! M-F: the Workspace menu's "Install shell command 'mighty'": `~/.local/bin/mighty`, mode 0755, the script of Swift
//! `OSIntegration.swift:38-76` (`ShellCommand`), then whether that folder is on the login shell's PATH (the CLI environment the runs
//! use). Only on the menu item, where the write policy lets the app change the machine (plan §6), never in a smoke; tests use a
//! temporary HOME (Swift `OSIntegrationTests.swift:18` "theShellCommandInstallsExecutable").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use crate::host::Host;

/// The command's name.
pub const NAME: &str = "mighty";

/// The `mighty` shell command (hardened version of Swift `ShellCommand.script`, with `--` guards so dash-prefixed folders or prompts
/// cannot be read as flags or injected into `osascript`; its file ends with one more newline).
pub const SCRIPT: &str = r#"#!/bin/sh
# Mighty Code: open a folder (default: the current one) as a workspace,
# with an optional request put in the composer (never sent).
#   mighty                 open this folder
#   mighty ../other        open another folder
#   mighty "fix the tests" open this folder with a request
dir="$PWD"
if [ $# -gt 0 ] && [ -d "$1" ]; then case "$1" in -*) dir="$(CDPATH= cd -- "./$1" && pwd)" ;; *) dir="$(CDPATH= cd -- "$1" && pwd)" ;; esac; shift; fi
enc() { osascript -l JavaScript -e 'function run(a) { return encodeURIComponent(a[0]) }' -- "$1"; }
url="mightycode://open?path=$(enc "$dir")"
if [ $# -gt 0 ]; then url="$url&prompt=$(enc "$*")"; fi
exec open "$url""#;

/// `~/.local/bin`, the folder the command goes in (no administrator needed).
pub fn folder(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

/// What [`install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The command is written there.
    Written(PathBuf),
    /// Something that is not the app's command is there (another program's `mighty`, a link): left alone until the person says.
    Foreign(PathBuf),
}

/// Whether `path` is the app's own command: a plain file (not a link) that starts as [`SCRIPT`] does.
pub fn ours(path: &Path) -> bool {
    let plain = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file());
    let head: String = SCRIPT.lines().take(2).collect::<Vec<_>>().join("\n");
    plain && std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(head.as_bytes()))
}

/// Writes the command under `home`: staged in a file of its own made new (never through a link) and executable from the start, then
/// moved in place; the staged file removed if anything fails (review item 8). A file there that is not the app's is replaced only with
/// `replace`.
pub fn install(home: &Path, replace: bool) -> Result<Installed, String> {
    let folder = folder(home);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(NAME);
    if !replace && std::fs::symlink_metadata(&path).is_ok() && !ours(&path) {
        return Ok(Installed::Foreign(path));
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let staged = folder.join(format!(".{NAME}.{}.{stamp}.tmp", std::process::id()));
    crate::platform::write_new_executable(&staged, format!("{SCRIPT}\n").as_bytes()).map_err(|e| e.to_string())?;
    if let Err(error) = std::fs::rename(&staged, &path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.to_string());
    }
    Ok(Installed::Written(path))
}

/// Swift `ShellCommand.onPath`: whether `folder` is one of `path`'s entries, compared standardized.
pub fn on_path(path: Option<&str>, folder: &Path) -> bool {
    let wanted = crate::deep_link::standardized(&folder.to_string_lossy());
    path.unwrap_or("").split(':').any(|entry| entry.starts_with('/') && crate::deep_link::standardized(entry) == wanted)
}

/// The words the app has no catalogue key for yet (the locale files are the C# page's too, whose build must not change): why this run
/// may not install, and the question before replacing someone else's `mighty`, its message and its button.
pub fn words(language: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match language {
        "ko" => (
            "이 실행은 이 Mac을 바꿀 수 없어서 'mighty' 명령을 설치하지 않았습니다. 설치된 앱이나 --allow-state-write로 실행하세요.",
            "'mighty'를 바꿀까요?",
            "{path}는 Mighty Code의 명령이 아닙니다. Mighty Code의 명령으로 바꿀까요?",
            "바꾸기",
        ),
        "ja" => (
            "この実行はこの Mac を変更できないため、'mighty' コマンドをインストールしませんでした。インストール済みのアプリか --allow-state-write で実行してください。",
            "'mighty' を置き換えますか?",
            "{path} は Mighty Code のコマンドではありません。Mighty Code のコマンドに置き換えますか?",
            "置き換える",
        ),
        "zh" => (
            "此次运行不能更改这台 Mac,因此没有安装 'mighty' 命令。请使用已安装的应用或 --allow-state-write 运行。",
            "要替换 'mighty' 吗?",
            "{path} 不是 Mighty Code 的命令。要替换为 Mighty Code 的命令吗?",
            "替换",
        ),
        _ => (
            "The 'mighty' command was not installed: this run may not change this Mac. Use the installed app, or run with --allow-state-write.",
            "Replace 'mighty'?",
            "{path} is not Mighty Code's command. Replace it with Mighty Code's?",
            "Replace",
        ),
    }
}

/// The menu item (Swift `installShellCommand`): the command written, and the page told where and whether the shell finds it. Nothing
/// is written while a sheet is up (review item 9) or where the run may not change the machine.
pub async fn install_from_menu(host: &Arc<Host>) {
    let l = &host.locale;
    let (refused, question, replace_message, replace) = words(l.language());
    if !host.may_change_machine() {
        host.log.line("shell command: not installed (this run may not change the machine)");
        host.view().error = Some(refused.to_string());
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return };
    let path = match install(&home, false) {
        Ok(Installed::Written(path)) => path,
        Ok(Installed::Foreign(path)) => {
            let message = replace_message.replace("{path}", &path.display().to_string());
            let (_, answer) = crate::dialogs::of(host).open(crate::deep_link::confirmation(l, question, &message, replace));
            if answer.await.ok() != Some(serde_json::Value::Bool(true)) {
                return;
            }
            match install(&home, true) {
                Ok(Installed::Written(path)) => path,
                Ok(Installed::Foreign(_)) => return,
                Err(error) => {
                    host.view().error = Some(error);
                    return;
                }
            }
        }
        Err(error) => {
            host.view().error = Some(error);
            return;
        }
    };
    // The login shell's PATH, as the runs see it (a GUI app's own PATH is launchd's short one).
    let shell_path = match host.runs() {
        Some(runs) => runs.catalog.environment().resolve(None, false).await.values.get("PATH").cloned(),
        None => None,
    };
    let (path_text, folder_text) = (path.display().to_string(), folder(&home).display().to_string());
    let message = if on_path(shell_path.as_deref(), &folder(&home)) {
        l.get_with("shellCommand.installed.ready", &[("path", &path_text)])
    } else {
        l.get_with("shellCommand.installed.addPath", &[("path", &path_text), ("folder", &folder_text)])
    };
    let (_, answer) = crate::dialogs::of(host).open(json!({
        "type": "confirm", "title": l.get("shellCommand.installed.title"), "message": message, "close": l.get("common.close"),
    }));
    let _ = answer.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `OSIntegrationTests.swift:18` "theShellCommandInstallsExecutable": under a temporary HOME only.
    #[test]
    fn the_shell_command_installs_executable() {
        let home = std::env::temp_dir().join(format!("mighty-home-{}-{}", std::process::id(), mighty_core::wire::new_id()));
        let Installed::Written(path) = install(&home, false).unwrap() else { panic!("written") };
        assert!(path.ends_with(".local/bin/mighty"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#!/bin/sh") && text.ends_with("exec open \"$url\"\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        }
        // The PATH check is the Mac's (POSIX paths, compared standardized), whatever OS runs the test.
        let mac_folder = Path::new("/Users/me/.local/bin");
        assert!(on_path(Some("/usr/bin:/Users/me/.local/bin/"), mac_folder));
        assert!(on_path(Some("/usr/bin:/Users/me/./.local/bin"), mac_folder));
        assert!(!on_path(Some("/usr/bin"), mac_folder));
        assert!(!on_path(None, mac_folder));
        // Installed again: its own command replaced in place, nothing left beside it.
        assert!(ours(&path));
        assert_eq!(install(&home, false).unwrap(), Installed::Written(path.clone()));
        assert_eq!(std::fs::read_dir(folder(&home)).unwrap().count(), 1);
        // Review item 8: someone else's `mighty` is left alone until the person says, then replaced.
        std::fs::write(&path, "#!/bin/sh\necho another tool\n").unwrap();
        assert!(!ours(&path));
        assert_eq!(install(&home, false).unwrap(), Installed::Foreign(path.clone()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "#!/bin/sh\necho another tool\n");
        assert_eq!(install(&home, true).unwrap(), Installed::Written(path.clone()));
        assert!(ours(&path));
        assert_eq!(std::fs::read_dir(folder(&home)).unwrap().count(), 1, "no staged file left");
        std::fs::remove_dir_all(&home).unwrap();
        for language in mighty_core::locale::LANGUAGES {
            assert!(words(language).2.contains("{path}"));
        }
    }

    /// P0-SEC-1: both `cd` and `osascript` terminate option parsing with `--` so dash-prefixed folder names and prompts cannot inject
    /// flags or JXA code.
    #[test]
    fn the_script_guards_cd_and_osascript_against_option_injection() {
        assert!(SCRIPT.contains("cd -- "), "cd must use --: {SCRIPT}");
        assert!(
            SCRIPT.contains("osascript -l JavaScript -e 'function run(a) { return encodeURIComponent(a[0]) }' -- \"$1\""),
            "osascript must terminate flags with -- before \"$1\": {SCRIPT}"
        );
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::disallowed_methods, reason = "the test runs the installed shell script itself, no child of the app")]
    #[test]
    fn dash_prefixed_prompts_and_folders_are_encoded_literally_not_executed() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;

        let root = std::env::temp_dir().join(format!("mighty-sec1-{}-{}", std::process::id(), mighty_core::wire::new_id()));
        let home = root.join("home");
        let work = root.join("work");
        let fake_bin = root.join("bin");
        let marker = root.join("injected-marker");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&fake_bin).unwrap();
        // Fake `open` captures the URL argument instead of launching the app.
        let fake_open = fake_bin.join("open");
        std::fs::write(&fake_open, "#!/bin/sh\nprintf '%s' \"$1\"\n").unwrap();
        std::fs::set_permissions(&fake_open, std::fs::Permissions::from_mode(0o755)).unwrap();

        let Installed::Written(mighty) = install(&home, false).unwrap() else { panic!("written") };
        let path_env = format!("{}:/usr/bin:/bin", fake_bin.display());
        let run = |cwd: &Path, args: &[&str]| -> String {
            let output =
                Command::new(&mighty).args(args).current_dir(cwd).env("PATH", &path_env).output().expect("run installed mighty script");
            assert!(output.status.success(), "mighty failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };

        // 1. Prompt starting with `-e` trying to inject JXA code must be URL-encoded literally, never executed.
        let payload = format!("-e 'ObjC.import(\"stdlib\");$.system(\"touch {}\")'", marker.display());
        let url = run(&work, &[&payload]);
        assert!(!marker.exists(), "JXA injection must never execute");
        let expected_work = std::fs::canonicalize(&work).unwrap();
        let parsed = tauri::Url::parse(&url).unwrap();
        assert_eq!(parsed.scheme(), "mightycode");
        assert_eq!(parsed.host_str(), Some("open"));
        let params: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        assert_eq!(params.get("path").map(String::as_str), Some(expected_work.to_str().unwrap()));
        assert_eq!(params.get("prompt").map(String::as_str), Some(payload.as_str()));

        // 2. Other flag-shaped prompts (`-e`, `-l AppleScript`, `--`) are also encoded literally.
        for prompt in ["-e", "-l AppleScript", "--"] {
            let url = run(&work, &[prompt]);
            let parsed = tauri::Url::parse(&url).unwrap();
            let params: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            assert_eq!(params.get("prompt").map(String::as_str), Some(prompt), "prompt {prompt:?}");
        }

        // 3. Folders starting with `-` (including `-L`, `-e`, and literal `-`) are resolved safely, not read as flags or OLDPWD.
        for name in ["-L", "-e", "-"] {
            let dash_dir = work.join(name);
            std::fs::create_dir_all(&dash_dir).unwrap();
            let expected = std::fs::canonicalize(&dash_dir).unwrap();
            let url = run(&work, &[name, "-e", "ObjC.import(\"stdlib\")"]);
            let parsed = tauri::Url::parse(&url).unwrap();
            let params: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            assert_eq!(params.get("path").map(String::as_str), Some(expected.to_str().unwrap()), "folder {name:?}");
            assert_eq!(params.get("prompt").map(String::as_str), Some("-e ObjC.import(\"stdlib\")"), "folder {name:?} prompt");
        }

        std::fs::remove_dir_all(&root).unwrap();
    }
}

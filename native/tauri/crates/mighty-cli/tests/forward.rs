//! MC-43: the built `mighty` program hands its arguments, standard input and output to `<app> --cli …` and ends with the app's exit
//! code. Run against a stand-in app (a shell script) where scripts run as programs; Windows' real check is in
//! docs/windows-v060-checklist.md.

#![cfg(unix)]
#![allow(clippy::disallowed_methods, reason = "the test runs the built program itself, no child of the app")]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn stand_in(folder: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let app = folder.join("MightyCode");
    std::fs::write(&app, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
    app
}

fn mighty(args: &[&str], app: &Path, input: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mighty"))
        .args(args)
        .env("MIGHTY_CODE_APP", app)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    (output.status.code(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn the_command_runs_the_apps_cli_with_its_words_pipes_and_exit_code() {
    let root = std::env::temp_dir().join(format!("mighty-cli-forward-{}", std::process::id()));
    let app = stand_in(&root, r#"printf '%s|' "$@"; cat; exit 2"#);
    let (code, out, _) = mighty(&["agent", "send", "p1", "a&b | \"c\" %PATH%", "-"], &app, "from stdin");
    assert_eq!(code, Some(2), "the app's exit code");
    assert_eq!(out, "--cli|agent|send|p1|a&b | \"c\" %PATH%|-|from stdin", "words as given, then standard input");

    let (code, out, _) = mighty(&[], &app, "");
    assert_eq!((code, out.as_str()), (Some(2), "--cli|"), "no arguments: the app's usage");

    let (code, out, err) = mighty(&["C:/code/app", "fix it"], &app, "");
    assert_eq!((code, out.as_str()), (Some(1), ""), "a folder never reaches the app");
    assert!(err.contains("not on Windows yet"), "{err}");

    let (code, _, err) = mighty(&["status"], &root.join("missing"), "");
    assert_eq!(code, Some(1));
    assert!(err.contains("is not there"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

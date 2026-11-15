//! The login shell's environment (Swift `CLIEnvironmentTests.swift`): captured by a stand-in shell that, like the checks' Swift
//! fake, insists on `-ilc`, reads its settings from the folder it runs in and execs the capture's command.
#![cfg(target_os = "macos")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use mighty_platform::{EnvironmentSource, ShellEnvironment};
use mighty_platform_macos::environment::{EnvironmentOptions, MacShellEnvironment};
use mighty_platform_macos::process::MacProcessHost;

fn fake_shell(root: &Path) -> PathBuf {
    script(
        root,
        "login-shell",
        r#"[ "$1" = '-ilc' ] || exit 10
printf 'startup chatter\n'
printf 'call\n' >> "$PWD/shell-calls"
. "$PWD/shell-settings"
printf 'ready\n' > "$PWD/shell-ready"
[ ! -f "$PWD/shell-delay" ] || /bin/sleep 0.3
exec /bin/sh -c "$2""#,
    )
}

fn settings(folder: &Path, model: &str, bin: Option<&Path>) {
    let path = bin.map(|bin| bin.to_string_lossy().into_owned()).unwrap_or_else(|| "/usr/bin:/bin".into());
    std::fs::write(
        folder.join("shell-settings"),
        format!("export PATH='{path}'\nexport FIXTURE_MODEL='{model}'\nexport AWS_BEARER_TOKEN_BEDROCK='fixture-rotated'\nunset AWS_ACCESS_KEY_ID\n"),
    )
    .unwrap();
}

fn base(root: &Path) -> BTreeMap<String, String> {
    [
        ("HOME", root.to_string_lossy().as_ref()),
        ("PATH", "/usr/bin:/bin"),
        ("AWS_ACCESS_KEY_ID", "fixture-stale"),
        ("AWS_BEARER_TOKEN_BEDROCK", "fixture-old"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

/// The capture is reused for 10 minutes, not the app's 10 s, so a busy runner cannot expire it between a check's resolves; every
/// refresh these checks mean is a forced one (Swift's `resolver`).
fn resolver(root: &Path, shell: PathBuf, timeout: Duration) -> Arc<MacShellEnvironment> {
    let options = EnvironmentOptions {
        base: Some(base(root)),
        shell,
        home: root.to_path_buf(),
        timeout,
        cache_for: Duration::from_secs(600),
        appended: None,
    };
    Arc::new(MacShellEnvironment::new(Arc::new(MacProcessHost::new()), options))
}

fn model(environment: &mighty_platform::CliEnvironment) -> Option<&str> {
    environment.values.get("FIXTURE_MODEL").map(String::as_str)
}

/// Swift `CLIEnvironmentTests.swift:45 "shellSnapshotReplacesUnsetValuesAndRefreshesPerWorkspace"`.
#[tokio::test(flavor = "multi_thread")]
async fn shell_snapshot_replaces_unset_values_and_refreshes_per_workspace() {
    let root = temp("env-snapshot");
    let shell = fake_shell(&root);
    settings(&root, "model-one", None);
    let resolver = resolver(&root, shell, Duration::from_secs(30));
    let first = resolver.resolve(None, false).await;
    assert_eq!(first.source, EnvironmentSource::Profile);
    assert!(!first.values.contains_key("AWS_ACCESS_KEY_ID"), "a value the profile unset is not inherited");
    assert_eq!(first.values.get("AWS_BEARER_TOKEN_BEDROCK").map(String::as_str), Some("fixture-rotated"));
    assert!(!format!("{first:?}").contains("fixture-") && !format!("{first:?}").contains("AWS_"), "no value or name in Debug");
    settings(&root, "model-two", None);
    assert_eq!(model(&resolver.resolve(None, false).await), Some("model-one"), "reused within its time");
    assert_eq!(model(&resolver.resolve(None, true).await), Some("model-two"), "a forced resolve captures again");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    settings(&workspace, "workspace-model", None);
    assert_eq!(model(&resolver.resolve(Some(&workspace), false).await), Some("workspace-model"), "captured in the workspace");
    assert_eq!(model(&resolver.resolve(None, false).await), Some("model-two"));
    remove(&root);
}

/// Swift `CLIEnvironmentTests.swift:63 "concurrentRefreshesShareWorkAndInvalidationUsesTheNewEnvironment"`.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_refreshes_share_work_and_invalidation_uses_the_new_environment() {
    let root = temp("env-concurrent");
    let shell = fake_shell(&root);
    settings(&root, "model-one", None);
    std::fs::write(root.join("shell-delay"), "").unwrap();
    let resolver = resolver(&root, shell, Duration::from_secs(30));
    let (one, two) = (resolver.clone(), resolver.clone());
    let first = tokio::spawn(async move { one.resolve(None, true).await });
    let second = tokio::spawn(async move { two.resolve(None, true).await });
    assert_eq!(model(&first.await.unwrap()), Some("model-one"));
    assert_eq!(model(&second.await.unwrap()), Some("model-one"));
    assert_eq!(std::fs::read_to_string(root.join("shell-calls")).unwrap().lines().count(), 1, "one capture served both");
    std::fs::remove_file(root.join("shell-ready")).unwrap();
    let three = resolver.clone();
    let old = tokio::spawn(async move { three.resolve(None, true).await });
    until(|| root.join("shell-ready").exists(), 30_000, "the capture started").await;
    settings(&root, "model-two", None);
    let current = resolver.resolve(None, true).await;
    assert_eq!((current.source, model(&current)), (EnvironmentSource::Profile, Some("model-two")));
    assert_eq!(model(&old.await.unwrap()), Some("model-two"), "a superseded capture answers with the one that replaced it");
    remove(&root);
}

/// Swift `CLIEnvironmentTests.swift:82 "malformedFailedAndTimedOutShellsFallBackWithoutEchoingTheirOutput"` (the parse half is
/// environment.rs' unit test).
#[tokio::test(flavor = "multi_thread")]
async fn malformed_failed_and_timed_out_shells_fall_back_without_echoing_their_output() {
    let root = temp("env-bad");
    for (index, content) in ["printf fixture-secret; exit 1", "printf fixture-secret", "/bin/sleep 5"].into_iter().enumerate() {
        let shell = script(&root, &format!("bad-shell-{index}"), content);
        let snapshot = resolver(&root, shell, Duration::from_millis(50)).resolve(None, false).await;
        assert_eq!(snapshot.source, EnvironmentSource::ProcessFallback, "{content}");
        assert_eq!(snapshot.values.get("AWS_ACCESS_KEY_ID").map(String::as_str), Some("fixture-stale"));
        assert!(!format!("{snapshot:?}").contains("fixture-secret"));
    }
    remove(&root);
}

/// Swift `CLIEnvironmentTests.swift:155 "theLoginPathEndsWithTheInstallersFoldersOnlyForTheRealEnvironment"` (its capture half).
#[tokio::test(flavor = "multi_thread")]
async fn the_login_path_ends_with_the_installers_folders_only_for_the_real_environment() {
    let root = temp("env-path");
    let shell = fake_shell(&root);
    settings(&root, "model-one", None);
    let plain = resolver(&root, shell.clone(), Duration::from_secs(30)).resolve(Some(&root), false).await;
    assert_eq!(plain.values.get("PATH").map(String::as_str), Some("/usr/bin:/bin"), "a given environment gets nothing appended");
    let local = format!("{}/.local/bin", root.display());
    let options = EnvironmentOptions {
        base: Some(base(&root)),
        shell,
        home: root.clone(),
        timeout: Duration::from_secs(30),
        cache_for: Duration::from_secs(600),
        appended: Some(vec![local.clone(), "/usr/bin".into()]),
    };
    let appended = MacShellEnvironment::new(Arc::new(MacProcessHost::new()), options).resolve(Some(&root), false).await;
    assert_eq!(appended.source, EnvironmentSource::Profile);
    assert_eq!(appended.values.get("PATH").cloned(), Some(format!("/usr/bin:/bin:{local}")));
    remove(&root);
}

/// Before any resolve, `current` is the app's environment; afterwards the last capture; an invalidated folder is captured again.
#[tokio::test(flavor = "multi_thread")]
async fn current_is_the_last_capture_and_invalidate_forgets_one() {
    let root = temp("env-current");
    let shell = fake_shell(&root);
    settings(&root, "model-one", None);
    let resolver = resolver(&root, shell, Duration::from_secs(30));
    assert_eq!(resolver.current().source, EnvironmentSource::ProcessFallback);
    assert_eq!(model(&resolver.resolve(None, false).await), Some("model-one"));
    assert_eq!(model(&resolver.current()), Some("model-one"));
    settings(&root, "model-two", None);
    resolver.invalidate(None);
    assert_eq!(model(&resolver.resolve(None, false).await), Some("model-two"));
    remove(&root);
}

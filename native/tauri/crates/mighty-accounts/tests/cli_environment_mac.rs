//! Swift `CLIEnvironmentTests.swift:98 "providerDiscoveryMetadataAndAccountCommandsUseResolvedShellEnvironment"`, over the Mac process
//! host and the Mac login-shell environment: the provider catalog's discovery and model list, the accounts' status and sign-out and a
//! run all start the CLI with what the login shell exported (a rotated Bedrock token, an unset stale key, its PATH), and a forced
//! refresh moves them to the profile's new PATH. Mac only: the shell and the CLIs are `/bin/sh` scripts, as Swift's.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_accounts::accounts::{CliAccountsCoordinator, HostRunner};
use mighty_core::model::Workspace;
use mighty_platform::ShellEnvironment;
use mighty_platform_macos::environment::{EnvironmentOptions, MacShellEnvironment};
use mighty_run::manager::ResolveWorkspace;
use mighty_run::model::StartRunRequest;
use mighty_run::providers::ProviderCatalog;
use mighty_run::{RunManager, RunManagerOptions};

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-cli-env-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(path).unwrap()
}

fn script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn settings(root: &Path, model: &str, bin: &Path) {
    std::fs::write(
        root.join("shell-settings"),
        format!(
            "export PATH='{}'\nexport FIXTURE_MODEL='{model}'\nexport AWS_BEARER_TOKEN_BEDROCK='fixture-rotated'\nunset AWS_ACCESS_KEY_ID\n",
            bin.display()
        ),
    )
    .unwrap();
}

const SHELL: &str = r#"[ "$1" = '-ilc' ] || exit 10
printf 'startup chatter\n'
printf 'call\n' >> "$PWD/shell-calls"
. "$PWD/shell-settings"
printf 'ready\n' > "$PWD/shell-ready"
exec /bin/sh -c "$2""#;

const CODEX: &str = r#"[ "$AWS_BEARER_TOKEN_BEDROCK" = 'fixture-rotated' ] || exit 11
[ -z "${AWS_ACCESS_KEY_ID+x}" ] || exit 12
if [ "$1" = '--version' ]; then printf '0.153.4\n'; exit 0; fi
if [ "$1" = 'logout' ]; then printf done > "$PWD/logout-marker"; exit 0; fi
if [ "$1" = 'login' ]; then
  if [ -f "$PWD/logout-marker" ]; then printf 'Not logged in\n'; else printf 'Logged in using ChatGPT\n'; fi
  exit 0
fi
case " $* " in
  *' exec '* )
    IFS= read -r prompt
    printf done > "$PWD/run-marker"
    printf '{"type":"thread.started","thread_id":"fixture-thread"}\n{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"Fixture completed"}}\n'
    exit 0;;
esac
IFS= read -r init
printf '{"id":1,"result":{}}\n'
IFS= read -r notification; IFS= read -r models
printf '{"id":2,"result":{"data":[{"model":"%s","displayName":"Fixture"}]}}\n' "$FIXTURE_MODEL"
/bin/cat >/dev/null"#;

async fn wait_for(file: &Path) {
    let end = Instant::now() + Duration::from_secs(30);
    while !file.exists() {
        assert!(Instant::now() < end, "{} was never written", file.display());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Swift `CLIEnvironmentTests.swift:98 "providerDiscoveryMetadataAndAccountCommandsUseResolvedShellEnvironment"`.
#[tokio::test(flavor = "multi_thread")]
async fn provider_discovery_metadata_and_account_commands_use_resolved_shell_environment() {
    mighty_run::strings::set_language("ko");
    let root = temp("discovery");
    let shell = root.join("login-shell");
    script(&shell, SHELL);
    let (first_bin, second_bin) = (root.join("first"), root.join("second"));
    for bin in [&first_bin, &second_bin] {
        std::fs::create_dir_all(bin).unwrap();
        script(&bin.join("codex"), CODEX);
    }
    settings(&root, "model-one", &first_bin);
    let base: BTreeMap<String, String> = [
        ("HOME", root.to_string_lossy().as_ref()),
        ("PATH", "/usr/bin:/bin"),
        ("AWS_ACCESS_KEY_ID", "fixture-stale"),
        ("AWS_BEARER_TOKEN_BEDROCK", "fixture-old"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect();
    let options = EnvironmentOptions {
        base: Some(base),
        shell,
        home: root.clone(),
        timeout: Duration::from_secs(30),
        cache_for: Duration::from_secs(600),
        coalesce: Duration::from_millis(50),
        appended: None,
    };
    let processes = mighty_platform_macos::process_host();
    let environment: Arc<dyn ShellEnvironment> = Arc::new(MacShellEnvironment::new(processes.clone(), options));
    let catalog = Arc::new(ProviderCatalog::new(processes.clone(), environment.clone(), None));
    let folder = root.to_string_lossy().into_owned();

    catalog.refresh_environment(false).await;
    let first = catalog.refresh_provider("codex", Some(&folder), true).await;
    assert!(first.available, "{first:?}");
    assert_eq!(first.model_catalog.models.last().map(|m| m.value.as_str()), Some("model-one"));
    let found = catalog.find("codex").await.expect("codex is found on the login PATH");
    assert_eq!(PathBuf::from(&found.binary), first_bin.join("codex"));

    settings(&root, "model-two", &second_bin);
    catalog.refresh_environment(true).await;
    let fresh = catalog.refresh_provider("codex", Some(&folder), true).await;
    assert_eq!(fresh.model_catalog.models.last().map(|m| m.value.as_str()), Some("model-two"), "{fresh:?}");
    let found = catalog.find("codex").await.expect("codex is found on the new login PATH");
    assert_eq!(PathBuf::from(&found.binary), second_bin.join("codex"));

    let accounts = CliAccountsCoordinator::new(Arc::new(HostRunner::new(processes.clone())), catalog.clone(), folder.clone());
    accounts.refresh(&["codex"]).await;
    assert_eq!(accounts.statuses()["codex"].logged_in, Some(true), "{:?}", accounts.statuses()["codex"]);
    assert_eq!(accounts.logout("codex").await.logged_in, Some(false));
    assert!(root.join("logout-marker").exists(), "the sign-out ran in the home with the login environment");

    let workspace = Workspace { id: Some("fixture-workspace".into()), path: Some(folder.clone()), ..Workspace::default() };
    let resolve: ResolveWorkspace = {
        let workspace = workspace.clone();
        Arc::new(move |_| {
            let workspace = workspace.clone();
            Box::pin(async move { Ok(workspace) })
        })
    };
    let manager = RunManager::new(catalog.clone(), RunManagerOptions::new("", resolve, Arc::new(|_| {})));
    manager
        .start(StartRunRequest::new("fixture-session", "fixture-workspace", "claude", "Fixture input").with_provider("codex"))
        .await
        .unwrap();
    wait_for(&root.join("run-marker")).await;
    manager.dispose().await;
    let _ = std::fs::remove_dir_all(&root);
}

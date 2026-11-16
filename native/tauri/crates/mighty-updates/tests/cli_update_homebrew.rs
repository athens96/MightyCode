//! Mac stage M-J: the Mac's CLI updater (`PlatformProfile::cli_updater` = `Homebrew`), the Swift checks of `CLIUpdateTests.swift`
//! ported by name over private temp trees, on every OS. The CLIs never start: a fake runner answers `--version` and the update, and
//! the Homebrew check goes through the app's runner over `mighty_platform::fake`'s process host, so the request it makes (the
//! program, the arguments, the Homebrew switches) is what is checked. The same trees under `Winget` (Windows' planner) update
//! nothing, so each value of the field is tested.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::*;
use mighty_platform::CliUpdater;
use mighty_platform::fake::{FakeProcessHost, FakeScript};
use mighty_updates::cli_update::CliUpdateService;
use mighty_updates::cli_update::service::BREW_SWITCHES;
use mighty_updates::runner::{CancellationToken, CliRunResult, CliRunner, HostCliRunner, RunError};

fn none() -> CancellationToken {
    CancellationToken::new()
}

fn mac(runner: Arc<dyn CliRunner>, environment: BTreeMap<String, String>, home: &Path) -> CliUpdateService {
    CliUpdateService::new(runner, environment, text(home)).with_updater(CliUpdater::Homebrew)
}

fn windows(runner: Arc<dyn CliRunner>, environment: BTreeMap<String, String>, home: &Path) -> CliUpdateService {
    CliUpdateService::new(runner, environment, text(home)).with_updater(CliUpdater::Winget)
}

/// A fixture CLI: `--version` reads the current version, anything else is the update, which records its arguments and moves the
/// version to 2.0.0 (Swift's `versionScript` + `successfulUpdate`).
fn versioned(update_exit: i32) -> (Arc<FakeRunner>, Arc<Mutex<String>>) {
    let version = Arc::new(Mutex::new("1.0.0".to_string()));
    let current = version.clone();
    let runner = FakeRunner::new(move |_, args| -> Result<CliRunResult, RunError> {
        if args.iter().any(|a| a == "--version") {
            return Ok(CliRunResult::new(0, format!("{}\n", current.lock().unwrap()), "", false));
        }
        if update_exit != 0 {
            return Ok(CliRunResult::new(update_exit, "", "network unavailable", false));
        }
        *current.lock().unwrap() = "2.0.0".into();
        ok("")
    });
    (runner, version)
}

fn updates(runner: &FakeRunner) -> Vec<Vec<String>> {
    runner.args().into_iter().filter(|a| !a.iter().any(|x| x == "--version")).collect()
}

fn package_json(root: &Path, name: &str, binary: &str, entry: &str, version: &str) {
    std::fs::create_dir_all(root).unwrap();
    let json = serde_json::json!({ "name": name, "version": version, "bin": { binary: entry } });
    std::fs::write(root.join("package.json"), json.to_string()).unwrap();
}

/// `CLIUpdateTests.swift:88 "homebrewUsesActualCaskOrFormulaAndOnlyNamedPackage"`: a CLI under Homebrew's Caskroom or Cellar is
/// upgraded by that Homebrew (`<prefix>/bin/brew`), as a cask or a formula, the one package only, with the switches that keep it
/// from cleaning up or upgrading dependents; through the app's runner the process host gets exactly that request.
#[tokio::test]
async fn homebrew_uses_actual_cask_or_formula_and_only_named_package() {
    korean();
    for (container, provider, package) in [("Caskroom", "codex", "codex"), ("Cellar", "claude", "claude-code")] {
        let root = temp("brew");
        let prefix = dir(&root, &["brew installation"]);
        let bin = dir(&prefix, &[container, package, "1.0.0", "bin"]);
        exe(&bin, provider);
        let brew = exe(&dir(&prefix, &["bin"]), "brew");
        let host = Arc::new(FakeProcessHost::default());
        for answer in ["1.0.0\n", "", "2.0.0\n"] {
            host.push(FakeScript { stdout: answer.as_bytes().to_vec(), ..FakeScript::default() });
        }
        let service = mac(Arc::new(HostCliRunner::new(host.clone())), env(&[&bin]), &root);
        let result = service.update(provider, &none()).await;
        let cask = container == "Caskroom";
        assert!(result.status == "updated" && result.method == if cask { "brew-cask" } else { "brew-formula" }, "{container}: {result:?}");
        assert_eq!((result.before_version.as_deref(), result.after_version.as_deref()), (Some("1.0.0"), Some("2.0.0")));
        let spawned = host.spawned();
        let upgrade = &spawned[1];
        assert_eq!(upgrade.program, brew, "the Homebrew that installed it");
        assert_eq!(upgrade.args, ["upgrade", if cask { "--cask" } else { "--formula" }, package], "the one package, as installed");
        for name in BREW_SWITCHES {
            assert_eq!(upgrade.env.get(name).map(String::as_str), Some("1"), "{name}");
        }
        assert!(!spawned[0].env.contains_key("HOMEBREW_NO_ASK"), "only the upgrade gets the switches");

        // Windows' planner never runs a Homebrew.
        let (runner, _) = versioned(0);
        let other = windows(runner.clone(), env(&[&bin]), &root).update(provider, &none()).await;
        assert!(other.status == "skipped" && other.method == "unknown" && updates(&runner).is_empty(), "{other:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The Homebrew a Cellar install names but that is not there: recognised, never run.
#[tokio::test]
async fn a_missing_homebrew_is_named_and_nothing_runs() {
    korean();
    let root = temp("brew-missing");
    let bin = dir(&root, &["opt", "Cellar", "codex", "1.0.0", "bin"]);
    exe(&bin, "codex");
    let (runner, _) = versioned(0);
    let result = mac(runner.clone(), env(&[&bin]), &root).update("codex", &none()).await;
    assert!(result.status == "skipped" && result.method == "brew-formula", "{result:?}");
    assert_eq!(result.detail, ko("settings.cliUpdate.detailBrewRuntimeMissing"));
    assert!(updates(&runner).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:73 "nativeClaudeUsesLauncherAndReportsFreshVersion"`: Claude in `~/.local/share/claude/versions` updates
/// itself; a second update reports it current.
#[tokio::test]
async fn native_claude_uses_launcher_and_reports_fresh_version() {
    korean();
    let root = temp("mac-native");
    let versions = dir(&root, &[".local", "share", "claude", "versions"]);
    let claude = exe(&versions, "claude");
    let (runner, _) = versioned(0);
    let service = mac(runner.clone(), env(&[&versions]), &root);
    let inspected = service.inspect("claude", &none()).await;
    assert!(inspected.method == "native" && inspected.can_update, "{inspected:?}");
    assert_eq!(inspected.executable_path, Some(text(&claude)));
    let result = service.update("claude", &none()).await;
    assert!(result.status == "updated" && result.after_version.as_deref() == Some("2.0.0"), "{result:?}");
    assert_eq!(updates(&runner), vec![strings(&["update"])]);
    let same = service.update("claude", &none()).await;
    assert!(same.status == "current" && same.before_version == same.after_version, "{same:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:107 "npmTargetsOfficialPackageAndOriginalPrefixWithPairedNode"`: an npm install is updated by the Node
/// beside npm's own launcher, with npm's cli script, into the prefix it is installed in (one argument, whatever it holds), that Node
/// first on PATH.
#[tokio::test]
async fn npm_targets_official_package_and_original_prefix_with_paired_node() {
    korean();
    for (provider, name) in [("claude", "@anthropic-ai/claude-code"), ("codex", "@openai/codex")] {
        let root = temp("mac-npm");
        let prefix = dir(&root, &["npm prefix $(touch SENTINEL)"]);
        let mut below = vec!["lib", "node_modules"];
        below.extend(name.split('/'));
        let package = dir(&prefix, &below);
        package_json(&package, name, provider, &format!("bin/{provider}"), "1.0.0");
        let bin = dir(&package, &["bin"]);
        exe(&bin, provider);
        let npm_package = dir(&root, &["node installation", "lib", "node_modules", "npm"]);
        package_json(&npm_package, "npm", "npm", "bin/npm", "10.0.0");
        let npm_bin = dir(&npm_package, &["bin"]);
        let script = exe(&npm_bin, "npm");
        let node = exe(&npm_bin, "node");
        let (runner, _) = versioned(0);
        let service = mac(runner.clone(), env(&[&bin, &npm_bin]), &root);
        let inspected = service.inspect(provider, &none()).await;
        assert!(inspected.method == "npm" && inspected.can_update, "{provider}: {inspected:?}");
        let result = service.update(provider, &none()).await;
        assert_eq!(result.status, "updated", "{result:?}");
        let call = runner.calls().into_iter().find(|c| !c.arguments.iter().any(|a| a == "--version")).unwrap();
        assert_eq!(call.executable, text(&node), "the Node beside npm");
        assert_eq!(
            call.arguments,
            strings(&[
                &text(&script),
                "install",
                "--global",
                "--prefix",
                &text(&prefix),
                &format!("{name}@latest"),
                "--no-audit",
                "--no-fund"
            ])
        );
        let path = call.environment.as_ref().and_then(|e| e.get("PATH")).cloned().unwrap_or_default();
        assert!(path.starts_with(&text(&npm_bin)), "the Node's folder first on PATH: {path}");
        assert!(!root.join("SENTINEL").exists());

        // Windows' planner reads npm's Windows layout (the shim in the prefix): this one is not it.
        let (runner, _) = versioned(0);
        let other = windows(runner.clone(), env(&[&bin, &npm_bin]), &root).update(provider, &none()).await;
        assert!(other.method != "npm" && updates(&runner).is_empty(), "{other:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}

/// `CLIUpdateTests.swift:139 "npmRejectsWrongManifestAndPreservesPrereleaseChannels"`: a prerelease channel or a version that is
/// not one is left alone; a package.json naming another package, or a bin entry outside the package, is no npm install.
#[tokio::test]
async fn npm_rejects_wrong_manifest_and_preserves_prerelease_channels() {
    korean();
    let fixture = |version: &str| {
        let root = temp("mac-npm-channel");
        let package = dir(&root, &["prefix", "lib", "node_modules", "@openai", "codex"]);
        package_json(&package, "@openai/codex", "codex", "bin/codex", version);
        let bin = dir(&package, &["bin"]);
        exe(&bin, "codex");
        (root, package, bin)
    };
    for version in ["0.44.0-preview.1", "0.44.0-nightly.20260916", "not-semver"] {
        let (root, _, bin) = fixture(version);
        let (runner, _) = versioned(0);
        let result = mac(runner.clone(), env(&[&bin]), &root).update("codex", &none()).await;
        assert!(result.status == "skipped" && result.method == "npm", "{version}: {result:?}");
        assert!(updates(&runner).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
    let (root, package, bin) = fixture("1.0.0");
    package_json(&package, "unrelated-package", "codex", "bin/codex", "1.0.0");
    let (runner, _) = versioned(0);
    assert_eq!(mac(runner.clone(), env(&[&bin]), &root).update("codex", &none()).await.status, "skipped");
    package_json(&package, "@openai/codex", "codex", "../outside.js", "1.0.0");
    assert_eq!(mac(runner.clone(), env(&[&bin]), &root).update("codex", &none()).await.status, "skipped");
    assert!(updates(&runner).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:123 "missingUnknownAndUnhealthyInstallationsAreNeverInstalled"`.
#[tokio::test]
async fn missing_unknown_and_unhealthy_installations_are_never_installed() {
    korean();
    let root = temp("mac-never");
    let bin = dir(&root, &["bin"]);
    let (runner, _) = versioned(0);
    let service = mac(runner.clone(), env(&[&bin]), &root);
    let missing = service.update("claude", &none()).await;
    assert!(missing.status == "skipped" && missing.method == "missing", "{missing:?}");
    exe(&bin, "claude");
    let manual = service.update("claude", &none()).await;
    assert!(manual.status == "skipped" && manual.method == "unknown", "{manual:?}");
    assert_eq!(service.update("other", &none()).await.status, "skipped");
    assert!(updates(&runner).is_empty());
    let unhealthy = FakeRunner::new(|_, _| Ok(CliRunResult::new(7, "", "", false)));
    exe(&bin, "codex");
    assert_eq!(mac(unhealthy.clone(), env(&[&bin]), &root).update("codex", &none()).await.status, "skipped");
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:159 "failureDoesNotBlockSubsequentProviderUpdate"`: a failed update says its exit code and keeps the
/// output for the details; Antigravity in `~/.local/bin` updates itself afterwards.
#[tokio::test]
async fn failure_does_not_block_subsequent_provider_update() {
    korean();
    let root = temp("mac-failure");
    let versions = dir(&root, &[".local", "share", "claude", "versions"]);
    exe(&versions, "claude");
    let local = dir(&root, &[".local", "bin"]);
    exe(&local, "agy");
    let (failing, _) = versioned(23);
    let failed = mac(failing, env(&[&versions]), &root).update("claude", &none()).await;
    assert!(failed.status == "failed" && failed.before_version.as_deref() == Some("1.0.0") && failed.after_version.is_none());
    assert!(failed.detail.contains("23") && failed.output.contains("network unavailable"), "{failed:?}");
    let (runner, _) = versioned(0);
    let antigravity = mac(runner.clone(), env(&[&local]), &root).update("gemini", &none()).await;
    assert!(antigravity.status == "updated" && antigravity.method == "native", "{antigravity:?}");
    assert_eq!(updates(&runner), vec![strings(&["update"])]);
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:185 "antigravityElsewhereIsNeverUpdated"`: an agy outside `~/.local/bin` is left alone, and the detail
/// names Antigravity's own install command.
#[tokio::test]
async fn antigravity_elsewhere_is_never_updated() {
    korean();
    let root = temp("mac-agy");
    let bin = dir(&root, &["bin"]);
    exe(&bin, "agy");
    let (runner, _) = versioned(0);
    let service = mac(runner.clone(), env(&[&bin]), &root);
    let inspected = service.inspect("gemini", &none()).await;
    assert!(!inspected.can_update && inspected.version.as_deref() == Some("1.0.0") && inspected.detail.contains("install.sh"));
    assert_eq!(service.update("gemini", &none()).await.status, "skipped");
    assert!(updates(&runner).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// `CLIUpdateTests.swift:173 "piUpdatesItselfWithUpdateSelfWhereverItIsInstalled"`.
#[tokio::test]
async fn pi_updates_itself_with_update_self_wherever_it_is_installed() {
    korean();
    let root = temp("mac-pi");
    let bin = dir(&root, &["bin"]);
    exe(&bin, "pi");
    let (runner, _) = versioned(0);
    let service = mac(runner.clone(), env(&[&bin]), &root);
    let inspected = service.inspect("pi", &none()).await;
    assert!(inspected.can_update && inspected.method == "pi" && inspected.version.as_deref() == Some("1.0.0"));
    let updated = service.update("pi", &none()).await;
    assert!(updated.status == "updated" && updated.after_version.as_deref() == Some("2.0.0"));
    assert_eq!(updates(&runner), vec![strings(&["update", "self"])]);
    let _ = std::fs::remove_dir_all(root);
}

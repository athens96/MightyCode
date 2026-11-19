//! 필요한 도구 on the Mac (Swift `SetupPlanTests.swift`, `SetupProbeTests.swift`, AppStore+Setup.swift): the screen of the catalog's
//! `macos` entries, their install order, the bash script built from them and run in a terminal pane, and the Mac's probes (the
//! command line tools, an app bundle). Nothing is installed and no script runs: the screen goes through the `SetupFiles` fake and a
//! recording launcher, the probes through a finder and command readers the checks give; the script is only parsed (`bash -n`).

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_platform::fake::RecordingTerminalLauncher;
use mighty_platform::setup::FakeSetupFiles;
use mighty_run::strings;
use mighty_tools::setup::catalog::{SetupCatalog, SetupGroup, SetupPlatform, SetupProbe};
use mighty_tools::setup::mac::{self, MacScriptLabels};
use mighty_tools::setup::plan;
use mighty_tools::setup::probe::{
    self, CommandReader, Finder, SetupFirstRun, SetupProbeResult, SetupProbeState, SetupProber, SetupRowState, SetupRows, VersionReader,
};
use mighty_tools::setup::screen::{Gate, InstallStart, SetupScreen, WatchTick};

/// Every item the Mac lists, in the catalog's order: the agent CLIs, the command line tools, Homebrew, Node, Bun, uv, the Claude
/// plugins, gstack, Tailscale, gh and glab.
const MAC_ITEMS: [&str; 17] = [
    "claude",
    "codex",
    "agy",
    "pi",
    "opencode",
    "hermes",
    "git",
    "homebrew",
    "node",
    "bun",
    "uv",
    "ouroboros",
    "omc",
    "gstack",
    "tailscale",
    "gh",
    "glab",
];

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn ids(items: &[String]) -> Vec<&str> {
    items.iter().map(String::as_str).collect()
}

fn catalog() -> Arc<SetupCatalog> {
    SetupCatalog::bundled().expect("the bundled catalog decodes")
}

fn labels() -> MacScriptLabels {
    MacScriptLabels {
        step: Box::new(|index, count, title| format!("[{index}/{count}] {title}")),
        failed: "failed's step".into(),
        waiting: "waiting".into(),
        done: "done".into(),
    }
}

/// The Mac screen with every Mac item missing.
fn mac_screen() -> SetupScreen {
    let mut screen = SetupScreen::for_platform(SetupPlatform::MacOS);
    let catalog = screen.load_catalog().unwrap();
    let missing: HashMap<String, SetupProbeResult> = catalog
        .items
        .iter()
        .filter(|item| item.entry(SetupPlatform::MacOS).is_some())
        .map(|item| (item.id.clone(), SetupProbeResult::missing()))
        .collect();
    screen.finish_refresh(missing, None, true, Instant::now());
    screen
}

/// The platform comes from the OS the app runs on; the Mac screen lists the catalog's Mac entries (Homebrew, Bun and gstack, which
/// Windows has none of), each titled and described in the Mac's words, which every language has.
#[test]
fn the_mac_screen_lists_the_mac_items() {
    common::korean();
    assert_eq!(SetupPlatform::of(mighty_platform::Os::Macos), SetupPlatform::MacOS);
    assert_eq!(SetupPlatform::of(mighty_platform::Os::Windows), SetupPlatform::Windows);
    assert_eq!(SetupScreen::new().platform(), SetupPlatform::Windows, "the default screen stays Windows'");
    let screen = mac_screen();
    assert_eq!(screen.platform(), SetupPlatform::MacOS);
    let listed: Vec<&str> = screen.items().iter().map(|item| item.id.as_str()).collect();
    assert_eq!(listed, MAC_ITEMS);
    let mut windows = SetupScreen::new();
    windows.load_catalog();
    let on_windows: Vec<&str> = windows.items().iter().map(|item| item.id.as_str()).collect();
    for id in ["homebrew", "bun", "gstack"] {
        assert!(listed.contains(&id) && !on_windows.contains(&id), "{id} is the Mac's alone");
    }
    let catalog = catalog();
    let git = catalog.item("git").unwrap();
    assert_eq!(SetupCatalog::title_key_on(git, SetupPlatform::MacOS), "setup.item.git.title");
    assert_eq!(SetupCatalog::title_key_on(git, SetupPlatform::Windows), "setup.windows.item.git.title");
    assert_eq!(SetupCatalog::detail_key_on(git, SetupPlatform::MacOS), "setup.item.git.detail");
    for language in ["ko", "en", "ja", "zh"] {
        let text = std::fs::read_to_string(format!("{}/../../../../locales/{language}.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let strings: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
        for item in catalog.items.iter().filter(|item| item.entry(SetupPlatform::MacOS).is_some()) {
            for key in [SetupCatalog::title_key_on(item, SetupPlatform::MacOS), SetupCatalog::detail_key_on(item, SetupPlatform::MacOS)] {
                assert!(strings.contains_key(key), "{language} misses {key}");
            }
        }
        for key in [
            "setup.subtitle",
            "setup.confirm.body",
            "setup.terminal.title",
            "setup.script.waiting",
            "setup.attention.gitUnusable",
            "setup.attention.xcodeLicense",
            "setup.error.terminal",
        ] {
            assert!(strings.contains_key(key), "{language} misses {key}");
        }
    }
    // A page is no install; the command line tools are, and the first look ticks them and Claude Code.
    let tailscale = catalog.item("tailscale").unwrap();
    assert!(!SetupRows::is_selectable_on(SetupRowState::Missing, tailscale, SetupPlatform::MacOS));
    assert!(SetupRows::is_selectable_on(SetupRowState::Missing, catalog.item("homebrew").unwrap(), SetupPlatform::MacOS));
    assert!(!SetupRows::is_selectable(SetupRowState::Missing, catalog.item("homebrew").unwrap()), "Windows has no Homebrew row");
    let mut screen = screen;
    screen.preselect_once();
    assert_eq!(screen.selection, set(&["git", "claude"]));
    assert!(screen.needs_attention() && screen.should_present(false) && !screen.should_present(true));
    assert!(SetupFirstRun::should_present_on(&catalog, &screen.results, false, SetupPlatform::MacOS));
}

/// The plan over the Mac's prerequisites: gh needs Homebrew, which needs the command line tools; gstack needs them, Bun and Claude
/// Code; what is there already is not added; the catalog's order breaks ties.
#[test]
fn the_mac_plan_puts_prerequisites_first() {
    let catalog = catalog();
    let order = |picked: &[&str], there: &[&str]| {
        let there = set(there);
        plan::order(&set(picked), &catalog, SetupPlatform::MacOS, &|id| there.contains(id))
    };
    assert_eq!(ids(&order(&["gh"], &[])), ["git", "homebrew", "gh"]);
    assert_eq!(ids(&order(&["gh", "gstack"], &[])), ["claude", "git", "homebrew", "bun", "gstack", "gh"]);
    assert_eq!(ids(&order(&["gh", "gstack"], &["git", "claude"])), ["homebrew", "bun", "gstack", "gh"]);
    assert_eq!(ids(&order(&["ouroboros"], &[])), ["claude", "uv", "ouroboros"]);
    assert_eq!(ids(&order(&["pi"], &[])), ["git", "homebrew", "node", "pi"]);
    // The screen's plan is the same, and says which rows were added for the selection.
    let mut screen = mac_screen();
    screen.toggle("gh");
    assert_eq!(ids(&screen.plan_ids()), ["git", "homebrew", "gh"]);
    assert_eq!(screen.added(), set(&["git", "homebrew"]));
    screen.finish_refresh([("git".to_string(), SetupProbeResult::installed(None))], None, true, Instant::now());
    assert_eq!(ids(&screen.plan_ids()), ["homebrew", "gh"]);
}

/// Swift `SetupPlanTests`: the confirmation shows the bash preamble and each Mac command exactly as the script runs it, every value in
/// POSIX single quotes; nothing of the Windows script (PowerShell, winget) is in it; the rendered script makes its markers, waits for
/// the command line tools' window, keeps going after a failed step and parses as bash.
#[test]
fn the_mac_script_is_built_from_the_mac_entries_only() {
    common::korean();
    assert_eq!(mac::quote("it's"), r"'it'\''s'");
    assert_eq!(mac::quote("$HOME `x`"), "'$HOME `x`'");
    let mut screen = mac_screen();
    for id in ["git", "homebrew", "gh", "gstack", "opencode", "tailscale", "codex"] {
        screen.toggle(id);
    }
    let home = "/Users/it's me";
    let environment: BTreeMap<String, String> = [("HOME".to_string(), home.to_string())].into();
    let confirmation = screen.confirmation(&environment).unwrap();
    assert_eq!(confirmation.platform, SetupPlatform::MacOS);
    assert_eq!(
        confirmation.preamble,
        [
            "cd \"$HOME\" || exit 1".to_string(),
            r#"export PATH="$PATH":'/Users/it'\''s me/.local/bin':'/Users/it'\''s me/.npm-global/bin':'/Users/it'\''s me/.bun/bin':'/opt/homebrew/bin':'/usr/local/bin'"#
                .to_string(),
            "export npm_config_registry='https://registry.npmjs.org/'".to_string(),
        ]
    );
    let steps = &confirmation.steps;
    let order: Vec<&str> = steps.iter().map(|step| step.item_id.as_str()).collect();
    // Node comes with OpenCode; Tailscale opens a page and is no step.
    assert_eq!(order, ["claude", "codex", "git", "homebrew", "node", "opencode", "bun", "gstack", "gh"]);
    let body = |id: &str| steps.iter().find(|step| step.item_id == id).unwrap().body.clone();
    let git = steps.iter().find(|step| step.item_id == "git").unwrap();
    assert!(git.waits_outside && steps.iter().filter(|step| step.waits_outside).count() == 1);
    assert!(
        body("git").starts_with("'/usr/bin/xcode-select' '--install'; trap 'break' INT; mighty_waited=0; until /usr/bin/xcode-select -p")
    );
    assert_eq!(
        body("homebrew"),
        "mighty_script=\"$(/usr/bin/curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL 'https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh')\" && /bin/bash -c \"$mighty_script\""
    );
    assert!(body("codex").ends_with("'https://chatgpt.com/codex/install.sh')\" && /bin/sh -c \"$mighty_script\""));
    assert_eq!(body("gh"), "brew install 'gh'");
    assert_eq!(body("node"), "brew install 'node'");
    assert_eq!(body("opencode"), "npm uninstall -g 'opencode-ai' >/dev/null 2>&1; npm install -g '@opencode/cli'");
    let gstack = body("gstack");
    assert!(
        gstack.contains("'https://github.com/garrytan/gstack.git' 'd258bcffb0cb7bda5f133f3e957f932d816d282f'")
            && gstack.contains("\"$HOME\"/'.claude/skills/gstack'/'.mighty-setup'")
            && gstack.contains("(cd \"$HOME\"/'.claude/skills/gstack' && './setup')")
            && gstack.ends_with("&& : > \"$HOME\"/'.claude/skills/gstack'/'.mighty-setup-ok'"),
        "{gstack}"
    );
    for step in steps {
        for windows in ["Invoke-MightyScript", "winget", "$env:", "Write-Host", "powershell"] {
            assert!(!step.body.contains(windows), "{} has {windows}: {}", step.item_id, step.body);
        }
    }
    let started = "/tmp/setup it's/started-1";
    let done = "/tmp/setup it's/done-1";
    let script = mac::render(steps, &labels(), home, started, done);
    assert!(!script.contains('\r') && script.ends_with("exit $mighty_failed\n"));
    let lines: Vec<&str> = script.lines().collect();
    assert_eq!(lines[0], "#!/bin/bash");
    assert_eq!(lines[1], r"mkdir -- '/tmp/setup it'\''s/started-1' 2>/dev/null");
    assert!(lines.contains(&r"mighty_marker='/tmp/setup it'\''s/done-1'"));
    assert!(lines.contains(&"trap 'mkdir -- \"$mighty_marker\" 2>/dev/null' EXIT"));
    assert!(lines.contains(&r"printf '\n\033[1m%s\033[0m\n' '[3/9] Git (Xcode Command Line Tools)'"), "{script}");
    let waiting = lines.iter().position(|line| *line == "printf '%s\\n' 'waiting'").expect("the wait is said");
    assert_eq!(lines[waiting + 1], "trap ':' INT");
    assert!(lines[waiting + 2].starts_with("if ( '/usr/bin/xcode-select' '--install';"));
    assert_eq!(lines[waiting + 3], "trap - INT");
    assert!(lines.contains(&r"if ( brew install 'gh' ); then :; else mighty_failed=1; printf '%s\n' 'failed'\''s step'; fi"));
    assert!(script.find("export npm_config_registry").unwrap() < script.find("[1/9]").unwrap(), "the preamble runs first");
    let pane = mac::launch_arguments("/a b/install-1.command");
    assert_eq!(pane, ["/bin/bash", "/a b/install-1.command"]);
    // Without a local workspace the same script goes to Terminal.app alone, its `#!/bin/bash` line running it there.
    assert_eq!(mac::terminal_app_arguments(&pane), Some(vec!["/a b/install-1.command".to_string()]));
    assert_eq!(mac::terminal_app_arguments(&["/bin/sh".to_string(), "/a".to_string()]), None);
    assert_eq!(mac::terminal_app_arguments(&["claude".to_string()]), None);
}

/// The rendered script, every Mac step in it, parses as bash (`bash -n` through the Mac's process host: read, never run).
#[cfg(target_os = "macos")]
#[tokio::test]
async fn the_mac_script_parses_as_bash() {
    common::korean();
    let mut screen = mac_screen();
    for id in MAC_ITEMS {
        screen.toggle(id);
    }
    let environment: BTreeMap<String, String> = [("HOME".to_string(), "/Users/it's me".to_string())].into();
    let steps = screen.confirmation(&environment).unwrap().steps;
    assert_eq!(steps.len(), MAC_ITEMS.len() - 1, "every Mac item but the page");
    let folder = common::temp("mac-script");
    let script = mac::render(&steps, &labels(), "/Users/it's me", "/tmp/a'b/started", "/tmp/a'b/done");
    let file = mighty_tools::setup::files::write_shell_script(&script, &folder).unwrap();
    assert!(file.extension().is_some_and(|e| e == "command") && std::fs::read(&file).unwrap().starts_with(b"#!/bin/bash"));
    let host = common::processes();
    let arguments = ["-n".to_string(), file.to_string_lossy().into_owned()];
    let request = host.cli_request(std::path::Path::new(mac::BASH), &arguments, &folder, &BTreeMap::new()).unwrap();
    let parsed = mighty_tools::process::run(host.as_ref(), request, None, Duration::from_secs(10), 1 << 16).await.unwrap();
    assert!(parsed.exit_code == 0 && !parsed.timed_out, "bash -n: {}", parsed.error_output);
    common::remove(&folder);
}

/// An install on the Mac over the fakes: the bash script written in the setup folder and handed to the terminal pane (bash on its
/// path, the setup terminal's title); an app running as root is not refused (Swift's is not); the pane's own refusal is the screen's
/// message as it is; the end marker, which the script's EXIT trap makes, ends the run.
#[test]
fn the_mac_install_runs_the_script_in_a_terminal_pane() {
    common::korean();
    let mut screen = mac_screen();
    screen.toggle("gh");
    let environment: BTreeMap<String, String> = [("HOME".to_string(), "/Users/me".to_string())].into();
    let steps = screen.confirmation(&environment).unwrap().steps;
    let files = FakeSetupFiles::default();
    files.set_elevated(true);
    let local = common::temp("mac-install");
    let folder = local.join("setup");
    let now = Instant::now();
    assert!(screen.may_confirm(&files, &environment).is_some(), "the Mac does not refuse as Windows does when elevated");
    let refusing = RecordingTerminalLauncher::default();
    refusing.refuse();
    assert_eq!(screen.start_install(&steps, Gate::Allowed, &environment, &files, &refusing, &folder, now), InstallStart::Refused);
    assert_eq!(screen.message.as_deref(), Some("the terminal did not start"), "the pane's refusal is said as it is");
    let pane = RecordingTerminalLauncher::default();
    assert_eq!(screen.start_install(&steps, Gate::ReadOnly, &environment, &files, &pane, &folder, now), InstallStart::NotAllowed);
    assert_eq!(screen.start_install(&steps, Gate::Allowed, &environment, &files, &pane, &folder, now), InstallStart::Started);
    let opened = pane.opened();
    assert_eq!(opened.len(), 1);
    let launch = &opened[0];
    assert_eq!(launch.argv.len(), 2);
    assert_eq!(launch.argv[0], "/bin/bash");
    assert_eq!(launch.title.as_deref(), Some(strings::get("setup.terminal.title").as_str()));
    assert!(!launch.prefer_terminal_app && launch.directory.as_ref() == Some(&folder));
    let script_path = std::path::PathBuf::from(&launch.argv[1]);
    assert!(script_path.starts_with(&folder) && script_path.extension().is_some_and(|e| e == "command"));
    let script = std::fs::read_to_string(&script_path).unwrap();
    assert!(script.starts_with("#!/bin/bash\n") && script.contains("brew install 'gh'") && !script.contains("powershell"));
    assert!(screen.running() && screen.message.is_none() && screen.terminals_opened == 1);
    assert_eq!(screen.state_of(screen.catalog().unwrap().item("gh").unwrap()), SetupRowState::Installing);
    let run = screen.run.clone().unwrap();
    std::fs::create_dir(&run.started).unwrap();
    assert_eq!(screen.watch_tick(now + Duration::from_secs(2), &files), WatchTick::Running { probe: None, redraw: false });
    std::fs::create_dir(&run.done).unwrap();
    assert_eq!(screen.watch_tick(now + Duration::from_secs(4), &files), WatchTick::Ended);
    assert!(screen.finish_run_of(run.opened) && !screen.running());
    common::remove(&local);
}

/// Swift `SetupProbeTests`: the command line tools count once `xcode-select -p` names a folder and the system's git runs; an
/// unaccepted Xcode license and a git that does not answer need attention, with the reason the row says; an app bundle counts by its
/// command on the PATH; every Mac item is probed, Homebrew among them.
#[test]
fn the_mac_probes_the_command_line_tools_and_app_bundles() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let folder = common::temp("mac-probe");
        let selected = folder.to_string_lossy().into_owned();
        let finder: Finder =
            Arc::new(
                |name: &str, _: &BTreeMap<String, String>| {
                    if name == "tailscale" || name == "brew" { vec![format!("/opt/homebrew/bin/{name}")] } else { vec![] }
                },
            );
        let versions: VersionReader = Arc::new(|_| Box::pin(async { Some("1.0.0".to_string()) }));
        let commands = |xcode: Option<(i32, String)>, git: Option<(i32, String)>| -> CommandReader {
            Arc::new(move |program: String, arguments: Vec<String>| {
                let answer = match (program.as_str(), arguments.first().map(String::as_str)) {
                    (probe::XCODE_SELECT, Some("-p")) => xcode.clone(),
                    (probe::SYSTEM_GIT, Some("--version")) => git.clone(),
                    _ => None,
                };
                Box::pin(async move { answer })
            })
        };
        let prober = |reader: Option<CommandReader>| {
            let prober = SetupProber::new(BTreeMap::new(), finder.clone(), versions.clone()).on(SetupPlatform::MacOS);
            match reader {
                Some(reader) => prober.with_commands(reader),
                None => prober,
            }
        };
        let tools = SetupProbe::CommandLineTools;
        let ok = commands(Some((0, format!("{selected}\n"))), Some((0, "git version 2.50.1".into())));
        assert_eq!(prober(Some(ok.clone())).probe(&tools).await, SetupProbeResult::installed(None));
        let license = commands(Some((0, selected.clone())), Some((69, "You have not agreed to the Xcode license agreements.".into())));
        let found = prober(Some(license)).probe(&tools).await;
        assert!(found.state == SetupProbeState::NeedsAttention && found.attention == Some(probe::ATTENTION_XCODE_LICENSE));
        let silent = prober(Some(commands(Some((0, selected.clone())), None))).probe(&tools).await;
        assert_eq!(silent.attention, Some(probe::ATTENTION_GIT_UNUSABLE));
        let failing = prober(Some(commands(Some((0, selected.clone())), Some((1, "error".into()))))).probe(&tools).await;
        assert_eq!(failing.attention, Some(probe::ATTENTION_GIT_UNUSABLE));
        assert_eq!(prober(Some(commands(Some((2, String::new())), None))).probe(&tools).await.state, SetupProbeState::Missing);
        let elsewhere = commands(Some((0, folder.join("gone").to_string_lossy().into_owned())), Some((0, String::new())));
        assert_eq!(prober(Some(elsewhere)).probe(&tools).await.state, SetupProbeState::Missing, "the selected folder must be there");
        assert_eq!(prober(None).probe(&tools).await.state, SetupProbeState::Missing);
        let bundle = |executable: Option<&str>| SetupProbe::AppBundle {
            name: "Mighty Code Probe Fixture.app".into(),
            executable_name: executable.map(str::to_string),
        };
        assert_eq!(prober(None).probe(&bundle(Some("tailscale"))).await.state, SetupProbeState::Installed);
        assert_eq!(prober(None).probe(&bundle(Some("nothing"))).await.state, SetupProbeState::Missing);
        assert_eq!(prober(None).probe(&bundle(None)).await.state, SetupProbeState::Missing);
        let all = prober(Some(ok)).probe_all(&catalog(), None).await;
        assert_eq!(all.keys().map(String::as_str).collect::<Vec<_>>(), MAC_ITEMS);
        assert!(all["homebrew"].is_installed() && all["git"].is_installed() && all["gh"].state == SetupProbeState::Missing);
        let windows = SetupProber::new(BTreeMap::new(), finder.clone(), versions.clone()).probe_all(&catalog(), None).await;
        assert!(!windows.contains_key("homebrew"), "the default prober stays Windows'");
        common::remove(&folder);
    });
}

/// The groups the Mac screen shows, in order, each with at least one row.
#[test]
fn the_mac_screen_shows_every_group() {
    let screen = mac_screen();
    let items = screen.items();
    for group in SetupGroup::ALL {
        assert!(items.iter().any(|item| item.group == group), "{group:?} has a row on the Mac");
    }
}

/// The Mac's terminal opens after the screen handed its launch over; when neither the pane nor Terminal.app opens, the run is taken
/// back and the refusal's words are the screen's message; a stale run is not touched.
#[test]
fn a_terminal_that_does_not_open_takes_the_run_back() {
    common::korean();
    let mut screen = mac_screen();
    screen.toggle("bun");
    let environment: BTreeMap<String, String> = [("HOME".to_string(), "/Users/me".to_string())].into();
    let steps = screen.confirmation(&environment).unwrap().steps;
    let (files, handed) = (FakeSetupFiles::default(), RecordingTerminalLauncher::default());
    let local = common::temp("mac-pane-refused");
    let now = Instant::now();
    assert_eq!(screen.start_install(&steps, Gate::Allowed, &environment, &files, &handed, &local, now), InstallStart::Started);
    let words = "Terminal.app is not on this Mac".to_string();
    let refusal = || mighty_platform::PlatformError::Refused(words.clone());
    assert!(!screen.launch_failed(now + Duration::from_secs(1), refusal()), "another run is not taken back");
    assert!(screen.running());
    assert!(screen.launch_failed(now, refusal()));
    assert!(!screen.running() && screen.run.is_none() && screen.terminals_opened == 0);
    assert_eq!(screen.message.as_deref(), Some(words.as_str()));
    assert_eq!(screen.state_of(screen.catalog().unwrap().item("bun").unwrap()), SetupRowState::Missing, "nothing ran");
    common::remove(&local);
}

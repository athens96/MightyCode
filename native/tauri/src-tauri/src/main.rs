//! Mighty Code's Rust + Tauri 2 host: one window drawing the page of native/windows/web, answering it through `host.rs`. What it
//! may write follows the write policy (startup.rs): a release build installed in its folder writes, any other run only with
//! `--allow-state-write`, never with `--read-only`.
//!
//! Identity: tauri.conf.json's `identifier` is `dev.mightycode.native` (W9-F), the toast registration's AppUserModelId and the root of
//! this machine's local folders (`%LOCALAPPDATA%\dev.mightycode.native\…`); a release build on the default profile takes C#'s own
//! WebView2 folders (`paths::web_folders`).
//!
//! W9 Step 0 fixed the order `main` runs in (plan §1.1): the exe's other modes (the Agent IO and delegation MCP servers, the fake Claude,
//! the help capture's demo CLI, then the update helper), the environment taken back after a bare start, the arguments, the write
//! policy (startup.rs), the platform, the pre-UI migration, then the store and Tauri.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// A guard kept through a match or if-let body has called out (host.view(), a redraw, Tauri's main thread) while held: bind first.
#![deny(clippy::significant_drop_in_scrutinee)]

mod accounts;
mod agent_io;
mod antigravity;
mod app_update;
mod args;
mod ask;
mod bodies;
mod build_info;
mod cards;
mod companion;
mod composer;
mod conversation_search;
mod deep_link;
mod delegation;
mod delta;
mod dialogs;
mod direct;
mod editor;
mod files;
mod frame;
#[cfg(feature = "dev-harness")]
mod help_capture;
mod history;
mod host;
mod logfile;
mod menu;
mod methods;
mod mighty;
mod mirror;
mod mobile;
mod models;
mod native;
mod notifications;
mod ouroboros;
mod pane_worktree;
mod panes;
#[cfg(feature = "dev-harness")]
mod perf;
mod phone;
mod platform;
mod plugins;
mod remote;
mod review;
mod runs;
mod settings;
mod settings_place;
mod setup;
mod shell_command;
mod sign_in_pane;
#[cfg(feature = "dev-harness")]
mod smoke;
#[cfg(feature = "dev-harness")]
mod smoke_accounts;
#[cfg(feature = "dev-harness")]
mod smoke_acp;
#[cfg(feature = "dev-harness")]
mod smoke_app_update;
#[cfg(feature = "dev-harness")]
mod smoke_companion;
#[cfg(feature = "dev-harness")]
mod smoke_delegation;
#[cfg(feature = "dev-harness")]
mod smoke_direct;
#[cfg(feature = "dev-harness")]
mod smoke_files;
#[cfg(feature = "dev-harness")]
mod smoke_frame;
#[cfg(feature = "dev-harness")]
mod smoke_help;
#[cfg(feature = "dev-harness")]
mod smoke_history;
#[cfg(feature = "dev-harness")]
mod smoke_phone;
#[cfg(feature = "dev-harness")]
mod smoke_remote;
#[cfg(feature = "dev-harness")]
mod smoke_runs;
#[cfg(feature = "dev-harness")]
mod smoke_settings;
#[cfg(feature = "dev-harness")]
mod smoke_setup;
#[cfg(feature = "dev-harness")]
mod smoke_styles;
#[cfg(feature = "dev-harness")]
mod smoke_terminal;
#[cfg(feature = "dev-harness")]
mod smoke_updates;
mod startup;
mod status_line;
mod styles;
mod surface;
mod surface_terminal;
mod tails;
mod terminal;
mod update_helper;
mod updates;
mod usage;
#[cfg(feature = "dev-harness")]
mod verify_resources;
mod workmap;
mod workspace_create;

use std::path::Path;
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::store::{self, LoadSource};
use serde_json::Value;
#[cfg(feature = "dev-harness")]
use serde_json::json;
use tauri::webview::PageLoadEvent;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

use crate::host::{Host, Smoke, WINDOW};
use crate::logfile::LogFile;
use crate::surface::SurfaceManager;

/// Page → host: one message (`{kind:"call", ...}`), queued for the dispatcher; the answer comes back on `host-message`. Only the
/// page's own webview showing the app's own page is heard (C# `WebShell.Trusted(args.Source)`): any other webview, or the page's
/// webview somewhere else, never reaches the host. And only with this run's nonce, which only the page's top frame on the
/// app's own origin defines ([`HostNonce`]): a frame inside the page (an HTML preview) that reaches Tauri's IPC is refused too.
#[tauri::command]
fn host_post(
    message: Value,
    nonce: Option<String>,
    webview: tauri::Webview,
    inbound: tauri::State<'_, Inbound>,
    origin: tauri::State<'_, AppOrigin>,
    expected: tauri::State<'_, HostNonce>,
) {
    if webview.label() == WINDOW && webview.url().is_ok_and(|url| origin.trusts(&url)) && expected.accepts(nonce.as_deref()) {
        let _ = inbound.0.send(message);
    }
}

/// A secret of this run that only the page's top frame on the app's own origin learns (`window.__MIGHTY_HOST_NONCE__`; the page sends
/// it with every `host_post`). The script itself checks where it runs: on Windows wry injects initialization scripts into every frame
/// whatever Tauri's main-frame-only says (review, mac-int), so an iframe's code, another origin's, never defines it.
struct HostNonce(String);

impl HostNonce {
    fn new() -> HostNonce {
        HostNonce(format!("{}{}", mighty_core::wire::new_id(), mighty_core::wire::new_id()).replace('-', ""))
    }

    /// The top frame's script on the app's origin `origin` (read-only, non-enumerable, configurable so the page's bridge can capture it
    /// in a closure and delete it from `window` at boot); in any other frame or on any other address it defines nothing. Protocol and host
    /// are compared (`tauri://` has no web origin).
    fn script(&self, origin: &tauri::Url) -> String {
        let host = match origin.port() {
            Some(port) => format!("{}:{port}", origin.host_str().unwrap_or("")),
            None => origin.host_str().unwrap_or("").to_string(),
        };
        format!(
            "if (window.top === window && location.protocol === '{}:' && location.host === '{host}') {{ Object.defineProperty(window, '__MIGHTY_HOST_NONCE__', {{ value: '{}', writable: false, enumerable: false, configurable: true }}); }}",
            origin.scheme(),
            self.0
        )
    }

    /// Whether a post carries this run's nonce (compared in full, whatever its length).
    fn accepts(&self, nonce: Option<&str>) -> bool {
        let Some(nonce) = nonce else { return false };
        nonce.len() == self.0.len() && nonce.bytes().zip(self.0.bytes()).fold(0u8, |differ, (a, b)| differ | (a ^ b)) == 0
    }
}

/// This run's host nonce, for smoke probes that run via `page.eval` after the bridge has captured and deleted `window.__MIGHTY_HOST_NONCE__`.
#[cfg(feature = "dev-harness")]
pub(crate) fn host_nonce(app: &tauri::AppHandle) -> String {
    app.try_state::<HostNonce>().map(|nonce| nonce.0.clone()).unwrap_or_default()
}

/// Where the app's own page is served (tauri's `get_app_url`): the dev server in a dev build, else the `tauri` protocol
/// (`http://tauri.localhost` on Windows, `tauri://localhost` on the Mac).
struct AppOrigin(tauri::Url);

impl AppOrigin {
    fn new(config: &tauri::Config) -> AppOrigin {
        #[cfg(dev)]
        if let Some(url) = config.build.dev_url.clone() {
            return AppOrigin(url);
        }
        let _ = config;
        let base = if cfg!(windows) { "http://tauri.localhost" } else { "tauri://localhost" };
        AppOrigin(tauri::Url::parse(base).expect("the tauri protocol's address"))
    }

    /// C# `WebShell.Trusted`: an address on the app's own origin (scheme, host and port; `tauri://` has no web origin to compare).
    fn trusts(&self, url: &tauri::Url) -> bool {
        url.scheme() == self.0.scheme()
            && url.host_str() == self.0.host_str()
            && url.port_or_known_default() == self.0.port_or_known_default()
    }

    /// Where the page's webview may navigate (its `on_navigation`): the app's own origin, and off Windows an HTML frame's page on a
    /// frame host (`mightyhtml://h….html.mighty.invalid/…`, files.rs). WebKit asks this for every frame, so on the Mac it is also what
    /// keeps a frame on the frame hosts (C#'s `FrameNavigationStarting`); it cannot tell a frame from the top page, so the page's own
    /// load start sends a top page that got there back home ([`AppOrigin::home`]). WebView2 asks it for the top page only: Windows
    /// keeps the app's origin alone. A frame host is never trusted to post to the host ([`AppOrigin::trusts`]).
    fn may_navigate(&self, url: &tauri::Url) -> bool {
        self.trusts(url)
            || !cfg!(windows)
                && url.scheme() == files::PROTOCOL
                && url.host_str().is_some_and(|host| host.to_ascii_lowercase().ends_with(mighty_files::frames::HOST_SUFFIX))
    }

    /// The app's own page, where a top page found elsewhere is sent back (the Mac; the window opens `index.html`).
    fn home(&self) -> Option<tauri::Url> {
        self.0.join("index.html").ok()
    }
}

/// The smoke's probe script hands what it read of the page here (smoke.rs).
#[cfg(feature = "dev-harness")]
#[tauri::command]
fn smoke_probe(probe: Value, host: tauri::State<'_, Arc<Host>>) {
    if let Some(smoke) = &host.smoke {
        smoke.probed(&host, probe);
    }
}

struct Inbound(tokio::sync::mpsc::UnboundedSender<Value>);

fn surfaces(app: &tauri::AppHandle) -> Option<Arc<SurfaceManager>> {
    app.try_state::<Arc<SurfaceManager>>().map(|state| state.inner().clone())
}

fn source_name(source: &LoadSource) -> String {
    match source {
        LoadSource::State => "state".into(),
        LoadSource::Backup => "backup".into(),
        LoadSource::Legacy(path) => format!("legacy {}", path.display()),
        LoadSource::Empty => "empty".into(),
        LoadSource::DamagedEmpty => "damaged, no backup: empty".into(),
        LoadSource::Unreadable => "unreadable: empty".into(),
    }
}

/// The exe's other modes (C# Program.cs:142-151), served before anything else: before the arguments are parsed (which refuses these),
/// before Tauri, WebView2, the profile, the log or any mutex, so nothing but their protocol reaches stdout. An agent CLI starts
/// `--agent-io-mcp` and `--agent-delegation-mcp` from a run's MCP configuration; the delegation smoke and its checks start
/// `--fake-claude <absolute root> …` in the CLI's place (mighty-delegation `modes`, `fake_claude`, W7-C). W9, in C#'s order after
/// them: the help capture's canned CLI `--help-demo-cli <absolute root> …` (mighty-delegation `help_demo_cli`, W9-H), then the update
/// helper `--update-helper …` (update_helper.rs, W9-B), which the 0.4.x app starts from its staged folder with only `SystemRoot` set.
/// The release build's GUI subsystem still reads the stdio handles a CLI hands it.
fn exe_mode(args: &[String]) -> Option<i32> {
    use mighty_delegation::modes;
    match args {
        [only] if only == modes::AGENT_IO_MCP_ARGUMENT => Some(modes::agent_io_mcp(platform::agent_io_connector())),
        [only] if only == modes::DELEGATION_MCP_ARGUMENT => Some(modes::delegation_mcp(platform::agent_io_connector())),
        #[cfg(feature = "dev-harness")]
        [first, root, rest @ ..] if first == mighty_delegation::fake_claude::ARGUMENT && mighty_core::normalize::fully_qualified(root) => {
            Some(mighty_delegation::fake_claude::run(Path::new(root), rest, platform::process_host()))
        }
        _ => {
            #[cfg(feature = "dev-harness")]
            if let Some(code) = mighty_delegation::help_demo_cli::try_run(args) {
                return Some(code);
            }
            if let Some(code) = update_helper::try_run(args) {
                return Some(code);
            }
            #[cfg(feature = "dev-harness")]
            if let Some(code) = verify_resources::try_run(args) {
                return Some(code);
            }
            None
        }
    }
}

fn main() {
    // Lossy, never `std::env::args()`, which panics on an argument that is not valid UTF-16: the update helper must reach its
    // `install.log` whatever it was given (W9-B review L1); the parser then refuses what it cannot read.
    let raw: Vec<String> = std::env::args_os().skip(1).map(|argument| argument.to_string_lossy().into_owned()).collect();
    if let Some(code) = exe_mode(&raw) {
        std::process::exit(code);
    }
    // C# `UserEnvironment.RepairCurrentProcess`: the user's own variables back after a bare start, before any thread (W9-B).
    startup::repair_environment();
    let options = match args::parse(raw) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    // What this run may write (W9: a release build's default, `--allow-state-write`, `--read-only`; startup.rs, W9-F).
    let context = tauri::generate_context!();
    let policy = startup::write_policy(&options, &context.config().identifier);
    // The OS's ports (platform.rs). Their paths are the data folder (`--profile` or %APPDATA%\MightyCode) and, for the default
    // profile only, the folders from before the rename (C# Program.cs).
    let platform = match platform::platform(options.profile.clone(), &context.config().identifier) {
        Ok(platform) => platform,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    // C# `MigrateRenamedIdentifiers`: the data from before the rename moved before anything reads it (W9-F).
    startup::before_ui(&options, &platform, policy);
    // What a crash of an earlier run left running is ended, and this run's children written down, before the first child starts.
    platform::record_child_groups(platform.paths.local_dir());
    let data_dir = platform.paths.data_dir().to_path_buf();
    let legacy = platform.paths.legacy_data_dirs().to_vec();
    // The log: the profile's `tauri-logs` only where the run writes or is a smoke, else this machine's own (startup.rs, W9-F).
    let log_dir = startup::log_dir(&options, platform.paths.as_ref(), policy);
    let log = LogFile::new(log_dir.clone());
    // C# StateStore: the state loaded (and, only where the write policy writes, the damaged file put aside and the legacy one copied);
    // its notes in the OS's language, as C# loads before the saved preference is applied. A file that does not read stops every
    // save: the app starts empty in memory and says why.
    let legacy = legacy.into_iter().find(|dir| dir.join(store::STATE_FILE).exists());
    let mut state = store::Store::new(data_dir.clone(), legacy, policy.writes);
    // The split state reads the active workspace's panes whole and the others' when something needs them (bodies.rs).
    state.lazy = true;
    let loaded = state.load(&Locale::new(methods::chosen_language("system")));
    let (mut snapshot, failure) = match loaded {
        Ok(snapshot) => (snapshot, None),
        Err(error) => (mighty_core::AppSnapshot::default(), Some(error)),
    };
    // Mac stage M-J: the settings the Swift app keeps in UserDefaults (the language, the app update's, the web-open choices, ...) read
    // from there, as 0.4.x left them (settings_place.rs, by `PlatformProfile::settings_store`; nothing on Windows or in a smoke).
    settings_place::load(&platform.profile, platform.prefs.as_ref(), &mut snapshot, options.smoke_test);
    if failure.is_none() && platform.profile.settings_store == mighty_platform::SettingsStore::UserDefaults {
        state.snapshot = snapshot.clone();
    }
    // A help capture reads the page in its own language (C# `UpdateAsync(LanguagePreference = helpLanguage)` after the load; W9-H).
    if let Some(language) = options.help_language.clone().filter(|_| options.help_capture) {
        snapshot.language_preference = Some(language.clone());
        state.snapshot.language_preference = Some(language);
    }
    let locale = Locale::new(methods::chosen_language(snapshot.language_preference()));
    let banner = match (&state.load_notice, &failure) {
        (Some(notice), _) => Some(notice.clone()),
        (None, Some(error)) => Some(format!("{} ({error})", error.message(&locale))),
        _ => None,
    };
    let source = state.source.clone().unwrap_or(LoadSource::Empty);
    #[cfg(feature = "dev-harness")]
    let load = json!({
        "path": state.state_path(),
        "source": source_name(&source),
        "notice": state.load_notice,
        "movedAsideTo": state.aside,
        "error": failure.as_ref().map(|e| e.to_string()),
        "writes": policy.writes,
        "workspaces": snapshot.workspaces().len(),
        "panes": snapshot.sessions().len(),
    });
    log.line(&format!(
        "start {} profile {} load {} ({} workspaces, {} panes) language {} writes {}",
        env!("CARGO_PKG_VERSION"),
        data_dir.display(),
        source_name(&source),
        snapshot.workspaces().len(),
        snapshot.sessions().len(),
        locale.language(),
        if policy.writes { "on" } else { "off" }
    ));
    if let Some(error) = &failure {
        log.line(&format!("load: {error}"));
    }
    if failure.is_some() {
        state.snapshot = snapshot.clone();
    }
    #[cfg(windows)]
    {
        state.replace = host::replace_file;
    }
    #[cfg(feature = "dev-harness")]
    let smoke = options.smoke_test.then(|| {
        let mode = match (&options.smoke_fake_cli, options.smoke_real_claude) {
            (Some(_), _) => smoke::Mode::FakeClaude,
            (None, true) => smoke::Mode::RealClaude,
            (None, false) => smoke::Mode::Page,
        };
        let record = log_dir.join("fake-claude").join("replay");
        // The help capture and the perf scenario run in the smoke's place (W9-H, smoke.rs `Special`).
        let special = match (&options.help_language, options.perf_scenario) {
            (Some(language), _) if options.help_capture => smoke::Special::HelpCapture { language: language.clone() },
            (_, true) => smoke::Special::Perf,
            _ => smoke::Special::None,
        };
        Smoke::new(
            options.smoke_out.clone().unwrap_or_else(|| data_dir.join("smoke-result.json")),
            options.smoke_exit,
            load.clone(),
            mode,
            options.smoke_fake_cli.is_some().then_some(record),
        )
        .special(special)
    });
    #[cfg(not(feature = "dev-harness"))]
    let smoke: Option<Smoke> = None;
    let theme = snapshot.theme().to_string();
    let mut parts = Some((state, banner, log, smoke));
    let debugging_port = options.remote_debugging_port;
    let native_dump = options.native_dump.clone();
    // The smoke's stand-in Claude records what it was sent beside the smoke's logs.
    #[cfg(feature = "dev-harness")]
    let fake_claude = options.smoke_fake_cli.clone().map(|binary| runs::FakeClaude {
        binary,
        recording: smoke::RECORDING.into(),
        record: log_dir.join("fake-claude").join("replay"),
    });
    #[cfg(not(feature = "dev-harness"))]
    let fake_claude: Option<runs::FakeClaude> = None;
    // The picture cache: the profile's own when the app may write it, else this machine's local folder for the profile.
    // These stores follow the write policy (W9: `policy.writes` replaced the flag here; a release build writes the profile's own).
    let image_dir = if policy.writes {
        platform.paths.image_cache_dir()
    } else {
        platform.paths.local_dir().join("image-cache").join(platform.paths.profile_key())
    };
    // The CLIs' catalogue's storage (C# ProviderCatalog's storageDirectory: the last list each CLI gave, `model-lists.json`, and beside
    // it the picker the last Hermes session offered, `hermes-models.json`) likewise: the profile folder, else this machine's local folder
    // for the profile.
    let catalog_dir = if policy.writes {
        data_dir.clone()
    } else {
        platform.paths.local_dir().join("provider-catalog").join(platform.paths.profile_key())
    };
    // The work map's summaries likewise, and its git's empty hooks folder this machine's own (workmap.rs).
    let workmap_folders = workmap::Folders::new(platform.paths.as_ref(), policy.writes);
    // The Antigravity rules' ledger (antigravity.rs, C# `AntigravityRuleLeases`) likewise.
    let antigravity_ledger = antigravity::ledger_path(&data_dir, platform.paths.local_dir(), platform.paths.profile_key(), policy.writes);
    let (local_dir, profile_key, allow_state_write) =
        (platform.paths.local_dir().to_path_buf(), platform.paths.profile_key().to_string(), policy.writes);
    // The file originals (files.rs, C# `FileOriginalStore`) likewise: the profile's `file-originals`, else this machine's own.
    let originals_dir = if policy.writes {
        platform.paths.file_originals_dir()
    } else {
        platform.paths.local_dir().join("file-originals").join(platform.paths.profile_key())
    };
    // The snapshots of the runs' folders (mighty-run checkpoints.rs) likewise: the profile's `snapshots`, else this machine's own.
    let snapshots_dir = if policy.writes {
        platform.paths.data_dir().join("snapshots")
    } else {
        platform.paths.local_dir().join("snapshots").join(platform.paths.profile_key())
    };
    // The page's web data (paths.rs `web_folders`, W9-F: C#'s own folder in a release build on the default profile, so the page's
    // storage survives the update; this machine's local one otherwise, as before W9).
    // Only a run that writes adopts them (review M1): never beside a C# app that may still hold them.
    let adopted = startup::adopts_csharp_web_data(&options, policy);
    let web_folders = platform.paths.web_folders(adopted);

    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "dev-harness")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        host_post,
        smoke_probe,
        smoke::smoke_surface,
        smoke_runs::smoke_step,
        smoke_terminal::smoke_terminal,
        smoke_history::smoke_history,
        smoke_settings::smoke_settings,
        smoke_setup::smoke_setup,
        smoke_accounts::smoke_accounts,
        smoke_updates::smoke_updates
    ]);
    #[cfg(not(feature = "dev-harness"))]
    let builder = builder.invoke_handler(tauri::generate_handler![host_post]);
    let app = builder
        // HTML previews' folders, each under a host of its own, for the page's sandboxed frame (files.rs).
        .register_asynchronous_uri_scheme_protocol(files::PROTOCOL, files::protocol)
        .setup(move |app| {
            let (state, banner, log, smoke) = parts.take().expect("setup runs once");
            let version = app.package_info().version.to_string();
            let host = Host::new(
                app.handle().clone(),
                data_dir.clone(),
                platform.processes.clone(),
                state,
                locale,
                version,
                banner,
                log,
                smoke,
                methods::registry(),
            );
            // The native forms' pushes also to `--native-dump`'s file (native.rs).
            native::configure(&host, native_dump.clone());
            // The files pane's state and the file originals, which the run engine keeps through it (files.rs).
            files::start(&host, originals_dir.clone());
            workmap::configure(&host, workmap_folders.clone());
            // The frame's seams (frame.rs): the add menu's files entry opens files.rs's pane, and an agent pane added from the menu
            // offers the folder's earlier sessions through history.rs.
            let _ = frame::OPEN_FILES.set(files::open_files);
            let _ = frame::RESUME.set(Arc::new(history::HistoryResume));
            // The run engine: the run service owns the state from here on (runs.rs).
            #[cfg(feature = "dev-harness")]
            if let Some(fake) = &fake_claude
                && let Some(folder) = fake.record.parent()
            {
                let _ = std::fs::create_dir_all(folder);
            }
            // The smoke's stand-in agy writes its own settings, never the user's, and keeps its ledger beside them.
            #[cfg(feature = "dev-harness")]
            let agy_settings = fake_claude.as_ref().map(runs::FakeClaude::agy_settings);
            #[cfg(not(feature = "dev-harness"))]
            let agy_settings: Option<std::path::PathBuf> = None;
            let agy_ledger = match agy_settings.as_deref().and_then(Path::parent).and_then(Path::parent).and_then(Path::parent) {
                Some(home) => home.join(antigravity::LEDGER),
                None => antigravity_ledger.clone(),
            };
            // The user's own settings: what a launch with `--allow-state-write` the other way round recorded goes too (C4), the profile's
            // ledger read, never written, without the flag.
            let other_ledger =
                agy_settings.is_none().then(|| antigravity::ledger_path(&data_dir, &local_dir, &profile_key, !allow_state_write));
            let agy_rules = antigravity::rules(
                agy_settings.as_deref(),
                &agy_ledger,
                other_ledger.as_deref().map(|other| (other, allow_state_write)),
                &[data_dir.clone(), local_dir.clone()],
            );
            runs::Runs::start(
                &host,
                platform.processes.clone(),
                platform.environment.clone(),
                fake_claude.clone(),
                image_dir.clone(),
                catalog_dir.clone(),
                agy_rules,
            );
            // Each request's start, steps, turns and end snapshotted, so its result lists the files every part changed (not in a smoke).
            if host.smoke.is_none() {
                runs::start_checkpoints(&host, snapshots_dir.clone(), workmap_folders.hooks.clone());
            }
            // Terminal panes, the agents' terminals, terminal prompts and shell block runs (terminal.rs).
            terminal::Terminals::start(&host, &platform);
            // Delegation over the run service (delegation.rs, W7-D) and the agents' MCP listener (agent_io.rs, W7-C; not in a smoke).
            delegation::start(&host);
            agent_io::start(&host);
            // W8: the phone remote (mobile.rs, W8-A), the direct path (direct.rs, W8-C) and remote workspaces (remote.rs, W8-D); each acts
            // only where the host allows it (`Host::may_reach_relay`, `Host::may_listen`). The companion starts with the window, below.
            mobile::start(&host, &platform);
            direct::start(&host, &platform);
            remote::start(&host, &platform);
            // W9 (Step 0 stubs that start nothing until their parts fill them): the remote mirror (W9-J), per-pane worktrees (W9-I),
            // styles (W9-D), background output tails and the Ouroboros dashboard (W9-E), the app update's housekeeping and loop
            // (W9-C, never in a smoke).
            mirror::start(&host, &platform);
            pane_worktree::start(&host);
            styles::start(&host, &platform);
            tails::start(&host);
            ouroboros::start(&host);
            app_update::start(&host, &platform);
            host.start_clocks();
            // Mac stage: the menu bar (menu.rs, M-F; nothing on Windows, Tauri's default macOS menu until then).
            if let Err(error) = menu::install(app.handle()) {
                host.log.line(&format!("menu: {error}"));
            }
            let (inbound, mut calls) = tokio::sync::mpsc::unbounded_channel::<Value>();
            app.manage(Inbound(inbound));
            app.manage(host.clone());
            // C# `BeginAutomaticUpdates`: the automatic updates' 60 s tick from the start (updates.rs, W6-D).
            let _ = updates::of(&host);
            // The user's settings outside the state (the text setting: frame.rs).
            app.manage(platform.prefs.clone());
            // W6's ports: the terminal window, the font families, the HTTPS GET (platform.rs; a smoke's fakes).
            app.manage(platform::ports(host.smoke.is_some()));
            // The pseudo consoles, for a CLI's hidden background sign-in (accounts.rs, W6-C).
            app.manage(platform.ptys.clone());
            // The app's folders, for the toolkit's store (settings/tools.rs, W6-B).
            app.manage(platform.paths.clone());
            // Completion notifications and the toast activator: registered with the OS only where the app may change the machine,
            // never in a smoke (notifications.rs).
            if host.smoke.is_none() {
                notifications::start(&host, &platform);
            }
            let dispatcher = host.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(message) = calls.recv().await {
                    // Each call is a task of its own (C#'s `async void Receive`); the next starts once this one first returned, so
                    // what each does at once happens in the order the page posted them (host.rs).
                    let (first, returned) = tokio::sync::oneshot::channel();
                    let host = dispatcher.clone();
                    tauri::async_runtime::spawn(async move {
                        let surfaces = surfaces(&host.app);
                        // NativeSlot: who draws what reaches the page before `ready` pushes the snapshot (surface.rs).
                        if message.get("method").and_then(Value::as_str) == Some("ready")
                            && let Some(surfaces) = &surfaces
                        {
                            surfaces.announce();
                        }
                        let ready = message.get("method").and_then(Value::as_str) == Some("ready");
                        host.clone().dispatch(message, first).await;
                        // A page that loaded again gets back its open dialogs and the text setting (frame.rs).
                        if ready {
                            frame::page_ready(&host);
                        }
                        // A pane the call closed takes its native view with it.
                        if let Some(surfaces) = surfaces {
                            surfaces.retain(&surface::live_panes(&host)).await;
                        }
                    });
                    let _ = returned.await;
                }
            });
            #[cfg(feature = "dev-harness")]
            if host.smoke.is_some() {
                Smoke::watch(host.clone());
            }

            // Its own WebView2 data, never the C# app's (<profile>\web-profile): under this identity's local data folder, one
            // folder per profile (Paths::webview_dir); W9-F: C#'s own in a release build on the default profile (`web_folders`).
            let webview_data = web_folders.page.clone();
            host.log.line(&format!("webview data {}", webview_data.display()));
            let (r, g, b) = if theme == "light" { (0xEC, 0xEE, 0xF3) } else { (0x0B, 0x0F, 0x19) };
            let origin = AppOrigin::new(app.config());
            let origin_url = origin.0.clone();
            app.manage(origin);
            let nonce = HostNonce::new();
            let nonce_script = nonce.script(&origin_url);
            app.manage(nonce);
            // Made again while another process still holds the data folder with other options (C#'s WebView2 right after the update
            // swap; startup.rs, review M1), at most about 10 s; the setup runs before the event loop, so the wait blocks nothing else.
            let window = startup::retry_while_held(
                || {
                    let page = host.clone();
                    let navigation = AppOrigin(origin_url.clone());
                    let loaded = AppOrigin(origin_url.clone());
                    let mut window = WebviewWindowBuilder::new(app.handle(), WINDOW, WebviewUrl::App("index.html".into()))
                        // The host's nonce: the script defines it in the top frame on the app's origin only (wry runs it in every
                        // frame on Windows).
                        .initialization_script(nonce_script.clone())
                        .title("Mighty Code")
                        .inner_size(1440.0, 920.0)
                        .min_inner_size(940.0, 650.0)
                        .background_color(tauri::window::Color(r, g, b, 255))
                        .data_directory(webview_data.clone())
                        // The page takes dropped files itself (HTML5 drop → addAttachments), as C#'s WebView2 does; Tauri's own handler
                        // would swallow the drop on Windows.
                        .disable_drag_drop_handler()
                        // C# WebShell: the page never leaves the app's own origin (a stray drop or link would navigate it away), and it
                        // opens no window of its own.
                        .on_navigation(move |url| navigation.may_navigate(url))
                        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                        .on_page_load(move |webview, payload| {
                            if payload.event() == PageLoadEvent::Started {
                                // Off Windows a frame host may be navigated to (`may_navigate`): the top page itself never stays there.
                                if !cfg!(windows) && !loaded.trusts(payload.url()) {
                                    if let Some(home) = loaded.home() {
                                        tauri::async_runtime::spawn(async move {
                                            let _ = webview.navigate(home);
                                        });
                                    }
                                    return;
                                }
                                page.page_started();
                                // On the UI thread: the views hide from a task, never here (a view may be waiting for this thread).
                                if let Some(surfaces) = surfaces(&page.app) {
                                    tauri::async_runtime::spawn(async move { surfaces.page_started().await });
                                }
                            }
                        });
                    if let Some(port) = debugging_port {
                        // These replace wry's own arguments, which are kept.
                        window = window.additional_browser_args(&format!(
                            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={port}"
                        ));
                    } else if adopted {
                        // C#'s folder opens with C#'s options: no browser arguments (review M1).
                        window = window.additional_browser_args(startup::CSHARP_BROWSER_ARGUMENTS);
                    }
                    window.build()
                },
                std::thread::sleep,
                |line| host.log.line(line),
            )?;
            // W9-C: the session-ending watch on the main window, from its own thread (an install on quit waits while Windows ends it).
            app_update::watch_window(&host, &window.as_ref().window());
            // The native views over the page (the Mac's terminal, the pet), in the OS's implementation.
            let factory = platform::surfaces(window.as_ref().window());
            // The companion pet (companion.rs, W8-E): its window is one of the main window's native views.
            companion::start(&host, &platform, factory.clone());
            let push = surface::page_push(app.handle().clone());
            app.manage(Arc::new(SurfaceManager::new(factory, platform.profile.os, push)));
            Ok(())
        })
        .build(context)
        .expect("the Tauri app");
    // Tauri's event loop ends with 0 whatever `exit(code)` asked (tauri-runtime-wry's RequestExit sets ControlFlow::Exit): the
    // code is kept here and given to the process at the end, so a failed `--smoke-exit` run exits 1.
    let mut exit_code = 0;
    app.run(move |app, event| match event {
        RunEvent::ExitRequested { code: Some(code), .. } => exit_code = code,
        // Mac stage: an address the OS opened the app with (mightycode://, deep_link.rs, M-F).
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => deep_link::opened(app, &urls),
        RunEvent::Exit => {
            if let Some(host) = app.try_state::<Arc<Host>>() {
                // Quit began (C# QuitAsync sets `closing` first): no run, sign-in, retry or read starts during the teardown below.
                host.begin_closing();
                // W9-C: a ready update installs on quit (`--relaunch 0`) before anything shuts down, as C#'s quit path starts it.
                app_update::quit_hook(&host, false);
                // The model lists read and reconcile nothing more (C# `modelRefreshes?.Shutdown()`, before the last save).
                models::shutdown(&host);
                // The accounts' sign-ins and reads let go, and the account usage's pending reads cancelled (accounts.rs, usage.rs, W6-C).
                accounts::shutdown(&host);
                tauri::async_runtime::block_on(usage::shutdown(&host));
                // Every terminal closed first (C# CloseTerminalsAsync), the wait bounded.
                terminal::close_all(app);
                // The CLI updates and plugin operations cancelled, the wait bounded (updates.rs, W6-D).
                updates::shutdown(&host);
                // The agents' bindings revoked and their listener closed (agent_io.rs, W7-C), delegation's retries stopped (W7-D).
                agent_io::shutdown(&host);
                // W8: the pet closed, the phone's relay and direct listeners and remote sharing ended before the runs are disposed.
                companion::shutdown(&host);
                mobile::shutdown(&host);
                direct::shutdown(&host);
                remote::shutdown(&host);
                // W9: the mirror (W9-J), the style watcher (W9-D), the tails poller and the dashboard's polling (W9-E).
                mirror::shutdown(&host);
                styles::shutdown(&host);
                tails::shutdown(&host);
                ouroboros::shutdown(&host);
                // Every run stopped and ended before delegation stops (C# DisposeAsync: `local.DisposeAsync`, then `StopDelegation`), so a
                // child killed by quitting is recorded with its quit end; `host.close` below then makes the last save.
                if let Some(runs) = host.runs() {
                    tauri::async_runtime::block_on(runs.service.dispose());
                }
                delegation::shutdown(&host);
                // The last save (C# DesktopService.DisposeAsync), written only with --allow-state-write.
                host.close();
                host.write_unimplemented();
                host.log.line("exit");
            }
            if exit_code != 0 {
                app.cleanup_before_exit();
                std::process::exit(exit_code);
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review M-E 1(b): `host_post` hears only this run's nonce, which the main frame's script holds; none, another or a part of it is
    /// refused.
    #[test]
    fn only_the_runs_nonce_reaches_the_host() {
        let nonce = HostNonce::new();
        assert_eq!(nonce.0.len(), 64);
        assert!(nonce.accepts(Some(&nonce.0.clone())));
        assert!(!nonce.accepts(None));
        assert!(!nonce.accepts(Some("")));
        assert!(!nonce.accepts(Some(&nonce.0[..63])));
        assert!(!nonce.accepts(Some(&HostNonce::new().0)), "each run has its own");
        let script = nonce.script(&url("http://tauri.localhost"));
        assert!(script.contains(&nonce.0) && script.contains("__MIGHTY_HOST_NONCE__"));
        assert!(script.contains("writable: false, enumerable: false, configurable: true"), "{script}");
        assert!(script.starts_with("if (window.top === window && location.protocol === 'http:' && location.host === 'tauri.localhost')"));
        assert!(nonce.script(&url("tauri://localhost")).contains("location.protocol === 'tauri:' && location.host === 'localhost'"));
        assert!(nonce.script(&url("http://localhost:5173")).contains("location.host === 'localhost:5173'"), "the dev server's port");
    }

    /// P0-SEC-2: `tauri.conf.json` configures the main WebView CSP (with `devCsp: null` for Vite HMR in dev).
    #[test]
    fn tauri_config_sets_main_webview_csp() {
        let raw: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let security = &raw["app"]["security"];
        let csp = security["csp"].as_str().expect("app.security.csp must be a policy string");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-src http://*.html.mighty.invalid mightyhtml:"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("connect-src 'self' ipc: http://ipc.localhost"));
        assert!(security["devCsp"].is_null());
        assert_eq!(security["dangerousDisableAssetCspModification"], serde_json::json!(["style-src"]));
    }

    fn url(text: &str) -> tauri::Url {
        tauri::Url::parse(text).unwrap()
    }

    /// C# `WebShell.Trusted`: the app's own origin only, however a page elsewhere names itself.
    #[test]
    fn only_the_apps_own_page_is_trusted() {
        let windows = AppOrigin(url("http://tauri.localhost"));
        assert!(windows.trusts(&url("http://tauri.localhost/index.html")));
        assert!(windows.trusts(&url("http://TAURI.localhost:80/a?b#c")));
        for elsewhere in [
            "https://tauri.localhost/",
            "http://tauri.localhost:8080/",
            "http://tauri.localhost.example.com/",
            "http://localhost/",
            "file:///C:/Users/me/Downloads/page.html",
            "https://example.com/",
            "about:blank",
        ] {
            assert!(!windows.trusts(&url(elsewhere)), "{elsewhere}");
        }
        let mac = AppOrigin(url("tauri://localhost"));
        assert!(mac.trusts(&url("tauri://localhost/index.html")));
        assert!(!mac.trusts(&url("tauri://elsewhere/index.html")));
        let dev = AppOrigin(url("http://localhost:5173"));
        assert!(dev.trusts(&url("http://localhost:5173/src/main.tsx")));
        assert!(!dev.trusts(&url("http://localhost:5174/")));
    }

    /// The page's webview may show an HTML frame's page from a frame host (WebKit asks for frames too), but nothing else outside the
    /// app's origin, and a frame host never counts as the app's own page.
    #[test]
    fn a_frame_host_may_be_navigated_to_but_is_never_trusted() {
        let frame = url("mightyhtml://h9d5aa952066d4c308a02be2c3ef27854.html.mighty.invalid/index.html");
        for origin in [AppOrigin(url("tauri://localhost")), AppOrigin(url("http://tauri.localhost"))] {
            // Windows' WebView2 asks for the top page only, which never goes to a frame host.
            assert_eq!(origin.may_navigate(&frame), !cfg!(windows));
            assert!(!origin.trusts(&frame));
            assert_eq!(origin.may_navigate(&url("mightyhtml://H1.HTML.MIGHTY.INVALID/a.html")), !cfg!(windows));
            assert!(origin.may_navigate(&origin.0));
            assert!(origin.home().is_some_and(|home| origin.trusts(&home) && home.path() == "/index.html"));
            for elsewhere in [
                "mightyhtml://example.com/index.html",
                "mightyhtml://h1.html.mighty.invalid.example.com/",
                "https://h1.html.mighty.invalid/",
                "https://example.com/",
                "file:///Users/me/page.html",
            ] {
                assert!(!origin.may_navigate(&url(elsewhere)), "{elsewhere}");
            }
        }
    }

    /// The DLLs a PE image imports (its import directory's names), read from the file: enough of the format for the check below.
    #[cfg(windows)]
    fn imported_dlls(image: &[u8]) -> Vec<String> {
        let u16_at = |at: usize| u16::from_le_bytes([image[at], image[at + 1]]) as usize;
        let u32_at = |at: usize| u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]]) as usize;
        let pe = u32_at(0x3C);
        assert_eq!(&image[pe..pe + 4], b"PE\0\0", "a PE image");
        let (sections, optional) = (u16_at(pe + 6), pe + 24);
        assert_eq!(u16_at(optional), 0x20B, "a PE32+ image");
        let imports = u32_at(optional + 120);
        let table = optional + u16_at(pe + 20);
        let offset = |rva: usize| {
            (0..sections).map(|index| table + index * 40).find_map(|section| {
                let (size, address, raw) = (u32_at(section + 8).max(u32_at(section + 16)), u32_at(section + 12), u32_at(section + 20));
                (address..address + size).contains(&rva).then(|| rva - address + raw)
            })
        };
        let mut names = Vec::new();
        let mut descriptor = offset(imports).expect("the import directory lies in a section");
        while u32_at(descriptor + 12) != 0 {
            let name = offset(u32_at(descriptor + 12)).expect("an import's name lies in a section");
            let end = image[name..].iter().position(|byte| *byte == 0).expect("a NUL-terminated name");
            names.push(String::from_utf8_lossy(&image[name..name + end]).to_ascii_lowercase());
            descriptor += 20;
        }
        names
    }

    /// W9 (plan fact 8, D7): `.cargo/config.toml` links the C runtime statically, so no binary of this workspace needs the VC++
    /// redistributable — the update helper must start on a machine without it. This test's own exe is built with the same flags as
    /// MightyCode.exe; `scripts/check-imports.ps1` checks a built MightyCode.exe the same way.
    #[cfg(windows)]
    #[test]
    fn the_exe_imports_no_c_runtime_dll() {
        let image = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let dlls = imported_dlls(&image);
        assert!(dlls.iter().any(|dll| dll == "kernel32.dll"), "the import table read: {dlls:?}");
        let runtime: Vec<&String> =
            dlls.iter().filter(|dll| ["vcruntime", "msvcp", "api-ms-win-crt-"].iter().any(|prefix| dll.starts_with(prefix))).collect();
        assert!(runtime.is_empty(), "the C runtime is linked statically, yet the exe imports {runtime:?}");
    }
}

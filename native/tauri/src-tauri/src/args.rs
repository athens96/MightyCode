//! The command line (C# `StartupOptions.Parse`, MightyCode.WinUI/Program.cs), with the skeleton's own switches.
//!
//! `--profile <absolute folder>`  the state folder instead of `%APPDATA%\MightyCode` (never read from the legacy folders)
//! `--smoke-test`                 run the smoke (smoke.rs); needs `--profile`
//! `--smoke-exit`                 quit when the smoke has its result, exit code 0 when it passed, else 1
//! `--smoke-out <file>`           where the smoke writes its result (default `<profile>\smoke-result.json`)
//! `--log-dir <folder>`           the host's logs (default `<profile>\tauri-logs`)
//! `--remote-debugging-port <n>`  opens WebView2's DevTools protocol on that port (screenshots, inspection); only in a development
//!                                build or with `--smoke-test` (any local process could drive the page and the app's IPC through it)
//! `--allow-state-write`          lets the host write the profile's state (saves, the damaged-file recovery, the legacy copy) as
//!                                C# does; without it nothing in the profile is written but the logs and the smoke's result
//! `--read-only`                  W9: writes nothing, whatever the build (startup.rs's write policy; wins over `--allow-state-write`)
//! `--smoke-fake-cli <exe>`       the smoke's Claude is mighty-run's stand-in CLI (`mighty-fake-cli --replay-cli`), every other
//!                                provider missing; the smoke then runs its composer, cards and Mighty steps against it
//! `--smoke-real-claude`          the smoke sends one real request with the installed `claude` CLI and waits for its answer
//! `--native-dump <file>`         every `native` and `text` push written to the file as sent, one per line (native.rs; the parity
//!                                capture's Rust side, plan W6 §1.8)
//! `--help-capture`               W9-H: the help site's pictures in the smoke's place (a smoke run: needs `--profile`)
//! `-language ko|en|zh|ja`        W9-H: the help capture's language (only with `--help-capture`; default `ko`)
//! `--perf-scenario`              W9-H: the timing scenario in the smoke's place (needs `--smoke-test`, not with `--help-capture`)
//! `-Embedding`                   W9-F: COM started the app for a toast's activation (the app starts as usual; COM calls it)
//! `----AppNotificationActivated:…`  W9-F: a toast left by C#'s Windows App SDK registration; accepted and ignored
//! `--server`                     MC-3: the app without a window (the headless server, server.rs): the host, the run engine, the phone
//!                                relay, the control API, the mirror, the schedules and the daemon link run; a window shows when
//!                                asked (the Dock, a second start, `mighty server show`). Not `--daemon --headless`, the process
//!                                daemon (daemon/entry.rs), which main.rs serves before this parser and which runs no host at all
//! `--for-schedules`              MC-39: with `--server`, the OS started the app for a schedule (the LaunchAgent, the scheduled task;
//!                                automation/wake.rs): it ends when an app already runs for the profile, and quits once idle
//! `-psn_…`                       Mac stage: the process serial number older LaunchServices passed; accepted and ignored on the Mac
//!                                (a refused argument would quit an app 0.4.x's updater relaunched with `open -n`; plan §1.1)
//!
//! C#'s other rule, a smoke's profile must be empty, holds only for `--help-capture` and `--perf-scenario`, which seed their own (W9-H):
//! `scripts/smoke.ps1` runs the plain smoke on a seeded demo profile and `-Profile` on a copy of a real one.

use std::path::PathBuf;

/// C#'s `Locale.Languages`, the help capture's languages.
const HELP_LANGUAGES: [&str; 4] = ["ko", "en", "zh", "ja"];

/// The argument a toast registered through C#'s Windows App SDK starts the exe with (its prefix; the rest is the toast's).
const APP_SDK_ACTIVATION: &str = "----AppNotificationActivated:";

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub profile: Option<PathBuf>,
    pub smoke_test: bool,
    pub smoke_exit: bool,
    pub smoke_out: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub remote_debugging_port: Option<u16>,
    pub allow_state_write: bool,
    pub smoke_fake_cli: Option<PathBuf>,
    pub smoke_real_claude: bool,
    pub native_dump: Option<PathBuf>,
    /// W9: writing off whatever the build (startup.rs).
    pub read_only: bool,
    /// W9-H: `--help-capture` (also sets `smoke_test`, as C#'s does) and its language (`ko` unless `-language` said).
    pub help_capture: bool,
    pub help_language: Option<String>,
    /// W9-H: `--perf-scenario`.
    pub perf_scenario: bool,
    /// W9-F: started by COM for a toast's activation (`-Embedding`).
    pub embedding: bool,
    /// MC-3: `--server`, the app without a window (server.rs).
    pub server: bool,
    /// MC-39: `--for-schedules` (with `--server`): started by the OS for a schedule.
    pub for_schedules: bool,
}

fn absolute(flag: &str, value: Option<String>) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.ok_or_else(|| format!("{flag} needs a path."))?);
    if !path.is_absolute() {
        return Err(format!("{flag} needs an absolute path."));
    }
    Ok(path)
}

/// LaunchServices' `-psn_<serial>` (Mac stage, plan §1.1): ignored on the Mac, refused elsewhere as before.
fn mac_launch_argument(argument: &str) -> bool {
    cfg!(target_os = "macos") && argument.starts_with("-psn_")
}

/// Whether the DevTools port may open (W9 review L9): a development build, or a smoke run in its own profile; never a user's release app.
fn debugging_port_allowed(debug_build: bool, smoke_test: bool) -> bool {
    debug_build || smoke_test
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" if options.profile.is_none() => options.profile = Some(absolute("--profile", args.next())?),
            "--smoke-test" => options.smoke_test = true,
            "--smoke-exit" => options.smoke_exit = true,
            "--allow-state-write" => options.allow_state_write = true,
            "--read-only" => options.read_only = true,
            "--smoke-fake-cli" => options.smoke_fake_cli = Some(absolute("--smoke-fake-cli", args.next())?),
            "--smoke-real-claude" => options.smoke_real_claude = true,
            "--smoke-out" => options.smoke_out = Some(absolute("--smoke-out", args.next())?),
            "--log-dir" => options.log_dir = Some(absolute("--log-dir", args.next())?),
            "--native-dump" => options.native_dump = Some(absolute("--native-dump", args.next())?),
            "--remote-debugging-port" => {
                let port = args.next().and_then(|p| p.parse().ok()).ok_or("--remote-debugging-port needs a port number.")?;
                options.remote_debugging_port = Some(port);
            }
            // Developer switches, never shown to a user: their refusals are not translated (C# Program.cs:33-38).
            "--help-capture" => {
                options.help_capture = true;
                options.smoke_test = true;
            }
            "--perf-scenario" => options.perf_scenario = true,
            "-language" => {
                let language = args.next().filter(|l| HELP_LANGUAGES.contains(&l.as_str())).ok_or("-language needs ko, en, zh or ja.")?;
                options.help_language = Some(language);
            }
            "-Embedding" => options.embedding = true,
            crate::server::ARGUMENT => options.server = true,
            mighty_core::automation::wake::SWITCH => options.for_schedules = true,
            other if other.starts_with(APP_SDK_ACTIVATION) => {}
            other if mac_launch_argument(other) => {}
            other => return Err(format!("Unsupported argument: {other}")),
        }
    }
    if options.smoke_exit && !options.smoke_test {
        return Err("--smoke-exit needs --smoke-test.".into());
    }
    if options.perf_scenario && !options.smoke_test {
        return Err("--perf-scenario needs --smoke-test.".into());
    }
    if options.help_language.is_some() && !options.help_capture {
        return Err("-language goes with --help-capture.".into());
    }
    if options.help_capture && options.perf_scenario {
        return Err("--help-capture and --perf-scenario do not go together.".into());
    }
    if (options.smoke_fake_cli.is_some() || options.smoke_real_claude) && !options.smoke_test {
        return Err("--smoke-fake-cli and --smoke-real-claude need --smoke-test.".into());
    }
    if options.smoke_fake_cli.is_some() && options.smoke_real_claude {
        return Err("--smoke-fake-cli and --smoke-real-claude go one at a time.".into());
    }
    if options.remote_debugging_port.is_some() && !debugging_port_allowed(cfg!(debug_assertions), options.smoke_test) {
        return Err("--remote-debugging-port works only in a development build or with --smoke-test.".into());
    }
    // The headless server draws nothing a smoke could read, and opens no DevTools port.
    if options.server && (options.smoke_test || options.remote_debugging_port.is_some()) {
        return Err("--server does not go with --smoke-test or --remote-debugging-port.".into());
    }
    if options.for_schedules && !options.server {
        return Err("--for-schedules goes with --server.".into());
    }
    if options.smoke_test && options.profile.is_none() {
        return Err("--smoke-test needs --profile.".into());
    }
    if options.profile.as_ref().is_some_and(|p| p.is_file()) {
        return Err("--profile names a file, not a folder.".into());
    }
    // C#'s "a smoke needs an empty profile", for the two runs that seed their own (W9-H): the help capture's welcome shot is the empty
    // profile's first window, and the perf scenario measures only what it made.
    // A folder that is there but cannot be listed is not known to be empty, so it is refused too.
    if (options.help_capture || options.perf_scenario)
        && options.profile.as_ref().is_some_and(|p| p.exists() && std::fs::read_dir(p).map_or(true, |mut entries| entries.next().is_some()))
    {
        return Err("The smoke profile must be a new or empty folder.".into());
    }
    if options.help_capture && options.help_language.is_none() {
        options.help_language = Some("ko".into());
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn smoke_switches_go_together() {
        let root = if cfg!(windows) { "C:\\p" } else { "/p" };
        assert!(parsed(&["--smoke-test", "--smoke-exit", "--profile", root]).is_ok());
        assert!(parsed(&["--smoke-exit", "--profile", root]).is_err());
        assert!(parsed(&["--smoke-test"]).is_err());
        assert!(parsed(&["--profile", "relative"]).is_err());
        assert!(parsed(&["--bogus"]).is_err());
        // With --smoke-test the port is allowed in any build (a release build refuses it alone, W9-H L9).
        let port = parsed(&["--smoke-test", "--profile", root, "--remote-debugging-port", "9222"]);
        assert_eq!(port.unwrap().remote_debugging_port, Some(9222));
        assert_eq!(parsed(&["--remote-debugging-port", "9222"]).is_ok(), cfg!(debug_assertions), "alone only in a development build");
        assert!(!parsed(&[]).unwrap().allow_state_write, "writing is off unless asked for");
        assert!(parsed(&["--allow-state-write"]).unwrap().allow_state_write);
        let fake = if cfg!(windows) { "C:\\f.exe" } else { "/f" };
        assert!(parsed(&["--smoke-fake-cli", fake]).is_err(), "the stand-in CLI is the smoke's");
        assert!(parsed(&["--smoke-test", "--profile", root, "--smoke-fake-cli", fake]).unwrap().smoke_fake_cli.is_some());
        assert!(parsed(&["--smoke-test", "--profile", root, "--smoke-fake-cli", fake, "--smoke-real-claude"]).is_err());
        assert!(parsed(&["--smoke-test", "--profile", root, "--smoke-real-claude"]).unwrap().smoke_real_claude);
        let dump = if cfg!(windows) { "C:\\d\\native.jsonl" } else { "/d/native.jsonl" };
        assert!(parsed(&["--native-dump", dump]).unwrap().native_dump.is_some());
        assert!(parsed(&["--native-dump", "native.jsonl"]).is_err(), "the dump's path is absolute");
    }

    /// W9 Step 0: C#'s developer switches and their combination rules (Program.cs:34-46), `--read-only`, and the two ways COM or a
    /// leftover Windows App SDK toast starts the exe; anything else is still refused, `--update-helper` included (main.rs serves the
    /// helper before the parser, as C#'s `TryParse` does).
    #[test]
    fn the_w9_switches_follow_csharps_rules() {
        let root = if cfg!(windows) { "C:\\p" } else { "/p" };
        let help = parsed(&["--help-capture", "--profile", root]).unwrap();
        assert!(help.help_capture && help.smoke_test, "the help capture is a smoke run");
        assert_eq!(help.help_language.as_deref(), Some("ko"));
        assert_eq!(parsed(&["--help-capture", "--profile", root, "-language", "ja"]).unwrap().help_language.as_deref(), Some("ja"));
        assert_eq!(parsed(&["--help-capture", "--profile", root, "-language", "fr"]).unwrap_err(), "-language needs ko, en, zh or ja.");
        assert_eq!(parsed(&["-language", "en"]).unwrap_err(), "-language goes with --help-capture.");
        assert_eq!(parsed(&["--help-capture"]).unwrap_err(), "--smoke-test needs --profile.");
        assert!(parsed(&["--help-capture", "--smoke-exit", "--profile", root]).is_ok());
        assert_eq!(parsed(&["--perf-scenario", "--profile", root]).unwrap_err(), "--perf-scenario needs --smoke-test.");
        assert!(parsed(&["--smoke-test", "--perf-scenario", "--profile", root]).unwrap().perf_scenario);
        assert_eq!(
            parsed(&["--help-capture", "--perf-scenario", "--profile", root]).unwrap_err(),
            "--help-capture and --perf-scenario do not go together."
        );
        assert!(parsed(&["--read-only"]).unwrap().read_only);
        assert!(parsed(&["-Embedding"]).unwrap().embedding);
        assert!(parsed(&["----AppNotificationActivated:", "-Embedding"]).is_ok());
        assert!(parsed(&["----AppNotificationActivated:sessionId=abc"]).is_ok());
        assert!(parsed(&["-embedding"]).is_err(), "COM's switch as COM writes it");
        assert!(parsed(&["--update-helper", "--staged", root]).is_err());
    }

    /// MC-3: `--server` starts the app without a window, alone or with a profile and the write switches; never in a smoke, never with
    /// the daemon's own `--headless` (the process daemon is `--daemon --headless`, served before this parser).
    #[test]
    fn the_server_switch_reads_and_keeps_out_of_the_smoke() {
        let root = if cfg!(windows) { "C:\\p" } else { "/p" };
        assert!(parsed(&["--server"]).unwrap().server);
        assert!(!parsed(&[]).unwrap().server, "a window unless asked");
        let with_profile = parsed(&["--server", "--profile", root, "--allow-state-write"]).unwrap();
        assert!(with_profile.server && with_profile.allow_state_write && with_profile.profile.is_some());
        let refusal = "--server does not go with --smoke-test or --remote-debugging-port.";
        assert_eq!(parsed(&["--server", "--smoke-test", "--profile", root]).unwrap_err(), refusal);
        assert_eq!(parsed(&["--server", "--help-capture", "--profile", root]).unwrap_err(), refusal);
        assert_eq!(parsed(&["--server", "--smoke-test", "--profile", root, "--remote-debugging-port", "9222"]).unwrap_err(), refusal);
        assert_eq!(parsed(&["--headless"]).unwrap_err(), "Unsupported argument: --headless", "the daemon's switch is not the app's");
        assert_eq!(parsed(&["--daemon"]).unwrap_err(), "Unsupported argument: --daemon");
        // MC-39: the OS's start for a schedule is a server start.
        let woken = parsed(&["--server", "--for-schedules", "--profile", root]).unwrap();
        assert!(woken.server && woken.for_schedules);
        assert!(!parsed(&["--server"]).unwrap().for_schedules);
        assert_eq!(parsed(&["--for-schedules"]).unwrap_err(), "--for-schedules goes with --server.");
        assert_eq!(parsed(&["--server", "--for-schedules", "--smoke-test", "--profile", root]).unwrap_err(), refusal);
    }

    /// Mac stage (plan §1.1): an app the 0.4.x updater relaunched with `open -n` may get LaunchServices' `-psn_…`; the Mac ignores it,
    /// Windows still refuses it.
    #[test]
    fn the_process_serial_number_is_ignored_only_on_the_mac() {
        assert_eq!(parsed(&["-psn_0_12345"]).is_ok(), cfg!(target_os = "macos"));
        assert!(parsed(&["-psn"]).is_err());
    }

    /// W9 review L9: the DevTools port opens only in a development build or a smoke run.
    #[test]
    fn the_debugging_port_is_a_development_or_smoke_switch() {
        assert!(debugging_port_allowed(true, false));
        assert!(debugging_port_allowed(false, true));
        assert!(!debugging_port_allowed(false, false), "a release app opens no DevTools port");
    }

    /// W9-H: the help capture and the perf scenario seed their own profile, so it must be new or empty (C# Program.cs:49-50); a plain
    /// smoke still runs on a seeded one (smoke.ps1).
    #[test]
    fn the_help_capture_and_the_perf_scenario_need_an_empty_profile() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-args-empty-{}-{nanos}", std::process::id()));
        let missing = root.join("new");
        let empty = root.join("empty");
        let seeded = root.join("seeded");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&seeded).unwrap();
        std::fs::write(seeded.join("workspace-state.json"), "{}").unwrap();
        let text = |path: &std::path::Path| path.to_string_lossy().into_owned();
        for folder in [&missing, &empty] {
            assert!(parsed(&["--help-capture", "--profile", &text(folder)]).is_ok());
            assert!(parsed(&["--smoke-test", "--perf-scenario", "--profile", &text(folder)]).is_ok());
        }
        let refusal = "The smoke profile must be a new or empty folder.";
        assert_eq!(parsed(&["--help-capture", "--profile", &text(&seeded)]).unwrap_err(), refusal);
        assert_eq!(parsed(&["--smoke-test", "--perf-scenario", "--profile", &text(&seeded)]).unwrap_err(), refusal);
        assert!(parsed(&["--smoke-test", "--profile", &text(&seeded)]).is_ok(), "a plain smoke keeps its seeded profile");
        let _ = std::fs::remove_dir_all(&root);
    }
}

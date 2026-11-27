//! The `mighty` CLI's arguments (MC-9): read into one request of the control API, and its exit codes (0 ok, 1 error, 2 timeout).
//! Every command prints JSON; `events` prints one event per line.

use std::path::PathBuf;
use std::time::Duration;

use super::events::{ControlEvent, WaitUntil};
use super::protocol::{Command, DEFAULT_READ_LINES, ErrorCode, MAX_READ_LINES, MAX_WAIT};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_TIMEOUT: i32 = 2;

/// The words that make `mighty` a control command rather than a folder or a request to open.
pub const SUBCOMMANDS: [&str; 9] = ["status", "workspace", "pane", "agent", "events", "server", "bot", "event", "loop"];

pub const USAGE: &str = "\
usage: mighty <command> [options]

  mighty status [--json]                          workspaces and panes (state, attention, worktree, ports)
  mighty workspace list
  mighty pane list [--workspace <id>]
  mighty pane read <pane> [--lines N]             recent transcript, secrets redacted
  mighty pane wait-output <pane> --regex <re> [--timeout S]
  mighty agent send <pane> <text…>                as if typed in the pane's composer (- reads stdin)
  mighty agent new --workspace <id> [--agent <name>] [--worktree]
  mighty agent wait <pane> --until idle|blocked|done [--timeout S]
  mighty events [--pane <id>] [--type pane.state|attention|run.completed]…
  mighty server start                             the app without a window (its phone link, schedules and this API)
  mighty server status                            headless or windowed, version, pid, uptime
  mighty server show                              open the window of the running app
  mighty server stop [--stop-all]                 quit it (--stop-all also ends the background daemon's work)
  mighty server ssh-pair                          a one-time code for another computer's Mighty Code over SSH
  mighty bot list                                 the bots, their schedules, triggers and next run
  mighty bot new --spec <file|->                  a bot from a JSON spec, once allowed in the app (bot update: change one)
  mighty bot pause|resume <@bot>                  put a bot to sleep, or wake it
  mighty bot run <@bot> [--job <name>]            run a bot's schedule or trigger now
  mighty event emit <name> [--data <json>]        wake the triggers listening for an event (release.done)
  mighty loop start <pane> --goal <text> --check <name=command>… [--max-iterations N] [--max-minutes M] [--max-cost USD]
                                                  drive a pane until every check passes (Loops (beta) on)
  mighty loop status [<id>]                       the loops, or one
  mighty loop stop <id>                           stop a loop

  --profile <folder>   the app's data folder when it runs with --profile
  Output is JSON. Exit codes: 0 ok, 1 error, 2 timeout.
  mighty [folder] [request]   opens a folder in Mighty Code, as before (macOS).";

/// What the arguments asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    /// The usage text (`help`, `--help`, `-h`).
    Help,
    /// One request; `stdin_text` when `agent send`'s text is `-` (read from standard input by the caller).
    Run { command: Command, profile: Option<PathBuf>, stdin_text: bool },
    /// `server start`: the app started without a window (`--server`), unless one already runs for the profile.
    ServerStart { profile: Option<PathBuf> },
}

/// A usage error: its message, printed with the usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

fn usage(message: impl Into<String>) -> UsageError {
    UsageError(message.into())
}

fn seconds(text: &str) -> Result<Duration, UsageError> {
    let value: f64 = text.parse().map_err(|_| usage(format!("--timeout takes seconds, not '{text}'")))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(usage("--timeout must be more than 0 seconds"));
    }
    Ok(Duration::from_secs_f64(value.min(MAX_WAIT.as_secs_f64())))
}

/// Reads the CLI's arguments (after the program's own name and its `--cli`).
pub fn parse(args: &[String]) -> Result<Invocation, UsageError> {
    let mut profile = None;
    let mut words: Vec<&str> = Vec::new();
    let mut options: Vec<(&str, Option<&str>)> = Vec::new();
    let mut at = 0;
    let takes_value = |name: &str| {
        matches!(
            name,
            "--profile"
                | "--workspace"
                | "--agent"
                | "--lines"
                | "--until"
                | "--timeout"
                | "--regex"
                | "--pane"
                | "--type"
                | "--spec"
                | "--job"
                | "--data"
                | "--goal"
                | "--check"
                | "--max-iterations"
                | "--max-minutes"
                | "--max-cost"
        )
    };
    let mut after_dashes = false;
    while at < args.len() {
        let arg = args[at].as_str();
        at += 1;
        if after_dashes {
            words.push(arg);
            continue;
        }
        match arg {
            "--" => after_dashes = true,
            "-h" | "--help" | "help" if words.is_empty() => return Ok(Invocation::Help),
            "--json" => {}
            name if name.starts_with("--") => {
                let (name, inline) = match name.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (name, None),
                };
                if takes_value(name) {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            let value = args.get(at).map(String::as_str).ok_or_else(|| usage(format!("{name} needs a value")))?;
                            at += 1;
                            value
                        }
                    };
                    if name == "--profile" {
                        profile = Some(PathBuf::from(value));
                    } else {
                        options.push((name, Some(value)));
                    }
                } else if name == "--worktree" || name == "--stop-all" {
                    options.push((name, None));
                } else {
                    return Err(usage(format!("unknown option {name}")));
                }
            }
            word => words.push(word),
        }
    }
    let option = |name: &str| options.iter().rev().find(|(n, _)| *n == name).and_then(|(_, v)| *v);
    let flag = |name: &str| options.iter().any(|(n, _)| *n == name);
    let all = |name: &str| options.iter().filter(|(n, _)| *n == name).filter_map(|(_, v)| *v).collect::<Vec<_>>();
    let timeout = || option("--timeout").map(seconds).transpose();
    let pane_at = |index: usize| words.get(index).map(|w| w.to_string()).ok_or_else(|| usage("which pane? give its id (mighty pane list)"));
    let allowed = |names: &[&str]| -> Result<(), UsageError> {
        match options.iter().find(|(n, _)| !names.contains(n)) {
            Some((name, _)) => Err(usage(format!("{name} does not go with this command"))),
            None => Ok(()),
        }
    };
    let extra = |count: usize| -> Result<(), UsageError> {
        if words.len() > count { Err(usage(format!("unexpected '{}'", words[count]))) } else { Ok(()) }
    };
    let mut stdin_text = false;
    let command = match words.as_slice() {
        [] => return Ok(Invocation::Help),
        ["status", ..] => {
            allowed(&[])?;
            extra(1)?;
            Command::Status
        }
        ["workspace", "list", ..] | ["workspace", "ls", ..] => {
            allowed(&[])?;
            extra(2)?;
            Command::WorkspaceList
        }
        ["pane", "list", ..] | ["pane", "ls", ..] => {
            allowed(&["--workspace"])?;
            extra(2)?;
            Command::PaneList { workspace: option("--workspace").map(str::to_string) }
        }
        ["pane", "read", ..] => {
            allowed(&["--lines"])?;
            extra(3)?;
            let lines = match option("--lines") {
                Some(text) => text.parse::<usize>().map_err(|_| usage(format!("--lines takes a number, not '{text}'")))?,
                None => DEFAULT_READ_LINES,
            };
            Command::PaneRead { pane: pane_at(2)?, lines: lines.clamp(1, MAX_READ_LINES) }
        }
        ["pane", "wait-output", ..] => {
            allowed(&["--regex", "--timeout"])?;
            extra(3)?;
            let regex = option("--regex").ok_or_else(|| usage("--regex is needed"))?.to_string();
            Command::PaneWaitOutput { pane: pane_at(2)?, regex, timeout: timeout()? }
        }
        ["agent", "send", ..] => {
            allowed(&[])?;
            let pane = pane_at(2)?;
            let text = words[3.min(words.len())..].join(" ");
            if text.trim().is_empty() {
                return Err(usage("what to send? give the text, or - to read it from standard input"));
            }
            stdin_text = text == "-";
            Command::AgentSend { pane, text }
        }
        ["agent", "new", ..] => {
            allowed(&["--workspace", "--agent", "--worktree"])?;
            extra(2)?;
            let workspace = option("--workspace").ok_or_else(|| usage("--workspace is needed (mighty workspace list)"))?.to_string();
            let agent = option("--agent").unwrap_or("claude").to_string();
            Command::AgentNew { workspace, agent, worktree: flag("--worktree") }
        }
        ["agent", "wait", ..] => {
            allowed(&["--until", "--timeout"])?;
            extra(3)?;
            let until = option("--until").ok_or_else(|| usage("--until idle|blocked|done is needed"))?;
            let until = WaitUntil::parse(until).ok_or_else(|| usage("--until takes idle, blocked or done"))?;
            Command::AgentWait { pane: pane_at(2)?, until, timeout: timeout()? }
        }
        ["events", ..] => {
            allowed(&["--pane", "--type"])?;
            extra(1)?;
            let types: Vec<String> = all("--type").iter().flat_map(|t| t.split(',')).map(str::to_string).collect();
            if let Some(unknown) = types.iter().find(|t| !ControlEvent::TYPES.contains(&t.as_str())) {
                return Err(usage(format!("unknown event type '{unknown}'")));
            }
            Command::Events { pane: option("--pane").map(str::to_string), types }
        }
        // MC-65: bots; `--spec`'s file is read by the caller (its path, or `-` for standard input, is in `spec` until then).
        ["bot", "list", ..] | ["bot", "ls", ..] => {
            allowed(&[])?;
            extra(2)?;
            Command::BotList
        }
        ["bot", verb @ ("new" | "update"), ..] => {
            allowed(&["--spec"])?;
            extra(2)?;
            let spec = option("--spec").ok_or_else(|| usage("--spec <file> is needed (or - to read it from standard input)"))?;
            Command::BotNew { spec: spec.to_string(), update: *verb == "update" }
        }
        ["bot", verb @ ("pause" | "resume" | "sleep" | "wake"), ..] => {
            allowed(&[])?;
            extra(3)?;
            let bot = words.get(2).ok_or_else(|| usage("which bot? give its @name (mighty bot list)"))?;
            let bot = crate::model::PaneBot::handle_of(bot).ok_or_else(|| usage(format!("'{bot}' is not a bot's name")))?;
            Command::BotSleep { bot: format!("@{bot}"), asleep: matches!(*verb, "pause" | "sleep") }
        }
        ["bot", "run", ..] => {
            allowed(&["--job"])?;
            extra(3)?;
            let bot = words.get(2).ok_or_else(|| usage("which bot? give its @name (mighty bot list)"))?;
            let bot = crate::model::PaneBot::handle_of(bot).ok_or_else(|| usage(format!("'{bot}' is not a bot's name")))?;
            Command::BotRun { bot: format!("@{bot}"), job: option("--job").map(str::to_string) }
        }
        // MC-66
        ["event", "emit", ..] => {
            allowed(&["--data"])?;
            extra(3)?;
            let name = words.get(2).ok_or_else(|| usage("which event? e.g. mighty event emit release.done"))?;
            if !crate::automation::spec::valid_event_name(name, false) {
                return Err(usage(format!("'{name}' is not an event name: lowercase letters, digits, . _ -")));
            }
            let data = option("--data").map(str::to_string);
            if let Some(data) = &data
                && serde_json::from_str::<serde_json::Value>(data).is_err()
            {
                return Err(usage("--data takes JSON, e.g. --data '{\"version\":\"0.6.3\"}'"));
            }
            Command::EventEmit { name: name.to_string(), data }
        }
        // MC-118
        ["loop", "start", ..] => {
            allowed(&["--goal", "--check", "--max-iterations", "--max-minutes", "--max-cost"])?;
            extra(3)?;
            let pane = pane_at(2)?;
            let goal = option("--goal").filter(|g| !g.trim().is_empty()).ok_or_else(|| usage("--goal <text> is needed"))?;
            let mut checks = Vec::new();
            for check in all("--check") {
                let (name, command) = check
                    .split_once('=')
                    .map(|(n, c)| (n.trim(), c.trim()))
                    .filter(|(n, c)| !n.is_empty() && !c.is_empty())
                    .ok_or_else(|| usage(format!("--check takes name=command, not '{check}'")))?;
                checks.push((name.to_string(), command.to_string()));
            }
            if checks.is_empty() {
                return Err(usage("at least one --check name=command is needed"));
            }
            let whole = |name: &str| -> Result<Option<u32>, UsageError> {
                option(name).map(|v| v.parse::<u32>().map_err(|_| usage(format!("{name} takes a whole number, not '{v}'")))).transpose()
            };
            let cost = option("--max-cost")
                .map(|v| {
                    v.parse::<f64>()
                        .ok()
                        .filter(|c| c.is_finite() && *c > 0.0)
                        .ok_or_else(|| usage(format!("--max-cost takes dollars, not '{v}'")))
                })
                .transpose()?;
            Command::LoopStart {
                pane,
                goal: goal.to_string(),
                checks,
                max_iterations: whole("--max-iterations")?,
                max_minutes: whole("--max-minutes")?,
                max_cost_usd: cost,
            }
        }
        ["loop", "status", ..] | ["loop", "list", ..] => {
            allowed(&[])?;
            extra(3)?;
            Command::LoopStatus { id: words.get(2).map(|w| w.to_string()) }
        }
        ["loop", "stop", ..] => {
            allowed(&[])?;
            extra(3)?;
            Command::LoopStop {
                id: words.get(2).map(|w| w.to_string()).ok_or_else(|| usage("which loop? give its id (mighty loop status)"))?,
            }
        }
        ["server", "start", ..] => {
            allowed(&[])?;
            extra(2)?;
            return Ok(Invocation::ServerStart { profile });
        }
        ["server", "status", ..] => {
            allowed(&[])?;
            extra(2)?;
            Command::ServerStatus
        }
        ["server", "show", ..] => {
            allowed(&[])?;
            extra(2)?;
            Command::AppShow
        }
        ["server", "stop", ..] => {
            allowed(&["--stop-all"])?;
            extra(2)?;
            Command::ServerStop { stop_all: flag("--stop-all") }
        }
        ["server", "ssh-pair", ..] => {
            allowed(&[])?;
            extra(2)?;
            Command::ServerSshPair
        }
        ["server"] => return Err(usage("server takes start, status, show or stop")),
        ["server", other, ..] => return Err(usage(format!("unknown server command '{other}': use start, status, show or stop"))),
        [first, ..] => return Err(usage(format!("unknown command '{first}'"))),
    };
    Ok(Invocation::Run { command, profile, stdin_text })
}

/// The exit code of a failed request.
pub fn exit_code(code: ErrorCode) -> i32 {
    if code == ErrorCode::Timeout { EXIT_TIMEOUT } else { EXIT_ERROR }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Invocation, UsageError> {
        parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    fn command(args: &[&str]) -> Command {
        match run(args).unwrap() {
            Invocation::Run { command, .. } => command,
            Invocation::Help | Invocation::ServerStart { .. } => panic!("not a request"),
        }
    }

    /// MC-65/66: the bot and event commands.
    #[test]
    fn bot_and_event_commands_read_into_their_requests() {
        assert_eq!(command(&["bot", "list"]), Command::BotList);
        assert_eq!(command(&["bot", "new", "--spec", "bot.json"]), Command::BotNew { spec: "bot.json".into(), update: false });
        assert_eq!(command(&["bot", "update", "--spec", "-"]), Command::BotNew { spec: "-".into(), update: true });
        assert_eq!(command(&["bot", "pause", "콩이"]), Command::BotSleep { bot: "@콩이".into(), asleep: true });
        assert_eq!(command(&["bot", "resume", "@콩이"]), Command::BotSleep { bot: "@콩이".into(), asleep: false });
        assert_eq!(command(&["bot", "run", "@콩이", "--job", "아침"]), Command::BotRun { bot: "@콩이".into(), job: Some("아침".into()) });
        assert_eq!(
            command(&["event", "emit", "release.done", "--data", r#"{"version":"0.6.3"}"#]),
            Command::EventEmit { name: "release.done".into(), data: Some(r#"{"version":"0.6.3"}"#.into()) }
        );
        let error = |args: &[&str]| run(args).unwrap_err().0;
        assert!(error(&["bot", "new"]).contains("--spec"));
        assert!(error(&["bot", "pause", "two words"]).contains("not a bot's name"));
        assert!(error(&["event", "emit", "Release!"]).contains("not an event name"));
        assert!(error(&["event", "emit", "release.done", "--data", "{oops"]).contains("JSON"));
        assert!(SUBCOMMANDS.contains(&"bot") && SUBCOMMANDS.contains(&"event") && USAGE.contains("mighty event emit"));
    }

    /// MC-118: the loop commands.
    #[test]
    fn loop_commands_read_into_their_requests() {
        assert_eq!(
            command(&[
                "loop",
                "start",
                "p1",
                "--goal",
                "CI 초록불까지",
                "--check",
                "test=npm test",
                "--check=lint=npm run lint",
                "--max-cost",
                "2.5"
            ]),
            Command::LoopStart {
                pane: "p1".into(),
                goal: "CI 초록불까지".into(),
                checks: vec![("test".into(), "npm test".into()), ("lint".into(), "npm run lint".into())],
                max_iterations: None,
                max_minutes: None,
                max_cost_usd: Some(2.5),
            }
        );
        assert_eq!(command(&["loop", "status"]), Command::LoopStatus { id: None });
        assert_eq!(command(&["loop", "status", "loop-1"]), Command::LoopStatus { id: Some("loop-1".into()) });
        assert_eq!(command(&["loop", "stop", "loop-1"]), Command::LoopStop { id: "loop-1".into() });
        let error = |args: &[&str]| run(args).unwrap_err().0;
        assert!(error(&["loop", "start", "p1", "--check", "t=x"]).contains("--goal"));
        assert!(error(&["loop", "start", "p1", "--goal", "g"]).contains("--check"));
        assert!(error(&["loop", "start", "p1", "--goal", "g", "--check", "npm test"]).contains("name=command"));
        assert!(error(&["loop", "start", "p1", "--goal", "g", "--check", "t=x", "--max-iterations", "many"]).contains("whole number"));
        assert!(error(&["loop", "stop"]).contains("which loop"));
        assert!(SUBCOMMANDS.contains(&"loop") && USAGE.contains("mighty loop start"));
    }

    #[test]
    fn each_command_reads_into_its_request() {
        assert_eq!(command(&["status", "--json"]), Command::Status);
        assert_eq!(command(&["workspace", "list"]), Command::WorkspaceList);
        assert_eq!(command(&["pane", "list", "--workspace", "w1"]), Command::PaneList { workspace: Some("w1".into()) });
        assert_eq!(command(&["pane", "read", "p1"]), Command::PaneRead { pane: "p1".into(), lines: DEFAULT_READ_LINES });
        assert_eq!(command(&["pane", "read", "p1", "--lines=5"]), Command::PaneRead { pane: "p1".into(), lines: 5 });
        assert_eq!(
            command(&["agent", "send", "p1", "run", "the", "tests"]),
            Command::AgentSend { pane: "p1".into(), text: "run the tests".into() }
        );
        assert_eq!(
            command(&["agent", "send", "p1", "--", "--not-an-option"]),
            Command::AgentSend { pane: "p1".into(), text: "--not-an-option".into() }
        );
        assert_eq!(
            command(&["agent", "new", "--workspace", "w1", "--agent", "codex", "--worktree"]),
            Command::AgentNew { workspace: "w1".into(), agent: "codex".into(), worktree: true }
        );
        assert_eq!(
            command(&["agent", "new", "--workspace", "w1"]),
            Command::AgentNew { workspace: "w1".into(), agent: "claude".into(), worktree: false }
        );
        assert_eq!(
            command(&["agent", "wait", "p1", "--until", "blocked", "--timeout", "1.5"]),
            Command::AgentWait { pane: "p1".into(), until: WaitUntil::Blocked, timeout: Some(Duration::from_millis(1500)) }
        );
        assert_eq!(
            command(&["pane", "wait-output", "p1", "--regex", "^ok$"]),
            Command::PaneWaitOutput { pane: "p1".into(), regex: "^ok$".into(), timeout: None }
        );
        assert_eq!(
            command(&["events", "--pane", "p1", "--type", "pane.state,attention"]),
            Command::Events { pane: Some("p1".into()), types: vec!["pane.state".into(), "attention".into()] }
        );
        let Ok(Invocation::Run { profile, stdin_text, .. }) = run(&["--profile", "/tmp/p", "agent", "send", "p1", "-"]) else { panic!() };
        assert_eq!(profile, Some(PathBuf::from("/tmp/p")));
        assert!(stdin_text);
        assert_eq!(run(&["--help"]).unwrap(), Invocation::Help);
        assert_eq!(run(&[]).unwrap(), Invocation::Help);
    }

    /// MC-3's headless server: `server start` is the CLI's own (it starts the app), the others are requests to the running app.
    #[test]
    fn the_server_commands_read() {
        assert_eq!(run(&["server", "start"]).unwrap(), Invocation::ServerStart { profile: None });
        assert_eq!(
            run(&["--profile", "/tmp/p", "server", "start"]).unwrap(),
            Invocation::ServerStart { profile: Some(PathBuf::from("/tmp/p")) }
        );
        assert_eq!(
            run(&["server", "start", "--profile=/tmp/q"]).unwrap(),
            Invocation::ServerStart { profile: Some(PathBuf::from("/tmp/q")) }
        );
        assert_eq!(command(&["server", "status", "--json"]), Command::ServerStatus);
        assert_eq!(command(&["server", "show"]), Command::AppShow);
        assert_eq!(command(&["server", "stop"]), Command::ServerStop { stop_all: false });
        assert_eq!(command(&["server", "stop", "--stop-all"]), Command::ServerStop { stop_all: true });
        assert_eq!(command(&["server", "ssh-pair"]), Command::ServerSshPair);
        let error = |args: &[&str]| run(args).unwrap_err().0;
        assert_eq!(error(&["server"]), "server takes start, status, show or stop");
        assert_eq!(error(&["server", "restart"]), "unknown server command 'restart': use start, status, show or stop");
        assert_eq!(error(&["server", "start", "--stop-all"]), "--stop-all does not go with this command");
        assert_eq!(error(&["server", "status", "now"]), "unexpected 'now'");
        assert_eq!(error(&["agent", "new", "--workspace", "w", "--stop-all"]), "--stop-all does not go with this command");
        assert!(SUBCOMMANDS.contains(&"server") && USAGE.contains("mighty server start"));
    }

    #[test]
    fn usage_errors_say_what_is_wrong() {
        let error = |args: &[&str]| run(args).unwrap_err().0;
        assert_eq!(error(&["dance"]), "unknown command 'dance'");
        assert_eq!(error(&["pane", "read"]), "which pane? give its id (mighty pane list)");
        assert_eq!(error(&["agent", "wait", "p1"]), "--until idle|blocked|done is needed");
        assert_eq!(error(&["agent", "wait", "p1", "--until", "later"]), "--until takes idle, blocked or done");
        assert_eq!(error(&["agent", "wait", "p1", "--until", "idle", "--timeout", "0"]), "--timeout must be more than 0 seconds");
        assert_eq!(error(&["agent", "send", "p1"]), "what to send? give the text, or - to read it from standard input");
        assert_eq!(error(&["agent", "new"]), "--workspace is needed (mighty workspace list)");
        assert_eq!(error(&["status", "--lines", "3"]), "--lines does not go with this command");
        assert_eq!(error(&["status", "--frobnicate"]), "unknown option --frobnicate");
        assert_eq!(error(&["events", "--type", "secrets"]), "unknown event type 'secrets'");
        assert_eq!(error(&["pane", "read", "p1", "p2"]), "unexpected 'p2'");
    }

    #[test]
    fn exit_codes_tell_a_timeout_from_an_error() {
        assert_eq!(exit_code(ErrorCode::Timeout), EXIT_TIMEOUT);
        assert_eq!(exit_code(ErrorCode::NotFound), EXIT_ERROR);
        assert_eq!(exit_code(ErrorCode::Unauthorized), EXIT_ERROR);
        assert_eq!((EXIT_OK, EXIT_ERROR, EXIT_TIMEOUT), (0, 1, 2));
    }
}

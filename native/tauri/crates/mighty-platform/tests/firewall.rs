//! C#'s `WindowsFirewallVerification` (Tests\WindowsFirewallVerification.cs; plan W8-C, L495-L497), ported by name: the one command the
//! app ever asks Windows to elevate, built from constants, the app's own path and validated ports. Nothing here runs netsh: the runner
//! is given a recorder (`fake::RecordingNetsh`) in place of a process start, so these run on both OSes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::fake::RecordingNetsh;
use mighty_platform::firewall::netsh::{self, NetshCommand};
use mighty_platform::firewall::{FirewallChange, FirewallRule, FirewallRuleSpec, NetshFailure, NetshRunner};
use mighty_platform::{Firewall, PlatformError};

const PROGRAM: &str = r"C:\Program Files\Mighty Code\MightyCode.exe";
const SYSTEM32: &str = r"C:\Windows\System32";
const RANGES: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn visible(text: &str) -> String {
    text.chars().map(|c| if (' '..'\u{7f}').contains(&c) { c.to_string() } else { format!("\\u{:04X}", c as u32) }).collect()
}

/// "windows firewall: the rule is built from constants" (L495).
#[test]
fn windows_firewall_the_rule_is_built_from_constants() {
    let add = netsh::add_arguments(PROGRAM, &[43139]).expect("the add rule's argv");
    let program = format!("program={PROGRAM}");
    let (localip, remoteip) = (format!("localip={RANGES}"), format!("remoteip={RANGES}"));
    assert_eq!(
        add,
        strings(&[
            "advfirewall",
            "firewall",
            "add",
            "rule",
            "name=Mighty Code (Tailscale)",
            "dir=in",
            "action=allow",
            "protocol=TCP",
            "localport=43139",
            &localip,
            &remoteip,
            &program,
            "profile=any"
        ]),
        "the add rule's argv"
    );
    assert_eq!(
        netsh::command_text(&add),
        "advfirewall firewall add rule name=\"Mighty Code (Tailscale)\" dir=in action=allow protocol=TCP localport=43139 localip=100.64.0.0/10,fd7a:115c:a1e0::/48 remoteip=100.64.0.0/10,fd7a:115c:a1e0::/48 program=\"C:\\Program Files\\Mighty Code\\MightyCode.exe\" profile=any",
        "netsh's own spelling: key=\"value\" around the name and the path"
    );
    assert!(
        netsh::command_text(&netsh::add_arguments(r"C:\Mighty\MightyCode.exe", &[43139]).unwrap())
            .contains(" program=\"C:\\Mighty\\MightyCode.exe\" "),
        "the path is quoted even without a space"
    );
    assert_eq!(netsh::add_arguments(PROGRAM, &[43139, 43137, 43139]).unwrap()[8], "localport=43137,43139", "a port set: sorted, each once");
    let set = netsh::set_arguments(PROGRAM, &[43139, 43137]).expect("the update-in-place argv");
    assert_eq!(
        set,
        strings(&[
            "advfirewall",
            "firewall",
            "set",
            "rule",
            "name=Mighty Code (Tailscale)",
            "new",
            "dir=in",
            "action=allow",
            "protocol=TCP",
            "localport=43137,43139",
            &localip,
            &remoteip,
            &program,
            "profile=any",
            "enable=yes"
        ]),
        "the update-in-place argv"
    );
    assert!(
        netsh::set_arguments("MightyCode.exe", &[43139]).is_none() && netsh::set_arguments(PROGRAM, &[]).is_none(),
        "refused like the add"
    );
    assert_eq!(netsh::delete_arguments(), strings(&["advfirewall", "firewall", "delete", "rule", "name=Mighty Code (Tailscale)"]));
    assert_eq!(netsh::show_arguments(), strings(&["advfirewall", "firewall", "show", "rule", "name=Mighty Code (Tailscale)"]));
    assert!(
        netsh::arguments(&[43139], PROGRAM).unwrap() == add && netsh::arguments(&[], PROGRAM).unwrap() == netsh::delete_arguments(),
        "a port set adds; none deletes"
    );
    for bad in [
        "",
        "MightyCode.exe",
        "C:\\Mighty\\Mighty\"Claude.exe",
        "C:\\Mighty\\Mighty\nClaude.exe",
        r"C:\Mighty\%TEMP%\MightyCode.exe",
        r"C:\Mighty\MightyCode.bat",
        r"\\server\share\MightyCode.exe",
        r"C:\Mighty\..\MightyCode.exe",
        r"C:\Mighty\x.exe:stream.exe",
        "C:/Mighty/MightyCode.exe",
        r"C:\Mighty\\MightyCode.exe",
        "C:\\Mighty\\MightyCode.exe\n",
        "C:\\Mi\u{7f}ghty\\MightyCode.exe",
        "C:\\Mighty\u{202E}\\MightyCode.exe",
        "C:\\Mighty\\.\\MightyCode.exe",
        "C:\\\u{FEFF}Mighty\\MightyCode.exe",
        "C:\\Mighty\u{A0}Claude\\MightyCode.exe",
        "C:\\Mighty\tClaude\\MightyCode.exe",
        "C:\\Mighty\u{85}\\MightyCode.exe",
        r"C:\Program Files\dotnet\dotnet.exe",
        r"C:\Mighty\NotMightyCode.exe",
        r"C:\Mighty\MightyCode.exe ",
    ] {
        assert!(
            netsh::add_arguments(bad, &[43139]).is_none() && netsh::valid_program(Some(bad)).is_none(),
            "a refused program path: {}",
            visible(bad)
        );
    }
    assert!(netsh::valid_program(Some(r"d:\apps\mightycode.EXE")).is_some(), "the exe name is matched without case");
    let seventeen: Vec<i32> = (43100..43117).collect();
    for ports in [vec![], vec![1023], vec![65536], vec![0], vec![43139, -1], seventeen] {
        assert!(netsh::add_arguments(PROGRAM, &ports).is_none(), "refused ports: {ports:?}");
    }
    let mut sixteen: Vec<i32> = (43100..43116).collect();
    sixteen.push(43100);
    assert!(
        netsh::add_arguments(PROGRAM, &[1024, 65535]).is_some() && netsh::add_arguments(PROGRAM, &sixteen).is_some(),
        "1024 and 65535 are allowed, and 16 ports"
    );
    assert!(
        netsh::valid_system_directory(Some(SYSTEM32))
            && !netsh::valid_system_directory(Some(""))
            && !netsh::valid_system_directory(None)
            && !netsh::valid_system_directory(Some("System32"))
            && !netsh::valid_system_directory(Some("C:\\Win\"dows"))
            && !netsh::valid_system_directory(Some(&format!("{SYSTEM32}\n")))
            && !netsh::valid_system_directory(Some("C:\\Win\u{202E}dows")),
        "the system folder is an absolute drive path"
    );
    let query = netsh::query_launch(SYSTEM32);
    assert!(
        query.exe == format!("{SYSTEM32}\\netsh.exe")
            && !query.elevated
            && query.command_line == netsh::command_text(&netsh::show_arguments())
            && query.arguments == netsh::show_arguments()
            && query.working_directory == SYSTEM32,
        "the query runs netsh from the system folder, hidden and unelevated"
    );
    let elevated = netsh::elevated_launch(SYSTEM32, add.clone());
    assert!(
        elevated.exe == format!("{SYSTEM32}\\netsh.exe")
            && elevated.elevated
            && elevated.command_line == netsh::command_text(&add)
            && elevated.working_directory == SYSTEM32,
        "a change asks Windows to elevate netsh from the system folder"
    );
    // Step 0's builder: the same lines, every value validated.
    let (exe, built) = netsh::command_line(SYSTEM32, &NetshCommand::Add { ports: vec![43139], program: PROGRAM.into() }).unwrap();
    assert!(exe == query.exe && built == add, "command_line builds the add");
    assert!(matches!(netsh::command_line("System32", &NetshCommand::Show), Err(PlatformError::Invalid(_))), "a relative folder is refused");
    assert!(
        netsh::command_line(SYSTEM32, &NetshCommand::Set { ports: vec![80], program: PROGRAM.into() }).is_err(),
        "a low port is refused"
    );
    // The shown rule's ports: every "label: value" line holding a plain port list, in any display language.
    let shown = "Rule Name:      Mighty Code (Tailscale)\r\nLocalPort:      43137,43139\r\nRemotePort:     Any\r\n";
    assert_eq!(netsh::shown_ports(Some(shown)), Some(vec![43137, 43139]));
    assert_eq!(netsh::shown_ports(Some("규칙 이름: x\n로컬 포트: 43139\n")), Some(vec![43139]));
    assert_eq!(netsh::shown_ports(Some("LocalPort: 43000-43200\n")), None, "a range is unknown");
    assert_eq!(netsh::shown_ports(Some("LocalPort: 0\n")), None, "port 0 makes the reading unknown");
    assert_eq!(netsh::shown_ports(Some("")), None);
    assert_eq!(netsh::shown_ports(None), None);
}

/// "windows firewall: the runner elevates only that command" (L496), over the recorder.
#[tokio::test]
async fn windows_firewall_the_runner_elevates_only_that_command() {
    let recorder = Arc::new(RecordingNetsh::default());
    let elevations = || recorder.launched().into_iter().filter(|l| l.elevated).collect::<Vec<_>>();
    let runner = NetshRunner::new(recorder.clone(), Some(SYSTEM32.into()), Some(PROGRAM.into()), Some(SYSTEM32.into()));

    recorder.answer(&[1]);
    assert_eq!(runner.read_rule().await, FirewallRule::Missing, "exit 1: no such rule");
    recorder.answer(&[0]);
    assert_eq!(runner.read_rule().await, FirewallRule::Present { ports: None }, "exit 0: a rule by that name is there");
    recorder.answer(&[2]);
    assert_eq!(runner.read_rule().await, FirewallRule::Unreadable, "any other exit: unknown");
    assert!(recorder.launched().iter().all(|l| !l.elevated), "reading the rule never elevates");

    recorder.answer(&[1, 0]);
    let added = runner.add_rule(&[43139]).await;
    let launched = recorder.launched();
    assert!(
        added == FirewallChange::Applied(Some(FirewallRuleSpec { ports: "43139".into(), program: PROGRAM.into() }))
            && launched.len() == 2
            && !launched[0].elevated
            && elevations().len() == 1
            && elevations()[0].command_line == netsh::command_text(&netsh::add_arguments(PROGRAM, &[43139]).unwrap()),
        "no rule yet: read, then one elevated add; the result says what was written"
    );
    recorder.answer(&[0, 0]);
    let applied = runner.apply(vec![43139, 43137]).await.unwrap();
    assert!(
        matches!(&applied, FirewallChange::Applied(Some(spec)) if spec.ports == "43137,43139" && spec.program == PROGRAM)
            && elevations().len() == 1
            && elevations()[0].command_line == netsh::command_text(&netsh::set_arguments(PROGRAM, &[43137, 43139]).unwrap()),
        "a rule by the name already there: one elevated set, so rules never pile up and a dropped port closes"
    );
    recorder.answer(&[1, 0]);
    assert!(
        matches!(runner.apply(vec![43139]).await.unwrap(), FirewallChange::Applied(_))
            && elevations()[0].command_line.starts_with("advfirewall firewall add rule "),
        "apply adds when there is none"
    );
    recorder.answer(&[2]);
    assert!(
        runner.add_rule(&[43139]).await == FirewallChange::Failed("queryFailed".into()) && elevations().is_empty(),
        "a rule that cannot be read is not added blindly"
    );
    recorder.answer(&[0]);
    assert!(
        runner.remove_rule().await == FirewallChange::Applied(None)
            && recorder.launched().len() == 1
            && recorder.launched()[0].command_line == netsh::command_text(&netsh::delete_arguments()),
        "remove elevates the delete"
    );
    recorder.answer(&[0]);
    assert!(
        runner.apply(vec![]).await.unwrap() == FirewallChange::Applied(None)
            && recorder.launched()[0].command_line == netsh::command_text(&netsh::delete_arguments()),
        "an empty port set removes the rule"
    );

    recorder.fail_elevated(Some(NetshFailure::Cancelled));
    recorder.answer(&[1]);
    assert_eq!(runner.add_rule(&[43139]).await, FirewallChange::Declined, "a declined prompt is a cancel, not an error");
    recorder.fail_elevated(Some(NetshFailure::Failed("Access is denied.".into())));
    recorder.answer(&[1]);
    assert!(matches!(runner.add_rule(&[43139]).await, FirewallChange::Failed(_)), "any other launch error is a failure");
    recorder.fail_elevated(Some(NetshFailure::TimedOut));
    recorder.answer(&[1]);
    assert!(
        runner.add_rule(&[43139]).await == FirewallChange::Failed("timeout".into())
            && runner.remove_rule().await == FirewallChange::Failed("timeout".into()),
        "an elevated netsh that does not finish is a timeout"
    );
    recorder.fail_elevated(None);
    recorder.fail_query(Some(NetshFailure::Failed("no netsh".into())));
    assert_eq!(runner.read_rule().await, FirewallRule::Unreadable, "a query that cannot run is unknown");
    recorder.fail_query(Some(NetshFailure::TimedOut));
    assert_eq!(runner.read_rule().await, FirewallRule::Unreadable, "a query that does not finish is unknown");
    recorder.fail_query(None);
    recorder.answer(&[1, 1]);
    assert_eq!(runner.add_rule(&[43139]).await, FirewallChange::Failed("1".into()), "netsh's exit code is kept");

    recorder.answer(&[]);
    let refused = |detail: &str| FirewallChange::Refused(detail.into());
    assert!(runner.add_rule(&[]).await == refused("noPorts") && recorder.launched().is_empty(), "no port, no command");
    assert!(runner.add(vec![]).await.unwrap() == refused("noPorts") && recorder.launched().is_empty(), "add through the port too");
    assert!(runner.add_rule(&[80]).await == refused("badPorts") && recorder.launched().is_empty(), "a port below 1024, no command");
    let seventeen: Vec<u16> = (43100..43117).collect();
    assert!(runner.apply(seventeen).await.unwrap() == refused("badPorts") && recorder.launched().is_empty(), "more than 16 ports");
    let with_program = |program: Option<&str>| {
        NetshRunner::new(recorder.clone(), Some(SYSTEM32.into()), program.map(str::to_string), Some(SYSTEM32.into()))
    };
    assert!(with_program(Some("MightyCode.exe")).add_rule(&[43139]).await == refused("badProgram"), "a relative program path");
    assert!(with_program(Some(r"C:\Program Files\dotnet\dotnet.exe")).add_rule(&[43139]).await == refused("badProgram"), "another host");
    assert!(with_program(None).apply(vec![43139]).await.unwrap() == refused("badProgram"), "no program path");
    assert!(recorder.launched().is_empty(), "no refused change ran anything");
    let homeless = NetshRunner::new(recorder.clone(), None, Some(PROGRAM.into()), Some(SYSTEM32.into()));
    assert!(
        homeless.read_rule().await == FirewallRule::Unreadable
            && homeless.add_rule(&[43139]).await == refused("noSystemDirectory")
            && homeless.remove_rule().await == refused("noSystemDirectory")
            && homeless.apply(vec![]).await.unwrap() == refused("noSystemDirectory")
            && recorder.launched().is_empty(),
        "no system folder, nothing runs"
    );
    let elsewhere = NetshRunner::new(recorder.clone(), Some(r"D:\Tools".into()), Some(PROGRAM.into()), Some(format!("{SYSTEM32}\\")));
    assert!(
        elsewhere.add_rule(&[43139]).await == refused("noSystemDirectory")
            && elsewhere.remove_rule().await == refused("noSystemDirectory")
            && recorder.launched().is_empty(),
        "a folder other than the one Windows reports: netsh never starts from it"
    );
    // A dropped call is the caller's cancel: it starts nothing it had not started, and nothing is turned into a result.
    let dropped = runner.add_rule(&[43139]);
    drop(dropped);
    assert!(recorder.launched().is_empty(), "a call dropped before it ran starts nothing");
}

fn tauri_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn sources(folder: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` holds `word` as a whole word (ASCII letters, digits and `_` around it break it), ignoring case.
fn word_positions(text: &str, word: &str) -> Vec<usize> {
    let lower = text.to_ascii_lowercase();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = lower.as_bytes();
    lower
        .match_indices(word)
        .map(|(at, _)| at)
        .filter(|at| (*at == 0 || !is_word(bytes[at - 1])) && bytes.get(at + word.len()).is_none_or(|b| !is_word(*b)))
        .collect()
}

/// "windows firewall: only the rule file elevates" (L497; D17): C# scanned its Core and WinUI sources for the verb and a set `Verb`; the
/// Rust form scans every crate's and the app's own sources (`crates/*/src`, `src-tauri/src`; tests are not sources). The elevation verb
/// is named only in mighty-platform-windows' firewall.rs, but where a check guards that a step does not elevate
/// (`.contains("<verb>")`); `ShellExecuteExW`'s verb is set only there and in files.rs, whose verb is "open".
#[test]
fn windows_firewall_only_the_rule_file_elevates() {
    let root = tauri_root();
    let mut files = vec![];
    sources(&root.join("src-tauri").join("src"), &mut files);
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        sources(&krate.path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "the scan found the sources");
    let verb = ["run", "as"].concat();
    let name = |file: &Path| file.strip_prefix(&root).unwrap_or(file).to_string_lossy().replace('\\', "/");
    let mut naming = vec![];
    let mut setting = vec![];
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let guarded = |at: usize| text[..at].ends_with("contains(\"");
        if word_positions(&text, &verb).into_iter().any(|at| !guarded(at)) {
            naming.push(name(file));
        }
        if text
            .match_indices("lpVerb")
            .any(|(at, _)| text[at + 6..].trim_start().starts_with('=') && !text[at + 6..].trim_start().starts_with("=="))
        {
            setting.push(name(file));
        }
    }
    assert_eq!(naming, vec!["crates/mighty-platform-windows/src/firewall.rs".to_string()], "the elevation verb is named only there");
    setting.sort();
    assert_eq!(
        setting,
        vec!["crates/mighty-platform-windows/src/files.rs".to_string(), "crates/mighty-platform-windows/src/firewall.rs".to_string()],
        "a shell verb is set only by the file opener and the rule"
    );
    let opener = std::fs::read_to_string(root.join("crates/mighty-platform-windows/src/files.rs")).unwrap();
    assert!(opener.contains("wide(OsStr::new(\"open\"))"), "the file opener's verb is open");
}

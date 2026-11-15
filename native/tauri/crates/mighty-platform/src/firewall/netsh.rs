//! The `netsh advfirewall` command lines of the one rule the app keeps (C# `WindowsFirewallRule`: the argv builder, its validators and
//! the reader of `show rule`'s ports; Core\WindowsFirewallRule.cs; plan §1.4): shared and pure, so the builder's checks run on both
//! OSes; the Windows crate runs them (`show` unelevated through `ProcessHost`, the change elevated; mighty-platform-windows
//! `firewall.rs`, the only file that may name the elevation verb). Owner: W8-C.
//!
//! The rule: inbound TCP on the app's own ports, only from and to Tailscale's ranges, only for the app's own executable —
//! `%SystemRoot%\System32\netsh.exe advfirewall firewall add rule name="Mighty Code (Tailscale)" dir=in action=allow protocol=TCP
//! localport=<ports> localip=<ranges> remoteip=<ranges> program="<exe>" profile=any` (`set rule … new … enable=yes` when the rule is
//! there), built from these constants, the validated exe path and the validated ports.

use crate::{PlatformError, Result};

/// `WindowsFirewallRule.RuleName`: shared with remote sharing.
pub const RULE_NAME: &str = "Mighty Code (Tailscale)";
/// The rule's `remoteip=`: Tailscale's ranges.
pub const REMOTE_RANGES: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
/// The rule's `localip=`: the same ranges, a backstop for `profile=any` so no other adapter's address is opened.
pub const LOCAL_RANGES: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
/// The only exe the rule may name, so a development run under another host never opens ports for every program of that host.
pub const PROGRAM_FILE_NAME: &str = "MightyCode.exe";
/// At most this many ports in one rule.
pub const MAX_PORTS: usize = 16;
/// `ERROR_CANCELLED`: the user declined the elevation prompt.
pub const ELEVATION_CANCELLED: i32 = 1223;

/// What one command does to the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetshCommand {
    /// `show rule name=…` (unelevated; exit 0 present, 1 absent, else unknown).
    Show,
    /// `add rule …` with these ports for this exe.
    Add { ports: Vec<u16>, program: String },
    /// `set rule name=… new … enable=yes`.
    Set { ports: Vec<u16>, program: String },
    /// `delete rule name=…`.
    Delete,
}

/// `%SystemRoot%\System32\netsh.exe` and its arguments for `command`, every value validated (ports 1024-65535, 1-16 of them, sorted
/// and distinct; the program an absolute drive path to `MightyCode.exe`; the system folder a drive path of at most 260 characters).
pub fn command_line(system_folder: &str, command: &NetshCommand) -> Result<(String, Vec<String>)> {
    if !valid_system_directory(Some(system_folder)) {
        return Err(PlatformError::Invalid("the system folder is no absolute drive path".into()));
    }
    let ports = |ports: &[u16]| ports.iter().map(|port| i32::from(*port)).collect::<Vec<_>>();
    let arguments = match command {
        NetshCommand::Show => Some(show_arguments()),
        NetshCommand::Delete => Some(delete_arguments()),
        NetshCommand::Add { ports: list, program } => add_arguments(program, &ports(list)),
        NetshCommand::Set { ports: list, program } => set_arguments(program, &ports(list)),
    };
    let arguments = arguments.ok_or_else(|| PlatformError::Invalid("a refused program path or port list".into()))?;
    Ok((netsh(system_folder), arguments))
}

/// `WindowsFirewallRule.ValidPort`.
pub fn valid_port(port: i32) -> bool {
    (1024..=65535).contains(&port)
}

/// The length C# measures (UTF-16 units).
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// A Unicode format character (category Cf: bidi overrides, the BOM, zero-width marks), as `char.GetUnicodeCategory` names them.
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// `PlainCharacters`: no control character (C0, DEL, C1), no format character, no whitespace but ' '.
fn plain_characters(text: &str) -> bool {
    text.chars().all(|c| !c.is_control() && !is_format(c) && (c == ' ' || !c.is_whitespace()))
}

/// A character C#'s path patterns refuse inside a segment: `\ / : * ? " < > | %` and the C0 controls and DEL.
fn refused_in_segment(c: char) -> bool {
    matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '%') || (c as u32) <= 0x1f || c == '\u{7f}'
}

/// `X:\` at the start.
fn drive_root(path: &str) -> Option<&str> {
    let mut chars = path.chars();
    let (drive, colon, slash) = (chars.next()?, chars.next()?, chars.next()?);
    (drive.is_ascii_alphabetic() && colon == ':' && slash == '\\').then_some(chars.as_str())
}

/// `ProgramPattern`: `X:\` then segments of allowed characters, the last ending `.exe` (any case) after at least one character.
fn program_pattern(path: &str) -> bool {
    let Some(rest) = drive_root(path) else { return false };
    let segments: Vec<&str> = rest.split('\\').collect();
    let Some(last) = segments.last() else { return false };
    segments.iter().all(|segment| !segment.is_empty() && !segment.chars().any(refused_in_segment))
        && last.len() > 4
        && last.get(last.len() - 4..).is_some_and(|tail| tail.eq_ignore_ascii_case(".exe"))
}

/// `WindowsFirewallRule.ValidSystemDirectory`: an absolute drive path of 4-260 characters with no quote, `%` or control.
pub fn valid_system_directory(path: Option<&str>) -> bool {
    let Some(path) = path else { return false };
    (4..=260).contains(&units(path))
        && plain_characters(path)
        && drive_root(path).is_some_and(|rest| !rest.chars().any(|c| c == '"' || c == '%' || (c as u32) <= 0x1f || c == '\u{7f}'))
}

/// `WindowsFirewallRule.ValidProgram`: `path` when it is a drive-letter absolute path to [`PROGRAM_FILE_NAME`] with no quote, `%`,
/// control or format character, whitespace other than a space, `/`, second `:` or `.`/`..` segment; else none.
pub fn valid_program(path: Option<&str>) -> Option<&str> {
    let path = path?;
    let name = &path[path.rfind('\\').map_or(0, |at| at + 1)..];
    ((8..=1024).contains(&units(path))
        && plain_characters(path)
        && program_pattern(path)
        && !path.split('\\').skip(1).any(|segment| segment == "." || segment == "..")
        && name.eq_ignore_ascii_case(PROGRAM_FILE_NAME))
    .then_some(path)
}

/// `WindowsFirewallRule.PortList`: the ports for `localport=`, each 1024-65535, sorted, each once, 1-16 of them; else none.
pub fn port_list(ports: &[i32]) -> Option<String> {
    let mut list = ports.to_vec();
    list.sort_unstable();
    list.dedup();
    ((1..=MAX_PORTS).contains(&list.len()) && list.iter().all(|port| valid_port(*port)))
        .then(|| list.iter().map(i32::to_string).collect::<Vec<_>>().join(","))
}

fn rule_body(ports: &str, program: &str) -> Vec<String> {
    vec![
        "dir=in".into(),
        "action=allow".into(),
        "protocol=TCP".into(),
        format!("localport={ports}"),
        format!("localip={LOCAL_RANGES}"),
        format!("remoteip={REMOTE_RANGES}"),
        format!("program={program}"),
        "profile=any".into(),
    ]
}

fn head(verb: &str) -> Vec<String> {
    vec!["advfirewall".into(), "firewall".into(), verb.into(), "rule".into(), format!("name={RULE_NAME}")]
}

/// `WindowsFirewallRule.AddArguments`: `add rule name=… dir=in action=allow protocol=TCP localport=… localip=… remoteip=… program=…
/// profile=any`; none when the program or a port is refused.
pub fn add_arguments(program: &str, ports: &[i32]) -> Option<Vec<String>> {
    let (valid, list) = (valid_program(Some(program))?, port_list(ports)?);
    let mut arguments = head("add");
    arguments.extend(rule_body(&list, valid));
    Some(arguments)
}

/// `WindowsFirewallRule.SetArguments`: the rule already there rewritten in place (no second rule by the name): `set rule name=… new
/// <the add rule's body> enable=yes`.
pub fn set_arguments(program: &str, ports: &[i32]) -> Option<Vec<String>> {
    let (valid, list) = (valid_program(Some(program))?, port_list(ports)?);
    let mut arguments = head("set");
    arguments.push("new".into());
    arguments.extend(rule_body(&list, valid));
    arguments.push("enable=yes".into());
    Some(arguments)
}

pub fn delete_arguments() -> Vec<String> {
    head("delete")
}

/// `show rule name=… verbose`: `verbose` makes netsh print the rule's program, which [`shown_program`] reads (C#'s query, plus
/// `verbose`: without it netsh leaves the program out, and a rule of the name made for another exe would read current).
pub fn show_arguments() -> Vec<String> {
    let mut arguments = head("show");
    arguments.push("verbose".into());
    arguments
}

/// `WindowsFirewallRule.Arguments`: the add argv for a port set, the delete argv for none; none when the path or a port is refused.
pub fn arguments(ports: &[i32], program: &str) -> Option<Vec<String>> {
    if ports.is_empty() { Some(delete_arguments()) } else { add_arguments(program, ports) }
}

/// `WindowsFirewallRule.CommandLine`, netsh's own spelling: `key="value"` for the rule name and the program path every time and for any
/// value with a space, the token as it is otherwise. Values never hold a quote (validated).
pub fn command_text(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let always = ["name=", "program="].iter().any(|key| argument.starts_with(key));
            if !argument.contains(' ') && !always {
                return argument.clone();
            }
            match argument.find('=') {
                Some(equals) if equals > 0 => format!("{}\"{}\"", &argument[..=equals], &argument[equals + 1..]),
                _ => format!("\"{argument}\""),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `netsh.exe` in the system folder.
pub fn netsh(system_directory: &str) -> String {
    format!("{}\\netsh.exe", system_directory.trim_end_matches('\\'))
}

/// One netsh start as the runner asks for it (C#'s `ProcessStartInfo`): unelevated it runs hidden with its output read
/// (`QueryStartInfo`); elevated it goes through Windows' own prompt with its window hidden (`ElevatedStartInfo`). Either way from the
/// system folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetshLaunch {
    /// `netsh.exe` in the system folder.
    pub exe: String,
    /// The argv (the unelevated start passes it as it is).
    pub arguments: Vec<String>,
    /// [`command_text`] of `arguments`: what the elevated start hands Windows.
    pub command_line: String,
    pub working_directory: String,
    pub elevated: bool,
}

/// `WindowsFirewallRule.QueryStartInfo`: whether a rule by the name exists, `show rule` unelevated and hidden.
pub fn query_launch(system_directory: &str) -> NetshLaunch {
    let arguments = show_arguments();
    NetshLaunch {
        exe: netsh(system_directory),
        command_line: command_text(&arguments),
        arguments,
        working_directory: system_directory.to_string(),
        elevated: false,
    }
}

/// `WindowsFirewallRule.ElevatedStartInfo`: a change, netsh from the system folder, elevated through Windows' prompt, hidden.
pub fn elevated_launch(system_directory: &str, arguments: Vec<String>) -> NetshLaunch {
    NetshLaunch {
        exe: netsh(system_directory),
        command_line: command_text(&arguments),
        arguments,
        working_directory: system_directory.to_string(),
        elevated: true,
    }
}

/// The program a shown rule names, from `show rule … verbose`'s output: the value of every "label: value" line that reads as a drive
/// path to an `.exe` (in any display language: the labels are translated, the values are not). None when there is none, or when more
/// than one rule has the name and they name different programs.
pub fn shown_program(output: Option<&str>) -> Option<String> {
    let output = output.filter(|o| !o.is_empty())?;
    let mut found: Option<String> = None;
    for line in output.split('\n') {
        let Some(colon) = line.find(':') else { continue };
        let value = line[colon + 1..].trim();
        let exe = value.len() > 4 && value.get(value.len() - 4..).is_some_and(|tail| tail.eq_ignore_ascii_case(".exe"));
        if !exe || drive_root(value).is_none() {
            continue;
        }
        match &found {
            Some(known) if !known.eq_ignore_ascii_case(value) => return None,
            Some(_) => {}
            None => found = Some(value.to_string()),
        }
    }
    found
}

/// `PortListPattern`: digits and commas, 1-5 digits each.
fn port_list_pattern(value: &str) -> bool {
    value.split(',').all(|part| (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
}

/// `WindowsFirewallRule.ShownPorts`: the ports a shown rule opens, from `show rule`'s output — every "label: value" line whose value is
/// a plain list of ports (each 1-65535). For this rule that is only its local port line, in any display language (the labels are
/// translated, the values are not); the union when more than one rule has the name. None when no line reads so (a range, Any, or
/// output that is not netsh's): the ports are then unknown, never taken as the ones in use.
pub fn shown_ports(output: Option<&str>) -> Option<Vec<u16>> {
    let output = output.filter(|o| !o.is_empty())?;
    let mut ports = std::collections::BTreeSet::new();
    for line in output.split('\n') {
        let Some(colon) = line.find(':') else { continue };
        let value = line[colon + 1..].trim();
        if value.is_empty() || !port_list_pattern(value) {
            continue;
        }
        for part in value.split(',') {
            match part.parse::<u32>() {
                Ok(port @ 1..=65535) => {
                    ports.insert(port as u16);
                }
                _ => return None,
            }
        }
    }
    (!ports.is_empty()).then(|| ports.into_iter().collect())
}

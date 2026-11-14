//! The inbound rule a Tailscale direct connection needs on Windows (C# `WindowsFirewallRule`, `WindowsFirewallRuleRunner`,
//! Core\WindowsFirewallRule.cs; plan §1.4, D4): one rule named for the app, allowing this executable on the given TCP ports from and to
//! Tailscale's ranges. The Mac has no such step ([`crate::PlatformProfile::direct_connection_firewall`] is false) and answers
//! `Unsupported`.
//!
//! [`NetshRunner`] is C#'s runner, shared and pure: it validates, reads the rule (unelevated) and builds the one elevated command, and
//! starts netsh only through a [`NetshRun`] — the Windows crate's real one (the only place that elevates), or a recorder that starts
//! nothing (`fake::RecordingNetsh`: the checks and the smoke). A dropped call is the caller's cancel: nothing is turned into a result.
//! Owner: W8-C.

pub mod netsh;

use std::sync::Arc;
use std::time::Duration;

use crate::{BoxFuture, Result};
use netsh::NetshLaunch;

/// What `show rule` says (C# `FirewallRuleReading`): `Present` means only that a rule by [`netsh::RULE_NAME`] exists, not that its ports
/// or program are the ones the app would write now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallRule {
    Missing,
    /// The ports the rule opens as far as they read as a plain list (`netsh::shown_ports`); none when they could not be read.
    Present {
        ports: Option<Vec<u16>>,
    },
    /// `netsh` could not run or answered something else.
    Unreadable,
}

impl FirewallRule {
    /// C# `RemoteWorkspaceFeatures.FirewallNeedsUpdate`: the rule that is there must be updated — it does not open every port in use now
    /// (`needed`), or its ports could not be read, so it is never said to be current without proof. False with no rule or nothing to open.
    pub fn needs_update(&self, needed: &[u16]) -> bool {
        match self {
            FirewallRule::Present { ports } => {
                !needed.is_empty() && ports.as_ref().is_none_or(|ports| !needed.iter().all(|port| ports.contains(port)))
            }
            _ => false,
        }
    }
}

/// The ports (`localport=` spelling) and the exe a done add or update wrote (C# `FirewallRuleSpec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallRuleSpec {
    pub ports: String,
    pub program: String,
}

/// C# `FirewallChangeResult`: what a change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallChange {
    /// Done; after an add or update, the ports and program now in the rule.
    Applied(Option<FirewallRuleSpec>),
    /// The user said no to the elevation prompt (C# `Cancelled`).
    Declined,
    /// Nothing ran: `noPorts`, `badPorts`, `badProgram` or `noSystemDirectory`.
    Refused(String),
    /// It ran and failed: netsh's exit code, `timeout`, `queryFailed` (the rule could not be read before an add) or the launch error's
    /// words (never shown: Settings says its own words for anything but the codes).
    Failed(String),
}

/// Contract: `read` needs no elevation; `apply` makes the rule allow exactly `ports` (each 1024–65535) for this executable and asks
/// for elevation once (Windows: UAC); an empty list removes the rule. `add` refuses an empty list (`noPorts`) and `remove` is `apply`
/// with none (C# `AddAsync`, `RemoveAsync`, `ApplyAsync`).
pub trait Firewall: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<FirewallRule>>;
    fn apply(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>>;

    fn add(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        if ports.is_empty() {
            return Box::pin(async { Ok(FirewallChange::Refused("noPorts".into())) });
        }
        self.apply(ports)
    }

    fn remove(&self) -> BoxFuture<'_, Result<FirewallChange>> {
        self.apply(vec![])
    }
}

/// How long a started netsh may take before the runner stops waiting (C# `WindowsFirewallRuleRunner.Limit`; the unelevated query is
/// killed then, an elevated netsh left to finish).
pub const LIMIT: Duration = Duration::from_secs(30);

/// A netsh that ran to its end: its exit code and, unelevated, what it printed (at most 64 KiB; empty when nothing readable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetshExit {
    pub code: i32,
    pub output: String,
}

/// Why a netsh start gave no exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetshFailure {
    /// The elevation prompt was declined (`ERROR_CANCELLED`, [`netsh::ELEVATION_CANCELLED`]).
    Cancelled,
    /// It did not finish within [`LIMIT`].
    TimedOut,
    /// It could not start, or its wait failed (the OS's words, kept for the log).
    Failed(String),
}

/// Starts one netsh and waits for it (C#'s `run`/`read` delegates). The real one is mighty-platform-windows' `WindowsNetsh`; tests and
/// the smoke give a recorder.
pub trait NetshRun: Send + Sync {
    fn run(&self, launch: NetshLaunch) -> BoxFuture<'_, std::result::Result<NetshExit, NetshFailure>>;
}

/// C# `WindowsFirewallRuleRunner`: the rule's commands, from `system_directory` only, for `program` only.
pub struct NetshRunner {
    run: Arc<dyn NetshRun>,
    system_directory: Option<String>,
    program: Option<String>,
    /// The system folder Windows reports (C# `Environment.SystemDirectory`); netsh never starts from another. None off Windows and for
    /// a recorder: no comparison.
    reported: Option<String>,
}

impl NetshRunner {
    pub fn new(run: Arc<dyn NetshRun>, system_directory: Option<String>, program: Option<String>, reported: Option<String>) -> NetshRunner {
        NetshRunner { run, system_directory, program, reported }
    }

    /// `ReadRuleAsync`: whether a rule by the name exists and, when it does, the ports it opens as far as they can be read.
    pub async fn read_rule(&self) -> FirewallRule {
        let Some(system) = &self.system_directory else { return FirewallRule::Unreadable };
        match self.run.run(netsh::query_launch(system)).await {
            Ok(NetshExit { code: 0, output }) => FirewallRule::Present { ports: netsh::shown_ports(Some(&output)) },
            Ok(NetshExit { code: 1, .. }) => FirewallRule::Missing,
            _ => FirewallRule::Unreadable,
        }
    }

    /// `AddAsync`: the rule for exactly `ports` as one elevated netsh — `set rule` when a rule by the name is there (so rules never pile
    /// up and a dropped port closes), `add rule` when none is, nothing when it cannot be read.
    pub async fn add_rule(&self, ports: &[u16]) -> FirewallChange {
        let list: Vec<i32> = ports.iter().map(|port| i32::from(*port)).collect();
        if list.is_empty() {
            return FirewallChange::Refused("noPorts".into());
        }
        let Some(port_list) = netsh::port_list(&list) else { return FirewallChange::Refused("badPorts".into()) };
        let Some(program) = netsh::valid_program(self.program.as_deref()).map(str::to_string) else {
            return FirewallChange::Refused("badProgram".into());
        };
        if self.system_directory_refused() {
            return FirewallChange::Refused("noSystemDirectory".into());
        }
        let arguments = match self.read_rule().await {
            FirewallRule::Present { .. } => netsh::set_arguments(&program, &list),
            FirewallRule::Missing => netsh::add_arguments(&program, &list),
            FirewallRule::Unreadable => None,
        };
        let Some(arguments) = arguments else { return FirewallChange::Failed("queryFailed".into()) };
        match self.elevated(arguments).await {
            FirewallChange::Applied(_) => FirewallChange::Applied(Some(FirewallRuleSpec { ports: port_list, program })),
            other => other,
        }
    }

    /// `RemoveAsync`: the delete, elevated.
    pub async fn remove_rule(&self) -> FirewallChange {
        self.elevated(netsh::delete_arguments()).await
    }

    /// No folder, or a folder other than the one Windows reports: netsh is never started from anywhere else.
    fn system_directory_refused(&self) -> bool {
        match (&self.system_directory, &self.reported) {
            (None, _) => true,
            (Some(system), Some(reported)) => !system.trim_end_matches('\\').eq_ignore_ascii_case(reported.trim_end_matches('\\')),
            (Some(_), None) => false,
        }
    }

    async fn elevated(&self, arguments: Vec<String>) -> FirewallChange {
        if self.system_directory_refused() {
            return FirewallChange::Refused("noSystemDirectory".into());
        }
        let Some(system) = &self.system_directory else { return FirewallChange::Refused("noSystemDirectory".into()) };
        match self.run.run(netsh::elevated_launch(system, arguments)).await {
            Ok(NetshExit { code: 0, .. }) => FirewallChange::Applied(None),
            Ok(NetshExit { code, .. }) => FirewallChange::Failed(code.to_string()),
            Err(NetshFailure::Cancelled) => FirewallChange::Declined,
            Err(NetshFailure::TimedOut) => FirewallChange::Failed("timeout".into()),
            Err(NetshFailure::Failed(words)) => FirewallChange::Failed(words),
        }
    }
}

impl Firewall for NetshRunner {
    fn read(&self) -> BoxFuture<'_, Result<FirewallRule>> {
        Box::pin(async move { Ok(self.read_rule().await) })
    }

    /// `ApplyAsync`: added or updated in place for a set, removed for none.
    fn apply(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        Box::pin(async move { Ok(if ports.is_empty() { self.remove_rule().await } else { self.add_rule(&ports).await }) })
    }

    fn add(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        Box::pin(async move { Ok(self.add_rule(&ports).await) })
    }

    fn remove(&self) -> BoxFuture<'_, Result<FirewallChange>> {
        Box::pin(async move { Ok(self.remove_rule().await) })
    }
}

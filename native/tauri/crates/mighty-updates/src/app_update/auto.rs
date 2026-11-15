//! The automatic update's rules (C# `AppUpdateAuto`, `AppIdleFacts`, `AppIdleBlocker`, `AppUpdateRestart*`, AppUpdateAutoPolicy.cs; the
//! Mac's `AppUpdateAuto.swift`; plan §1.4). Owner: W9-A; W9-C's host loop asks them.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// How long after the last check (or attempt) the next one is due (C# `CheckInterval`).
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How often the running app looks (C# `Tick`).
pub const TICK: Duration = Duration::from_secs(10 * 60);
/// Input this recent means the user is at the app (C# `UserIdleDelay`).
pub const USER_IDLE_DELAY: Duration = Duration::from_secs(5 * 60);
/// A run that ended or input sent to an agent this recently means work may follow (C# `AgentIdleDelay`).
pub const AGENT_IDLE_DELAY: Duration = Duration::from_secs(3 * 60);
/// The restart banner's countdown (C# `Countdown`).
pub const COUNTDOWN: Duration = Duration::from_secs(60);
/// How often the app looks while the banner counts down (C# `CountdownTick`).
pub const COUNTDOWN_TICK: Duration = Duration::from_secs(1);
/// A countdown found more than this past its end was slept through: cancelled, never acted on (C# `CountdownGrace`).
pub const COUNTDOWN_GRACE: Duration = Duration::from_secs(5);
/// "Later" keeps the banner away this long (C# `Snooze`).
pub const SNOOZE: Duration = Duration::from_secs(4 * 60 * 60);

/// What the app is doing right now (C# `AppIdleFacts`); each field feeds one [`IdleBlocker`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdleFacts {
    /// `AgentRun`: agent sessions running (remote ones included), runs waiting or held, delegation runs ending or waiting.
    pub agent_runs: u32,
    /// `PendingCard`: permission prompts, questions, plan cards and web-open requests waiting for the user.
    pub pending_cards: u32,
    /// `QueuedInput`: queued inputs, without those delegation holds.
    pub queued_inputs: u32,
    /// `Draft`: composers with non-blank text, attachment drafts and attachments still importing.
    pub drafts: u32,
    /// `Terminal`: shells running a command, live agent processes, setup runs and terminals, CLI sign-ins.
    pub terminals: u32,
    /// `CliWork`: a CLI update, plugin change, model reset or tool install in progress.
    pub cli_work: bool,
    /// `Remote`: screen sharing, phone uploads, runs hosted for another computer, workspaces shared.
    pub screen_sharing: bool,
    pub phone_uploads: u32,
    pub remote_host_runs: u32,
    pub sharing_workspaces: bool,
    /// `Modal`: a blocking dialog, the setup screen or the settings open.
    pub blocking_modal: bool,
    /// `UserActive`: the last keyboard or mouse input in the session.
    pub last_user_input: Option<DateTime<Utc>>,
    /// `RecentAgentActivity`: the last time a run ended or input went to an agent.
    pub last_agent_activity: Option<DateTime<Utc>>,
}

/// Why the app is not idle, in the macOS order (C# `AppIdleBlocker`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdleBlocker {
    AgentRun,
    PendingCard,
    QueuedInput,
    Draft,
    Terminal,
    CliWork,
    Remote,
    Modal,
    UserActive,
    RecentAgentActivity,
}

impl IdleBlocker {
    /// Every blocker in enum order (C# `Enum.GetValues<AppIdleBlocker>()`).
    pub const ALL: [IdleBlocker; 10] = [
        IdleBlocker::AgentRun,
        IdleBlocker::PendingCard,
        IdleBlocker::QueuedInput,
        IdleBlocker::Draft,
        IdleBlocker::Terminal,
        IdleBlocker::CliWork,
        IdleBlocker::Remote,
        IdleBlocker::Modal,
        IdleBlocker::UserActive,
        IdleBlocker::RecentAgentActivity,
    ];
}

/// Where the idle-restart offer stands (C# `AppUpdateRestartState`; `Until` unused while idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartState {
    /// No banner; offered the next time the app is idle.
    #[default]
    Idle,
    /// The banner is up; the app restarts at the time unless something other than input got in the way.
    CountingDown(DateTime<Utc>),
    /// "Later": no banner before the time.
    Snoozed(DateTime<Utc>),
}

/// What the app does after a step (C# `AppUpdateRestartAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartAction {
    None,
    StartCountdown,
    Cancel,
    Restart,
}

/// C# `AppUpdateRestartDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartDecision {
    pub state: RestartState,
    pub action: RestartAction,
}

fn span(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::TimeDelta::MAX)
}

/// C# `CheckDue(enabled, hasKey, lastCheck, lastAttempt, now)`: due [`CHECK_INTERVAL`] after the later of the last check and the last
/// attempt; never with the switch off or without a key. A time later than `now` (the clock was set back) is ignored, so a future last
/// check alone does not stop checks, while a past attempt still holds the next try back.
pub fn check_due(
    enabled: bool,
    has_key: bool,
    last_check: Option<DateTime<Utc>>,
    last_attempt: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    if !enabled || !has_key {
        return false;
    }
    match [last_check, last_attempt].into_iter().flatten().filter(|date| *date <= now).max() {
        None => true,
        Some(latest) => now - latest >= span(CHECK_INTERVAL),
    }
}

/// C# `IdleBlockers(facts, now)`: every reason the app is not idle, in [`IdleBlocker`] order; empty when it may restart. A time in the
/// future (the clock set back) counts as just now.
pub fn idle_blockers(facts: &IdleFacts, now: DateTime<Utc>) -> Vec<IdleBlocker> {
    let recent = |date: Option<DateTime<Utc>>, delay: Duration| date.is_some_and(|date| now - date < span(delay));
    IdleBlocker::ALL
        .into_iter()
        .filter(|blocker| match blocker {
            IdleBlocker::AgentRun => facts.agent_runs > 0,
            IdleBlocker::PendingCard => facts.pending_cards > 0,
            IdleBlocker::QueuedInput => facts.queued_inputs > 0,
            IdleBlocker::Draft => facts.drafts > 0,
            IdleBlocker::Terminal => facts.terminals > 0,
            IdleBlocker::CliWork => facts.cli_work,
            IdleBlocker::Remote => {
                facts.screen_sharing || facts.phone_uploads > 0 || facts.remote_host_runs > 0 || facts.sharing_workspaces
            }
            IdleBlocker::Modal => facts.blocking_modal,
            IdleBlocker::UserActive => recent(facts.last_user_input, USER_IDLE_DELAY),
            IdleBlocker::RecentAgentActivity => recent(facts.last_agent_activity, AGENT_IDLE_DELAY),
        })
        .collect()
}

/// C# `IsIdle(facts, now)`.
pub fn is_idle(facts: &IdleFacts, now: DateTime<Utc>) -> bool {
    idle_blockers(facts, now).is_empty()
}

/// C# `RestartOffer(state, ready, automatic, blockers, now)`: one step of the idle-restart offer, on every tick (every
/// [`COUNTDOWN_TICK`] while counting down). A ready, automatic app with no blocker starts a [`COUNTDOWN`]; the countdown restarts the
/// app when it runs out and is cancelled quietly as soon as any blocker other than `UserActive` appears, the switch goes off or the
/// update is no longer ready (input alone does not cancel it, so the pointer can reach the banner's buttons); one found more than
/// [`COUNTDOWN_GRACE`] past its end (the computer slept through it) is cancelled, never acted on; a snooze holds until it ends.
pub fn restart_offer(state: RestartState, ready: bool, automatic: bool, blockers: &[IdleBlocker], now: DateTime<Utc>) -> RestartDecision {
    let decision = |state, action| RestartDecision { state, action };
    match state {
        RestartState::CountingDown(until) => {
            if !ready
                || !automatic
                || blockers.iter().any(|blocker| *blocker != IdleBlocker::UserActive)
                || now - until > span(COUNTDOWN_GRACE)
            {
                decision(RestartState::Idle, RestartAction::Cancel)
            } else if now >= until {
                decision(RestartState::Idle, RestartAction::Restart)
            } else {
                decision(state, RestartAction::None)
            }
        }
        RestartState::Snoozed(until) if now < until => decision(state, RestartAction::None),
        _ if !ready || !automatic || !blockers.is_empty() => decision(RestartState::Idle, RestartAction::None),
        _ => decision(RestartState::CountingDown(now + span(COUNTDOWN)), RestartAction::StartCountdown),
    }
}

/// C# `Later(now)`: snoozed for [`SNOOZE`].
pub fn later(now: DateTime<Utc>) -> RestartState {
    RestartState::Snoozed(now + span(SNOOZE))
}

/// C# `InstallsOnQuit(ready, automatic, hasKey, poweringOff)`: quitting installs the staged update (the helper with `--relaunch 0`).
/// `ready`: staged, no install under way; `has_key`: the build verifies updates and this is not a smoke; `powering_off`: Windows signs
/// out, restarts or shuts down, and the swap could be cut off half way, so the update waits for the next quit.
pub fn installs_on_quit(ready: bool, automatic: bool, has_key: bool, powering_off: bool) -> bool {
    ready && automatic && has_key && !powering_off
}

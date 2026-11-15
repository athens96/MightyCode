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

/// C# `CheckDue(enabled, hasKey, lastCheck, lastAttempt, now)` (a time later than `now` ignored). Step 0 stub (W9-A writes it): never due.
pub fn check_due(
    _enabled: bool,
    _has_key: bool,
    _last_check: Option<DateTime<Utc>>,
    _last_attempt: Option<DateTime<Utc>>,
    _now: DateTime<Utc>,
) -> bool {
    false
}

/// C# `IdleBlockers(facts, now)`: every reason the app is not idle, in [`IdleBlocker`] order. Step 0 stub (W9-A writes it): every
/// blocker, so nothing ever counts as idle.
pub fn idle_blockers(_facts: &IdleFacts, _now: DateTime<Utc>) -> Vec<IdleBlocker> {
    IdleBlocker::ALL.to_vec()
}

/// C# `RestartOffer(state, ready, automatic, blockers, now)`. Step 0 stub (W9-A writes it): idle, nothing done.
pub fn restart_offer(
    _state: RestartState,
    _ready: bool,
    _automatic: bool,
    _blockers: &[IdleBlocker],
    _now: DateTime<Utc>,
) -> RestartDecision {
    RestartDecision { state: RestartState::Idle, action: RestartAction::None }
}

/// C# `Later(now)`: snoozed for [`SNOOZE`].
pub fn later(now: DateTime<Utc>) -> RestartState {
    RestartState::Snoozed(now + chrono::Duration::from_std(SNOOZE).unwrap_or_default())
}

/// C# `InstallsOnQuit(ready, automatic, hasKey, poweringOff)`. Step 0 stub (W9-A writes it): quitting never installs.
pub fn installs_on_quit(_ready: bool, _automatic: bool, _has_key: bool, _powering_off: bool) -> bool {
    false
}

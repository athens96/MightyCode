//! Which agent the pet follows and what its bubble does (C# WinUI\MainWindow.Companion.cs `RefreshCompanion` and the pager's and the
//! pet's actions in `CompanionAction`; the Mac's CompanionBubbleController.swift:4-38, AgentCompanionViews.swift:130-155): only Claude
//! panes; the pinned page while its agent is busy, else the most pressing agent (waiting, then working, then failed, then finished; the
//! latest seen among equals); a new request or another agent opens the bubble; a finished run shows its result for six seconds, the pet
//! jumping, then puts the bubble away; a click on the pet cancels that. The app reads its panes and requests into [`Agent`]s; this keeps
//! the choice between refreshes. Times are seconds on the caller's clock. Owner: W8-E.

use mighty_core::model::AgentActivity;
use sha2::{Digest, Sha256};

use crate::{animation, carousel};

/// One Claude pane as the pet sees it: its status with the waits folded in (a request for the user, an activity that waits, a run alive
/// only for the notices it awaits all read "waiting", C# `CompanionStatus`), its activity, and its run's start and last sight.
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub id: String,
    pub status: String,
    pub activity: Option<AgentActivity>,
    /// When its run started (the identity of a run: a new one opens the bubble again).
    pub started: Option<i64>,
    /// When it was last seen working (the tie-breaker of the automatic choice).
    pub last_observed: Option<i64>,
}

/// What a refresh decided.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Followed {
    /// The agent shown, if any.
    pub shown: Option<String>,
    /// Its status ("idle" with none).
    pub status: String,
    /// The busy agents, in the panes' order.
    pub active: Vec<String>,
    /// The page's number among them and how many there are (both 0 for one page).
    pub position: usize,
    pub count: usize,
    /// The animation row.
    pub row: i32,
    /// The task bubble is open (a request shows itself regardless).
    pub bubble: bool,
}

/// The choice kept between refreshes (C#'s `companionPinned`, `companionShown`, `companionAutomatic`, `companionIdentity`,
/// `companionHideAt`, `companionBubble`, `companionRow`, `companionAnimationStart`).
#[derive(Debug, Clone, Default)]
pub struct Follower {
    pinned: Option<String>,
    shown: Option<String>,
    automatic: Option<String>,
    identity: Option<(Option<String>, Option<i64>, bool)>,
    hide_at: Option<f64>,
    bubble: bool,
    row: i32,
    animation_start: f64,
}

fn busy(status: &str) -> bool {
    matches!(status, "running" | "waiting" | "starting" | "queued")
}

impl Follower {
    /// C# `InitializeCompanionAsync`: the bubble starts as the task setting says.
    pub fn new(shows_task: bool) -> Follower {
        Follower { bubble: shows_task, ..Follower::default() }
    }

    /// C# `RefreshCompanion`'s choice, at `now`.
    pub fn refresh(&mut self, agents: &[Agent], shows_task: bool, now: f64) -> Followed {
        let active: Vec<String> = agents.iter().filter(|a| busy(&a.status)).map(|a| a.id.clone()).collect();
        // OrderByDescending(Priority).ThenByDescending(LastObservedAt).FirstOrDefault(): the first of the greatest.
        let mut automatic: Option<&Agent> = None;
        for agent in agents {
            let key = (animation::priority(&agent.status), agent.last_observed.unwrap_or(i64::MIN));
            if automatic.is_none_or(|best| key > (animation::priority(&best.status), best.last_observed.unwrap_or(i64::MIN))) {
                automatic = Some(agent);
            }
        }
        self.automatic = automatic.map(|a| a.id.clone());
        self.shown = carousel::shown(self.pinned.as_deref(), &active, self.automatic.as_deref()).map(str::to_string);
        if self.pinned.as_ref().is_some_and(|pin| !active.contains(pin)) {
            self.pinned = None;
        }
        let current = agents.iter().find(|a| Some(&a.id) == self.shown.as_ref());
        let status = current.map_or_else(|| "idle".to_string(), |a| a.status.clone());
        let identity = (current.map(|a| a.id.clone()), current.and_then(|a| a.started), status == "completed");
        if self.identity.as_ref() != Some(&identity) {
            self.identity = Some(identity);
            self.bubble = current.is_some() && shows_task;
            self.hide_at = (status == "completed").then_some(now + 6.0);
        }
        let row = animation::row(&status, current.and_then(|a| a.activity.as_ref()), self.hide_at.is_some_and(|hide| hide > now));
        if row != self.row {
            self.row = row;
            self.animation_start = now;
        }
        if self.hide_at.is_some_and(|hide| hide <= now) {
            self.bubble = false;
            self.hide_at = None;
        }
        let position = if active.len() > 1 { carousel::position(self.shown.as_deref(), &active).unwrap_or(0) } else { 0 };
        let count = if position > 0 { active.len() } else { 0 };
        Followed { shown: self.shown.clone(), status, active, position, count, row: self.row, bubble: self.bubble }
    }

    /// Seconds into the current row's loop (for [`crate::pet::frame`]).
    pub fn animation_elapsed(&self, now: f64) -> f64 {
        now - self.animation_start
    }

    pub fn shown(&self) -> Option<&str> {
        self.shown.as_deref()
    }

    /// A click on the pet (`toggle`): with the task setting off it is turned on (the caller saves it: true) and the bubble opens; else
    /// the bubble opens or closes. Either way a finished run's bubble no longer puts itself away.
    pub fn toggle(&mut self, shows_task: bool) -> bool {
        let turn_on = !shows_task;
        self.bubble = if turn_on { true } else { !self.bubble };
        self.hide_at = None;
        turn_on
    }

    /// The task setting switched: the bubble follows it.
    pub fn set_bubble(&mut self, open: bool) {
        self.bubble = open;
    }

    /// The pager's raised hand (`pending`): the first other agent with a request becomes the page.
    pub fn pin_pending(&mut self, requesting: &[String]) {
        self.pinned = requesting.iter().find(|id| Some(id.as_str()) != self.shown.as_deref()).cloned();
    }

    /// `previous` or `next` among `pages` (the busy agents and those with a request): pinned there, unless it is the one the pet would
    /// follow anyway.
    pub fn page(&mut self, pages: &[String], offset: i64) {
        if let Some(next) = carousel::step(self.shown.as_deref(), pages, offset) {
            self.pinned = if Some(&next) == self.automatic.as_ref() { None } else { Some(next) };
        }
    }
}

/// C# `PermissionKey(request)`: a request by its run and its id.
pub fn permission_key(run_id: &str, request_id: &str) -> String {
    format!("{run_id}|{request_id}")
}

/// The key a card's presses answer for (C# `companionCardKey`): a request's key and the SHA-256 of its tool and input (so a request
/// whose input changed is another card), else the agent shown, else `idle`.
pub fn card_key(request: Option<(&str, &str, &str, &str)>, shown: Option<&str>) -> String {
    match request {
        Some((run_id, request_id, tool, input)) => {
            let digest = Sha256::digest(format!("{tool}{input}").as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02X}")).collect();
            format!("{}|{hex}", permission_key(run_id, request_id))
        }
        None => shown.unwrap_or("idle").to_string(),
    }
}

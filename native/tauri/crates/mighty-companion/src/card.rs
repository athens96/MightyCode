//! What the bubble shows (C# WinUI\CompanionOverlay.cs `CompanionOverlayCard`, `CompanionOverlayRequest`, `CompanionOverlayOption`; the
//! Mac's AgentCompanionViews.swift:173-245, 299-452): one agent's card — its presence mark, the workspace over the title, the state
//! word over the clock, the request and the work, its page among the busy agents — or the request waiting on the user in its place.
//! The app fills them from its panes (src-tauri companion.rs, C# `CompanionCard` and `CompanionRequest`); the overlay lays them out
//! ([`crate::scene`]). Owner: W8-E.

use crate::bubble::BubbleButton;
use crate::progress::CompanionProgress;

/// C# `CompanionOverlayOption(Label, Description, Picked)`: one choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanionOption {
    pub label: String,
    pub description: String,
    pub picked: bool,
}

/// C# `CompanionOverlayRequest`: an approval (`detail`, `headline` and its `code`, in a box when `boxed`), one question (`headline`) with
/// its choices, or a finished `plan` — its Markdown, whole — with when it was `received`. Its buttons are [`crate::bubble`]'s; `busy`
/// turns the small spinner before the answers while one is on its way.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanionRequest {
    pub question: bool,
    pub title: String,
    pub origin: String,
    pub detail: String,
    pub headline: Option<String>,
    pub code: Option<String>,
    pub boxed: bool,
    pub multi_select: bool,
    pub options: Vec<CompanionOption>,
    pub buttons: Vec<BubbleButton>,
    pub error: Option<String>,
    pub plan: Option<String>,
    pub received: Option<String>,
    pub busy: bool,
}

/// C# `CompanionOverlayCard(Key, Light, Status, Workspace, Title, State, Timing, Request, Work, Position, Count, Pending, Approval)`:
/// `elapsed` is the run clock's words (`AgentRunTiming.Label()`, None without a timing), read again by the app each second; `position`
/// of `count` pages it among the busy agents; `pending` is another agent's request behind this page. `progress` is the progress card
/// over the task bubble (the Mac's `CompanionProgress`; never over a request), folded to its heading when `progress_collapsed`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompanionCard {
    pub key: String,
    pub light: bool,
    pub status: String,
    pub workspace: String,
    pub title: String,
    pub state: String,
    pub elapsed: Option<String>,
    pub request: Option<String>,
    pub work: String,
    pub position: usize,
    pub count: usize,
    pub pending: bool,
    pub approval: Option<CompanionRequest>,
    pub progress: Option<CompanionProgress>,
    pub progress_collapsed: bool,
}

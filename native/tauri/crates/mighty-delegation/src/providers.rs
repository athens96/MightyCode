//! Which agent panes take part in delegation and how (C# Core/DelegationProviders.cs:1-236; M/DelegationProviders.swift): agents,
//! modes, capabilities, the widest child mode, the human's choices, `CanParentByAgent`, `AttachesDelegation`, `AgentName`; the gate
//! (`DelegationIOHandler.Gate`) and the second send's choice (`ComposerSecondSends.Choose`, `MenuOnly`, mighty-composer's) over them.
//! Owner: W7-A (`ChildAutoCommit`, :237-347, is W7-B's `git::auto_commit`).
//!
//! - Claude and Codex panes may be parents by agent: their runs get the delegation server (a Codex run only in acceptEdits or
//!   fullAccess), and their delegate call makes children, of their own agent unless the call names another.
//! - Claude, Codex and Antigravity panes may be parents by human (the second send, the worktree button's menu).
//! - A child of the parent's agent may start in a mode no wider than the parent's by rank; a child of another agent in a mode that may
//!   do nothing the parent's may not ([`capabilities`]). A child's mode is always one of its own agent's.
//! - Only a Claude run takes text while it runs (steer).
//! - A Claude child commits its own work; a Codex or Antigravity child cannot commit outside fullAccess, so the app commits it.
//! - A Claude parent's merge tool follows its CLI's permission mode; any other parent merges by tool only in fullAccess.

use crate::host::DelegationAgentAvailability;
use crate::request::{DETACHED_MESSAGE, DelegationCaller, DelegationReasonCode, DelegationResponse};

pub use mighty_composer::queue::{
    ComposerSecondSend, can_parent_by_human, child_can_write, choose_second_send, is_agent_pane, menu_only, returns_to_composer,
};

/// `Wire.Providers` without pi, opencode and hermes: the agents that take part, in the app's order.
pub const AGENT_IDS: [&str; 3] = ["claude", "codex", "gemini"];

/// Every permission mode of any provider, narrowest first. Modes are compared within one provider.
const MODE_RANK: [&str; 6] = ["plan", "manual", "acceptEdits", "onRequest", "auto", "fullAccess"];

/// `CanParentByAgent`: an agent pane running Claude or Codex may delegate by tool.
pub fn can_parent_by_agent(kind: &str, provider: &str) -> bool {
    kind == "claude" && matches!(provider, "claude" | "codex")
}

/// `AttachesDelegation`: every Claude run, and a Codex run in acceptEdits or fullAccess, gets the delegation tools.
pub fn attaches_delegation(kind: &str, provider: &str, mode: &str) -> bool {
    can_parent_by_agent(kind, provider) && (provider != "codex" || codex_approves_tools(mode))
}

/// `CodexApprovesTools`: a Codex run approves the delegation server's tools ahead where it may write and never asks.
pub fn codex_approves_tools(mode: &str) -> bool {
    matches!(mode, "acceptEdits" | "fullAccess")
}

/// `CanSteer`: a running turn of this pane takes delegation's notices and follow-ups — Claude only.
pub fn can_steer(kind: &str, provider: &str) -> bool {
    kind == "claude" && provider == "claude"
}

/// `StartingModes`: the modes a child of `provider` may start in, narrowest first.
pub fn starting_modes(provider: &str) -> Vec<&'static str> {
    let modes = mighty_run::providers::permission_modes(provider, true);
    MODE_RANK.iter().copied().filter(|mode| modes.contains(mode)).collect()
}

/// `IsNoWider(mode, stored, provider)`: within one agent.
pub fn is_no_wider(mode: &str, stored: &str, provider: &str) -> bool {
    is_no_wider_across(mode, provider, stored, provider)
}

/// `IsNoWider(mode, child, stored, parent)`: whether `mode`, one of the `child` agent's, lets it act no more freely than the `parent`
/// agent's `stored` mode — by rank for one agent; for another, the child's mode may do nothing the parent's may not. A mode that is
/// not its own agent's, on either side, is never no wider.
pub fn is_no_wider_across(mode: &str, child: &str, stored: &str, parent: &str) -> bool {
    if !starting_modes(child).contains(&mode) || !starting_modes(parent).contains(&stored) {
        return false;
    }
    if child == parent {
        let rank = |m: &str| MODE_RANK.iter().position(|r| *r == m);
        return rank(mode) <= rank(stored);
    }
    is_covered(&capabilities(child, mode), &capabilities(parent, stored))
}

/// `ModeCapability`: what a run in a permission mode may do on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModeCapability {
    /// It changes files only after the human approves each change.
    WritesAsking,
    /// It changes files without asking, sandboxed or not.
    WritesUnasked,
    /// It runs shell commands without asking, sandboxed or not.
    RunsUnasked,
    /// No sandbox and no asking.
    FullAccess,
}

impl ModeCapability {
    /// Every capability, in C#'s order.
    pub const ALL: [ModeCapability; 4] =
        [ModeCapability::WritesAsking, ModeCapability::WritesUnasked, ModeCapability::RunsUnasked, ModeCapability::FullAccess];

    /// The name the shared fixture (`delegation-modes.json`) gives it: camelCase.
    pub fn name(self) -> &'static str {
        match self {
            ModeCapability::WritesAsking => "writesAsking",
            ModeCapability::WritesUnasked => "writesUnasked",
            ModeCapability::RunsUnasked => "runsUnasked",
            ModeCapability::FullAccess => "fullAccess",
        }
    }
}

/// `Capabilities`: what a run of `provider` in `mode` may do, as the app starts it. Claude: plan edits nothing until its plan is
/// approved; manual asks before each edit; acceptEdits edits unasked; auto also runs commands unasked. Codex: manual runs commands
/// unasked but edits nothing; onRequest edits unasked in its sandbox; acceptEdits edits and runs commands unasked. Antigravity: plan and
/// manual edit nothing; acceptEdits edits unasked. fullAccess is everything for every agent. In [`ModeCapability::ALL`]'s order.
pub fn capabilities(provider: &str, mode: &str) -> Vec<ModeCapability> {
    use ModeCapability as C;
    match (provider, mode) {
        ("claude", "manual") => vec![C::WritesAsking],
        ("claude", "acceptEdits") | ("codex", "onRequest") | ("gemini", "acceptEdits") => vec![C::WritesUnasked],
        ("claude", "auto") | ("codex", "acceptEdits") => vec![C::WritesUnasked, C::RunsUnasked],
        ("codex", "manual") => vec![C::RunsUnasked],
        (_, "fullAccess") => C::ALL.to_vec(),
        _ => vec![],
    }
}

/// `IsCovered`: a mode with `child` capabilities does nothing a mode with `parent` capabilities may not — each is the parent's too,
/// except that editing after asking is covered by editing unasked.
pub fn is_covered(child: &[ModeCapability], parent: &[ModeCapability]) -> bool {
    child.iter().all(|capability| {
        parent.contains(capability) || *capability == ModeCapability::WritesAsking && parent.contains(&ModeCapability::WritesUnasked)
    })
}

/// `WidestChildMode`: the widest of the `child` agent's modes no wider than the `parent` agent's `parent_mode` and in which that child
/// can change files, or None — the one with the most capabilities, then the later in its own order; for the parent's own agent the
/// last by its own order.
pub fn widest_child_mode(child: &str, parent: &str, parent_mode: &str) -> Option<&'static str> {
    let fitting: Vec<&'static str> = starting_modes(child)
        .into_iter()
        .filter(|mode| is_no_wider_across(mode, child, parent_mode, parent) && child_can_write(child, mode))
        .collect();
    if child == parent {
        return fitting.last().copied();
    }
    // A stable order by capability count keeps the agent's own order among equals; the last is the widest.
    let mut ranked: Vec<(usize, &'static str)> = fitting.into_iter().enumerate().collect();
    ranked.sort_by_key(|(index, mode)| (capabilities(child, mode).len(), *index));
    ranked.last().map(|(_, mode)| *mode)
}

/// `HumanChildMode`: the mode a child of `child` the human starts from a `parent` pane in `parent_mode` begins in, or None when there is
/// none in which it could change files — the parent's mode (or plan when `plan_first` and the agent has one) for the parent's own
/// agent, [`widest_child_mode`] for another.
pub fn human_child_mode(child: &str, parent: &str, parent_mode: &str, plan_first: bool) -> Option<String> {
    if child != parent {
        return widest_child_mode(child, parent, parent_mode).map(str::to_string);
    }
    let mode = if plan_first && starting_modes(child).contains(&"plan") { "plan" } else { parent_mode };
    child_can_write(child, mode).then(|| mode.to_string())
}

/// `MayHaveHumanChild`: the pane, in `mode` now, may have a child the human starts of any agent.
pub fn may_have_human_child(kind: &str, provider: &str, mode: &str) -> bool {
    is_agent_pane(kind, provider) && AGENT_IDS.iter().any(|agent| human_child_mode(agent, provider, mode, false).is_some())
}

/// `Agent`: the provider id delegate's `agent` names — claude, codex or antigravity (or gemini), in any case; None for any other word.
pub fn agent(name: &str) -> Option<&'static str> {
    match name.trim().to_lowercase().as_str() {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "antigravity" | "gemini" => Some("gemini"),
        _ => None,
    }
}

/// `AgentName`: the name the delegation tools give the agent `provider` (antigravity for gemini).
pub fn agent_name(provider: &str) -> &str {
    if provider == "gemini" { "antigravity" } else { provider }
}

/// `Availability`: whether an agent's CLI can run a child, as the app last learnt it — `installed` from its CLI search, `account` from
/// its sign-in status. Either one unknown (None) does not hold a child back; the app never asks a CLI for this.
pub fn availability(installed: Option<bool>, account: Option<&mighty_run::acp::accounts::CliAccountStatus>) -> DelegationAgentAvailability {
    if installed == Some(false) || account.is_some_and(|status| !status.installed) {
        return DelegationAgentAvailability::NotInstalled;
    }
    if account.is_some_and(|status| status.logged_in == Some(false)) {
        return DelegationAgentAvailability::SignedOut;
    }
    DelegationAgentAvailability::Ready
}

/// `DelegationChildBlock`: why the worktree button's menu cannot run a child of one agent now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegationChildBlock {
    NotInstalled,
    SignedOut,
    /// No mode of the agent, no wider than the parent's, lets its child change files.
    NoMode,
    /// The parent has its 3 open children the human started.
    Cap,
}

/// `DelegationChildChoice`: one agent the composer's worktree button offers. `mode`: the mode the child would start in; `blocked`: None
/// when the child can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationChildChoice {
    pub agent: String,
    pub mode: Option<String>,
    pub blocked: Option<DelegationChildBlock>,
}

/// `HumanChildChoices`: what the worktree button's menu offers on a `parent` pane in `parent_mode` with `open_human_children` open
/// children the human started — one row per agent (Claude, Codex, Antigravity), each saying why it cannot run: the agent's CLI (never
/// asked for the parent's own agent, whose run is going), no mode in which its child could change files, or the cap.
pub fn human_child_choices(
    parent: &str,
    parent_mode: &str,
    plan_first: bool,
    open_human_children: usize,
    availability: impl Fn(&str) -> DelegationAgentAvailability,
) -> Vec<DelegationChildChoice> {
    AGENT_IDS
        .iter()
        .map(|agent| {
            let mode = human_child_mode(agent, parent, parent_mode, plan_first);
            let known = if *agent == parent { DelegationAgentAvailability::Ready } else { availability(agent) };
            let blocked = match known {
                DelegationAgentAvailability::NotInstalled => Some(DelegationChildBlock::NotInstalled),
                DelegationAgentAvailability::SignedOut => Some(DelegationChildBlock::SignedOut),
                DelegationAgentAvailability::Ready if mode.is_none() => Some(DelegationChildBlock::NoMode),
                DelegationAgentAvailability::Ready if open_human_children >= crate::coordinator::MAXIMUM_OPEN_CHILDREN => {
                    Some(DelegationChildBlock::Cap)
                }
                DelegationAgentAvailability::Ready => None,
            };
            DelegationChildChoice { agent: agent.to_string(), mode, blocked }
        })
        .collect()
}

/// `CanCommit`: a `provider` child in `mode` can commit on its own branch.
pub fn can_commit(provider: &str, mode: &str) -> bool {
    provider == "claude" || mode == "fullAccess"
}

/// `AgentMayMerge`: a `provider` parent whose run started in `mode` may merge a child by its merge tool.
pub fn agent_may_merge(provider: &str, mode: &str) -> bool {
    provider == "claude" || mode == "fullAccess"
}

/// `DelegationIOHandler.Gate`: the answer of the first three rules, or None when the call may go on to a tool — switched off reaches
/// nothing; a pane whose agent has no delegation tools is refused with claude_only; a run started without the server reaches nothing.
pub fn gate(caller: &DelegationCaller, switch_on: bool) -> Option<DelegationResponse> {
    if !switch_on {
        return Some(DelegationResponse::failure(DETACHED_MESSAGE));
    }
    if !can_parent_by_agent(&caller.kind, &caller.provider) {
        return Some(DelegationResponse::refusal(DelegationReasonCode::ClaudeOnly));
    }
    (!caller.delegation).then(|| DelegationResponse::failure(DETACHED_MESSAGE))
}

/// `DelegationSwitch.IsClaudePane`: an agent pane running Claude.
pub fn is_claude_pane(kind: &str, provider: &str) -> bool {
    kind == "claude" && provider == "claude"
}

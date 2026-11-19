//! The delegate tool (C# Core/DelegationDelegateTool.cs; M/DelegationDelegateTool.swift): a parent Claude or Codex pane starts a child
//! pane on a task, in the child's own worktree, one layer deep; and the human's second send starts one from any agent pane. Owner: W7-A.

use std::sync::Arc;

use mighty_core::model_label::provider_label;
use mighty_platform::BoxFuture;

use crate::coordinator::{Coordinator, MAXIMUM_DEPTH, MAXIMUM_OPEN_CHILDREN, host_task};
use crate::copy::{DelegationCopy, DelegationCopyKind, MAXIMUM_COPY_BYTES};
use crate::git::worktree::{ChildWorktree, ChildWorktreeBase, ChildWorktreeCheck, ChildWorktreeOutcome};
use crate::host::{DelegationAgentAvailability, DelegationChildPane, DelegationPaneState};
use crate::model::{
    ChildRecord, ChildRecordOptions, ChildState, ChildStateEvent, ChildStates, DeliveryLane, Notice, NoticeKind, request_key,
};
use crate::providers;
use crate::pruning::DelegationAdmission;
use crate::request::{DelegationCaller, DelegationReasonCode, DelegationResponse, prefix};
use crate::store::{DelegationFileError, SaveError};

/// How long `Make` may take (C#'s default, 300 s).
const MAKE_TIMEOUT: f64 = 300.0;

/// `HumanChildStart`: the answer to a human's run of a text in a new child worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanChildStart {
    /// The child's record is saved in creating; its worktree, pane and first run follow in the background.
    Started(Box<ChildRecord>),
    /// Refused with exactly this reason before anything was made.
    Refused(DelegationReasonCode),
    /// Nothing was made, for this reason in the app's language.
    Failed(String),
}

/// `HumanChildFirstRun`: the first run of a child the human started — its whole input, and the app's note as the request that follows
/// it when the text is a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanChildFirstRun {
    pub input: String,
    pub following_note: Option<String>,
}

/// `HumanChildStarter`: how the app starts a child the human made from its parent's composer. `start` starts the child's first run in
/// its pane (its run id, or None when none started); `failed` hears that the child's worktree, pane or first run could not be made
/// after the send was answered — the child is failed, and the human's text needs another way.
#[derive(Clone)]
pub struct HumanChildStarter {
    pub start: Arc<dyn Fn(String, HumanChildFirstRun) -> BoxFuture<'static, Option<String>> + Send + Sync>,
    pub failed: Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>,
}

/// `Unavailable`: why the `provider` agent cannot run a child, naming it, or None when it can.
pub fn unavailable(provider: &str, availability: DelegationAgentAvailability) -> Option<String> {
    let label = provider_label(provider);
    match availability {
        DelegationAgentAvailability::NotInstalled => {
            Some(format!("{label} is not installed on this PC; the user can check it in Mighty Code's Settings › Components"))
        }
        DelegationAgentAvailability::SignedOut => {
            Some(format!("{label} is not signed in on this PC; the user can check it in Mighty Code's Settings › Components"))
        }
        DelegationAgentAvailability::Ready => None,
    }
}

/// `FirstInput`: the first message of a child's first run — its task and how it reports. A Claude child commits its work itself; a
/// child of any other agent leaves it for the app's commit. It names a Claude parent as one.
pub fn first_input(task: &str, worktree: &ChildWorktree, provider: &str, parent_agent: Option<&str>) -> String {
    let from = if parent_agent.unwrap_or(provider) == "claude" { "Another Claude pane" } else { "Another pane" };
    let done = if provider == "claude" {
        format!(
            "When you are done, commit your work on this branch, leaving no uncommitted changes to tracked files, and then write your report to {}",
            worktree.report_file()
        )
    } else {
        format!("When you are done, write your report to {}", worktree.report_file())
    };
    let leave = if provider == "claude" { String::new() } else { format!(" {}", app_commit_note(worktree)) };
    format!(
        "{from} in Mighty Code delegated this task to you. You work in your own git worktree on the branch {}; the task is also saved in {}.\n\n{done}: what you did, what is left and what your parent should check.{leave} Your parent gets one notice for each of your runs that ends with that file changed, and may merge your branch. Do not merge, push or switch branches.\n\nTask:\n{task}",
        worktree.branch,
        worktree.task_file()
    )
}

/// `AppCommitNote`: how a child of an agent other than Claude leaves its work — the app commits it on the child's branch when the child
/// writes its report.
pub fn app_commit_note(worktree: &ChildWorktree) -> String {
    format!(
        "Do not commit: leave your changes in this worktree. When your run ends with {} changed, Mighty Code commits every change .gitignore lets in on {} for you.",
        worktree.report_file(),
        worktree.branch
    )
}

/// `HumanFirstRun`: the first run of a child the human started — their text as they typed it, then the app's note. A slash command (a
/// text starting with `/`) runs alone, and the note follows as the child's next request.
pub fn human_first_run(text: &str, worktree: &ChildWorktree, provider: &str, parent_agent: Option<&str>) -> HumanChildFirstRun {
    let note = human_note(worktree, provider, parent_agent);
    if text.starts_with('/') {
        HumanChildFirstRun { input: text.to_string(), following_note: Some(note) }
    } else {
        HumanChildFirstRun { input: format!("{text}\n\n{note}"), following_note: None }
    }
}

/// `HumanNote`: the app's note to a child the human started. A Claude child commits its work itself; a child of any other agent leaves
/// it for the app. It names a Claude parent as one.
pub fn human_note(worktree: &ChildWorktree, provider: &str, parent_agent: Option<&str>) -> String {
    let beside = if parent_agent.unwrap_or(provider) == "claude" { "another Claude pane" } else { "another pane" };
    let (branch, report) = (&worktree.branch, worktree.report_file());
    if provider == "claude" {
        format!(
            "[Mighty Code] This request runs in its own git worktree on the branch {branch}, opened beside {beside} so that pane keeps working undisturbed. When you are done, commit your work on this branch, leaving no uncommitted changes to tracked files, and then write a short report to {report}: what you did and what is left. The person who sent this merges your branch from the app. Do not merge, push or switch branches."
        )
    } else {
        format!(
            "[Mighty Code] This request runs in its own git worktree on the branch {branch}, opened beside {beside} so that pane keeps working undisturbed. When you are done, write a short report to {report}: what you did and what is left. {} The person who sent this merges your branch from the app. Do not merge, push or switch branches.",
            app_commit_note(worktree)
        )
    }
}

/// `"An"` before a label starting with A, else `"A"`.
fn article(label: &str) -> &'static str {
    if label.starts_with('A') { "An" } else { "A" }
}

impl Coordinator {
    /// delegate(task, mode, agent) from `caller`. The first rule that applies answers:
    /// 1. agent_unknown: `agent` names no agent. Left out or blank, the child runs the caller's own agent.
    /// 2. a mode that is not one of the child agent's or one in which it could not change files, or an empty task, one with a NUL
    ///    character or over 64 KiB: an error.
    /// 3. the same call earlier in the caller's current run (the same request key) gets that same child, in whatever state it is now.
    /// 4. child_cannot_delegate, width_cap, wider_mode, agent_unavailable.
    /// 5. not_git, detached_head, unborn_branch, low_disk: the caller's folder cannot host a child worktree.
    /// 6. store_full.
    ///
    /// Every refusal comes before anything is made and stores no key. Otherwise the child's record is saved in creating, with the call's
    /// request key and a copy of its TASK.md, and the answer names it while its start goes on in the background.
    pub(crate) async fn delegate_now(&self, task: &str, mode: &str, agent: Option<&str>, caller: &DelegationCaller) -> DelegationResponse {
        let child_agent = match agent.filter(|named| !named.trim().is_empty()) {
            Some(named) => match providers::agent(named) {
                Some(found) => found.to_string(),
                None => {
                    return DelegationResponse::refusal_with(
                        DelegationReasonCode::AgentUnknown,
                        format!(
                            "\"{}\" is not an agent; agent must be claude, codex or antigravity, or left out for your own",
                            prefix(named, 40)
                        ),
                    );
                }
            },
            None => caller.provider.clone(),
        };
        let modes = providers::starting_modes(&child_agent);
        let label = provider_label(&child_agent);
        if !modes.contains(&mode) {
            return DelegationResponse::failure(if child_agent == caller.provider {
                format!("mode must be one of {}.", modes.join(", "))
            } else {
                format!("mode must be one of a {label} child's: {}.", modes.join(", "))
            });
        }
        if !providers::child_can_write(&child_agent, mode) {
            let writing: Vec<&str> = modes.iter().copied().filter(|m| providers::child_can_write(&child_agent, m)).collect();
            return DelegationResponse::failure(format!(
                "{} {label} child in {mode} mode cannot change files, so it could not do the task or leave its work for a merge. Ask for one of {}, no wider than your own mode.",
                article(label),
                writing.join(", ")
            ));
        }
        if task.trim().is_empty() {
            return DelegationResponse::failure("delegate needs a task: what the child should do.");
        }
        if task.contains('\0') {
            return DelegationResponse::failure("A task may not contain a NUL character.");
        }
        if task.len() > MAXIMUM_COPY_BYTES {
            return DelegationResponse::failure("A task may be at most 64 KiB.");
        }
        let parent_id = caller.pane_id.as_str();
        let Some(parent) = self.host_pane_state(parent_id).await else { return DelegationResponse::failure("The calling pane is closed.") };
        let other_agent = (child_agent != caller.provider).then_some(child_agent.as_str());
        let key = request_key(parent_id, parent.run_id.as_deref().unwrap_or(""), task, mode, other_agent);
        {
            let state = self.lock();
            if let Some(same) = same_call(&state.file.children, &key, parent_id) {
                return DelegationResponse::delegated(same);
            }
            if let Some(reason) = delegate_refusal(&state.file.children, parent_id, &parent, mode, &child_agent, false) {
                return DelegationResponse::refusal(reason);
            }
        }
        if other_agent.is_some()
            && let Some(why) = unavailable(&child_agent, self.host_agent_availability(&child_agent).await)
        {
            return DelegationResponse::refusal_with(DelegationReasonCode::AgentUnavailable, why);
        }
        let start = match self.inner.worktrees.check(&parent.folder).await {
            ChildWorktreeCheck::Ready(start) => start,
            ChildWorktreeCheck::Refused(reason) => return DelegationResponse::refusal(reason),
        };
        let context = self.prune_context().await;
        let child = {
            // Nothing below suspends until the record is saved, so two calls at once cannot both pass these checks.
            let mut state = self.lock();
            if let Some(repeated) = same_call(&state.file.children, &key, parent_id) {
                return DelegationResponse::delegated(repeated);
            }
            if let Some(late) = delegate_refusal(&state.file.children, parent_id, &parent, mode, &child_agent, false) {
                return DelegationResponse::refusal(late);
            }
            let id = mighty_core::wire::new_id();
            let child = ChildRecord::make_with(
                &id,
                parent_id,
                &self.inner.worktrees.worktree_path(&id),
                &start.parent_branch,
                &start.base_commit,
                mode,
                &key,
                ChildRecordOptions {
                    parent_checkout: Some(start.repository.clone()),
                    provider: Some(child_agent.clone()),
                    ..Default::default()
                },
            );
            let task_copy = DelegationCopy::of(&id, DelegationCopyKind::Task, 0, task.as_bytes(), None);
            let DelegationAdmission::Admitted(admitted) = state.file.admitting(child.clone(), &context, Some(task_copy)) else {
                return DelegationResponse::refusal(DelegationReasonCode::StoreFull);
            };
            match self.commit(&mut state, admitted, &context) {
                Ok(()) => {}
                Err(SaveError::File(DelegationFileError::Full)) => return DelegationResponse::refusal(DelegationReasonCode::StoreFull),
                Err(_) => return DelegationResponse::failure("The delegation file could not be saved, so no child was made."),
            }
            self.flags().starting.insert(id);
            child
        };
        self.spawn_start(child.id.clone(), start, task.to_string(), parent.provider.clone(), None);
        DelegationResponse::delegated(&child)
    }

    /// Starts the child's start on a task of its own, once delegate (or the human's send) has answered.
    fn spawn_start(&self, id: String, start: ChildWorktreeBase, task: String, parent_agent: String, human: Option<HumanChildStarter>) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            let guard = StartingGuard { coordinator: &coordinator, id: &id };
            Box::pin(coordinator.start_child(&id, &start, &task, &parent_agent, human.as_ref())).await;
            drop(guard);
        });
    }

    /// `StartChild`: the child's start — its worktree, then its pane of its own agent in the starting mode, then its first run, which
    /// `human` starts when the human's send made the child and the host otherwise. The child moves to running once that run has
    /// started, as the run its parent awaits, and otherwise to failed, with one failed_to_start notice when the parent's delegate made
    /// it, or `human`'s failure call when the human's send did, exactly once. Whatever a failed start made stays until a human discards
    /// the child; the app never removes it on its own.
    async fn start_child(&self, id: &str, start: &ChildWorktreeBase, task: &str, parent_agent: &str, human: Option<&HumanChildStarter>) {
        let mut failed_called = false;
        // The human's text needs another way once, whichever way the start ends without a run.
        let fail_once = |failed_called: &mut bool| {
            let call = human.filter(|_| !*failed_called).map(|human| (human.failed)(id.to_string()));
            *failed_called = true;
            async move {
                if let Some(call) = call {
                    host_task(call).await;
                }
            }
        };
        let child = match self.file().child(id) {
            Some(child) if child.state == ChildState::Creating => child.clone(),
            // A human's child that will never start: their text needs another way.
            _ => return fail_once(&mut failed_called).await,
        };
        let mut run_id = None;
        if let ChildWorktreeOutcome::Created(worktree) = self.inner.worktrees.make(id, start, task, MAKE_TIMEOUT).await {
            let pane = DelegationChildPane {
                session_id: id.to_string(),
                parent_session_id: child.parent_session_id.clone(),
                mode: child.starting_mode.clone(),
                folder: worktree.working_folder.clone(),
                started_by_human: !child.notifies_parent(),
                provider: child.agent().to_string(),
            };
            // A host that fails here has started nothing: the child is failed, never left creating.
            if self.host_create_pane(pane).await {
                run_id = match human {
                    Some(human) => {
                        let first = human_first_run(task, &worktree, child.agent(), Some(parent_agent));
                        host_task((human.start)(id.to_string(), first)).await.flatten()
                    }
                    None => self.host_start_run(id, &first_input(task, &worktree, child.agent(), Some(parent_agent))).await,
                };
            }
        }
        let context = self.prune_context().await;
        let after = {
            let mut state = self.lock();
            match state.file.child_index(id) {
                // With no run, the human's text has not gone anywhere.
                None => StartEnd::Gone,
                Some(index) => {
                    let mut next = crate::store::DelegationFile::clone(&state.file);
                    let moved = match &run_id {
                        // Once the host has reported this run's start, or even its end, the record is past creating.
                        Some(run) => next.children[index]
                            .noting_run(run)
                            .filter(|_| next.children[index].state == ChildState::Creating)
                            .map(|running| next.children[index] = running),
                        None => next.children[index].applying(ChildStateEvent::FailStart).map(|failed| {
                            if failed.notifies_parent() {
                                let notice =
                                    Notice::new(&mighty_core::wire::new_id(), id, 0, NoticeKind::FailedToStart, DeliveryLane::Pending);
                                next.notices.push(notice);
                            }
                            next.children[index] = failed;
                        }),
                    };
                    match moved {
                        None => StartEnd::Gone,
                        Some(()) => {
                            // The admitted record kept room for this child's report copy, far more than this needs. Should the disk
                            // still refuse, the app goes on with the move and the next save writes it.
                            self.save_or_keep(&mut state, next, &context);
                            if run_id.is_some() { StartEnd::Running } else { StartEnd::Failed }
                        }
                    }
                }
            }
        };
        match after {
            StartEnd::Running => {}
            StartEnd::Gone if run_id.is_some() => {}
            StartEnd::Gone => fail_once(&mut failed_called).await,
            // A failed start's notice goes to the parent's pane while the child still counts as starting.
            StartEnd::Failed => {
                self.deliver_pending().await;
                fail_once(&mut failed_called).await;
            }
        }
    }

    /// `StartHumanChild`: the human's run of `text` in a new child of the Claude, Codex or Antigravity pane `parent_id`, from its
    /// composer's second send or its worktree button's menu, which names the child's `agent`. The child runs the parent's agent unless
    /// `agent` names another, in [`providers::human_child_mode`]. It is marked started by the human, so it sends the parent no notices
    /// and the parent's tools neither list it nor act on it; it has its own cap of 3 open children started by the human. The first rule
    /// that applies answers, and nothing is made by any but the last:
    /// 1. delegation switched off, or `text` is no task (empty, with a NUL character or over 64 KiB): a failure in the app's language;
    /// 2. parent_closed, claude_only, agent_unknown, another agent whose CLI is not installed or signed out, or no mode in which the child
    ///    could change files; then delegate's own refusals (child_cannot_delegate, width_cap, wider_mode; not_git, detached_head,
    ///    unborn_branch, low_disk; store_full);
    /// 3. a delegation file the disk refuses: a failure.
    ///
    /// Otherwise the child's record is saved in creating and the answer names it, while its worktree, pane and first run follow in the
    /// background through `starter`.
    pub(crate) async fn start_human_child_now(
        &self,
        text: &str,
        parent_id: &str,
        starter: HumanChildStarter,
        plan_first: bool,
        agent: Option<&str>,
    ) -> HumanChildStart {
        let locale = self.locale();
        if !(self.inner.switch_on)() {
            return HumanChildStart::Failed(locale.get("delegation.human.error.switchedOff"));
        }
        if text.trim().is_empty() || text.contains('\0') || text.len() > MAXIMUM_COPY_BYTES {
            return HumanChildStart::Failed(locale.get("delegation.human.error.invalidTask"));
        }
        let Some(parent) = self.host_pane_state(parent_id).await else {
            return HumanChildStart::Refused(DelegationReasonCode::ParentClosed);
        };
        if !providers::is_agent_pane(&parent.kind, &parent.provider) {
            return HumanChildStart::Refused(DelegationReasonCode::ClaudeOnly);
        }
        let child_agent = agent.unwrap_or(&parent.provider).to_string();
        if !mighty_core::wire::PROVIDERS.contains(&child_agent.as_str()) {
            return HumanChildStart::Refused(DelegationReasonCode::AgentUnknown);
        }
        let label = provider_label(&child_agent);
        if child_agent != parent.provider {
            match self.host_agent_availability(&child_agent).await {
                DelegationAgentAvailability::NotInstalled => {
                    return HumanChildStart::Failed(
                        locale.get_with("delegation.human.error.agentNotInstalledWindows", &[("agent", label)]),
                    );
                }
                DelegationAgentAvailability::SignedOut => {
                    return HumanChildStart::Failed(locale.get_with("delegation.human.error.agentSignedOut", &[("agent", label)]));
                }
                DelegationAgentAvailability::Ready => {}
            }
        }
        let Some(mode) = providers::human_child_mode(&child_agent, &parent.provider, &parent.permission_mode, plan_first) else {
            return HumanChildStart::Failed(if child_agent == parent.provider {
                locale.get("delegation.human.error.readOnlyMode")
            } else {
                locale.get_with("delegation.human.error.agentNoMode", &[("agent", label)])
            });
        };
        if let Some(reason) = delegate_refusal(&self.lock().file.children, parent_id, &parent, &mode, &child_agent, true) {
            return HumanChildStart::Refused(reason);
        }
        let start = match self.inner.worktrees.check(&parent.folder).await {
            ChildWorktreeCheck::Ready(start) => start,
            ChildWorktreeCheck::Refused(reason) => return HumanChildStart::Refused(reason),
        };
        let context = self.prune_context().await;
        let child = {
            // Nothing below suspends until the record is saved, so two sends at once cannot both pass the width cap.
            let mut state = self.lock();
            if let Some(late) = delegate_refusal(&state.file.children, parent_id, &parent, &mode, &child_agent, true) {
                return HumanChildStart::Refused(late);
            }
            let id = mighty_core::wire::new_id();
            // No repeat to recognise: each send is its own child.
            let key = request_key(parent_id, &format!("human-{id}"), text, &mode, None);
            let child = ChildRecord::make_with(
                &id,
                parent_id,
                &self.inner.worktrees.worktree_path(&id),
                &start.parent_branch,
                &start.base_commit,
                &mode,
                &key,
                ChildRecordOptions {
                    parent_checkout: Some(start.repository.clone()),
                    started_by_human: Some(true),
                    provider: Some(child_agent.clone()),
                    ..Default::default()
                },
            );
            let task_copy = DelegationCopy::of(&id, DelegationCopyKind::Task, 0, text.as_bytes(), None);
            let DelegationAdmission::Admitted(admitted) = state.file.admitting(child.clone(), &context, Some(task_copy)) else {
                return HumanChildStart::Refused(DelegationReasonCode::StoreFull);
            };
            match self.commit(&mut state, admitted, &context) {
                Ok(()) => {}
                Err(SaveError::File(DelegationFileError::Full)) => return HumanChildStart::Refused(DelegationReasonCode::StoreFull),
                Err(_) => return HumanChildStart::Failed(locale.get("delegation.human.error.notSaved")),
            }
            self.flags().starting.insert(id);
            child
        };
        self.spawn_start(child.id.clone(), start, text.to_string(), parent.provider.clone(), Some(starter));
        HumanChildStart::Started(Box::new(child))
    }
}

/// How a child's start ended in the file.
enum StartEnd {
    /// It runs.
    Running,
    /// It is failed, saved with its notice when its parent's delegate made it.
    Failed,
    /// Its record is gone, or past the move the start would have made.
    Gone,
}

/// Takes the child out of `starting` once its start is over, however it ends.
struct StartingGuard<'a> {
    coordinator: &'a Coordinator,
    id: &'a str,
}

impl Drop for StartingGuard<'_> {
    fn drop(&mut self) {
        self.coordinator.flags().starting.remove(self.id);
    }
}

/// `SameCall`: the parent's child made by the same call (the same request key).
fn same_call<'a>(children: &'a [ChildRecord], key: &str, parent_id: &str) -> Option<&'a ChildRecord> {
    children.iter().find(|child| child.parent_session_id == parent_id && child.request_key == key)
}

/// `DelegateRefusal`: why the pane `parent_id`, now `parent`, may not have a new child of the `child` agent in `mode`, or None when it
/// may. The cap counts the open children of the same kind, of every agent: those its delegate made, or (`by_human`) those the human
/// started, so neither kind blocks the other.
fn delegate_refusal(
    children: &[ChildRecord],
    parent_id: &str,
    parent: &DelegationPaneState,
    mode: &str,
    child: &str,
    by_human: bool,
) -> Option<DelegationReasonCode> {
    // A pane with a parent link is already one layer down.
    let depth = usize::from(parent.parent_session_id.is_some() || children.iter().any(|record| record.id == parent_id));
    if depth >= MAXIMUM_DEPTH {
        return Some(DelegationReasonCode::ChildCannotDelegate);
    }
    let open = children
        .iter()
        .filter(|record| record.parent_session_id == parent_id && record.state.is_open() && record.notifies_parent() != by_human)
        .count();
    if open >= MAXIMUM_OPEN_CHILDREN {
        return Some(DelegationReasonCode::WidthCap);
    }
    (!providers::is_no_wider_across(mode, child, &parent.permission_mode, &parent.provider)).then_some(DelegationReasonCode::WiderMode)
}

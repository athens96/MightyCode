//! Report revisions (C# Core/DelegationReports.cs:51-273; M/DelegationReports.swift:47-236): the host tells the coordinator each run
//! that starts and ends in a child's pane, in order, and the coordinator records what the run left for the parent. These run events take
//! turns in the order they came, so one never sees another half recorded. Notices are saved pending, and then delivery offers them to
//! the parent's pane. REPORT.md itself is read by W7-B's [`crate::git::report_file`]. Owner: W7-A.

use crate::coordinator::Coordinator;
use crate::copy::{DelegationCopy, DelegationCopyKind};
use crate::git::auto_commit::{self, AutoCommitOutcome, AutoCommitResult};
use crate::git::{merge, report_file};
use crate::model::{ChildRecord, ChildState, ChildStateEvent, DeliveryLane, Notice, NoticeKind};

/// `ChildRunEnd`: how a run in a child's pane ended, as the host saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChildRunEnd {
    /// It finished normally.
    Finished,
    /// A human stopped it.
    Stopped,
    /// It ended with an error.
    Errored,
    /// Quitting the app killed it.
    Quit,
}

impl ChildRecord {
    /// `NotingRun`: the record brought to its run `run_id` going, as a new run — any new run sets running and clears reported, and a
    /// child's first run is the one its parent awaits. When the run going had a different id, its end was never reported; the new one
    /// carries on what the parent awaited. None, changing nothing, when this run's end is recorded already or the child takes no runs.
    pub fn noting_run(&self, run_id: &str) -> Option<ChildRecord> {
        if self.run_id.as_deref() == Some(run_id) {
            return matches!(self.state, ChildState::Running | ChildState::Waiting).then(|| self.clone());
        }
        let mut next = match self.state {
            ChildState::Creating => {
                let first = self.applying(ChildStateEvent::StartRun)?;
                ChildRecord { awaited_run_id: Some(run_id.to_string()), ..first }
            }
            ChildState::Running | ChildState::Waiting => {
                let mut next =
                    if self.state == ChildState::Waiting { self.applied(ChildStateEvent::AnswerPermission) } else { self.clone() };
                if self.run_id.is_some() && self.awaited_run_id == self.run_id {
                    next.awaited_run_id = Some(run_id.to_string());
                }
                next
            }
            _ => self.applying(ChildStateEvent::StartRun)?,
        };
        next.run_id = Some(run_id.to_string());
        Some(next)
    }
}

/// `IsIdleBetweenRuns`: a child in `state` is idle in an open pane after a run, so a new run there moves it to running.
pub fn is_idle_between_runs(state: ChildState) -> bool {
    matches!(state, ChildState::Reported | ChildState::Ended | ChildState::Interrupted | ChildState::Merged)
}

/// `MayEnd`: the end of the run `run_id` may be recorded for `child` — it is the run going now, or the child's first run.
pub fn may_end(child: &ChildRecord, run_id: &str) -> bool {
    if child.run_id.as_deref() == Some(run_id) {
        matches!(child.state, ChildState::Running | ChildState::Waiting)
    } else {
        child.state == ChildState::Creating
    }
}

impl Coordinator {
    /// `ChildRunStarted`: a run `run_id` started in the child `id`'s pane — the child is running it, which clears reported, so a merge
    /// waits for the next report. A run already recorded, or a child that takes no runs, changes nothing.
    pub(crate) async fn child_run_started_now(&self, id: &str, run_id: &str) {
        let _turn = self.inner.run_turns.lock().await;
        match self.file().child(id) {
            Some(found) if found.run_id.as_deref() != Some(run_id) => {}
            _ => return,
        }
        let context = self.prune_context().await;
        // Nothing below suspends until the start is saved.
        let mut state = self.lock();
        let Some(index) = state.file.child_index(id) else { return };
        if state.file.children[index].run_id.as_deref() == Some(run_id) {
            return;
        }
        let Some(running) = state.file.children[index].noting_run(run_id) else { return };
        let next = state.file.setting_child(index, running);
        self.save_or_keep(&mut state, next, &context);
    }

    /// `ChildRunEnded`: the run `run_id` in the child `id`'s pane ended. The first rule that applies records it:
    /// 1. quitting the app killed it: the child is interrupted, with no notice;
    /// 2. REPORT.md holds text that differs from the current revision's: the next revision is recorded at the head the child has checked
    ///    out, with a copy of the report, and the parent gets one reported notice. A child of an agent other than Claude has what it left
    ///    committed by the app first, and only when its run finished normally; when that commit finds an unresolved conflict or git
    ///    fails, nothing is committed, the child's pane says why and its report counts as not new yet;
    /// 3. a child reported before whose branch is still at its reported head, not merged since, is reported again at the same revision;
    ///    the run its parent awaits still sends one ended_without_report notice;
    /// 4. otherwise the child has ended, and when the run was the one its parent awaits the parent gets one ended_without_report notice.
    ///
    /// A child the human started is recorded the same way but never notifies its parent. Only the run going now (or the child's first
    /// run) is recorded. The notice sent, if any; once recorded the child is no longer marked ending, and what waited for it is offered.
    pub(crate) async fn child_run_ended_now(&self, id: &str, run_id: &str, end: ChildRunEnd) -> Option<Notice> {
        // The end is no longer marked ending however its record went, so the child's queue and delivery are never held for ever.
        let finish = FinishEnding { coordinator: self, id };
        let notice = Box::pin(self.record_run_end(id, run_id, end)).await;
        let waited = finish.finish();
        if notice.is_some() || waited {
            self.deliver_pending().await;
        }
        notice
    }

    async fn record_run_end(&self, id: &str, run_id: &str, end: ChildRunEnd) -> Option<Notice> {
        let _turn = self.inner.run_turns.lock().await;
        let found = self.file().child(id).cloned().filter(|found| may_end(found, run_id))?;
        let git = self.inner.worktrees.git().clone();
        let mut report = None;
        let mut head = None;
        let mut unchanged_head = None;
        let mut committed = AutoCommitResult::of(AutoCommitOutcome::Nothing);
        if end != ChildRunEnd::Quit {
            let path = crate::git::worktree::report_file_in(&found.worktree_path);
            report = tokio::task::spawn_blocking(move || report_file::read(&path)).await.ok().flatten();
            match &report {
                Some(read) if Some(&read.digest) != found.report_digest.as_ref() => {
                    if found.agent() != "claude" {
                        committed = if end == ChildRunEnd::Finished {
                            Box::pin(auto_commit::run(
                                &git,
                                &self.inner.paths,
                                &found.worktree_path,
                                id,
                                found.report_revision + 1,
                                found.agent(),
                            ))
                            .await
                        } else {
                            AutoCommitResult::of(AutoCommitOutcome::AfterNormalFinish)
                        };
                    }
                    // A report whose head git cannot read counts as no new report; its change is still there for the next run's end.
                    if matches!(committed.outcome, AutoCommitOutcome::Nothing | AutoCommitOutcome::Committed) {
                        head = merge::head(&git, &found.worktree_path).await;
                    }
                }
                _ if found.report_head.is_some() => unchanged_head = merge::head(&git, &found.worktree_path).await,
                _ => {}
            }
        }
        let context = self.prune_context().await;
        let notice = {
            // Nothing below suspends until the end is saved, so the same end reported twice finds it recorded.
            let mut state = self.lock();
            let mut next = crate::store::DelegationFile::clone(&state.file);
            let index = next.child_index(id)?;
            if !may_end(&next.children[index], run_id) || next.children[index].report_digest != found.report_digest {
                return None;
            }
            let mut child = next.children[index].noting_run(run_id)?;
            // The run's end settles any permission request it was waiting on.
            if child.state == ChildState::Waiting {
                child = child.applied(ChildStateEvent::AnswerPermission);
            }
            let new_notice = |child: &ChildRecord, kind: NoticeKind| {
                Notice::new(&mighty_core::wire::new_id(), id, child.report_revision, kind, DeliveryLane::Pending)
            };
            let mut notice = None;
            let last_merge_head = next.merges.iter().rev().find(|merge| merge.child_id == id).map(|merge| merge.child_head.clone());
            let reported = match (&report, &head) {
                (Some(read), Some(head)) if Some(&read.digest) != child.report_digest.as_ref() => {
                    child.recording_report(head).map(|reported| (read, reported))
                }
                _ => None,
            };
            if end == ChildRunEnd::Quit {
                child = child.applied(ChildStateEvent::QuitApp);
            } else if let Some((read, reported)) = reported {
                child = ChildRecord { report_digest: Some(read.digest.clone()), report_run_id: Some(run_id.to_string()), ..reported };
                next = next.setting_copy(DelegationCopy::of(
                    id,
                    DelegationCopyKind::Report,
                    i64::from(child.report_revision),
                    &read.start,
                    Some(read.total_bytes as i64),
                ));
                if child.notifies_parent() {
                    notice = Some(new_notice(&child, NoticeKind::Reported));
                }
            } else if let Some(standing) = unchanged_head
                .as_deref()
                .filter(|unchanged| {
                    child.report_head.as_deref().is_some_and(|reported| unchanged.eq_ignore_ascii_case(reported))
                        && !last_merge_head.as_deref().is_some_and(|merged| merged.eq_ignore_ascii_case(unchanged))
                })
                .and_then(|_| child.applying(ChildStateEvent::EndUnchanged))
            {
                // Nothing new since the report, which still stands: the card can merge it.
                child = standing;
                if child.awaited_run_id.as_deref() == Some(run_id) && child.notifies_parent() {
                    notice = Some(new_notice(&child, NoticeKind::EndedWithoutReport));
                }
            } else {
                child = child.applied(ChildStateEvent::EndWithoutReport);
                if child.awaited_run_id.as_deref() == Some(run_id) && child.notifies_parent() {
                    notice = Some(new_notice(&child, NoticeKind::EndedWithoutReport));
                }
            }
            next.children[index] = child;
            if let Some(notice) = &notice {
                next.notices.push(notice.clone());
            }
            self.save_or_keep(&mut state, next, &context);
            notice
        };
        let locale = self.locale();
        match committed.outcome {
            AutoCommitOutcome::Unmerged => self.host_note(locale.get("delegation.autoCommit.unmerged"), id).await,
            AutoCommitOutcome::Failed => {
                let why = committed.why.clone().unwrap_or_default();
                self.host_note(locale.get_with("delegation.autoCommit.failed", &[("error", &why)]), id).await;
            }
            // A stop or an error: nothing to say, the next normal finish commits.
            _ => {}
        }
        if !committed.left_out.is_empty() {
            let mut shown = committed.left_out.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
            if committed.left_out.len() > 10 {
                shown.push_str(", …");
            }
            let count = committed.left_out.len().to_string();
            self.host_note(locale.get_with("delegation.autoCommit.leftOut", &[("count", &count), ("paths", &shown)]), id).await;
        }
        notice
    }

    /// `AwaitRun`: the run `run_id` in the child `id`'s pane carries its parent's work — the run a follow-up went to. Its end without a
    /// new report sends the parent one ended_without_report notice; when the host has reported that end already, the notice is sent
    /// now. A follow-up that started a new run in the child's idle pane has the child running it at once, which clears reported. The
    /// notice sent, if any, which delivery then offers to the parent's pane.
    pub(crate) async fn await_run_now(&self, run_id: &str, id: &str) -> Option<Notice> {
        let notice = self.record_await(run_id, id).await;
        if notice.is_some() {
            self.deliver_pending().await;
        }
        notice
    }

    async fn record_await(&self, run_id: &str, id: &str) -> Option<Notice> {
        let _turn = self.inner.run_turns.lock().await;
        let found = self.file().child(id).cloned()?;
        if found.awaited_run_id.as_deref() == Some(run_id) {
            return None;
        }
        let context = self.prune_context().await;
        // Nothing below suspends until the await is saved.
        let mut state = self.lock();
        let mut next = crate::store::DelegationFile::clone(&state.file);
        let index = next.child_index(id)?;
        if next.children[index].awaited_run_id.as_deref() == Some(run_id) {
            return None;
        }
        let mut child = next.children[index].clone();
        let mut notice = None;
        // Ended with no new report: ended, or reported still from an earlier run.
        let ended_unreported =
            child.state == ChildState::Ended || child.state == ChildState::Reported && child.report_run_id.as_deref() != Some(run_id);
        if child.run_id.as_deref() == Some(run_id) && ended_unreported && child.notifies_parent() {
            let made =
                Notice::new(&mighty_core::wire::new_id(), id, child.report_revision, NoticeKind::EndedWithoutReport, DeliveryLane::Pending);
            next.notices.push(made.clone());
            notice = Some(made);
        } else if child.run_id.as_deref() != Some(run_id) && is_idle_between_runs(child.state) {
            child = child.noting_run(run_id).unwrap_or(child);
        }
        child.awaited_run_id = Some(run_id.to_string());
        next.children[index] = child;
        self.save_or_keep(&mut state, next, &context);
        notice
    }

    /// `InterruptRunsKilledByQuit`: at launch, before any pane runs again, every child still running or waiting had its run killed when
    /// the app quit. Each becomes interrupted, with no notice, and comes back stopped. The ids of those children.
    pub(crate) async fn interrupt_runs_killed_by_quit_now(&self) -> Vec<String> {
        let _turn = self.inner.run_turns.lock().await;
        let context = self.prune_context().await;
        let mut state = self.lock();
        let mut next = crate::store::DelegationFile::clone(&state.file);
        let mut ids = vec![];
        for child in next.children.iter_mut() {
            if !matches!(child.state, ChildState::Running | ChildState::Waiting) {
                continue;
            }
            let answered =
                if child.state == ChildState::Waiting { child.applied(ChildStateEvent::AnswerPermission) } else { child.clone() };
            *child = answered.applied(ChildStateEvent::QuitApp);
            ids.push(child.id.clone());
        }
        if !ids.is_empty() {
            self.save_or_keep(&mut state, next, &context);
        }
        ids
    }
}

/// Takes one reported end of the child off [`crate::coordinator::ChildRunsEnding`], however the record went (C#'s `finally`).
struct FinishEnding<'a> {
    coordinator: &'a Coordinator,
    id: &'a str,
}

impl FinishEnding<'_> {
    /// Finishes now: whether the end was marked.
    fn finish(self) -> bool {
        let waited = self.coordinator.ending_runs().finish(self.id);
        std::mem::forget(self);
        waited
    }
}

impl Drop for FinishEnding<'_> {
    fn drop(&mut self) {
        self.coordinator.ending_runs().finish(self.id);
    }
}

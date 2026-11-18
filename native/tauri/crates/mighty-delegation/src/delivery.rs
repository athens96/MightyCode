//! Delivery (C# Core/DelegationDelivery.cs; M/DelegationDelivery.swift:92-314): each notice goes to its child's parent pane and each
//! follow-up to its child's pane, exactly once. A new item is offered to its pane at once, by the first rule that applies:
//!
//! 0. The pane is a child whose run end is not recorded yet ([`crate::coordinator::ChildRunsEnding`]): the item stays pending, and the
//!    end's record offers it again.
//! 1. Items are held for the pane already: the item is held behind them, whatever the pane is doing.
//! 2. The pane runs Claude now: the item is steered into that run.
//! 3. The pane runs another agent now, whose run takes no text: the item stays pending, and the end of that run offers it again.
//! 4. The pane is idle after a run that finished normally since this launch, and a run can start in it now: the item starts its next run.
//! 5. Otherwise (stopped, errored, no run since this launch, cannot start a run now, or closed) the item is held.
//!
//! Only the human's next send or run next releases held items, all of the pane's together, oldest first, in one run. A closed parent's
//! held notices stay on its children's cards. Before a pane gets an item, the item is saved delivered with its receipt, so it is never
//! handed over twice; should the pane not take it after all, it goes back as it was. Should the pane not take a new item twice running,
//! the item stays pending and is offered again with the next one, or when the app sees one of its panes' runs start or end; until then
//! the pane's newer items wait behind it. What is still pending at launch is held. Owner: W7-A.

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::coordinator::Coordinator;
use crate::host::{DelegationPaneActivity, DelegationPaneState};
use crate::model::{DeliveryLane, DeliveryReceipt, DeliveryRoute, FollowUp, Notice, NoticeKind};
use crate::providers;
use crate::request::{DelegationReasonCode, delegation_names};
use crate::store::DelegationFile;

/// `DelegationCoordinator.DeliveryOffers`: how often one new item is offered to its pane in a row — a run that ends just as an item
/// is steered into it gets it as its next run.
pub const DELIVERY_OFFERS: usize = 2;

/// `DelegationDeliveryKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegationDeliveryKind {
    Notice,
    FollowUp,
}

delegation_names!(serde DelegationDeliveryKind { Notice => "notice", FollowUp => "follow_up" });

/// `DelegationDeliveryItem`: one notice or follow-up as delivery hands it to a pane. `session_id`: the pane it goes to — the child's
/// parent for a notice, the child itself for a follow-up; `text`: what the pane's run gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationDeliveryItem {
    pub id: String,
    pub kind: DelegationDeliveryKind,
    pub child_id: String,
    pub session_id: String,
    pub text: String,
}

/// `DelegationRelease`: what one release of a pane's held items did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationRelease {
    /// Nothing is held for the pane: the caller sends or runs next as it would anyway.
    NothingHeld,
    /// Every item held for the pane went to it, oldest first, in the one run `run_id`.
    Released { run_id: String, item_ids: Vec<String> },
    /// No run started (or the pane is closed), so they all stay held.
    NotStarted,
}

/// `DelegationQueueRows.RemoveRefusal`: held_not_removable when `item_id` is one of the pane's held rows (the remove route changes
/// nothing); None when it may go on.
pub fn remove_refusal(item_id: &str, held: &[DelegationDeliveryItem]) -> Option<DelegationReasonCode> {
    held.iter().any(|item| item.id == item_id).then_some(DelegationReasonCode::HeldNotRemovable)
}

/// One row of the phone's `queued` array (C# `MobileQueuedItem(Id, Text, RunNow?, RunNowReason?)`; mighty-remote's
/// `phone::desk::MobileQueuedItem` is the wire record): `run_now` and `run_now_reason` from `QueueRunNow.Wire`, both None when the row
/// has no button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileQueuedRow {
    pub id: String,
    pub text: String,
    pub run_now: Option<&'static str>,
    pub run_now_reason: Option<&'static str>,
}

/// `DelegationQueueRows.MobileItems` (M/DelegationDelivery.swift:31-52): the pane's rows in the phone's `queued` array, the items held
/// for it first (oldest first, as they run first), then the human's queued requests. `run_now`: what 바로 실행 does on the row with that
/// id; without it, or for `Hidden`, the row carries no `runNow`.
pub fn mobile_items(
    held: &[DelegationDeliveryItem],
    queued: &[mighty_composer::queue::QueuedComposerInput],
    run_now: Option<&dyn Fn(&str) -> mighty_composer::queue::QueueRunNowPath>,
) -> Vec<MobileQueuedRow> {
    let item = |id: &str, text: &str| {
        let path = run_now.map_or(mighty_composer::queue::QueueRunNowPath::Hidden, |f| f(id));
        let wire = path.wire();
        MobileQueuedRow { id: id.into(), text: text.into(), run_now: wire.map(|w| w.0), run_now_reason: wire.and_then(|w| w.1) }
    };
    held.iter().map(|h| item(&h.id, &h.text)).chain(queued.iter().map(|q| item(&q.id, &q.text))).collect()
}

/// `ChildWorktree.ReportFileIn` (W7-B's): `<worktree>/.mighty-delegation/REPORT.md`, where a child writes its report.
pub use crate::git::worktree::report_file_in;

impl DelegationFile {
    /// `HoldingPending`: this file with every notice and follow-up still pending held, or None when none was. At launch nothing is
    /// delivered on its own: what the last launch left pending waits for the human's next send or run next.
    pub fn holding_pending(&self) -> Option<DelegationFile> {
        let pending =
            self.notices.iter().any(|n| n.lane == DeliveryLane::Pending) || self.follow_ups.iter().any(|f| f.lane == DeliveryLane::Pending);
        pending.then(|| {
            let hold = |lane: DeliveryLane| if lane == DeliveryLane::Pending { DeliveryLane::Held } else { lane };
            DelegationFile {
                notices: self.notices.iter().map(|n| Notice { lane: hold(n.lane), ..n.clone() }).collect(),
                follow_ups: self.follow_ups.iter().map(|f| FollowUp { lane: hold(f.lane), ..f.clone() }).collect(),
                ..self.clone()
            }
        })
    }

    /// `Items`: the items in `lane`, notices first, each kind in the order its items were made.
    pub fn items(&self, lane: DeliveryLane) -> Vec<DelegationDeliveryItem> {
        let notices = self.notices.iter().filter(|n| n.lane == lane).filter_map(|n| self.notice_item(n));
        let follow_ups = self.follow_ups.iter().filter(|f| f.lane == lane).filter_map(|f| self.follow_up_item(f));
        notices.chain(follow_ups).collect()
    }

    /// `HeldItemsByPane`: the items held for each pane, oldest first — the rows its queued list shows. A pane is a parent with notices
    /// or a child with follow-ups, never both, so each list is in the order its items were made. Panes in the order of their first item.
    pub fn held_items_by_pane(&self) -> IndexMap<String, Vec<DelegationDeliveryItem>> {
        let mut panes: IndexMap<String, Vec<DelegationDeliveryItem>> = IndexMap::new();
        for item in self.items(DeliveryLane::Held) {
            panes.entry(item.session_id.clone()).or_default().push(item);
        }
        panes
    }

    /// `HeldNotices`: the held notices from the child `child_id`, oldest first.
    pub fn held_notices(&self, child_id: &str) -> Vec<Notice> {
        self.notices.iter().filter(|n| n.child_id == child_id && n.lane == DeliveryLane::Held).cloned().collect()
    }

    /// `DeliveryItem(notice)`: the notice as its child's parent pane gets it; None without the child.
    pub fn notice_item(&self, notice: &Notice) -> Option<DelegationDeliveryItem> {
        let child = self.child(&notice.child_id)?;
        Some(DelegationDeliveryItem {
            id: notice.id.clone(),
            kind: DelegationDeliveryKind::Notice,
            child_id: notice.child_id.clone(),
            session_id: child.parent_session_id.clone(),
            text: notice_text(notice),
        })
    }

    /// `DeliveryItem(followUp)`: the follow-up as its child's pane gets it; None without the child.
    pub fn follow_up_item(&self, follow_up: &FollowUp) -> Option<DelegationDeliveryItem> {
        let child = self.child(&follow_up.child_id)?;
        Some(DelegationDeliveryItem {
            id: follow_up.id.clone(),
            kind: DelegationDeliveryKind::FollowUp,
            child_id: follow_up.child_id.clone(),
            session_id: follow_up.child_id.clone(),
            text: follow_up_text(follow_up, &report_file_in(&child.worktree_path), child.agent()),
        })
    }

    /// `Updating`: this file with the lane and receipt of each of `items` changed by `change(lane, receipt)`.
    pub fn updating(
        &self,
        items: &[DelegationDeliveryItem],
        change: impl Fn(DeliveryLane, Option<&DeliveryReceipt>) -> (DeliveryLane, Option<DeliveryReceipt>),
    ) -> DelegationFile {
        let mut file = self.clone();
        for item in items {
            match item.kind {
                DelegationDeliveryKind::Notice => {
                    if let Some(notice) = file.notices.iter_mut().find(|n| n.id == item.id) {
                        (notice.lane, notice.receipt) = change(notice.lane, notice.receipt.as_ref());
                    }
                }
                DelegationDeliveryKind::FollowUp => {
                    if let Some(follow_up) = file.follow_ups.iter_mut().find(|f| f.id == item.id) {
                        (follow_up.lane, follow_up.receipt) = change(follow_up.lane, follow_up.receipt.as_ref());
                    }
                }
            }
        }
        file
    }

    /// The items for the pane `session_id` in `lane`, oldest first.
    pub fn pane_items(&self, session_id: &str, lane: DeliveryLane) -> Vec<DelegationDeliveryItem> {
        self.items(lane).into_iter().filter(|item| item.session_id == session_id).collect()
    }

    /// `LookUp`: the item `id` of `kind` while it is in `lane`.
    pub fn look_up(&self, id: &str, kind: DelegationDeliveryKind, lane: DeliveryLane) -> Option<DelegationDeliveryItem> {
        match kind {
            DelegationDeliveryKind::Notice => self.notices.iter().find(|n| n.id == id && n.lane == lane).and_then(|n| self.notice_item(n)),
            DelegationDeliveryKind::FollowUp => {
                self.follow_ups.iter().find(|f| f.id == id && f.lane == lane).and_then(|f| self.follow_up_item(f))
            }
        }
    }
}

/// `AutomaticDelivery`: how an item reaches a pane with no human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomaticDelivery {
    /// Steered into its running Claude run, or as its next run.
    Route(DeliveryRoute),
    /// Not now: the pane runs an agent whose run takes no text. The item stays pending, and the run's end offers it again.
    Later,
    /// Held until the human's next send or run next.
    Hold,
}

/// `Automatic`: steered into a running Claude run, later for any other running agent, or as the next run of a pane idle after a run
/// that finished normally since this launch. Held otherwise.
pub fn automatic(pane: Option<&DelegationPaneState>) -> AutomaticDelivery {
    match pane {
        Some(pane) if pane.activity == DelegationPaneActivity::Running && providers::can_steer(&pane.kind, &pane.provider) => {
            AutomaticDelivery::Route(DeliveryRoute::Steer)
        }
        Some(pane) if pane.activity == DelegationPaneActivity::Running && providers::is_agent_pane(&pane.kind, &pane.provider) => {
            AutomaticDelivery::Later
        }
        Some(pane) if pane.activity == DelegationPaneActivity::Finished => AutomaticDelivery::Route(DeliveryRoute::Queue),
        _ => AutomaticDelivery::Hold,
    }
}

/// `Text(notice)`: a notice as its parent's run reads it — a pointer naming the notice and the child. The report itself is read
/// through child_status.
pub fn notice_text(notice: &Notice) -> String {
    let head = format!("[Mighty Code notice {}]", notice.id);
    let (child, revision) = (&notice.child_id, notice.report_revision);
    match notice.kind {
        NoticeKind::Reported => format!(
            "{head} Your child {child} reported revision {revision}. Read the report with child_status; merge it with merge when it is right."
        ),
        NoticeKind::EndedWithoutReport => format!(
            "{head} Your child {child}'s run ended without a new report; its report revision is still {revision}. Call child_status to see where it stands."
        ),
        NoticeKind::FailedToStart => format!("{head} Your child {child} could not be started. Only a human can discard it, from its card."),
    }
}

/// `Text(followUp, reportFile, provider)`: a follow-up as its child's run reads it. A child of an agent other than Claude leaves its
/// work for the app's commit.
pub fn follow_up_text(follow_up: &FollowUp, report_file: &str, provider: &str) -> String {
    let done = if provider == "claude" {
        format!("When you are done, commit your work on your branch and write your updated report to {report_file}.")
    } else {
        format!(
            "When you are done, write your updated report to {report_file}. Do not commit: leave your changes, and Mighty Code commits them on your branch when your run ends with that file changed."
        )
    };
    format!("[Mighty Code follow-up {}] Your parent sent you this follow-up:\n\n{}\n\n{done}", follow_up.id, follow_up.text)
}

/// `DelegationCoordinator.Now`: `yyyy-MM-ddTHH:mm:ssZ` in UTC.
pub fn now() -> String {
    let o = mighty_core::time::DateTimeOffset::now().to_o();
    format!("{}Z", &o[..19])
}

impl Coordinator {
    /// `HeldItems`: the items held for the pane `session_id`, oldest first — rows its queued list shows, which cannot be removed.
    pub fn held_items(&self, session_id: &str) -> Vec<DelegationDeliveryItem> {
        self.file().pane_items(session_id, DeliveryLane::Held)
    }

    /// `HeldNotices`: the held notices from the child `child_id`. When its parent pane is closed nothing releases them: they stay on the
    /// child's card, which counts them.
    pub fn held_notices(&self, child_id: &str) -> Vec<Notice> {
        self.file().held_notices(child_id)
    }

    /// `HeldItemsByPane`, as the file is now.
    pub fn held_items_by_pane(&self) -> IndexMap<String, Vec<DelegationDeliveryItem>> {
        self.file().held_items_by_pane()
    }

    /// `Send`: the human's send of `text` in the pane `session_id`. When items are held for it, all of them go with the send, oldest
    /// first and ahead of `text`, in the one run the host starts — or `start` when given: the app's send starts the run with the
    /// human's attachments too, gets the run's whole input and answers the new run's id, or None when none started. Otherwise nothing
    /// changes and the caller sends `text` as it would anyway.
    pub(crate) async fn send_now(&self, text: &str, session_id: &str, start: Option<crate::coordinator::RunStarter>) -> DelegationRelease {
        self.release(session_id, Some(text), start).await
    }

    /// `RunNext`: run next in the pane `session_id`. When items are held for it, all of them and nothing else go to the pane, oldest
    /// first, in one run: steered into its running Claude run, or as its next run. Otherwise nothing changes and the caller runs its
    /// next queued row.
    pub(crate) async fn run_next_now(&self, session_id: &str) -> DelegationRelease {
        self.release(session_id, None, None).await
    }

    /// `DeliverPending`: offers every pending item to its pane, oldest first. The app calls it whenever one of its panes' runs starts
    /// or ends. A call while a pass is going asks that pass for one more and returns at once. An item its pane still did not take keeps
    /// the pane's newer items waiting behind it for the rest of the pass; the next pass offers them again, oldest first.
    ///
    /// A named future, not an `async fn`: the pass awaits run events that call it again, and a boxed type ends that cycle.
    pub fn deliver_pending(&self) -> mighty_platform::BoxFuture<'static, ()> {
        let coordinator = self.clone();
        Box::pin(async move { coordinator.isolated(|coordinator| async move { coordinator.deliver_pending_now().await }).await })
    }

    async fn deliver_pending_now(&self) {
        {
            let mut state = self.lock();
            state.wants_delivery = true;
            if state.is_delivering {
                return;
            }
            state.is_delivering = true;
        }
        // Should the pass end early (its task dropped or panicking), the flag is cleared all the same, so the next pass offers the rest.
        let mut delivering = DeliveringFlag { coordinator: self, armed: true };
        loop {
            let pending: Vec<(String, DelegationDeliveryKind)> = {
                let mut state = self.lock();
                if !state.wants_delivery {
                    // Cleared in the same hold that saw nothing more asked for, so a call meanwhile starts its own pass.
                    state.is_delivering = false;
                    delivering.armed = false;
                    break;
                }
                state.wants_delivery = false;
                state.file.items(DeliveryLane::Pending).into_iter().map(|item| (item.id, item.kind)).collect()
            };
            let mut waiting: HashSet<String> = HashSet::new();
            for (id, kind) in pending {
                let Some(item) = self.file().look_up(&id, kind, DeliveryLane::Pending) else { continue };
                if waiting.contains(&item.session_id) {
                    continue;
                }
                Box::pin(self.offer(&id, kind)).await;
                if self.file().look_up(&id, kind, DeliveryLane::Pending).is_some() {
                    waiting.insert(item.session_id);
                }
            }
        }
    }

    /// `Offer`: offers the pending item `id` to its pane as the pane is now.
    async fn offer(&self, id: &str, kind: DelegationDeliveryKind) {
        for _ in 0..DELIVERY_OFFERS {
            let Some(found) = self.file().look_up(id, kind, DeliveryLane::Pending) else { return };
            if self.ending_runs().contains(&found.session_id) {
                return;
            }
            let pane = self.host_pane_state(&found.session_id).await;
            let context = self.prune_context().await;
            let (item, route) = {
                // Nothing below suspends until the item's lane is saved, so nothing else moves it meanwhile.
                let mut state = self.lock();
                let Some(item) = state.file.look_up(id, kind, DeliveryLane::Pending) else { return };
                // Behind items already held for the pane it is held too, so the human's next send or run next releases them all.
                let decision = if state.file.pane_items(&item.session_id, DeliveryLane::Held).is_empty() {
                    automatic(pane.as_ref())
                } else {
                    AutomaticDelivery::Hold
                };
                let route = match decision {
                    AutomaticDelivery::Route(route) => route,
                    // A run that takes no text: its end offers the item again.
                    AutomaticDelivery::Later => return,
                    AutomaticDelivery::Hold => {
                        self.keep_moving(&mut state, std::slice::from_ref(&item), DeliveryLane::Held, None, &context);
                        return;
                    }
                };
                // A card's merge or undo is changing the pane's checkout: no run starts there now, and the item is offered again later.
                if route == DeliveryRoute::Queue && self.flags().card_panes.contains(&item.session_id) {
                    return;
                }
                let run = if route == DeliveryRoute::Steer {
                    pane.as_ref().and_then(|p| p.run_id.clone()).unwrap_or_default()
                } else {
                    String::new()
                };
                let receipt = DeliveryReceipt::new(&now(), route, &run);
                if !self.save_moving(&mut state, std::slice::from_ref(&item), DeliveryLane::Delivered, Some(receipt), &context) {
                    return;
                }
                (item, route)
            };
            if let Some(run_id) = self.host_deliver(&item.text, &item.session_id, route).await {
                self.received(std::slice::from_ref(&item), &run_id, &context).await;
                return;
            }
            let mut state = self.lock();
            self.keep_moving(&mut state, std::slice::from_ref(&item), DeliveryLane::Pending, None, &context);
        }
    }

    /// `Release`: all of `session_id`'s held items, oldest first, ahead of `text` (none for run next) in one run, which `start` starts
    /// (the host when None). Run next steers them into the pane's running Claude run instead.
    async fn release(&self, session_id: &str, text: Option<&str>, start: Option<crate::coordinator::RunStarter>) -> DelegationRelease {
        if self.file().pane_items(session_id, DeliveryLane::Held).is_empty() {
            return DelegationRelease::NothingHeld;
        }
        let Some(pane) = self.host_pane_state(session_id).await else { return DelegationRelease::NotStarted };
        let steers = text.is_none() && pane.activity == DelegationPaneActivity::Running && providers::can_steer(&pane.kind, &pane.provider);
        let context = self.prune_context().await;
        let (held, input) = {
            // Nothing below suspends until the release is saved, so two sends at once never take the same item.
            let mut state = self.lock();
            let held = state.file.pane_items(session_id, DeliveryLane::Held);
            if held.is_empty() {
                return DelegationRelease::NothingHeld;
            }
            let route = if steers { DeliveryRoute::Steer } else { DeliveryRoute::Queue };
            let receipt =
                DeliveryReceipt::new(&now(), route, &if steers { pane.run_id.clone().unwrap_or_default() } else { String::new() });
            if !self.save_moving(&mut state, &held, DeliveryLane::Delivered, Some(receipt), &context) {
                return DelegationRelease::NotStarted;
            }
            let mut parts: Vec<&str> = held.iter().map(|item| item.text.as_str()).collect();
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                parts.push(text);
            }
            let input = parts.join("\n\n");
            (held, input)
        };
        let run_id = if steers {
            self.host_deliver(&input, session_id, DeliveryRoute::Steer).await
        } else if let Some(start) = start {
            crate::coordinator::host_task(start(input)).await.flatten()
        } else {
            self.host_start_run(session_id, &input).await
        };
        let Some(run_id) = run_id else {
            let mut state = self.lock();
            self.keep_moving(&mut state, &held, DeliveryLane::Held, None, &context);
            return DelegationRelease::NotStarted;
        };
        self.received(&held, &run_id, &context).await;
        DelegationRelease::Released { run_id, item_ids: held.iter().map(|item| item.id.clone()).collect() }
    }

    /// `Received`: the run `run_id` got `items` — their receipts name it, and a follow-up's run is the one its child's parent now
    /// awaits.
    async fn received(&self, items: &[DelegationDeliveryItem], run_id: &str, context: &crate::pruning::DelegationPruneContext) {
        {
            let mut state = self.lock();
            let next = state.file.updating(items, |lane, receipt| {
                let receipt = match receipt {
                    Some(receipt) if lane == DeliveryLane::Delivered => {
                        Some(DeliveryReceipt { run_id: run_id.to_string(), ..receipt.clone() })
                    }
                    other => other.cloned(),
                };
                (lane, receipt)
            });
            self.save_or_keep(&mut state, next, context);
        }
        let mut children: Vec<&str> = vec![];
        for item in items.iter().filter(|item| item.kind == DelegationDeliveryKind::FollowUp) {
            if !children.contains(&item.child_id.as_str()) {
                children.push(&item.child_id);
            }
        }
        for child in children {
            Box::pin(self.await_run(run_id, child)).await;
        }
    }

    /// `SaveMoving`: saves `items` moved to `lane`; false, changing nothing, when the disk refuses it.
    fn save_moving(
        &self,
        state: &mut crate::coordinator::State,
        items: &[DelegationDeliveryItem],
        lane: DeliveryLane,
        receipt: Option<DeliveryReceipt>,
        context: &crate::pruning::DelegationPruneContext,
    ) -> bool {
        let next = state.file.updating(items, |_, _| (lane, receipt.clone()));
        self.commit(state, next, context).is_ok()
    }

    /// `KeepMoving`: moves `items` to `lane`; should the disk refuse it, the app goes on with the move and the next save writes it.
    fn keep_moving(
        &self,
        state: &mut crate::coordinator::State,
        items: &[DelegationDeliveryItem],
        lane: DeliveryLane,
        receipt: Option<DeliveryReceipt>,
        context: &crate::pruning::DelegationPruneContext,
    ) {
        let next = state.file.updating(items, |_, _| (lane, receipt.clone()));
        self.save_or_keep(state, next, context);
    }
}

/// Clears `is_delivering` when a delivery pass ends early, however it ends.
struct DeliveringFlag<'a> {
    coordinator: &'a Coordinator,
    armed: bool,
}

impl Drop for DeliveringFlag<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.coordinator.lock().is_delivering = false;
        }
    }
}

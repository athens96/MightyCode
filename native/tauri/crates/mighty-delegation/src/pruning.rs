//! Admission and pruning (C# Core/DelegationPruning.cs; M/DelegationPruning.swift): room for one more child, and pruning oldest first
//! when the file nears its 4 MiB cap — first the report copies of closed and discarded children, then whole closed and discarded
//! children that are fully delivered and have no undoable merge. Never open or failed children, held items of open panes or undoable
//! merges. Owner: W7-A.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde::Serialize;

use crate::copy::{DelegationCopy, DelegationCopyKind, MAXIMUM_COPY_BYTES};
use crate::model::{ChildRecord, ChildState, DeliveryLane, MergeRecord};
use crate::request::DelegationReasonCode;
use crate::store::{self, DelegationFile, MAXIMUM_FILE_BYTES};

/// `DelegationPruneContext`: what pruning needs to know from outside the file. `open_pane_ids`: every pane open now (a parent missing
/// here is closed). `can_undo`: whether a merge can still be undone; such a merge record, and its child's record, are never pruned.
pub struct DelegationPruneContext {
    pub open_pane_ids: HashSet<String>,
    pub can_undo: Box<dyn Fn(&MergeRecord) -> bool + Send + Sync>,
}

impl DelegationPruneContext {
    pub fn new(open_pane_ids: HashSet<String>, can_undo: impl Fn(&MergeRecord) -> bool + Send + Sync + 'static) -> DelegationPruneContext {
        DelegationPruneContext { open_pane_ids, can_undo: Box::new(can_undo) }
    }
}

impl std::fmt::Debug for DelegationPruneContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationPruneContext").field("open_pane_ids", &self.open_pane_ids).finish_non_exhaustive()
    }
}

/// `DelegationAdmission`: the answer to delegate's request for room for one more child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationAdmission {
    /// The file with the child's record (and its TASK copy) added, pruned so that the child's full report copy still fits. Saving it is
    /// the caller's step.
    Admitted(DelegationFile),
    /// No room even after pruning: always store_full. Nothing changed.
    Refused(DelegationReasonCode),
}

/// `DelegationFile.RecordAllowance`: the room kept for one more child's record, beside its report copy.
pub const RECORD_ALLOWANCE: usize = 4 * 1024;

/// `ReportCopyRoom`: the room the largest report copy of `child_id` takes in the file — cut at 64 KiB behind the marker, at the
/// longest revision and file size.
pub fn report_copy_room(child_id: &str) -> usize {
    let longer = vec![b'x'; MAXIMUM_COPY_BYTES + 1];
    element_bytes(&DelegationCopy::of(child_id, DelegationCopyKind::Report, i64::MAX, &longer, Some(i64::MAX)))
}

/// `NewChildRoom`: the room one more child takes — a record allowance and a full report copy.
pub fn new_child_room() -> usize {
    static ROOM: LazyLock<usize> = LazyLock::new(|| RECORD_ALLOWANCE + report_copy_room(&"x".repeat(36)));
    *ROOM
}

/// `ElementBytes`: the bytes one array element adds to the encoded file, its separator included.
pub fn element_bytes<T: Serialize + ?Sized>(item: &T) -> usize {
    store::encode(item).len() + 1
}

impl DelegationFile {
    /// `Pruned`: this file pruned until it, plus `headroom` more bytes, fits under the 4 MiB cap, or None when it does not fit even
    /// after everything allowed is pruned. A file that already fits comes back unchanged. Oldest first (records keep the order they
    /// were made in), stopping as soon as the file fits: first the report copies of closed records (closed or discarded), then closed
    /// records that are fully delivered, each with its notices, follow-ups, merge records and copies. A notice held for a child whose
    /// parent pane is closed counts as delivered here.
    pub fn pruned(&self, context: &DelegationPruneContext, headroom: i64) -> Option<DelegationFile> {
        let cap = MAXIMUM_FILE_BYTES as i64;
        let mut file = self.clone();
        let mut size = store::encode(&file).len() as i64;
        if size + headroom <= cap {
            return Some(file);
        }
        // After a removal `size` may undercount by a separator per emptied array, so a fit is confirmed with the exact size first.
        let fits = |file: &DelegationFile, size: &mut i64| {
            if *size + headroom > cap {
                return false;
            }
            *size = store::encode(file).len() as i64;
            *size + headroom <= cap
        };
        let closed: Vec<&ChildRecord> =
            self.children.iter().filter(|child| matches!(child.state, ChildState::Closed | ChildState::Discarded)).collect();
        for child in &closed {
            let Some(index) = file.copies.iter().position(|copy| copy.child_id == child.id && copy.kind == DelegationCopyKind::Report)
            else {
                continue;
            };
            size -= element_bytes(&file.copies[index]) as i64;
            file.copies.remove(index);
            if fits(&file, &mut size) {
                return Some(file);
            }
        }
        for child in closed {
            if !self.is_fully_delivered(child, context)
                || self.merges.iter().any(|merge| merge.child_id == child.id && (context.can_undo)(merge))
            {
                continue;
            }
            size -= file.everything_bytes(&child.id) as i64;
            file = file.removing_everything(&child.id);
            if fits(&file, &mut size) {
                return Some(file);
            }
        }
        None
    }

    /// `Admitting`: room for one more child — the file with `child`'s record (and its TASK copy) added and pruned until the child's full
    /// report copy still fits, or store_full when it does not fit even after pruning.
    pub fn admitting(&self, child: ChildRecord, context: &DelegationPruneContext, task: Option<DelegationCopy>) -> DelegationAdmission {
        let room = report_copy_room(&child.id) as i64;
        let mut file = self.clone();
        file.children.push(child);
        if let Some(task) = task {
            file = file.setting_copy(task);
        }
        match file.pruned(context, room) {
            Some(pruned) => DelegationAdmission::Admitted(pruned),
            None => DelegationAdmission::Refused(DelegationReasonCode::StoreFull),
        }
    }

    /// Every notice from the child and follow-up to it is delivered; a notice held for a child whose parent pane is closed counts as
    /// delivered.
    fn is_fully_delivered(&self, child: &ChildRecord, context: &DelegationPruneContext) -> bool {
        let parent_closed = !context.open_pane_ids.contains(&child.parent_session_id);
        self.notices.iter().all(|notice| {
            notice.child_id != child.id || notice.lane == DeliveryLane::Delivered || notice.lane == DeliveryLane::Held && parent_closed
        }) && self.follow_ups.iter().all(|follow_up| follow_up.child_id != child.id || follow_up.lane == DeliveryLane::Delivered)
    }

    /// The bytes the child's record and everything kept for it take.
    fn everything_bytes(&self, child_id: &str) -> usize {
        self.children.iter().filter(|c| c.id == child_id).map(element_bytes).sum::<usize>()
            + self.notices.iter().filter(|n| n.child_id == child_id).map(element_bytes).sum::<usize>()
            + self.follow_ups.iter().filter(|f| f.child_id == child_id).map(element_bytes).sum::<usize>()
            + self.merges.iter().filter(|m| m.child_id == child_id).map(element_bytes).sum::<usize>()
            + self.copies.iter().filter(|c| c.child_id == child_id).map(element_bytes).sum::<usize>()
    }

    /// This file without the child's record and everything kept for it.
    fn removing_everything(&self, child_id: &str) -> DelegationFile {
        DelegationFile {
            version: self.version,
            children: self.children.iter().filter(|c| c.id != child_id).cloned().collect(),
            notices: self.notices.iter().filter(|n| n.child_id != child_id).cloned().collect(),
            follow_ups: self.follow_ups.iter().filter(|f| f.child_id != child_id).cloned().collect(),
            merges: self.merges.iter().filter(|m| m.child_id != child_id).cloned().collect(),
            copies: self.copies.iter().filter(|c| c.child_id != child_id).cloned().collect(),
        }
    }
}

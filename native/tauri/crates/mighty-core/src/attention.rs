//! The panes waiting for the user (MC-26): a terminal's notification or bell, an agent pane's approval or question card, a worktree's
//! setup that failed. Each pane has at most one [`PaneAttention`]; the window marks it (the sidebar's dot, the pane's border), the
//! jump key (⇧⌘U, Ctrl+Shift+U) goes to the newest, and looking at the pane clears it. Pure: the host keeps one board and says when.

use std::collections::HashMap;

use serde::Serialize;

use crate::locale::Locale;

/// Why a pane waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AttentionKind {
    Notify,
    Bell,
    Approval,
    Question,
    SetupFailed,
}

impl AttentionKind {
    pub fn name(self) -> &'static str {
        match self {
            AttentionKind::Notify => "notify",
            AttentionKind::Bell => "bell",
            AttentionKind::Approval => "approval",
            AttentionKind::Question => "question",
            AttentionKind::SetupFailed => "setupFailed",
        }
    }

    /// Its word ("알림", "벨", "승인 대기", "질문 대기", "setup 실패").
    pub fn label(self, locale: &Locale) -> String {
        match self {
            AttentionKind::Notify => locale.get("attention.kind.notify"),
            AttentionKind::Bell => locale.get("attention.kind.bell"),
            AttentionKind::Approval => locale.get("attention.kind.approval"),
            AttentionKind::Question => locale.get("attention.kind.question"),
            AttentionKind::SetupFailed => locale.get("attention.kind.setupFailed"),
        }
    }

    /// A newer signal of a lower rank never hides a higher one still waiting (a bell does not cover an approval).
    fn rank(self) -> u8 {
        match self {
            AttentionKind::Bell => 0,
            AttentionKind::Notify => 1,
            AttentionKind::Approval | AttentionKind::Question | AttentionKind::SetupFailed => 2,
        }
    }

    /// A kind that stands for something the pane still has open (a card, a failure), not a one-off signal.
    pub fn is_standing(self) -> bool {
        self.rank() == 2
    }
}

/// One pane's reason to be looked at: what, its words (already safe to show), and when (Unix milliseconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAttention {
    pub kind: AttentionKind,
    pub text: String,
    pub at: i64,
    /// Orders raises that land in the same millisecond.
    seq: u64,
}

/// What a pane's row and tab show of it (the host's `PaneInfo.attention`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionInfo {
    pub kind: AttentionKind,
    pub text: String,
    /// "{word}: {text}", or the word alone.
    pub help: String,
}

impl PaneAttention {
    pub fn info(&self, locale: &Locale) -> AttentionInfo {
        let label = self.kind.label(locale);
        let help = if self.text.is_empty() { label } else { format!("{label}: {}", self.text) };
        AttentionInfo { kind: self.kind, text: self.text.clone(), help }
    }
}

/// Every pane's attention, and the requests already seen (so a card looked at once does not mark its pane again).
#[derive(Debug, Default, Clone)]
pub struct AttentionBoard {
    entries: HashMap<String, PaneAttention>,
    seq: u64,
    seen: HashMap<String, String>,
}

impl AttentionBoard {
    /// Marks `pane`; false when nothing changed (a lower-ranked signal over a waiting higher one).
    pub fn raise(&mut self, pane: &str, kind: AttentionKind, text: &str, at: i64) -> bool {
        if self.entries.get(pane).is_some_and(|old| old.kind.rank() > kind.rank()) {
            return false;
        }
        self.seq += 1;
        self.entries.insert(pane.to_string(), PaneAttention { kind, text: text.to_string(), at, seq: self.seq });
        true
    }

    /// The pane was looked at (or is gone): false when it had nothing.
    pub fn clear(&mut self, pane: &str) -> bool {
        self.entries.remove(pane).is_some()
    }

    /// Clears `pane`'s attention only when it is of `kind` (an answered card, a setup that now passed).
    pub fn clear_kind(&mut self, pane: &str, kinds: &[AttentionKind]) -> bool {
        if self.entries.get(pane).is_some_and(|a| kinds.contains(&a.kind)) {
            self.entries.remove(pane);
            return true;
        }
        false
    }

    pub fn get(&self, pane: &str) -> Option<&PaneAttention> {
        self.entries.get(pane)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The marked panes, newest first.
    pub fn order(&self) -> Vec<String> {
        let mut panes: Vec<(&String, &PaneAttention)> = self.entries.iter().collect();
        panes.sort_by_key(|(_, attention)| std::cmp::Reverse((attention.at, attention.seq)));
        panes.into_iter().map(|(pane, _)| pane.clone()).collect()
    }

    /// Where the jump key goes from `current`: the newest marked pane other than the one in view; the one in view when it is the
    /// only one. Looking at the target clears it, so the next press goes to the next newest.
    pub fn next_target(&self, current: Option<&str>) -> Option<String> {
        let order = self.order();
        order.iter().find(|pane| Some(pane.as_str()) != current).or(order.first()).cloned()
    }

    /// A pane's newest waiting card (`latest`: its kind, request id and words), as the run service holds it now: a request not seen
    /// before marks the pane; none left clears an approval or question mark. True when the board changed.
    pub fn sync_request(&mut self, pane: &str, latest: Option<(AttentionKind, &str, &str)>, at: i64) -> bool {
        match latest {
            Some((kind, id, text)) => {
                if self.seen.get(pane).map(String::as_str) == Some(id) {
                    return false;
                }
                self.seen.insert(pane.to_string(), id.to_string());
                self.raise(pane, kind, text, at)
            }
            None => {
                self.seen.remove(pane);
                self.clear_kind(pane, &[AttentionKind::Approval, AttentionKind::Question])
            }
        }
    }

    /// Drops the panes that are gone.
    pub fn retain(&mut self, alive: impl Fn(&str) -> bool) -> bool {
        let before = self.entries.len();
        self.entries.retain(|pane, _| alive(pane));
        self.seen.retain(|pane, _| alive(pane));
        before != self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jump_goes_to_the_newest_then_the_next() {
        let mut board = AttentionBoard::default();
        assert_eq!(board.next_target(None), None);
        board.raise("a", AttentionKind::Notify, "build done", 100);
        board.raise("b", AttentionKind::Bell, "", 200);
        board.raise("c", AttentionKind::Notify, "tests", 200);
        assert_eq!(board.order(), ["c", "b", "a"], "newest first; the same millisecond by arrival");
        assert_eq!(board.next_target(Some("x")).as_deref(), Some("c"));
        assert_eq!(board.next_target(Some("c")).as_deref(), Some("b"), "not the one in view");
        // Looking at it clears it: the next press goes on.
        assert!(board.clear("c"));
        assert_eq!(board.next_target(Some("c")).as_deref(), Some("b"));
        board.clear("b");
        assert_eq!(board.next_target(Some("a")).as_deref(), Some("a"), "the only one left");
        assert!(!board.clear("missing"));
    }

    #[test]
    fn a_bell_never_hides_a_waiting_card() {
        let mut board = AttentionBoard::default();
        board.raise("a", AttentionKind::Approval, "Bash", 1);
        assert!(!board.raise("a", AttentionKind::Bell, "", 2));
        assert!(!board.raise("a", AttentionKind::Notify, "x", 2));
        assert_eq!(board.get("a").unwrap().kind, AttentionKind::Approval);
        assert!(board.raise("a", AttentionKind::Question, "Which?", 3));
        board.raise("b", AttentionKind::Bell, "", 1);
        assert!(board.raise("b", AttentionKind::Notify, "done", 2), "a notification replaces a bell");
    }

    #[test]
    fn a_card_marks_its_pane_once_and_its_answer_clears_it() {
        let mut board = AttentionBoard::default();
        assert!(board.sync_request("a", Some((AttentionKind::Approval, "r1", "Bash")), 1));
        assert!(board.clear("a"), "looked at");
        assert!(!board.sync_request("a", Some((AttentionKind::Approval, "r1", "Bash")), 2), "the same card does not mark it again");
        assert!(board.sync_request("a", Some((AttentionKind::Question, "r2", "Which?")), 3), "a new card does");
        assert!(board.sync_request("a", None, 4), "answered");
        assert!(board.get("a").is_none());
        board.raise("b", AttentionKind::Notify, "done", 1);
        assert!(!board.sync_request("b", None, 2), "a notification stays until the pane is looked at");
        board.raise("c", AttentionKind::SetupFailed, "exit 1", 1);
        assert!(board.clear_kind("c", &[AttentionKind::SetupFailed]));
        assert!(board.retain(|pane| pane != "b") && board.is_empty());
    }

    #[test]
    fn the_row_says_what_and_why() {
        let ko = Locale::new("ko");
        let attention = PaneAttention { kind: AttentionKind::Notify, text: "빌드 끝".into(), at: 0, seq: 0 };
        let info = attention.info(&ko);
        assert_eq!(info.help, format!("{}: 빌드 끝", ko.get("attention.kind.notify")));
        let bell = PaneAttention { kind: AttentionKind::Bell, text: String::new(), at: 0, seq: 0 };
        assert_eq!(bell.info(&ko).help, ko.get("attention.kind.bell"));
        assert_eq!(serde_json::to_value(&info).unwrap()["kind"], "notify");
        assert_eq!(serde_json::to_value(AttentionKind::SetupFailed).unwrap(), "setupFailed");
    }
}

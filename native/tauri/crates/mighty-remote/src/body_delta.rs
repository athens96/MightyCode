//! MC-53: a pane's conversation sent to a mirroring computer or a phone as only what changed since the client's last answer.
//!
//! A detail carries the newest entries of a pane (the window: the phone's and the mirror's 80, pictures left out, fitted under a
//! relay frame). A client that says it takes deltas sends the body revision of the window it holds (`bodySince`); the host answers
//! the entries that changed or were added since that window and a mark (`body`): the window's revision, the one it was built on,
//! how many entries the window holds and a hash of their ids. The client lays the entries over what it holds (an entry it holds is
//! replaced, a new one goes last), keeps the newest `count` and checks the hash; anything that does not match is read again whole.
//!
//! The host builds a delta from the windows it sent lately ([`BodyHistory`]: the last [`KEPT_WINDOWS`] of each of the last
//! [`KEPT_PANES`] panes asked for, as entry fingerprints, not the entries), never from the disk: the body journal
//! (`mighty_core::store::read_body_deltas_since`) lags the run by a save, holds only what the store keeps of a pane (the stored
//! kinds, written logs, and only until the run ends or the pane outgrows 400 entries, when every save writes the file whole), so a
//! window of the pane as it runs now could not be rebuilt from it. A revision the host no longer holds (another computer's start,
//! a window pushed out of the history), a change that is not "the front dropped, some replaced, some added" (an entry gone from
//! the middle, reordered ids, a missing id) and a delta no smaller than the window are all answered whole, the mark saying so.
//!
//! The revision is the window's fingerprint (its ids and entries), so the same window has the same revision on any host and a
//! client never takes one host's window for another's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use indexmap::IndexMap;
use mighty_core::json::{JsonOut, Wire};
use mighty_core::model::LogEntry;
use serde_json::{Map, Value};

/// The windows a pane's history keeps: a client this many changes behind is answered whole.
pub const KEPT_WINDOWS: usize = 32;
/// The panes a history keeps, the ones asked for least lately forgotten first.
pub const KEPT_PANES: usize = 64;
/// The capability a phone host advertises (`/m1/info`) when its session details take `bodySince`.
pub const CAPABILITY: &str = "bodyDelta";
/// The query argument a client names the revision of the window it holds with (empty: it holds none but takes deltas).
pub const QUERY: &str = "bodySince";

/// What a detail says of its entries: the window's `revision`, the revision it was built on (`since`: a delta, the entries sent are
/// only the changed and the new ones; None: the whole window), the window's `count` and the hash of its ids (`hash`, [`ids_hash`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyMark {
    pub revision: String,
    pub since: Option<String>,
    pub count: usize,
    pub hash: String,
}

impl BodyMark {
    /// `{revision, kind: "full"|"delta", since?, count, hash}`.
    pub fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("revision", Some(&self.revision));
        out.prop_str("kind", Some(if self.since.is_some() { "delta" } else { "full" }));
        out.prop_str("since", self.since.as_deref());
        out.prop_int("count", self.count as i64);
        out.prop_str("hash", Some(&self.hash));
        out.end_object();
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("revision".into(), self.revision.clone().into());
        o.insert("kind".into(), (if self.since.is_some() { "delta" } else { "full" }).into());
        if let Some(since) = &self.since {
            o.insert("since".into(), since.clone().into());
        }
        o.insert("count".into(), (self.count as i64).into());
        o.insert("hash".into(), self.hash.clone().into());
        Value::Object(o)
    }

    /// Read strictly: a revision ([`valid_revision`]), `kind` `full` or `delta` (a delta names the revision it was built on), a
    /// count of at most 10,000, an 8-digit hash. None for anything else.
    pub fn read(value: &Value) -> Option<BodyMark> {
        let o = value.as_object()?;
        let revision = o.get("revision")?.as_str().filter(|r| valid_revision(r))?.to_string();
        let since = match (o.get("kind")?.as_str()?, o.get("since")) {
            ("full", None | Some(Value::Null)) => None,
            ("delta", Some(Value::String(since))) if valid_revision(since) => Some(since.clone()),
            _ => return None,
        };
        let count = usize::try_from(o.get("count")?.as_u64().filter(|c| *c <= 10_000)?).ok()?;
        let hash = o.get("hash")?.as_str().filter(|h| h.len() == 8 && hex(h))?.to_string();
        Some(BodyMark { revision, since, count, hash })
    }
}

fn hex(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A body revision as it travels: 1 to 64 lower-case hex digits.
pub fn valid_revision(text: &str) -> bool {
    (1..=64).contains(&text.len()) && hex(text)
}

/// `bodySince`'s value: Some(None) when the client holds no window (empty), Some(Some(revision)) when it holds one, None when the
/// value is no revision (the request is refused).
pub fn since_value(value: &str) -> Option<Option<&str>> {
    if value.is_empty() { Some(None) } else { valid_revision(value).then_some(Some(value)) }
}

const FNV32_OFFSET: u32 = 0x811c_9dc5;
const FNV32_PRIME: u32 = 0x0100_0193;
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv64(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV64_PRIME);
    }
    hash
}

/// The hash of a window's ids a client checks what it laid together with: FNV-1a (32 bits) over each id's Unicode scalar values,
/// each id followed by a line feed, as 8 lower-case hex digits. The phone app computes the same (`mobile/src/lib/body-delta.ts`).
pub fn ids_hash<'a>(ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut hash = FNV32_OFFSET;
    for id in ids {
        for c in id.chars().chain(std::iter::once('\n')) {
            hash ^= u32::from(c);
            hash = hash.wrapping_mul(FNV32_PRIME);
        }
    }
    format!("{hash:08x}")
}

/// [`ids_hash`] of entries.
pub fn entries_hash(entries: &[LogEntry]) -> String {
    ids_hash(entries.iter().map(LogEntry::id))
}

/// One entry of a sent window as the history keeps it: its id's and its JSON's fingerprints.
type Row = (u64, u64);

fn rows(window: &[LogEntry]) -> Vec<Row> {
    window.iter().map(|e| (fnv64(FNV64_OFFSET, e.id().as_bytes()), fnv64(FNV64_OFFSET, e.to_json().as_bytes()))).collect()
}

fn revision_of(rows: &[Row]) -> String {
    let mut hash = FNV64_OFFSET;
    for (id, entry) in rows {
        hash = fnv64(hash, &id.to_le_bytes());
        hash = fnv64(hash, &entry.to_le_bytes());
    }
    format!("{hash:016x}")
}

/// What a client holding `old` needs to reach `new`: the indexes in `new` of the entries to send. None when the change is not the
/// front dropped, the rest in place (some replaced) and new entries after them.
fn changed(old: &[Row], new: &[Row]) -> Option<Vec<usize>> {
    let unique = |rows: &[Row]| rows.iter().map(|(id, _)| *id).collect::<HashSet<u64>>().len() == rows.len();
    if !unique(old) || !unique(new) {
        return None;
    }
    let present: HashSet<u64> = new.iter().map(|(id, _)| *id).collect();
    let start = old.iter().position(|(id, _)| present.contains(id)).unwrap_or(old.len());
    let kept = &old[start..];
    if kept.len() > new.len() || kept.iter().zip(new).any(|((a, _), (b, _))| a != b) {
        return None;
    }
    let replaced = kept.iter().zip(new).enumerate().filter(|(_, ((_, a), (_, b)))| a != b).map(|(i, _)| i);
    Some(replaced.chain(kept.len()..new.len()).collect())
}

/// A pane's windows sent lately, oldest first, each with its revision.
type Sent = VecDeque<(String, Vec<Row>)>;

/// What to send of a window: its mark and, for a delta, only the changed and new entries (None: send the window whole).
#[derive(Debug, Clone, PartialEq)]
pub struct BodyAnswer {
    pub mark: BodyMark,
    pub entries: Option<Vec<LogEntry>>,
}

/// The windows sent of each pane lately (fingerprints only), which a delta is built from.
#[derive(Default)]
pub struct BodyHistory {
    panes: Mutex<IndexMap<String, Sent>>,
}

impl std::fmt::Debug for BodyHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BodyHistory").finish_non_exhaustive()
    }
}

impl BodyHistory {
    /// The window as it is sent now, kept for the next answers, and what a client holding `since` is sent of it: a delta when the
    /// history still holds `since` and the change is one a client can lay over it with fewer entries than the window, else the
    /// window whole. Entries without an id are never sent as a delta.
    pub fn answer(&self, pane_id: &str, window: &[LogEntry], since: Option<&str>) -> BodyAnswer {
        let now = rows(window);
        let revision = revision_of(&now);
        let mark = |since: Option<String>| BodyMark { revision: revision.clone(), since, count: window.len(), hash: entries_hash(window) };
        let mut panes = self.panes.lock().unwrap_or_else(|p| p.into_inner());
        let mut kept = panes.shift_remove(pane_id).unwrap_or_default();
        let base = since.and_then(|since| kept.iter().find(|(r, _)| r == since).map(|(r, rows)| (r.clone(), changed(rows, &now))));
        if kept.back().is_none_or(|(r, _)| *r != revision) {
            kept.push_back((revision.clone(), now));
            while kept.len() > KEPT_WINDOWS {
                kept.pop_front();
            }
        }
        panes.insert(pane_id.to_string(), kept);
        while panes.len() > KEPT_PANES {
            panes.shift_remove_index(0);
        }
        drop(panes);
        match base {
            Some((since, Some(indexes)))
                if window.iter().all(|e| !e.id().is_empty()) && (indexes.len() < window.len() || window.is_empty()) =>
            {
                BodyAnswer { mark: mark(Some(since)), entries: Some(indexes.into_iter().map(|i| window[i].clone()).collect()) }
            }
            _ => BodyAnswer { mark: mark(None), entries: None },
        }
    }

    /// Forgets the panes `keep` says no to (closed, no longer shared).
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.panes.lock().unwrap_or_else(|p| p.into_inner()).retain(|id, _| keep(id));
    }
}

/// A delta laid over the window a client holds: an entry it holds is replaced where it is, a new one goes last, and the newest
/// `count` are kept.
pub fn apply(held: &[LogEntry], entries: &[LogEntry], count: usize) -> Vec<LogEntry> {
    let mut out = held.to_vec();
    let mut at: HashMap<String, usize> = out.iter().enumerate().map(|(i, e)| (e.id().to_string(), i)).collect();
    for entry in entries {
        match at.get(entry.id()) {
            Some(&i) => out[i] = entry.clone(),
            None => {
                at.insert(entry.id().to_string(), out.len());
                out.push(entry.clone());
            }
        }
    }
    let drop = out.len().saturating_sub(count);
    out.drain(..drop);
    out
}

/// Whether a window is the one a mark describes (its count and ids).
pub fn verified(entries: &[LogEntry], mark: &BodyMark) -> bool {
    entries.len() == mark.count && entries_hash(entries) == mark.hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, text: &str) -> LogEntry {
        LogEntry { id: Some(id.into()), kind: Some("assistant".into()), text: Some(text.into()), ..LogEntry::default() }
    }

    #[test]
    fn the_ids_hash_is_fnv1a_over_scalar_values() {
        assert_eq!(ids_hash([]), "811c9dc5");
        // The phone app's test holds the same values.
        assert_eq!(ids_hash(["a"]), ids_hash(["a"]));
        assert_ne!(ids_hash(["ab"]), ids_hash(["a", "b"]));
        assert_eq!(ids_hash(["e1", "e2"]), "2c436f46");
        assert_eq!(ids_hash(["한글-✓"]), "7a09a9ad");
        assert_eq!(ids_hash(["😀"]), "05f7060f");
    }

    /// Random changes to a pane's entries, a window of the newest 20 each time: a delta laid over the window held is always the window
    /// whole (or the answer is whole), and the changes a client can lay over (grow, stream, edit the tail, the front dropping) are deltas.
    #[test]
    fn a_delta_laid_over_the_held_window_is_the_window_whole() {
        const WINDOW: usize = 20;
        let history = BodyHistory::default();
        let mut seed: u64 = 0x5eed;
        let mut roll = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let mut logs: Vec<LogEntry> = (0..10).map(|i| entry(&format!("e{i}"), "start")).collect();
        let window = |logs: &[LogEntry]| logs[logs.len().saturating_sub(WINDOW)..].to_vec();
        let first = history.answer("p", &window(&logs), None);
        let (mut held, mut revision) = (window(&logs), first.mark.revision);
        let (mut next, mut deltas) = (10, 0);
        for step in 0..400 {
            let nice = match roll(10) {
                0..=3 => {
                    for _ in 0..=roll(3) {
                        logs.push(entry(&format!("e{next}"), "new"));
                        next += 1;
                    }
                    true
                }
                4 | 5 => {
                    let last = logs.len() - 1;
                    logs[last].text = Some(format!("{} more", logs[last].text()));
                    true
                }
                6 => {
                    let at = logs.len() - 1 - roll(logs.len().min(WINDOW) as u64) as usize;
                    logs[at].text = Some(format!("edit {step}"));
                    true
                }
                7 if logs.len() > 3 => {
                    let at = logs.len() - 2;
                    logs.remove(at);
                    false
                }
                8 if logs.len() > 3 => {
                    let n = logs.len();
                    logs.swap(n - 1, n - 2);
                    false
                }
                _ => true,
            };
            let now = window(&logs);
            let answer = history.answer("p", &now, Some(&revision));
            match answer.entries {
                Some(entries) => {
                    held = apply(&held, &entries, answer.mark.count);
                    deltas += 1;
                }
                None => {
                    assert!(!nice || held.is_empty(), "step {step}: a change a client can lay over came whole");
                    held = now.clone();
                }
            }
            assert_eq!(held, now, "step {step}");
            assert!(verified(&held, &answer.mark), "step {step}");
            revision = answer.mark.revision;
        }
        assert!(deltas > 250, "most steps travel as deltas ({deltas})");
        let duplicate = vec![entry("a", "x"), entry("a", "y")];
        let base = history.answer("q", &duplicate, None).mark.revision;
        assert_eq!(history.answer("q", &[entry("a", "x"), entry("a", "z")], Some(&base)).entries, None, "repeated ids: whole");
        let unnamed = history.answer("r", &[entry("", "x")], None).mark.revision;
        assert_eq!(history.answer("r", &[entry("", "y")], Some(&unnamed)).entries, None, "an entry without an id: whole");
    }

    #[test]
    fn a_history_keeps_its_last_windows_and_panes() {
        let history = BodyHistory::default();
        let oldest = history.answer("p", &[entry("a", "0")], None).mark.revision;
        for i in 1..=KEPT_WINDOWS {
            history.answer("p", &[entry("a", &i.to_string())], None);
        }
        assert!(history.answer("p", &[entry("a", "x")], Some(&oldest)).entries.is_none(), "a window pushed out");
        let kept = history.answer("p", &[entry("a", "x")], None).mark.revision;
        for i in 0..KEPT_PANES {
            history.answer(&format!("other-{i}"), &[], None);
        }
        assert!(history.answer("p", &[entry("a", "x")], Some(&kept)).entries.is_none(), "a pane pushed out");
        let kept = history.answer("p", &[entry("a", "x")], None).mark.revision;
        history.retain(|id| id != "p");
        assert!(history.answer("p", &[entry("a", "x")], Some(&kept)).entries.is_none(), "a pane forgotten");
    }

    #[test]
    fn a_mark_reads_back_and_refuses_what_is_not_one() {
        let full = BodyMark { revision: "ab12".into(), since: None, count: 3, hash: "0011aabb".into() };
        let delta = BodyMark { since: Some("cd34".into()), ..full.clone() };
        assert_eq!(BodyMark::read(&full.to_value()), Some(full.clone()));
        assert_eq!(BodyMark::read(&delta.to_value()), Some(delta));
        let mut out = JsonOut::new();
        full.write(&mut out);
        assert_eq!(serde_json::from_str::<Value>(&out.finish()).unwrap(), full.to_value());
        for bad in [
            serde_json::json!({"revision": "AB", "kind": "full", "count": 1, "hash": "0011aabb"}),
            serde_json::json!({"revision": "ab", "kind": "delta", "count": 1, "hash": "0011aabb"}),
            serde_json::json!({"revision": "ab", "kind": "other", "count": 1, "hash": "0011aabb"}),
            serde_json::json!({"revision": "ab", "kind": "full", "count": -1, "hash": "0011aabb"}),
            serde_json::json!({"revision": "ab", "kind": "full", "count": 1, "hash": "0011aab"}),
        ] {
            assert_eq!(BodyMark::read(&bad), None, "{bad}");
        }
    }
}

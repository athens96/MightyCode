//! 바뀐 부분별 받기·버리기 (MC-30): a change taken or put back one hunk at a time instead of a whole file. The hunks are the 변경사항
//! view's ([`diff::compute`]: the base, the file as it was before, against the file now, 3 lines of context); each is named by a
//! fingerprint that does not move when another hunk of the file is taken or put back, so a decision outlives a reload and a restart.
//!
//! - 버리기 writes the file with only that hunk's lines put back as the base had them ([`discard`]). The hunk's new side (its context
//!   and added lines) must stand in the file now where the hunk says, or a few lines off when only one place fits; else the file
//!   changed after the diff was read and nothing is written (`conflict`). The lines put back take the file's line ending; the rest of
//!   the file is kept byte for byte.
//! - 받기 changes nothing: the decision is kept in a [`ReviewBook`] (a request's beside its originals, a worktree's under the app's
//!   data), with the hunks put back, so a file reads as 검토됨 once every hunk it has was taken or put back ([`Progress`]).
//! - A later request of the same pane that changed lines a hunk touches makes it refused (`laterRun`); hunks away from those lines
//!   stay free ([`touches`]).

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::diff::{self, DiffResult, Hunk};

/// How far (in lines, either way) a hunk is looked for when it is not where the diff put it.
pub const SHIFT: usize = 20;
/// The most decisions one book keeps (the oldest go first).
pub const MAXIMUM_DECISIONS: usize = 4_096;
/// The most a book's file may weigh before it is read as empty.
const MAXIMUM_BOOK_BYTES: u64 = 2 * 1_048_576;

/// The fingerprint of a hunk: its place on the base's side (which taking or putting back another hunk never moves) and its lines,
/// the first 32 hex digits of their SHA-256.
pub fn fingerprint(hunk: &Hunk) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{},{}\n", hunk.old_start, hunk.old_lines).as_bytes());
    for line in &hunk.lines {
        hasher.update([line.op as u8, u8::from(line.no_newline)]);
        hasher.update(line.text.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_string()
}

/// The diff of `base` to `current` and each hunk's fingerprint, in order; None when there is no diff (either side over the caps).
pub fn hunks(base: &str, current: &str) -> Option<(DiffResult, Vec<String>)> {
    let changes = diff::compute(base, current)?;
    let prints = changes.hunks.iter().map(fingerprint).collect();
    Some((changes, prints))
}

/// The file's lines, each with its line break as written (`\r\n`, `\n`, or none for a last line without one).
fn raw_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A line as the diff compares it: its text (a `\r\n` read as `\n`) and whether a line break ends it.
fn key(raw: &str) -> (&str, bool) {
    match raw.strip_suffix('\n') {
        Some(rest) => (rest.strip_suffix('\r').unwrap_or(rest), true),
        None => (raw, false),
    }
}

/// The line break most of `text`'s lines end with; None when none ends with one.
fn line_break(text: &str) -> Option<&'static str> {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for raw in raw_lines(text) {
        if raw.ends_with("\r\n") {
            crlf += 1;
        } else if raw.ends_with('\n') {
            lf += 1;
        }
    }
    (crlf + lf > 0).then_some(if crlf > lf { "\r\n" } else { "\n" })
}

/// Where (0-based, in the file now) the hunk's new side starts.
fn expected_start(hunk: &Hunk) -> usize {
    if hunk.new_lines == 0 { hunk.new_start } else { hunk.new_start.saturating_sub(1) }
}

/// Where the hunk's new side stands in `lines`: where the hunk says, else the one place within [`SHIFT`] lines; None when it stands
/// nowhere near or in more than one place.
fn locate(lines: &[&str], hunk: &Hunk) -> Option<usize> {
    let wanted: Vec<(&str, bool)> = hunk.lines.iter().filter(|l| l.op != '-').map(|l| (l.text.as_str(), !l.no_newline)).collect();
    let fits = |at: usize| at + wanted.len() <= lines.len() && wanted.iter().enumerate().all(|(i, w)| key(lines[at + i]) == *w);
    let start = expected_start(hunk);
    if fits(start) {
        return Some(start);
    }
    let mut found = (1..=SHIFT).flat_map(|d| [start.checked_sub(d), Some(start + d)]).flatten().filter(|at| fits(*at));
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// The changed stretch of a hunk placed at `at` in the file now (0-based, end exclusive): from its first added line, or the place of
/// its first removed one, to the end of its last added line.
fn stretch(hunk: &Hunk, at: usize) -> (usize, usize) {
    let (mut line, mut first, mut last) = (at, None, at);
    for row in &hunk.lines {
        match row.op {
            '+' => {
                first.get_or_insert(line);
                line += 1;
                last = line;
            }
            '-' => {
                first.get_or_insert(line);
                last = last.max(line);
            }
            _ => line += 1,
        }
    }
    let first = first.unwrap_or(at);
    (first, last.max(first))
}

/// The stretches (0-based in the file now, end exclusive; a removal is the empty stretch where it was) the given hunks changed.
pub fn changed_stretches(hunks: &[Hunk]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for hunk in hunks {
        let mut line = expected_start(hunk);
        let mut run: Option<(usize, usize)> = None;
        for row in &hunk.lines {
            match row.op {
                '+' => {
                    let (start, _) = run.get_or_insert((line, line));
                    let start = *start;
                    line += 1;
                    run = Some((start, line));
                }
                '-' => {
                    run.get_or_insert((line, line));
                }
                _ => {
                    out.extend(run.take());
                    line += 1;
                }
            }
        }
        out.extend(run);
    }
    out
}

/// Whether the hunk, its new side standing at `at` in the file now (where the diff put it when None), touches or borders one of
/// `stretches`.
pub fn touches(hunk: &Hunk, at: Option<usize>, stretches: &[(usize, usize)]) -> bool {
    let (start, end) = stretch(hunk, at.unwrap_or_else(|| expected_start(hunk)));
    stretches.iter().any(|&(s, e)| start <= e && s <= end)
}

/// Where the hunk's new side stands in `current` (0-based line): where the hunk says, else the one place within [`SHIFT`] lines.
pub fn place(current: &str, hunk: &Hunk) -> Option<usize> {
    locate(&raw_lines(current), hunk)
}

/// The file `current` with `hunk` put back: its removed lines written again (with the line break most of the file's lines carry,
/// else `fallback`'s) and its added lines taken out, every other line kept as written. None when the hunk's new side is not in the
/// file where it says (or, within [`SHIFT`] lines, in exactly one place).
pub fn discard(current: &str, hunk: &Hunk, fallback: &str) -> Option<String> {
    let lines = raw_lines(current);
    let at = locate(&lines, hunk)?;
    let ending = line_break(current).or_else(|| line_break(fallback)).unwrap_or("\n");
    let mut out = String::with_capacity(current.len());
    lines[..at].iter().for_each(|l| out.push_str(l));
    let mut cursor = at;
    for row in &hunk.lines {
        match row.op {
            '-' => {
                out.push_str(&row.text);
                if !row.no_newline {
                    out.push_str(ending);
                }
            }
            '+' => cursor += 1,
            _ => {
                out.push_str(lines[cursor]);
                cursor += 1;
            }
        }
    }
    lines[cursor..].iter().for_each(|l| out.push_str(l));
    Some(out)
}

/// Whether two texts read the same to the diff (a `\r\n` read as `\n`).
pub fn same_text(a: &str, b: &str) -> bool {
    a == b || a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
}

/// A person's decision on one hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accepted,
    Discarded,
}

impl Decision {
    pub fn word(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Discarded => "discarded",
        }
    }

    fn from_word(word: &str) -> Option<Decision> {
        match word {
            "accepted" => Some(Decision::Accepted),
            "discarded" => Some(Decision::Discarded),
            _ => None,
        }
    }
}

/// One decision as a book keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub fingerprint: String,
    pub decision: Decision,
}

/// The file name of the book of `scope` (a worktree pane's id): the first 32 hex digits of its SHA-256, so no id names a path.
pub fn book_name(scope: &str) -> String {
    let digest = Sha256::digest(scope.as_bytes());
    format!("{}.json", &digest.iter().map(|b| format!("{b:02x}")).collect::<String>()[..32])
}

/// Books are read and written one at a time (a read, a change, a write).
static BOOKS: Mutex<()> = Mutex::new(());

/// The decisions of one review (a request of a pane, or a worktree), kept in one JSON file (`{version: 1, entries: [{path, hunk,
/// decision}]}`) written to a temporary name beside it and moved over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewBook {
    file: PathBuf,
}

impl ReviewBook {
    pub fn at(file: impl Into<PathBuf>) -> ReviewBook {
        ReviewBook { file: file.into() }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    fn read_unlocked(&self) -> Vec<Entry> {
        let Ok(meta) = std::fs::symlink_metadata(&self.file) else { return vec![] };
        if !meta.file_type().is_file() || meta.len() > MAXIMUM_BOOK_BYTES {
            return vec![];
        }
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&self.file).and_then(|f| f.take(MAXIMUM_BOOK_BYTES + 1).read_to_end(&mut bytes));
        if read.is_err() {
            return vec![];
        }
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return vec![] };
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return vec![];
        }
        value
            .get("entries")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|e| {
                        Some(Entry {
                            path: e.get("path")?.as_str()?.to_string(),
                            fingerprint: e.get("hunk")?.as_str()?.to_string(),
                            decision: Decision::from_word(e.get("decision")?.as_str()?)?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every decision kept, oldest first; none when the file is missing, unreadable or not a book.
    pub fn read(&self) -> Vec<Entry> {
        let _guard = BOOKS.lock().unwrap_or_else(|p| p.into_inner());
        self.read_unlocked()
    }

    /// The decisions kept for `path`, by fingerprint.
    pub fn decisions(&self, path: &str) -> HashMap<String, Decision> {
        decisions_of(&self.read(), path)
    }

    /// Records `decision` for each of `fingerprints` of `path` (a decision made again replaces the one before).
    pub fn record(&self, path: &str, fingerprints: &[String], decision: Decision) -> std::io::Result<()> {
        let _guard = BOOKS.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = self.read_unlocked();
        let fresh: HashSet<&str> = fingerprints.iter().map(String::as_str).collect();
        entries.retain(|e| !(e.path == path && fresh.contains(e.fingerprint.as_str())));
        entries.extend(fingerprints.iter().map(|f| Entry { path: path.to_string(), fingerprint: f.clone(), decision }));
        if entries.len() > MAXIMUM_DECISIONS {
            entries.drain(..entries.len() - MAXIMUM_DECISIONS);
        }
        let list: Vec<Value> =
            entries.iter().map(|e| json!({"path": e.path, "hunk": e.fingerprint, "decision": e.decision.word()})).collect();
        let bytes = serde_json::to_vec(&json!({"version": 1, "entries": list})).unwrap_or_default();
        if let Some(folder) = self.file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        crate::changes::write_atomically(&self.file, &bytes)
    }
}

/// The decisions of `entries` for `path`, by fingerprint (the newest wins).
pub fn decisions_of(entries: &[Entry], path: &str) -> HashMap<String, Decision> {
    entries.iter().filter(|e| e.path == path).map(|e| (e.fingerprint.clone(), e.decision)).collect()
}

/// A file's review: of its hunks (those it has now and those put back), how many were handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub handled: usize,
    pub total: usize,
}

impl Progress {
    /// 검토됨: every hunk the file has now was taken (and any put back was put back), and there was something to review.
    pub fn reviewed(&self) -> bool {
        self.handled == self.total
    }
}

/// The progress of a file whose hunks now are `prints`, given its kept decisions: a hunk now counts when it was taken; a hunk put back
/// (no longer in the file) counts once as handled.
pub fn progress(prints: &[String], decisions: &HashMap<String, Decision>) -> Progress {
    let now: HashSet<&str> = prints.iter().map(String::as_str).collect();
    let accepted = prints.iter().filter(|p| decisions.get(*p) == Some(&Decision::Accepted)).count();
    let gone = decisions.iter().filter(|(p, d)| **d == Decision::Discarded && !now.contains(p.as_str())).count();
    Progress { handled: accepted + gone, total: prints.len() + gone }
}

/// One hunk of a file's review as the page reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkState {
    pub fingerprint: String,
    pub decision: Option<Decision>,
    pub refusal: Option<crate::revert::Refusal>,
}

impl HunkState {
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("fingerprint".into(), json!(self.fingerprint));
        if let Some(decision) = self.decision {
            out.insert("decision".into(), json!(decision.word()));
        }
        if let Some(refusal) = self.refusal {
            out.insert("refusal".into(), json!(refusal.word()));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(count: usize) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    /// Three changes far apart in a 40-line file, each its own hunk.
    fn three() -> (String, String) {
        let base = numbered(40);
        let current = base.replace("line 5\n", "FIVE\n").replace("line 20\n", "line 20\nadded\n").replace("line 35\n", "");
        (base, current)
    }

    #[test]
    fn discarding_one_of_three_hunks_leaves_the_other_two() {
        let (base, current) = three();
        let (changes, _) = hunks(&base, &current).unwrap();
        assert_eq!(changes.hunks.len(), 3);
        let next = discard(&current, &changes.hunks[1], &base).unwrap();
        assert_eq!(next, base.replace("line 5\n", "FIVE\n").replace("line 35\n", ""));
        let (after, _) = hunks(&base, &next).unwrap();
        assert_eq!(after.hunks.len(), 2);
    }

    #[test]
    fn discarding_every_hunk_gives_the_base_back() {
        let (base, mut current) = three();
        while let Some((changes, _)) = hunks(&base, &current).filter(|(c, _)| !c.unchanged) {
            // From the last hunk too: one put back never moves the others' fingerprints.
            current = discard(&current, changes.hunks.last().unwrap(), &base).unwrap();
        }
        assert_eq!(current, base);
    }

    #[test]
    fn fingerprints_stay_when_another_hunk_goes() {
        let (base, current) = three();
        let (changes, prints) = hunks(&base, &current).unwrap();
        let next = discard(&current, &changes.hunks[0], &base).unwrap();
        let (_, after) = hunks(&base, &next).unwrap();
        assert_eq!(after, prints[1..], "the hunks after the one put back keep their names");
        let (_, again) = hunks(&base, &current).unwrap();
        assert_eq!(again, prints, "read again, the same names");
        assert_ne!(prints[0], prints[1]);
    }

    #[test]
    fn neighbouring_hunks_are_put_back_one_at_a_time() {
        // Seven equal lines between two changes: two hunks whose context lines meet.
        let base = numbered(20);
        let current = base.replace("line 4\n", "FOUR\n").replace("line 12\n", "TWELVE\n");
        let (changes, prints) = hunks(&base, &current).unwrap();
        assert_eq!(changes.hunks.len(), 2, "{:?}", changes.hunks);
        let first = discard(&current, &changes.hunks[0], &base).unwrap();
        assert_eq!(first, base.replace("line 12\n", "TWELVE\n"));
        let (left, left_prints) = hunks(&base, &first).unwrap();
        assert_eq!(left_prints, prints[1..]);
        assert_eq!(discard(&first, &left.hunks[0], &base).unwrap(), base);
        // The second first, then the first.
        let second = discard(&current, &changes.hunks[1], &base).unwrap();
        assert_eq!(second, base.replace("line 4\n", "FOUR\n"));
    }

    #[test]
    fn the_last_line_keeps_or_loses_its_line_break_as_the_base_had_it() {
        let base = "a\nb";
        let current = "a\nb\nc\n";
        let (changes, _) = hunks(base, current).unwrap();
        assert_eq!(discard(current, &changes.hunks[0], base).unwrap(), base);
        let base = "a\nb\n";
        let current = "a\nB";
        let (changes, _) = hunks(base, current).unwrap();
        assert_eq!(discard(current, &changes.hunks[0], base).unwrap(), base);
    }

    #[test]
    fn crlf_lines_stay_crlf() {
        let base = numbered(30).replace('\n', "\r\n");
        let current = base.replace("line 3\r\n", "THREE\r\n").replace("line 25\r\n", "TWENTY-FIVE\r\n");
        let (changes, _) = hunks(&base, &current).unwrap();
        let next = discard(&current, &changes.hunks[0], &base).unwrap();
        assert_eq!(next, base.replace("line 25\r\n", "TWENTY-FIVE\r\n"));
        // A file the request turned to LF gets its lines back with LF.
        let lf = current.replace("\r\n", "\n");
        let (changes, _) = hunks(&base, &lf).unwrap();
        let next = discard(&lf, &changes.hunks[0], &base).unwrap();
        assert!(!next.contains('\r'), "{next:?}");
    }

    #[test]
    fn a_hunk_no_longer_where_it_was_is_found_nearby_or_refused() {
        let (base, current) = three();
        let (changes, _) = hunks(&base, &current).unwrap();
        let hunk = changes.hunks[1].clone();
        // Two lines written above it after the diff was read: found two lines down.
        let shifted = format!("x\ny\n{current}");
        let next = discard(&shifted, &hunk, &base).unwrap();
        assert_eq!(next, format!("x\ny\n{}", base.replace("line 5\n", "FIVE\n").replace("line 35\n", "")));
        // One of its lines changed after: refused.
        let changed = current.replace("added\n", "added later\n");
        assert_eq!(discard(&changed, &hunk, &base), None);
        // Moved off its place and its lines in two places near it: refused, not guessed.
        let lines: Vec<&str> = raw_lines(&current);
        let at = expected_start(&hunk);
        let block: String = lines[at..at + hunk.new_lines].concat();
        let doubled = format!("q\n{}{block}{}", lines[..at].concat(), lines[at..].concat());
        assert_eq!(discard(&doubled, &hunk, &base), None);
    }

    #[test]
    fn a_later_change_blocks_only_the_hunks_it_touches() {
        let (base, current) = three();
        let (changes, _) = hunks(&base, &current).unwrap();
        // A later request changed the line after the second hunk's added one.
        let before_later = current.clone();
        let later = before_later.replace("line 21\n", "twenty-one\n");
        let (now, _) = hunks(&base, &later).unwrap();
        let (later_changes, _) = hunks(&before_later, &later).unwrap();
        let stretches = changed_stretches(&later_changes.hunks);
        let touched: Vec<bool> = now.hunks.iter().map(|h| touches(h, None, &stretches)).collect();
        assert_eq!(touched, [false, true, false], "{:?}", now.hunks);
        assert_eq!(changes.hunks.len(), 3);
    }

    #[test]
    fn progress_counts_taken_and_put_back_hunks() {
        let prints = vec!["a".to_string(), "b".to_string()];
        let mut decisions = HashMap::new();
        assert_eq!(progress(&prints, &decisions), Progress { handled: 0, total: 2 });
        decisions.insert("a".to_string(), Decision::Accepted);
        decisions.insert("gone".to_string(), Decision::Discarded);
        let now = progress(&prints, &decisions);
        assert_eq!(now, Progress { handled: 2, total: 3 });
        assert!(!now.reviewed());
        decisions.insert("b".to_string(), Decision::Accepted);
        assert!(progress(&prints, &decisions).reviewed());
        assert!(progress(&[], &HashMap::new()).reviewed(), "nothing left to review");
    }

    #[test]
    fn a_book_keeps_decisions_across_reads() {
        let folder = std::env::temp_dir().join(format!("mighty-hunk-book-{}", uuid::Uuid::new_v4().simple()));
        let book = ReviewBook::at(folder.join("review.json"));
        assert!(book.read().is_empty());
        book.record("a.txt", &["f1".into(), "f2".into()], Decision::Accepted).unwrap();
        book.record("a.txt", &["f2".into()], Decision::Discarded).unwrap();
        book.record("b.txt", &["f1".into()], Decision::Accepted).unwrap();
        let again = ReviewBook::at(folder.join("review.json"));
        let a = again.decisions("a.txt");
        assert_eq!((a.get("f1"), a.get("f2")), (Some(&Decision::Accepted), Some(&Decision::Discarded)));
        assert_eq!(again.decisions("b.txt").len(), 1);
        std::fs::write(book.file(), b"not json").unwrap();
        assert!(book.read().is_empty(), "a broken book reads as empty");
        let _ = std::fs::remove_dir_all(&folder);
    }
}

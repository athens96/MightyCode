//! Background output tails (C# `WinUI\MainWindow.BackgroundOutput.cs` over `Core\BackgroundOutputTail.cs`, ported pure in mighty-run's
//! `background_output`; plan §1.8, W9-E).
//!
//! Once a second the last lines of every running task block's output file are read on a thread of their own, off the view's lock, into
//! the pane's [`Tails`]: the page's Mighty blocks and the background strip, and the phone, read them ([`output_for`]). A block that ended
//! is read once more and then left alone; a block redrawn from the session record that still has its file gets one read. A file that is
//! missing, unreadable, not a regular file, or on another computer (a remote workspace's pane, a share) gives no lines and no error. A
//! batch whose read stays on one file for 10 s is given up on (that block is skipped until its batch answers) so a hung read never stops
//! the polling. The phone hears of a pane's new lines at most every 3 s ([`phone_revision_for`]). Nothing is saved: the lines can move
//! every second, and the state file must not.
//!
//! Locks: callers read the tails with the view locked (view, then this module's state); this module never takes the view while it holds
//! its state.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use mighty_core::graph::MightyGraphRun;

use crate::host::{Host, lock};

/// One pane's lines by block id (mighty-graph's `structure::mobile::Tails`, `view::mobile::OutputTails`).
pub type Tails = HashMap<String, Vec<String>>;

/// The poller's own 1 s clock.
const TICK: Duration = Duration::from_secs(1);
/// `BackgroundOutputPhoneInterval`: the phone hears of a pane's new lines at most this often; lines that move in between go out together
/// with the next.
pub const PHONE_INTERVAL: Duration = Duration::from_secs(3);
/// `BackgroundOutputReadTimeout`: a batch of reads still on one file after this is given up on.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

static EMPTY: LazyLock<Arc<Tails>> = LazyLock::new(|| Arc::new(Tails::new()));
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// A monotonic clock (C#'s `Environment.TickCount64`): a wall-clock jump never holds a read or the phone.
fn clock() -> Duration {
    STARTED.elapsed()
}

/// One pane as the poller reads it: its runs (the requests read back from the session record that attach above the saved ones, then the
/// saved ones), and whether this computer can read its files (a Claude pane in a local workspace).
pub struct PaneRuns<'a> {
    pub id: &'a str,
    pub readable: bool,
    pub lists: Vec<&'a [MightyGraphRun]>,
}

/// One file to read for one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub id: String,
    pub pane: String,
    pub path: String,
}

/// `BackgroundOutputProgress`: the block a batch is reading and since when, so a batch given up on names the one it is stuck on, and a
/// merely slow batch is not.
#[derive(Debug, Default)]
pub struct Progress(Mutex<Option<(String, Duration)>>);

impl Progress {
    pub fn start(&self, id: &str, now: Duration) {
        *lock(&self.0) = Some((id.to_string(), now));
    }

    pub fn finish(&self) {
        *lock(&self.0) = None;
    }

    fn current(&self) -> Option<(String, Duration)> {
        lock(&self.0).clone()
    }
}

/// The reads one tick hands the reader thread.
#[derive(Debug)]
pub struct Batch {
    pub id: u64,
    pub reads: Vec<Read>,
    pub progress: Arc<Progress>,
}

/// What a tick decided: whether the batch on its way was given up on (its reader is replaced), the reads to make, and the panes whose
/// lines a prune changed.
#[derive(Debug, Default)]
pub struct Tick {
    pub gave_up: bool,
    pub batch: Option<Batch>,
    pub changed: Vec<String>,
}

/// The poller's state (C#'s `backgroundOutput*` fields): pure over the clock and the reads, so a check drives it.
#[derive(Debug, Default)]
pub struct Poller {
    /// By pane; each pane's map is replaced whole when it changes, never edited, so a cache can tell a change by reference.
    lines: HashMap<String, Arc<Tails>>,
    /// Running task blocks being followed; the ones that owe their last (or, redrawn, their one) read; the ones read since they ended.
    polling: HashSet<String>,
    owed: HashSet<String>,
    read: HashSet<String>,
    /// A block a read hung on, by the batch it hung in: skipped until that batch answers.
    hung: HashMap<u64, String>,
    reading: bool,
    batch: u64,
    shape: Option<u64>,
    progress: Option<Arc<Progress>>,
    /// Each pane's revision, moved whenever its lines change or are pruned; the one the phone's digest reads, which follows it at most
    /// once per [`PHONE_INTERVAL`]; and when that one last moved. One counter feeds them all, so a revision is never reused.
    revision: HashMap<String, i64>,
    phone_revision: HashMap<String, i64>,
    phone_moved: HashMap<String, Duration>,
    counter: i64,
}

impl Poller {
    /// `BackgroundOutputFor`: one pane's lines by block id; empty when it has none.
    pub fn lines_of(&self, pane: &str) -> Arc<Tails> {
        self.lines.get(pane).cloned().unwrap_or_else(|| EMPTY.clone())
    }

    /// `BackgroundOutputPhoneRevisionFor`: what the phone's digest hashes instead of the pane's lines.
    pub fn phone_revision(&self, pane: &str) -> i64 {
        self.phone_revision.get(pane).copied().unwrap_or(0)
    }

    /// `BackgroundOutputTick`: `all` is every pane's id (for the graphs' shape and the panes still open), `panes` the readable ones and
    /// those whose runs are drawn.
    pub fn tick(&mut self, all: &[&str], panes: &[PaneRuns], now: Duration) -> Tick {
        let mut tick = Tick::default();
        if self.reading {
            // Only one file read for the whole timeout is given up on; a batch that is merely slow, moving from file to file, is waited for.
            let Some((stuck, since)) = self.progress.as_ref().and_then(|p| p.current()) else { return tick };
            if now.saturating_sub(since) <= READ_TIMEOUT {
                return tick;
            }
            // That block is skipped until its batch answers, under that batch's id; the batch id moves on so the answer reads as stale,
            // and later batches read on a thread of their own rather than wait behind it.
            self.hung.insert(self.batch, stuck);
            self.batch += 1;
            tick.gave_up = true;
            self.reading = false;
            self.progress = None;
        }
        let hung: HashSet<String> = self.hung.values().cloned().collect();
        // The graphs' shape: a pane, a request or a block that came or went.
        let mut shape = DefaultHasher::new();
        for id in all {
            id.hash(&mut shape);
        }
        let mut queued = HashSet::new();
        let mut reads = vec![];
        for pane in panes.iter().filter(|p| p.readable) {
            let held = self.lines_of(pane.id);
            for runs in &pane.lists {
                runs.len().hash(&mut shape);
                for run in *runs {
                    run.id().hash(&mut shape);
                    run.agents().len().hash(&mut shape);
                    for agent in run.agents() {
                        let Some(path) = agent.output_file.as_deref().filter(|_| agent.is_task()) else { continue };
                        let id = agent.id();
                        if queued.contains(id) {
                            continue;
                        }
                        if !mighty_core::graph::terminal(agent.status()) {
                            self.polling.insert(id.to_string());
                        } else if self.polling.remove(id) || !self.read.contains(id) && !held.contains_key(id) {
                            // It owes its last read once it ended, or its one read when redrawn, until a read of it is applied.
                            self.owed.insert(id.to_string());
                        } else if !self.owed.contains(id) {
                            continue;
                        }
                        if hung.contains(id) {
                            continue;
                        }
                        queued.insert(id.to_string());
                        reads.push(Read { id: id.to_string(), pane: pane.id.to_string(), path: path.to_string() });
                    }
                }
            }
        }
        let shape = shape.finish();
        if self.shape != Some(shape) {
            self.shape = Some(shape);
            tick.changed = self.prune(all, panes);
        }
        if reads.is_empty() {
            return tick;
        }
        self.batch += 1;
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        self.reading = true;
        tick.batch = Some(Batch { id: self.batch, reads, progress });
        tick
    }

    /// `PruneBackgroundOutput`: drops what belongs to blocks no longer in any graph (a closed pane, a trimmed request) and the lines of
    /// panes that closed or cannot be read here. Run only when the graphs' shape moved.
    fn prune(&mut self, all: &[&str], panes: &[PaneRuns]) -> Vec<String> {
        let live: HashSet<&str> = all.iter().copied().collect();
        let present: HashSet<&str> = panes
            .iter()
            .filter(|p| p.readable)
            .flat_map(|p| p.lists.iter().flat_map(|runs| runs.iter()))
            .flat_map(|run| run.agents().iter().filter(|a| a.is_task()).map(|a| a.id()))
            .collect();
        self.polling.retain(|id| present.contains(id.as_str()));
        self.read.retain(|id| present.contains(id.as_str()));
        self.owed.retain(|id| present.contains(id.as_str()));
        self.hung.retain(|_, id| present.contains(id.as_str()));
        let mut changed = vec![];
        for (pane, lines) in self.lines.clone() {
            // A remote or closed pane keeps nothing.
            let kept: Tails = if live.contains(pane.as_str()) {
                lines.iter().filter(|(id, _)| present.contains(id.as_str())).map(|(id, l)| (id.clone(), l.clone())).collect()
            } else {
                Tails::new()
            };
            if kept.len() == lines.len() {
                continue;
            }
            if kept.is_empty() {
                self.lines.remove(&pane);
            } else {
                self.lines.insert(pane.clone(), Arc::new(kept));
            }
            changed.push(pane);
        }
        for pane in &changed {
            self.changed(pane);
        }
        // A closed pane's revisions go with it; its lines went above.
        let gone: Vec<String> = self.revision.keys().filter(|p| !live.contains(p.as_str())).cloned().collect();
        for pane in gone {
            self.revision.remove(&pane);
            self.phone_revision.remove(&pane);
            self.phone_moved.remove(&pane);
        }
        changed
    }

    /// `ApplyBackgroundOutput`: publishes only what changed. A file that gave nothing keeps the lines read before it — the last lines stay
    /// once a run ended, and a missing file shows none. A batch given up on is dropped when it finally answers. The panes whose lines moved.
    pub fn apply(&mut self, results: Vec<(String, String, Option<Vec<String>>)>, batch: u64, closing: bool) -> Vec<String> {
        // Whatever this batch was stuck on may be read again, and a batch given up on brings stale lines.
        self.hung.remove(&batch);
        if batch != self.batch {
            return vec![];
        }
        self.reading = false;
        self.progress = None;
        if closing {
            return vec![];
        }
        let mut changed: HashMap<String, Tails> = HashMap::new();
        let mut order = vec![];
        for (id, pane, lines) in results {
            // A block dropped while its read was out stays dropped.
            let owed = self.owed.remove(&id);
            if owed {
                self.read.insert(id.clone());
            }
            let Some(lines) = lines.filter(|l| !l.is_empty()) else { continue };
            if !owed && !self.polling.contains(&id) {
                continue;
            }
            let before = changed.get(&pane).and_then(|c| c.get(&id)).or_else(|| self.lines.get(&pane).and_then(|l| l.get(&id)));
            if before == Some(&lines) {
                continue;
            }
            let current = changed.entry(pane.clone()).or_insert_with(|| {
                order.push(pane.clone());
                self.lines.get(&pane).map(|l| (**l).clone()).unwrap_or_default()
            });
            current.insert(id, lines);
        }
        for pane in &order {
            if let Some(lines) = changed.remove(pane) {
                self.lines.insert(pane.clone(), Arc::new(lines));
                self.changed(pane);
            }
        }
        order
    }

    /// `BackgroundOutputChanged`'s own part: the pane's revision moves.
    fn changed(&mut self, pane: &str) {
        self.counter += 1;
        self.revision.insert(pane.to_string(), self.counter);
    }

    /// `BackgroundOutputToPhone`: moves the phone's revision of every pane whose lines changed, at most once per [`PHONE_INTERVAL`] a pane.
    /// Whether any moved, and when the next pane held back is due.
    pub fn phone_due(&mut self, now: Duration) -> (bool, Option<Duration>) {
        let mut moved = false;
        let mut next: Option<Duration> = None;
        for (pane, revision) in &self.revision {
            if self.phone_revision.get(pane) == Some(revision) {
                continue;
            }
            if let Some(last) = self.phone_moved.get(pane)
                && now.saturating_sub(*last) < PHONE_INTERVAL
            {
                let due = *last + PHONE_INTERVAL;
                next = Some(next.map_or(due, |n| n.min(due)));
                continue;
            }
            self.phone_revision.insert(pane.clone(), *revision);
            self.phone_moved.insert(pane.clone(), now);
            moved = true;
        }
        (moved, next)
    }
}

/// `ReadBackgroundOutput`: the file's last lines, or none. Only a regular file on this computer is opened (a pipe or a device would block
/// or answer forever, a share is another computer's); a link is followed first, so where it points is what is judged.
pub fn read_output(path: &str) -> Option<Vec<String>> {
    if !local_file_path(path) || device_name(path) {
        return None;
    }
    let full = std::path::absolute(path).ok()?;
    if !local_file_path(&full.to_string_lossy()) {
        return None;
    }
    let target = std::fs::canonicalize(&full).ok()?;
    let target = target.to_string_lossy().into_owned();
    if !local_file_path(&target) {
        return None;
    }
    let meta = std::fs::metadata(&target).ok()?;
    if !meta.is_file() || is_device(&meta) {
        return None;
    }
    mighty_run::background_output::read(&target)
}

/// `LocalFilePath`: a drive letter (`C:\…`), or the same behind `\\?\`. The device namespaces (`\\.\…`, `\\?\GLOBALROOT\…`,
/// `\\?\pipe\…`), shares (`\\server\share`, `\\?\UNC\…`) and relative paths are not read.
#[cfg(windows)]
fn local_file_path(path: &str) -> bool {
    let rest = path.strip_prefix(r"\\?\").unwrap_or(path);
    if rest.starts_with(r"\\") || rest.starts_with("//") {
        return false;
    }
    let bytes = rest.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}

/// An absolute path on this computer (a share is mounted under a path here, as the Mac's).
#[cfg(not(windows))]
fn local_file_path(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//")
}

/// A legacy device name as the file's name (`C:\x\CON`, `nul.txt`): Windows would open the device, which may never answer.
#[cfg(windows)]
fn device_name(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or("");
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit())
}

#[cfg(not(windows))]
fn device_name(_path: &str) -> bool {
    false
}

#[cfg(windows)]
fn is_device(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;
    meta.file_attributes() & FILE_ATTRIBUTE_DEVICE != 0
}

#[cfg(not(windows))]
fn is_device(_meta: &std::fs::Metadata) -> bool {
    false
}

/// `BackgroundOutputReader`: one thread running the reads in turn. Given up on, it is completed — a read hung on it still holds that one
/// thread until it answers, and then the thread ends.
struct Reader {
    work: Option<mpsc::Sender<Box<dyn FnOnce() + Send>>>,
}

impl Reader {
    fn new() -> Reader {
        let (work, jobs) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let started = std::thread::Builder::new().name("background-output".into()).spawn(move || {
            for job in jobs {
                let _ = catch_unwind(AssertUnwindSafe(job));
            }
        });
        Reader { work: started.ok().map(|_| work) }
    }

    fn post(&self, job: Box<dyn FnOnce() + Send>) {
        if let Some(work) = &self.work {
            let _ = work.send(job);
        }
    }

    /// No more reads: the thread ends once the one it runs (if any) answers.
    fn complete(&mut self) {
        self.work = None;
    }
}

struct State {
    poller: Poller,
    reader: Reader,
    stopped: bool,
    /// The wait before a pane's lines held back from the phone are sent is on its way.
    phone_timer: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// `BackgroundOutputFor`: one pane's lines by block id (the page's Mighty view and strip, the phone); empty when it has none.
pub fn output_for(pane: &str) -> Arc<Tails> {
    lock(&STATE).as_ref().map_or_else(|| EMPTY.clone(), |s| s.poller.lines_of(pane))
}

/// `BackgroundOutputPhoneRevisionFor`: what the phone's digest hashes instead of the pane's lines.
pub fn phone_revision_for(pane: &str) -> i64 {
    lock(&STATE).as_ref().map_or(0, |s| s.poller.phone_revision(pane))
}

/// After the run engine (main.rs): the poller's own 1 s clock (a tick with no background run costs one walk over the graphs).
pub fn start(host: &Arc<Host>) {
    *lock(&STATE) = Some(State { poller: Poller::default(), reader: Reader::new(), stopped: false, phone_timer: false });
    let host = host.clone();
    let _ = std::thread::Builder::new().name("background-output-clock".into()).spawn(move || {
        loop {
            std::thread::sleep(TICK);
            if host.closing() || lock(&STATE).as_ref().is_none_or(|s| s.stopped) {
                return;
            }
            let _ = catch_unwind(AssertUnwindSafe(|| tick(&host)));
        }
    });
}

/// At quit (main.rs), before the runs are disposed: no more ticks, and the reader ends once the read it runs answers.
pub fn shutdown(_host: &Arc<Host>) {
    if let Some(state) = lock(&STATE).as_mut() {
        state.stopped = true;
        state.reader.complete();
    }
}

/// One tick: the panes read with the view locked, then the poller with its own state.
fn tick(host: &Arc<Host>) {
    let (sessions, readable, history) = {
        let view = host.view();
        let snapshot = &view.snapshot;
        let remote: HashSet<&str> = snapshot.workspaces().iter().filter(|w| w.remote.is_some()).map(|w| w.id()).collect();
        let sessions = snapshot.sessions().to_vec();
        // The panes whose files this PC can read: Claude panes in local workspaces.
        let readable: HashSet<String> = sessions
            .iter()
            .filter(|s| s.kind() == "claude" && s.provider() == "claude" && !remote.contains(s.workspace_id()))
            .map(|s| s.id().to_string())
            .collect();
        // The requests read back from the session record that attach above the saved ones (as the page shows them).
        let history: HashMap<String, Arc<Vec<MightyGraphRun>>> = sessions
            .iter()
            .filter(|s| readable.contains(s.id()) && view.panes.contains_key(s.id()))
            .map(|s| (s.id().to_string(), crate::history::loaded(host, s)))
            .collect();
        (sessions, readable, history)
    };
    let all: Vec<&str> = sessions.iter().map(|s| s.id()).collect();
    let panes: Vec<PaneRuns> = sessions
        .iter()
        .filter(|s| readable.contains(s.id()))
        .map(|s| {
            let mut lists: Vec<&[MightyGraphRun]> = vec![];
            if let Some(older) = history.get(s.id()) {
                lists.push(older.as_slice());
            }
            lists.push(s.graph_runs());
            PaneRuns { id: s.id(), readable: true, lists }
        })
        .collect();
    let changed = {
        let mut guard = lock(&STATE);
        let Some(state) = guard.as_mut().filter(|s| !s.stopped) else { return };
        let tick = state.poller.tick(&all, &panes, clock());
        if tick.gave_up {
            state.reader.complete();
            state.reader = Reader::new();
        }
        if let Some(batch) = tick.batch {
            let host = host.clone();
            // A thread of its own, never the shared pool: a read stalled on a dead share holds that one thread, not one the app needs.
            state.reader.post(Box::new(move || {
                let results: Vec<(String, String, Option<Vec<String>>)> = batch
                    .reads
                    .into_iter()
                    .map(|read| {
                        batch.progress.start(&read.id, clock());
                        let lines = read_output(&read.path);
                        (read.id, read.pane, lines)
                    })
                    .collect();
                batch.progress.finish();
                apply(&host, results, batch.id);
            }));
        }
        tick.changed
    };
    changed_panes(host, &changed);
}

/// What a batch read, given to the poller.
fn apply(host: &Arc<Host>, results: Vec<(String, String, Option<Vec<String>>)>, batch: u64) {
    let changed = {
        let mut guard = lock(&STATE);
        let Some(state) = guard.as_mut() else { return };
        state.poller.apply(results, batch, host.closing() || state.stopped)
    };
    changed_panes(host, &changed);
}

/// `BackgroundOutputChanged`: only those panes are drawn again — their strip (the cards) and their Mighty view when shown — and the
/// phone hears of them as of a graph change, at most every 3 s a pane.
fn changed_panes(host: &Arc<Host>, panes: &[String]) {
    if panes.is_empty() {
        return;
    }
    {
        let mut view = host.view();
        for pane in panes {
            view.mighty.dirty(pane);
            if let Some(ui) = view.panes.get_mut(pane) {
                ui.dirty = true;
            }
        }
    }
    to_phone(host);
}

fn to_phone(host: &Arc<Host>) {
    if host.closing() {
        return;
    }
    let (moved, wait) = {
        let mut guard = lock(&STATE);
        let Some(state) = guard.as_mut() else { return };
        let now = clock();
        let (moved, next) = state.poller.phone_due(now);
        let wait = next.filter(|_| !state.phone_timer).map(|at| at.saturating_sub(now).max(Duration::from_millis(1)));
        if wait.is_some() {
            state.phone_timer = true;
        }
        (moved, wait)
    };
    if moved {
        crate::phone::changed(host);
    }
    if let Some(wait) = wait {
        let host = host.clone();
        let _ = std::thread::Builder::new().name("background-output-phone".into()).spawn(move || {
            std::thread::sleep(wait);
            if let Some(state) = lock(&STATE).as_mut() {
                state.phone_timer = false;
            }
            to_phone(&host);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::graph::MightyGraphAgent;
    use std::io::Write;

    fn temp(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("mighty-tails-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Compared as the file system names it (CI's 8.3 %TEMP%, the Mac's /var link).
        std::fs::canonicalize(&root).unwrap()
    }

    fn task(id: &str, status: &str, file: &str) -> MightyGraphAgent {
        MightyGraphAgent {
            id: Some(id.into()),
            kind: Some("task".into()),
            status: Some(status.into()),
            background: Some(true),
            output_file: Some(file.into()),
            ..MightyGraphAgent::default()
        }
    }

    fn runs(agents: Vec<MightyGraphAgent>) -> Vec<MightyGraphRun> {
        vec![MightyGraphRun {
            id: Some("run-1".into()),
            status: Some("completed".into()),
            agents: Some(agents),
            ..MightyGraphRun::default()
        }]
    }

    /// What the reader thread does with a batch, here and now.
    fn read_all(batch: &Batch) -> Vec<(String, String, Option<Vec<String>>)> {
        batch.reads.iter().map(|r| (r.id.clone(), r.pane.clone(), read_output(&r.path))).collect()
    }

    fn tick_and_read(poller: &mut Poller, runs: &[MightyGraphRun], now: Duration) -> Vec<String> {
        let panes = [PaneRuns { id: "pane", readable: true, lists: vec![runs] }];
        let tick = poller.tick(&["pane"], &panes, now);
        let Some(batch) = tick.batch else { return tick.changed };
        let results = read_all(&batch);
        let mut changed = tick.changed;
        changed.extend(poller.apply(results, batch.id, false));
        changed
    }

    /// A Rust-only check (plan W9-E): the poller follows a file another writer appends to, reads a block once more after it ended and then
    /// leaves it, gives the pane's lines whole (a new map, a new revision) and tells the phone at most every 3 s. L1244's phone half with
    /// real tails: what it read is what the phone's blocks carry, never the file.
    #[test]
    fn the_poller_follows_a_file_another_writer_appends_to() {
        let root = temp("append");
        let path = root.join("task.output");
        let file = path.to_string_lossy().into_owned();
        let mut writer = std::fs::File::create(&path).unwrap();
        writer.write_all(b"compiling\n").unwrap();
        writer.flush().unwrap();
        let mut poller = Poller::default();
        let running = runs(vec![task("graph-task", "running", &file)]);
        assert_eq!(tick_and_read(&mut poller, &running, Duration::from_secs(1)), ["pane"], "the first read changes the pane");
        let first = poller.lines_of("pane");
        assert_eq!(first.get("graph-task").map(Vec::as_slice), Some(&["compiling".to_string()][..]), "the file's lines");
        // Another writer appends on its own thread while the run goes on.
        let appender = std::thread::spawn(move || {
            writer.write_all(b"linking\n").unwrap();
            writer.flush().unwrap();
            writer
        });
        let mut writer = appender.join().unwrap();
        assert_eq!(tick_and_read(&mut poller, &running, Duration::from_secs(2)), ["pane"], "new lines change the pane");
        let second = poller.lines_of("pane");
        assert!(!Arc::ptr_eq(&first, &second), "the pane's map is replaced whole");
        assert_eq!(second["graph-task"], ["compiling", "linking"], "the appended line is read");
        assert!(tick_and_read(&mut poller, &running, Duration::from_secs(3)).is_empty(), "the same lines change nothing");
        assert!(Arc::ptr_eq(&second, &poller.lines_of("pane")), "and keep the map");
        // The phone hears at once, then not again within 3 s; the held-back pane is due when its interval is up.
        assert_eq!(poller.phone_due(Duration::from_secs(3)), (true, None), "the phone hears of the pane");
        let told = poller.phone_revision("pane");
        assert!(told > 0, "with a revision");
        writer.write_all(b"done\n").unwrap();
        writer.flush().unwrap();
        // The block ended: one more read, then it is left alone.
        let ended = runs(vec![task("graph-task", "completed", &file)]);
        assert_eq!(tick_and_read(&mut poller, &ended, Duration::from_secs(4)), ["pane"], "the last read after the end");
        assert_eq!(poller.lines_of("pane")["graph-task"].last().map(String::as_str), Some("done"), "it reads what came last");
        assert_eq!(poller.phone_due(Duration::from_secs(4)), (false, Some(Duration::from_secs(6))), "held back until 3 s passed");
        assert_eq!(poller.phone_revision("pane"), told, "the phone still has the older revision");
        assert!(poller.phone_due(Duration::from_secs(6)).0 && poller.phone_revision("pane") > told, "and hears of it then");
        writer.write_all(b"after the end\n").unwrap();
        writer.flush().unwrap();
        let panes = [PaneRuns { id: "pane", readable: true, lists: vec![&ended] }];
        assert!(poller.tick(&["pane"], &panes, Duration::from_secs(7)).batch.is_none(), "an ended block read since is left alone");
        // What the phone's blocks carry: the lines, never the file.
        let notice = MightyGraphRun { notice_run: Some(true), ..ended[0].clone() };
        let tails = poller.lines_of("pane");
        use mighty_graph::view::mobile::{MobileOut, RunsInputs};
        let json = mighty_graph::view::mobile::runs(
            &[notice],
            &RunsInputs { output_tails: Some(&tails), ..RunsInputs::default() },
            &mighty_core::locale::Locale::new("ko"),
        )
        .to_json();
        assert!(json.contains("\"outputTail\"") && json.contains("done") && !json.contains(&file), "the phone gets the lines: {json}");
        // A closed pane takes its lines with it.
        assert_eq!(poller.tick(&[], &[], Duration::from_secs(8)).changed, ["pane"], "a closed pane's lines go");
        assert!(poller.lines_of("pane").is_empty() && poller.phone_revision("pane") == 0, "with its revisions");
        drop(writer);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A Rust-only check (plan W9-E): a batch stuck on one file for more than 10 s (an injected clock) is given up on; that block is
    /// skipped until its batch answers, the others go on, and the late answer is dropped as stale.
    #[test]
    fn a_hung_read_is_skipped_after_ten_seconds() {
        let root = temp("hung");
        let (stuck, other) = (root.join("stuck.output"), root.join("other.output"));
        std::fs::write(&stuck, "stuck line\n").unwrap();
        std::fs::write(&other, "other line\n").unwrap();
        let runs = runs(vec![task("stuck", "running", &stuck.to_string_lossy()), task("other", "running", &other.to_string_lossy())]);
        let panes = [PaneRuns { id: "pane", readable: true, lists: vec![&runs] }];
        let mut poller = Poller::default();
        let first = poller.tick(&["pane"], &panes, Duration::ZERO).batch.expect("the first batch");
        assert_eq!(first.reads.len(), 2, "both files are read");
        // The reader hangs on the first file.
        first.progress.start("stuck", Duration::ZERO);
        assert!(poller.tick(&["pane"], &panes, Duration::from_secs(5)).batch.is_none(), "a read under 10 s is waited for");
        assert!(poller.tick(&["pane"], &panes, READ_TIMEOUT).batch.is_none(), "10 s exactly is still waited for");
        let tick = poller.tick(&["pane"], &panes, READ_TIMEOUT + Duration::from_secs(1));
        assert!(tick.gave_up, "past 10 s the batch is given up on");
        let second = tick.batch.expect("a new batch on a new thread");
        assert_eq!(second.reads.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["other"], "the hung block is skipped");
        let applied = poller.apply(read_all(&second), second.id, false);
        assert_eq!(applied, ["pane"], "the others go on");
        // The hung batch finally answers: stale, dropped, and its block may be read again.
        assert!(poller.apply(read_all(&first), first.id, false).is_empty(), "a batch given up on is dropped");
        assert!(!poller.lines_of("pane").contains_key("stuck"), "its lines are not taken");
        let third = poller.tick(&["pane"], &panes, Duration::from_secs(13)).batch.expect("the next batch");
        assert!(third.reads.iter().any(|r| r.id == "stuck"), "the block is read again once its batch answered");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// `ReadBackgroundOutput`'s path rule: a missing file, a folder and (Windows) a device name or a share give nothing.
    #[test]
    fn only_a_regular_local_file_is_read() {
        let root = temp("rule");
        let file = root.join("x.output");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        assert_eq!(read_output(&file.to_string_lossy()), Some(vec!["one".to_string(), "two".to_string()]), "a regular file");
        assert!(read_output(&root.join("missing").to_string_lossy()).is_none(), "a missing file");
        assert!(read_output(&root.to_string_lossy()).is_none(), "a folder");
        assert!(read_output("relative.output").is_none(), "a relative path");
        #[cfg(windows)]
        {
            assert!(read_output(r"\\server\share\x.output").is_none(), "a share");
            assert!(read_output(r"\\?\UNC\server\share\x.output").is_none(), "a share behind \\\\?\\");
            assert!(read_output(r"\\.\pipe\x").is_none(), "a pipe");
            assert!(read_output(&root.join("CON").to_string_lossy()).is_none(), "a device name");
            assert!(read_output(&root.join("nul.txt").to_string_lossy()).is_none(), "a device name with an extension");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

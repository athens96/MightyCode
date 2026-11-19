//! What a crash leaves running: the process groups the app's children lead, written down so the next launch can end them.
//!
//! Windows puts every child in a Job Object that the OS kills when the app's handle closes, a crash included. The Mac has no such
//! object: `process`'s `atexit` handler ends the groups at a normal exit, and a crash skips it, so an agent CLI (and what it started in
//! its group) kept running with nobody reading it. Here the app writes each group it starts (its id and its leader's start time,
//! `proc_bsdinfo`) to a small file of its own under the app's machine-local folder (`child-groups/<app pid>-<app start>.groups`),
//! rewritten whenever a group starts or ends and removed when none is left or the app exits. At the next launch,
//! [`record_child_groups`] reads the files of app runs that are no longer running and ends what they left:
//! - a group whose leader still runs with the start time written down: SIGKILL to the group;
//! - a leader whose pid now names another process (another start time): nothing, never a reused pid (macOS does not hand out a pid that
//!   is still a group's id, so the old group is gone);
//! - a leader that is gone while its group still has members: each member that started between the leader and the file's last write,
//!   so nothing a later process put in a group of that id is touched.
//!
//! The file of an app that still runs (another instance) is left alone. Only `process`'s pipe children are written down: a detached
//! start (`start_detached*`: the update's installer, the relaunched app, the Ouroboros daemon, each in a session of its own) and the
//! PTY shells (which the kernel hangs up when the app's end of the terminal closes) never are. Only the app's own process writes:
//! nothing is recorded until it calls [`record_child_groups`] (the exe's helper modes, `--update-helper` among them, never do).

use std::collections::BTreeMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use crate::process::lock;

/// The record files' extension.
const EXTENSION: &str = "groups";
/// A record file is read up to this much (a few thousand groups); the rest is ignored.
const MAXIMUM_RECORD_BYTES: u64 = 1_048_576;

/// When a process started, in microseconds since 1970 (`pbi_start_tvsec`, `pbi_start_tvusec`): with its pid, it names one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Started(pub u64);

/// What the next launch asks the system about processes, and how it ends them (a fake in the checks).
pub trait ProcessTable {
    /// The start time of the live process `pid` (a zombie counts as gone); None when there is none.
    fn started(&self, pid: libc::pid_t) -> Option<Started>;
    /// The processes whose process group is `group` now.
    fn members(&self, group: libc::pid_t) -> Vec<libc::pid_t>;
    /// SIGKILL to the process group `group`.
    fn kill_group(&self, group: libc::pid_t);
    /// SIGKILL to the process `pid`.
    fn kill(&self, pid: libc::pid_t);
}

/// The system's process table (libproc) and `kill`.
#[derive(Debug, Default, Clone, Copy)]
pub struct System;

fn bsd_info(pid: libc::pid_t) -> Option<libc::proc_bsdinfo> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: a zeroed plain C struct the call fills in, its size given.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is `info`, of `size` bytes.
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (read == size && info.pbi_pid == pid as u32 && info.pbi_status != libc::SZOMB).then_some(info)
}

fn started_of(info: &libc::proc_bsdinfo) -> Started {
    Started(info.pbi_start_tvsec.saturating_mul(1_000_000).saturating_add(info.pbi_start_tvusec))
}

impl ProcessTable for System {
    fn started(&self, pid: libc::pid_t) -> Option<Started> {
        bsd_info(pid).map(|info| started_of(&info))
    }

    fn members(&self, group: libc::pid_t) -> Vec<libc::pid_t> {
        if group <= 1 {
            return vec![];
        }
        let mut pids = vec![0 as libc::pid_t; 4_096];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer is `pids`, of `bytes` bytes.
        let count = unsafe { libc::proc_listpgrppids(group, pids.as_mut_ptr().cast(), bytes) };
        // Whatever the count is in (pids or bytes), each pid is checked to be in the group now.
        pids.truncate(usize::try_from(count).unwrap_or(0).min(pids.len()));
        pids.retain(|pid| *pid > 1 && bsd_info(*pid).is_some_and(|info| info.pbi_pgid == group as u32));
        pids
    }

    fn kill_group(&self, group: libc::pid_t) {
        if group > 1 {
            // SAFETY: a signal to one process group (never -1, every process of the user's).
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }

    fn kill(&self, pid: libc::pid_t) {
        if pid > 1 {
            // SAFETY: a signal to one process.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// One app run's record: its file and the groups it leads now.
#[derive(Debug)]
pub struct Record {
    file: PathBuf,
    groups: BTreeMap<libc::pid_t, Started>,
}

impl Record {
    /// The record of the app run `owner` (pid, start) in `folder`; nothing is written until a group is added.
    pub fn new(folder: &Path, owner: (libc::pid_t, Started)) -> Record {
        Record { file: folder.join(format!("{}-{}.{EXTENSION}", owner.0, owner.1.0)), groups: BTreeMap::new() }
    }

    pub fn add(&mut self, group: libc::pid_t, started: Started) {
        self.groups.insert(group, started);
        self.write();
    }

    pub fn remove(&mut self, group: libc::pid_t) {
        if self.groups.remove(&group).is_some() {
            self.write();
        }
    }

    /// The file as the groups are now (through a temporary file and a rename, so a crash mid-write leaves the last whole one); no
    /// file when none is left. A write that fails leaves the record as it was: the cleanup is a best effort.
    fn write(&self) {
        if self.groups.is_empty() {
            let _ = std::fs::remove_file(&self.file);
            return;
        }
        let text: String = self.groups.iter().map(|(group, started)| format!("{group} {}\n", started.0)).collect();
        let temporary = self.file.with_extension(format!("{EXTENSION}.tmp"));
        if std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &self.file).is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    /// The app is exiting and has ended its groups itself: no file is left.
    pub fn clear(&mut self) {
        self.groups.clear();
        self.write();
    }
}

static RECORD: Mutex<Option<Record>> = Mutex::new(None);

/// At the app's launch, before its first child: ends what earlier app runs left running when they ended without their exit (a crash),
/// then writes this run's groups to `folder` (the app's machine-local folder's `child-groups`) until it exits. Call it once, from the
/// app's own process only. An error when the folder cannot be made (nothing is then written down; the `atexit` cleanup still runs).
pub fn record_child_groups(folder: &Path) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    reap_leftovers(folder, &System, me);
    // Another launch tells this run is still going by its start time: without one, nothing is written down (it would read as gone).
    let started = System.started(me).ok_or_else(|| io::Error::other("this process's start time could not be read"))?;
    *lock(&RECORD) = Some(Record::new(folder, (me, started)));
    Ok(())
}

/// `process`'s start: the group `group` its new child leads, written down while the app records.
pub(crate) fn started_group(group: libc::pid_t) {
    let mut record = lock(&RECORD);
    if let Some(record) = record.as_mut()
        && let Some(started) = System.started(group)
    {
        record.add(group, started);
    }
}

/// `process`'s reaper: the group ended (its leader reaped, what it left ended).
pub(crate) fn ended_group(group: libc::pid_t) {
    if let Some(record) = lock(&RECORD).as_mut() {
        record.remove(group);
    }
}

/// The `atexit` handler, once it has killed the groups: the file goes. A lock another thread holds is not waited on.
pub(crate) fn exiting() {
    if let Ok(mut record) = RECORD.try_lock()
        && let Some(record) = record.as_mut()
    {
        record.clear();
    }
}

/// The record file's owner: the app run's pid and start time, from `<pid>-<start>.groups` (or its `.groups.tmp`).
fn owner_of(name: &str) -> Option<(libc::pid_t, Started)> {
    let stem = name.strip_suffix(".groups.tmp").or_else(|| name.strip_suffix(".groups"))?;
    let (pid, started) = stem.split_once('-')?;
    Some((pid.parse().ok()?, Started(started.parse().ok()?)))
}

/// The groups a record file lists: `<group> <leader's start>` per line; a line that does not read is skipped.
fn records_in(text: &str) -> Vec<(libc::pid_t, Started)> {
    text.lines()
        .filter_map(|line| {
            let (group, started) = line.trim().split_once(' ')?;
            Some((group.parse().ok()?, Started(started.parse().ok()?)))
        })
        .collect()
}

/// Ends what one group of an app run that is gone left running (see the module); `written` is when its record file was last written.
pub fn reap_group(table: &dyn ProcessTable, me: libc::pid_t, group: libc::pid_t, started: Started, written: Started) {
    if group <= 1 || group == me {
        return;
    }
    match table.started(group) {
        Some(now) if now == started => table.kill_group(group),
        // The pid is another process's now: neither it nor its group is ours.
        Some(_) => {}
        None => {
            for member in table.members(group) {
                if member > 1 && member != me && table.started(member).is_some_and(|at| at >= started && at <= written) {
                    table.kill(member);
                }
            }
        }
    }
}

/// Every record file in `folder` of an app run that no longer runs: what it lists is ended ([`reap_group`]), then the file goes. The
/// files of runs still going (another instance, this one) stay.
pub fn reap_leftovers(folder: &Path, table: &dyn ProcessTable, me: libc::pid_t) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(owner) = path.file_name().and_then(|name| name.to_str()).and_then(owner_of) else { continue };
        if owner.0 == me || table.started(owner.0) == Some(owner.1) {
            continue;
        }
        if path.extension().is_some_and(|extension| extension == EXTENSION) {
            let written = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(Started(0), |since| Started(u64::try_from(since.as_micros()).unwrap_or(u64::MAX)));
            let mut text = String::new();
            let read = std::fs::File::open(&path).and_then(|file| file.take(MAXIMUM_RECORD_BYTES).read_to_string(&mut text));
            if read.is_ok() {
                for (group, started) in records_in(&text) {
                    reap_group(table, me, group, started, written);
                }
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A made-up process table: pid → (start, group); what was killed is kept.
    #[derive(Default)]
    struct Fake {
        processes: HashMap<libc::pid_t, (Started, libc::pid_t)>,
        killed_groups: RefCell<Vec<libc::pid_t>>,
        killed: RefCell<Vec<libc::pid_t>>,
    }

    impl Fake {
        fn with(processes: &[(libc::pid_t, u64, libc::pid_t)]) -> Fake {
            Fake {
                processes: processes.iter().map(|(pid, started, group)| (*pid, (Started(*started), *group))).collect(),
                ..Fake::default()
            }
        }
    }

    impl ProcessTable for Fake {
        fn started(&self, pid: libc::pid_t) -> Option<Started> {
            self.processes.get(&pid).map(|p| p.0)
        }
        fn members(&self, group: libc::pid_t) -> Vec<libc::pid_t> {
            let mut pids: Vec<_> = self.processes.iter().filter(|(_, p)| p.1 == group).map(|(pid, _)| *pid).collect();
            pids.sort_unstable();
            pids
        }
        fn kill_group(&self, group: libc::pid_t) {
            self.killed_groups.borrow_mut().push(group);
        }
        fn kill(&self, pid: libc::pid_t) {
            self.killed.borrow_mut().push(pid);
        }
    }

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let path = std::env::temp_dir().join(format!("mighty-child-groups-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_leader_that_still_runs_as_written_has_its_group_ended() {
        let table = Fake::with(&[(500, 1_000, 500), (501, 1_100, 500)]);
        reap_group(&table, 42, 500, Started(1_000), Started(2_000));
        assert_eq!(*table.killed_groups.borrow(), [500]);
        assert!(table.killed.borrow().is_empty());
    }

    #[test]
    fn a_reused_pid_is_never_touched() {
        // pid 500 started later than the leader written down: another process, maybe leading a group of its own.
        let table = Fake::with(&[(500, 5_000, 500), (501, 5_100, 500)]);
        reap_group(&table, 42, 500, Started(1_000), Started(2_000));
        assert!(table.killed_groups.borrow().is_empty() && table.killed.borrow().is_empty());
    }

    #[test]
    fn a_gone_leader_leaves_only_members_started_while_the_app_wrote() {
        // 501 started after the leader and before the last write: ours. 502 started after the last write, 503 before the leader:
        // not known to be ours. 504 is in another group.
        let table = Fake::with(&[(501, 1_500, 500), (502, 3_000, 500), (503, 900, 500), (504, 1_500, 600)]);
        reap_group(&table, 42, 500, Started(1_000), Started(2_000));
        assert!(table.killed_groups.borrow().is_empty(), "no group-wide signal without its leader");
        assert_eq!(*table.killed.borrow(), [501]);
    }

    #[test]
    fn no_group_of_one_or_less_and_never_the_app_itself() {
        let table = Fake::with(&[(1, 10, 1), (42, 10, 42)]);
        for group in [-5, 0, 1, 42] {
            reap_group(&table, 42, group, Started(10), Started(2_000));
        }
        assert!(table.killed_groups.borrow().is_empty() && table.killed.borrow().is_empty());
    }

    #[test]
    fn the_record_follows_the_groups_and_goes_when_none_is_left() {
        let temp = Temp::new("record");
        let mut record = Record::new(&temp.0, (77, Started(123)));
        let file = temp.0.join("77-123.groups");
        assert!(!file.exists(), "nothing written before a group");
        record.add(500, Started(1_000));
        record.add(600, Started(1_100));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "500 1000\n600 1100\n");
        record.remove(500);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "600 1100\n");
        record.remove(600);
        assert!(!file.exists(), "no group left, no file");
        record.add(700, Started(1));
        record.clear();
        assert!(!file.exists(), "the exit removes it");
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0, "no temporary file stays");
    }

    #[test]
    fn leftovers_of_a_gone_app_are_ended_and_a_running_app_is_left_alone() {
        let temp = Temp::new("leftovers");
        // App 10 (start 5) is gone; app 11 (start 6) still runs; 12 is this app.
        let mut gone = Record::new(&temp.0, (10, Started(5)));
        gone.add(500, Started(1_000));
        gone.add(600, Started(1_200));
        let mut running = Record::new(&temp.0, (11, Started(6)));
        running.add(700, Started(1_300));
        std::fs::write(temp.0.join("13-9.groups.tmp"), "800 1\n").unwrap();
        std::fs::write(temp.0.join("notes.txt"), "not a record").unwrap();
        // 500 runs as written; 600's pid is another process now; 700 is the running app's.
        let table = Fake::with(&[(11, 6, 11), (500, 1_000, 500), (600, 9_999_999_999_999_999, 600), (700, 1_300, 700)]);
        reap_leftovers(&temp.0, &table, 12);
        assert_eq!(*table.killed_groups.borrow(), [500]);
        assert!(table.killed.borrow().is_empty());
        let mut left: Vec<_> = std::fs::read_dir(&temp.0).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["11-6.groups", "notes.txt"], "the gone app's files go, the running app's and others stay");
    }

    #[test]
    fn owners_and_records_read_strictly() {
        assert_eq!(owner_of("10-5.groups"), Some((10, Started(5))));
        assert_eq!(owner_of("10-5.groups.tmp"), Some((10, Started(5))));
        assert_eq!(owner_of("10.groups"), None);
        assert_eq!(owner_of("x-5.groups"), None);
        assert_eq!(records_in("500 1000\nbad\n 600 7 \n700\n"), [(500, Started(1_000)), (600, Started(7))]);
    }

    /// A real child in a group of its own, written down by an app run that is gone: the next launch ends it. Written down with another
    /// start time (its pid reused, as far as the record knows), it is left running.
    #[test]
    #[allow(clippy::disallowed_methods, reason = "the test's own stand-in agent, no child of the app")]
    fn a_real_leftover_group_is_ended_and_a_reused_pid_survives() {
        use std::os::unix::process::CommandExt as _;
        let temp = Temp::new("real");
        let spawn = || std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let (mut ours, mut other) = (spawn(), spawn());
        let (ours_pid, other_pid) = (ours.id() as libc::pid_t, other.id() as libc::pid_t);
        let started = System.started(ours_pid).expect("a live child has a start time");
        let other_started = System.started(other_pid).unwrap();
        assert_eq!(System.members(ours_pid), [ours_pid], "the child leads its own group");
        // A gone app (pid 1 never has this start time): one group as it runs, one written with another start.
        let mut record = Record::new(&temp.0, (1, Started(7)));
        record.add(ours_pid, started);
        record.add(other_pid, Started(other_started.0 - 1));
        // SAFETY: getpid has no preconditions.
        reap_leftovers(&temp.0, &System, unsafe { libc::getpid() });
        let status = ours.wait().unwrap();
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGKILL), "the leftover is ended");
        assert!(other.try_wait().unwrap().is_none(), "a pid written with another start time is never killed");
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0, "the gone app's record goes");
    }
}

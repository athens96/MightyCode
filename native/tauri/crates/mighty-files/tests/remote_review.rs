//! MC-40: 요청 변경 검토 and 바뀐 부분별 받기·버리기 of an SSH workspace's requests, over the far side's snapshots — never a real host.
//! The "far side" is this computer: the remote commands run in a real `/bin/sh` with a home of the check's own and this computer's
//! `git` (one check through a stand-in `ssh` script too, which refuses anything naming the installed app). The review's rules are the
//! local ones: the same refusals, hunks and fingerprints.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::ssh::command::sh;
use mighty_core::ssh::review::{Moment, snapshot_script};
use mighty_files::remote_review::{RemoteExec, RemoteReview, SshExec};
use mighty_files::revert::{self, Outcome, Refusal};

/// The far side as a real `/bin/sh` here (what sshd's login shell would do with the command).
struct Here {
    home: PathBuf,
}

impl RemoteExec for Here {
    fn exec(&self, command: String, input: Option<Vec<u8>>) -> Result<(i32, Vec<u8>), String> {
        use std::io::Write as _;
        #[allow(clippy::disallowed_methods, reason = "the check's far side is a real shell here")]
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .env("HOME", &self.home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            let _ = stdin.write_all(&input);
        }
        drop(stdin);
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        Ok((output.status.code().unwrap_or(-1), output.stdout))
    }
}

#[allow(clippy::disallowed_methods, reason = "a check makes its repository")]
fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn has_git() -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| p.join("git").is_file())
}

struct Machine {
    root: PathBuf,
    folder: PathBuf,
    here: Arc<Here>,
}

impl Machine {
    fn new(name: &str) -> Machine {
        let root = std::env::temp_dir().join(format!("mc-remote-review-{name}-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("home");
        let folder = root.join("it's the repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        Machine { root, folder, here: Arc::new(Here { home }) }
    }

    fn dir(&self) -> String {
        self.folder.to_string_lossy().into_owned()
    }

    fn snapshot(&self, run: &str, moment: Moment) {
        let (code, _) = self.here.exec(sh(&snapshot_script(&self.dir(), "pane-1", run, moment).unwrap()), None).unwrap();
        assert_eq!(code, 0);
    }

    fn review(&self) -> RemoteReview {
        RemoteReview::load(self.here.clone(), &self.dir(), "pane-1", self.root.join("books"), "ssh:m:/x:pane-1".into()).unwrap()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.folder.join(name)).unwrap()
    }
}

fn lines(range: std::ops::Range<u32>, changed: &[u32]) -> String {
    range.map(|n| if changed.contains(&n) { format!("CHANGED {n}\n") } else { format!("line {n}\n") }).collect()
}

/// Two requests of a pane: the first changed two far-apart stretches of a file and made a file; the second changed one of those
/// stretches again. The first's review: the made file to trash, the file's first hunk put back and its second refused (`laterRun`),
/// hunks taken, a file changed since it was read refused (`conflict`), permissions and CRLF kept; a request whose change was undone
/// reads `unchanged`.
#[test]
fn a_remote_request_is_reviewed_hunk_by_hunk_with_the_local_rules() {
    if !has_git() {
        return;
    }
    let machine = Machine::new("hunks");
    let file = machine.folder.join("a.txt");
    std::fs::write(&file, lines(0..40, &[]).replace('\n', "\r\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    git(&machine.folder, &["init", "-q"]);
    git(&machine.folder, &["add", "-A"]);
    git(&machine.folder, &["commit", "-q", "-m", "base"]);

    machine.snapshot("r1", Moment::Start);
    std::fs::write(&file, lines(0..40, &[2, 30]).replace('\n', "\r\n")).unwrap();
    std::fs::write(machine.folder.join("made.txt"), "new file\n").unwrap();
    machine.snapshot("r1", Moment::End);
    machine.snapshot("r2", Moment::Start);
    std::fs::write(&file, lines(0..40, &[2, 30, 31]).replace('\n', "\r\n")).unwrap();
    machine.snapshot("r2", Moment::End);

    let review = machine.review();
    assert_eq!(review.changed_runs(), ["r2", "r1"]);
    review.prefetch("r1");
    let items = revert::plan_on(&review, &review, "r1");
    let names: Vec<(&str, Option<Refusal>)> = items.iter().map(|i| (i.path.as_str(), i.refusal)).collect();
    assert_eq!(names, [("a.txt", Some(Refusal::LaterRun)), ("made.txt", None)]);
    assert_eq!(items[1].action, revert::Action::Trash);

    let hunks = revert::hunk_review_on(&review, &review, "r1", "a.txt").unwrap();
    let states = hunks["hunks"].as_array().unwrap();
    assert_eq!(states.len(), 2, "{hunks}");
    assert!(states[0].get("refusal").is_none(), "the first stretch is the first request's alone");
    assert_eq!(states[1]["refusal"], "laterRun", "the second request changed lines beside the second");
    let first = states[0]["fingerprint"].as_str().unwrap().to_string();
    let second = states[1]["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(revert::discard_hunk_on(&review, &review, "r1", "a.txt", &second, None), Outcome::Refused(Refusal::LaterRun));
    assert_eq!(revert::discard_hunk_on(&review, &review, "r1", "a.txt", &first, None), Outcome::Reverted);
    assert_eq!(machine.read("a.txt"), lines(0..40, &[30, 31]).replace('\n', "\r\n"), "only that hunk went back, CRLF kept");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640, "permissions kept");
    assert!(revert::accept_hunks_on(&review, &review, "r1", &["a.txt".to_string()], None));

    // The file changed over there after the review read it: nothing is written.
    let fresh = machine.review();
    let hunks = revert::hunk_review_on(&fresh, &fresh, "r2", "a.txt").unwrap();
    let only = hunks["hunks"][0]["fingerprint"].as_str().unwrap().to_string();
    std::fs::write(&file, "someone else\n").unwrap();
    assert_eq!(revert::discard_hunk_on(&fresh, &fresh, "r2", "a.txt", &only, None), Outcome::Refused(Refusal::Conflict));
    assert_eq!(machine.read("a.txt"), "someone else\n");

    // The made file goes to the store's trash, not away.
    let outcomes = revert::apply_on(&fresh, &fresh, "r1", Some(&["made.txt".to_string()]));
    assert_eq!(outcomes, [("made.txt".to_string(), Outcome::Reverted)]);
    assert!(!machine.folder.join("made.txt").exists());
    let trashed = walk(&machine.here.home).into_iter().any(|p| p.ends_with("made.txt"));
    assert!(trashed, "kept in the store's trash");

    // A request whose change was undone by hand reads unchanged.
    machine.snapshot("r3", Moment::Start);
    std::fs::write(machine.folder.join("b.txt"), "x\n").unwrap();
    git(&machine.folder, &["add", "b.txt"]);
    git(&machine.folder, &["commit", "-q", "-m", "b"]);
    machine.snapshot("r3", Moment::End);
    machine.snapshot("r4", Moment::Start);
    std::fs::write(machine.folder.join("b.txt"), "y\n").unwrap();
    machine.snapshot("r4", Moment::End);
    std::fs::write(machine.folder.join("b.txt"), "x\n").unwrap();
    let last = machine.review();
    let items = revert::plan_on(&last, &last, "r4");
    assert_eq!(items[0].refusal, Some(Refusal::Unchanged));
    std::fs::remove_dir_all(&machine.root).unwrap();
}

fn walk(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// The same reading through ssh (a stand-in `ssh` script that runs the command in a login shell here and refuses anything that
/// names the installed app).
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn the_review_reads_through_ssh() {
    if !has_git() {
        return;
    }
    let machine = Machine::new("ssh");
    std::fs::write(machine.folder.join("a.txt"), "one\n").unwrap();
    machine.snapshot("r1", Moment::Start);
    std::fs::write(machine.folder.join("a.txt"), "two\n").unwrap();
    machine.snapshot("r1", Moment::End);
    let ssh = machine.root.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--\" ]; then shift; break; fi; shift; done\nshift\n\
             case \"$1\" in *MightyCode.app*|*MightyCode-local*) exit 99;; esac\n\
             exec /usr/bin/env -i HOME='{home}' PATH=/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin SHELL=/bin/sh /bin/sh -c \"$1\"\n",
            home = machine.here.home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launch = mighty_run::ssh::SshLaunch {
        program: ssh,
        target: mighty_core::ssh::Target::parse("devbox").unwrap(),
        mux: mighty_core::ssh::Mux::None,
        dir: machine.dir(),
        local: machine.root.clone(),
    };
    let processes: Arc<dyn mighty_platform::ProcessHost> = Arc::new(mighty_platform_macos::process::MacProcessHost::new());
    let exec = Arc::new(SshExec { processes, launch, handle: tokio::runtime::Handle::current() });
    let (dir, books) = (machine.dir(), machine.root.join("books"));
    let items = tokio::task::spawn_blocking(move || {
        let review = RemoteReview::load(exec, &dir, "pane-1", books, "s".into()).unwrap();
        revert::plan_on(&review, &review, "r1")
    })
    .await
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!((items[0].restored, items[0].removed), (1, 1));
    std::fs::remove_dir_all(&machine.root).unwrap();
}

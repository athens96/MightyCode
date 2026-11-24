//! MC-47: pane worktrees of an SSH workspace — never a real host. The "machine" is this computer reached through a stand-in `ssh`
//! script: it reads the one-word remote command (`/bin/sh -c '…'`, hex-wrapped or not), logs what it decoded, refuses (99) anything
//! naming the installed app, and runs it with a real `/bin/sh`, a home of the check's own and this computer's `git`. The same
//! scenario runs on this computer's own machine (`Local`) and on the stand-in SSH machine (`Remote`) and must say the same thing at
//! every step: create, copy and share, status, the merge refused while work is uncommitted, keep on close, a file's diff, commit
//! (a secret-looking file left out), merge, undo, the list, its own place, the discard and its blocks. A machine that cannot be
//! reached never reads as "no work": the worktree is kept and the merge fails.
#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::model::{PaneWorktree, RunSession};
use mighty_core::ssh::{Mux, Target};
use mighty_delegation::git::cleanup::ChildDiscardOutcome;
use mighty_delegation::git::worktree::{ChildWorktreeCheck, ChildWorktreeOutcome};
use mighty_delegation::git::{Git, GitRunner};
use mighty_delegation::machine::{Local, Remote};
use mighty_delegation::pane_worktree::{self as pw, CommitOutcome, MergeOutcome, UndoOutcome, Worktrees};
use mighty_platform::fake::FixedDiskSpace;
use mighty_run::ssh::SshLaunch;
use mighty_run::ssh_git::SshFolder;

/// The stand-in's decoding of the one word (mighty-run's checks' `DECODE_REMOTE`).
const DECODE: &str = "decode() { w=${1#/bin/sh -c }; if [ \"$w\" = \"$1\" ]; then printf '%s' \"$1\"; return; fi; \
    b=$(eval \"printf %s $w\"); case \"$b\" in 'eval \"$(LC_ALL=C awk'*) eval \"printf %s ${b#eval }\";; *) printf '%s' \"$b\";; esac; }";

const GITCONFIG: &str =
    "[user]\n\tname = Mighty Check\n\temail = check@example.invalid\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n";

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The folder git is in here, for the far side's `PATH`.
fn git_folder() -> Option<PathBuf> {
    mighty_run::git::executable().and_then(|git| git.parent().map(Path::to_path_buf))
}

/// A fixture git with the check's home (its `.gitconfig`), never the user's.
#[allow(clippy::disallowed_methods, reason = "a check makes its repository")]
fn fixture(home: &Path, dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A check's own place: a home with a `.gitconfig`, a repository with `.env` (ignored), `node_modules/` (ignored) and a tracked
/// folder, and the stand-in `ssh`.
struct Place {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    ssh: PathBuf,
}

impl Drop for Place {
    fn drop(&mut self) {
        support::place::remove_folder(&self.root);
    }
}

impl Place {
    fn new(name: &str) -> Place {
        let root = support::place::temp(name);
        let root = std::fs::canonicalize(&root).unwrap();
        let home = root.join("home");
        let repo = root.join("it's the repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(repo.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(repo.join("tracked")).unwrap();
        std::fs::write(home.join(".gitconfig"), GITCONFIG).unwrap();
        fixture(&home, &repo, &["init", "-q"]);
        std::fs::write(repo.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
        std::fs::write(repo.join("README.md"), "# remote\n").unwrap();
        std::fs::write(repo.join("tracked/a.txt"), "a\n").unwrap();
        fixture(&home, &repo, &["add", "-A"]);
        fixture(&home, &repo, &["commit", "-q", "-m", "Start"]);
        std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(repo.join("node_modules/pkg/index.js"), "x\n").unwrap();
        let ssh = root.join("ssh");
        // `remote-bin` first: a check may put a stand-in tool there (a `netstat` the sandbox cannot spoil).
        std::fs::create_dir_all(root.join("remote-bin")).unwrap();
        let path = format!(
            "{}:{}:/usr/bin:/bin:/usr/sbin:/sbin",
            text(&root.join("remote-bin")),
            git_folder().map(|f| text(&f)).unwrap_or_default()
        );
        executable(
            &ssh,
            &format!(
                "#!/bin/sh\n{DECODE}\n\
                 while [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--\" ]; then shift; break; fi; shift; done\nshift\n\
                 if [ -f '{root}/lost' ]; then echo 'Connection to devbox closed by remote host.' >&2; exit 255; fi\n\
                 script=$(decode \"$1\")\n\
                 printf '%s\\n----\\n' \"$script\" >> '{root}/remote.log'\n\
                 case \"$script\" in *'/Applications/MightyCode'*|*MightyCode-local*) echo 'the installed app is never run' >&2; exit 99;; esac\n\
                 exec /usr/bin/env -i SHELL=/bin/sh PATH='{path}' HOME='{home}' GIT_CONFIG_NOSYSTEM=1 GIT_DIR=/nowhere /bin/sh -c \"$1\"\n",
                root = text(&root),
                home = text(&home),
            ),
        );
        Place { root, home, repo, ssh }
    }

    fn launch(&self) -> SshLaunch {
        SshLaunch {
            program: self.ssh.clone(),
            target: Target::parse("devbox").unwrap(),
            mux: Mux::None,
            dir: text(&self.repo),
            local: self.root.clone(),
        }
    }

    async fn remote(&self) -> Remote {
        Remote::connect(support::place::processes(), self.launch()).await.expect("the stand-in machine answers")
    }

    fn local(&self) -> Worktrees {
        let config = self.root.join("local.gitconfig");
        std::fs::write(&config, GITCONFIG).unwrap();
        let git = Git::new(support::place::processes(), PathBuf::from("/dev/null")).with_global_config(config);
        let machine = Local::new(git, support::place::paths(), Some(Arc::new(FixedDiskSpace(50_000_000_000))));
        Worktrees::new(Arc::new(machine), &text(&self.root.join("local-worktrees")))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("remote.log")).unwrap_or_default()
    }

    fn head(&self) -> String {
        fixture(&self.home, &self.repo, &["rev-parse", "HEAD"])
    }

    fn branches(&self) -> String {
        fixture(&self.home, &self.repo, &["for-each-ref", "--format=%(refname)", "refs/heads/mighty/"])
    }
}

/// One scenario, step by step, as words that never name a commit or a path: the same on every machine.
async fn scenario(place: &Place, worktrees: &Worktrees, id: &str) -> Vec<String> {
    let locale = Locale::new("en");
    let machine = worktrees.machine.as_ref();
    let mut said = vec![];
    let ChildWorktreeCheck::Ready(start) = worktrees.check(&text(&place.repo)).await else {
        panic!("the repository cannot host a worktree")
    };
    said.push(format!("check {} {:?}", start.parent_branch, start.prefix));
    let ChildWorktreeOutcome::Created(made) = pw::make_pane(worktrees, id, &start, 300.0).await else { panic!("no worktree") };
    let pane = pw::from_made(&made, &start.repository, 4141, None);
    let worktree = PathBuf::from(pane.worktree_path());
    said.push(format!("made {} {} {}", pane.branch(), pane.base_branch(), worktree.is_dir()));
    let copied = machine.copy_paths(&[".env".to_string(), "missing.txt".to_string()], &start.repository, pane.run_folder()).await;
    said.push(format!("copied {:?} {:?}", copied.copied, copied.skipped));
    let shared = machine.share(&["node_modules".to_string(), "tracked".to_string()], &start.repository, pane.run_folder()).await;
    said.push(format!("shared {:?} {:?}", shared.shared, shared.skipped));
    let exclude = std::fs::read_to_string(place.repo.join(".git/info/exclude")).unwrap_or_default();
    said.push(format!("excluded {}", exclude.contains("mighty-code shared") && exclude.contains("\n/node_modules\n")));
    said.push(format!("status {:?}", pw::status(machine, &pane).await));
    std::fs::write(worktree.join("feature.txt"), "feature\n").unwrap();
    std::fs::write(worktree.join("id_rsa"), "secret\n").unwrap();
    said.push(format!("merge with work {:?}", pw::merge(machine, &pane, &locale).await));
    said.push(format!(
        "unmerged {:?} kept {}",
        pw::has_unmerged_work(machine, &pane).await,
        pw::kept_on_close(machine, &pane, false).await
    ));
    said.push(format!("diff {:?}", pw::file_diff(machine, &pane, "feature.txt").await));
    said.push(format!("diff outside {:?}", pw::file_diff(machine, &pane, "../x").await));
    said.push(format!("commit {:?}", pw::commit_all(machine, &pane, &locale).await));
    said.push(format!("commit again {:?}", pw::commit_all(machine, &pane, &locale).await));
    std::fs::remove_file(worktree.join("id_rsa")).unwrap();
    let files = pw::changed_files(machine, &pane).await.unwrap_or_default();
    said.push(format!("changed {:?}", files.iter().map(|f| (f.path.clone(), f.status.clone())).collect::<Vec<_>>()));
    said.push(format!("stat {:?}", pw::diff_stat(machine, &pane).await));
    let before = place.head();
    let merged = pw::merge(machine, &pane, &locale).await;
    let MergeOutcome::Merged { commit, pre_merge } = merged else { panic!("not merged: {merged:?}") };
    said.push(format!("merged {} {}", pre_merge == before, place.head() == commit && place.repo.join("feature.txt").exists()));
    let recorded = PaneWorktree { merged_commit: Some(commit.clone()), pre_merge_commit: Some(pre_merge.clone()), ..pane.clone() };
    let undone = pw::undo_merge(machine, &recorded, &locale).await;
    said.push(format!("undo {undone:?} {} {}", place.head() == before, place.repo.join("feature.txt").exists()));
    said.push(format!("undo again {:?}", pw::undo_merge(machine, &recorded, &locale).await));
    said.push(format!("own place {}", pw::in_own_place(machine, &pane, &worktrees.root).await));
    let misplaced = PaneWorktree { worktree_path: Some(text(&place.repo)), ..pane.clone() };
    said.push(format!("misplaced {}", pw::in_own_place(machine, &misplaced, &worktrees.root).await));
    let listed = pw::worktrees(machine, &start.repository, &worktrees.root).await.unwrap_or_default();
    said.push(format!("listed {:?}", listed.iter().map(|d| (d.session_id.clone(), d.branch.clone())).collect::<Vec<_>>()));
    // Children block the discard; unmerged commits keep the worktree on close.
    let child = RunSession { id: Some("child".into()), parent_session_id: Some(id.to_string()), ..RunSession::default() };
    said.push(format!("blocked {}", pw::discard_blocker(id, [&child], &[], &locale).is_some()));
    said.push(format!("kept with commits {}", pw::kept_on_close(machine, &pane, false).await));
    let discarded = pw::discard(machine, &pane, id, &worktrees.root, &locale).await;
    let exclude = std::fs::read_to_string(place.repo.join(".git/info/exclude")).unwrap_or_default();
    said.push(format!("discard {discarded:?} {} {:?} {}", worktree.exists(), place.branches(), exclude.contains("mighty-code shared")));
    said
}

#[tokio::test]
async fn an_ssh_machines_pane_worktree_follows_the_local_rules_step_by_step() {
    let _lane = support::lane::enter().await;
    if git_folder().is_none() {
        return;
    }
    let place = Place::new("remote-worktree");
    let local = scenario(&place, &place.local(), "pane-local").await;
    let remote = Worktrees::new(Arc::new(place.remote().await), &text(&place.home.join(".mightycode/worktrees")));
    let over_ssh = scenario(&place, &remote, "pane-remote").await;
    assert_eq!(over_ssh.len(), local.len());
    for (here, there) in local.iter().zip(&over_ssh) {
        let there = there.replace("pane-remote", "pane-local");
        assert_eq!(&there, here, "\nlocal:  {here}\nremote: {there}");
    }
    // What the remote side was asked: every git without a program a repository could start and without a transport.
    let log = place.log();
    assert!(log.contains("worktree") && log.contains("merge-tree"), "the machine ran the steps");
    for script in log.split("\n----\n").filter(|s| s.contains("\ngit -c ")) {
        assert!(
            script.contains("'core.hooksPath=/dev/null'")
                && script.contains("'core.fsmonitor=false'")
                && script.contains("'protocol.allow=never'")
                && script.contains("GIT_NO_LAZY_FETCH=1"),
            "{script}"
        );
    }
    let expected = [
        "check main \"\"",
        "made mighty/pane-local main true",
        "copied [\".env\"] [(\"missing.txt\", \"missing\")]",
        "shared [\"node_modules\"] [(\"tracked\", \"notIgnored\")]",
        "excluded true",
    ];
    assert_eq!(&local[..5], expected);
    assert!(local.iter().any(|l| l == "merge with work Refused(\"uncommittedChanges\")"));
    assert!(local.iter().any(|l| l == "unmerged Some(true) kept true"));
    assert!(local.iter().any(|l| l.starts_with("commit Committed { left_out: [\"id_rsa\"] }")));
    assert!(local.iter().any(|l| l == "merged true true"));
    assert!(local.iter().any(|l| l == "undo Undone true false"));
    assert!(local.iter().any(|l| l == "own place true") && local.iter().any(|l| l == "misplaced false"));
    assert!(local.iter().any(|l| l == "blocked true") && local.iter().any(|l| l == "kept with commits true"));
    assert_eq!(local.last().unwrap(), "discard Discarded false \"\" false");
}

#[tokio::test]
async fn an_unreachable_machine_never_reads_as_a_worktree_without_work() {
    let _lane = support::lane::enter().await;
    if git_folder().is_none() {
        return;
    }
    let place = Place::new("remote-lost");
    let remote = place.remote().await;
    let root = remote.root();
    let worktrees = Worktrees::new(Arc::new(remote), &root);
    let ChildWorktreeCheck::Ready(start) = worktrees.check(&text(&place.repo)).await else { panic!("not ready") };
    let ChildWorktreeOutcome::Created(made) = pw::make_pane(&worktrees, "pane-lost", &start, 300.0).await else { panic!("no worktree") };
    let pane = pw::from_made(&made, &start.repository, 4141, None);
    let machine = worktrees.machine.as_ref();
    let locale = Locale::new("en");
    assert_eq!(pw::has_unmerged_work(machine, &pane).await, Some(false), "a clean worktree has no work");
    std::fs::write(place.root.join("lost"), "").unwrap();
    assert_eq!(pw::has_unmerged_work(machine, &pane).await, None, "no answer is no 'no work'");
    assert!(pw::kept_on_close(machine, &pane, false).await, "a worktree on a machine that cannot be asked is kept");
    assert!(matches!(pw::merge(machine, &pane, &locale).await, MergeOutcome::Failed(_)), "the merge fails, it is not refused as gone");
    assert!(!pw::in_own_place(machine, &pane, &root).await, "no write where the place cannot be read");
    assert!(matches!(pw::commit_all(machine, &pane, &locale).await, CommitOutcome::Failed(_)));
    assert!(matches!(pw::undo_merge(machine, &pane, &locale).await, UndoOutcome::Refused("notMerged")));
    assert!(matches!(pw::discard(machine, &pane, "pane-lost", &root, &locale).await, ChildDiscardOutcome::Failed(_)));
    assert!(Path::new(pane.worktree_path()).is_dir(), "nothing went");
    std::fs::remove_file(place.root.join("lost")).unwrap();
    assert_eq!(pw::discard(machine, &pane, "pane-lost", &root, &locale).await, ChildDiscardOutcome::Discarded);
}

#[tokio::test]
async fn the_stand_in_never_runs_the_installed_app_and_a_push_alone_may_use_a_transport() {
    let _lane = support::lane::enter().await;
    let place = Place::new("remote-guard");
    let folder = SshFolder::new(support::place::processes(), place.launch());
    for app in ["/Applications/MightyCode.app/Contents/MacOS/MightyCode --cli status", "\"$HOME\"/MightyCode-local/MightyCode"] {
        let ended = folder.exec(&format!("echo start\n{app}"), None, std::time::Duration::from_secs(10), 1 << 16).await.unwrap();
        assert_eq!(ended.code, 99, "the stand-in refuses {app}");
    }
    if git_folder().is_none() {
        return;
    }
    let remote = place.remote().await;
    let repo = text(&place.repo);
    let pushed = remote.git_run(&["push", "--porcelain", "origin", "refs/heads/x:refs/heads/x"], &repo, 10.0, None).await;
    assert!(pushed.ok().is_some_and(|r| r.exit_code != 0), "no origin: git says so");
    let log = place.log();
    let push = log.split("\n----\n").find(|s| s.contains("'push'")).unwrap();
    assert!(!push.contains("protocol.allow") && push.contains("'core.hooksPath=/dev/null'"), "{push}");
}

/// The setup and teardown commands run on the machine, in the worktree there, with the worktree's values; the remote port block skips what listens over there and is kept under the machine's own keys; a pane's PORT is
/// reached through an ssh forward of that port.
#[tokio::test]
async fn setup_ports_and_forward_go_to_the_machine() {
    use mighty_core::ssh::worktree::{command_script, listening_script, parse_listening};
    use mighty_delegation::worktree_ports::{self, PortsFile};
    let _lane = support::lane::enter().await;
    let place = Place::new("remote-setup");
    let folder = SshFolder::new(support::place::processes(), place.launch());
    let tree = place.root.join("tree with space");
    std::fs::create_dir_all(&tree).unwrap();
    let values = vec![("PORT".to_string(), "41234".to_string()), ("MIGHTY_WORKTREE".to_string(), text(&tree))];
    let setup = "printf '%s|%s' \"$PORT\" \"$(pwd -P)\" > made-by-setup.txt";
    let ended = folder
        .exec(&command_script(&text(&tree), setup, &values).unwrap(), None, std::time::Duration::from_secs(30), 1 << 16)
        .await
        .unwrap();
    assert_eq!(ended.code, 0, "{}", String::from_utf8_lossy(&ended.stderr));
    assert_eq!(std::fs::read_to_string(tree.join("made-by-setup.txt")).unwrap(), format!("41234|{}", text(&tree)));
    let failing = folder
        .exec(
            &command_script(&text(&tree), "false\necho never > never.txt", &[]).unwrap(),
            None,
            std::time::Duration::from_secs(30),
            1 << 16,
        )
        .await
        .unwrap();
    assert!(failing.code != 0 && !tree.join("never.txt").exists(), "a failing line ends a setup of several lines");

    // Something listens over there: the block starts past it. (`netstat` is the machine's; a stand-in here, as a sandboxed check
    // may not see this computer's sockets. A Linux machine's `/proc/net/tcp` is read first where it is.)
    let busy: u16 = 47_123;
    executable(
        &place.root.join("remote-bin/netstat"),
        &format!(
            "#!/bin/sh\nprintf 'Active Internet connections (including servers)\\nProto Recv-Q Send-Q  Local Address          Foreign Address        (state)\\n'\n\
             printf 'tcp4       0      0  127.0.0.1.{busy}        *.*                    LISTEN\\n'\n"
        ),
    );
    let ended = folder.exec(&listening_script(), None, std::time::Duration::from_secs(30), 8 << 20).await.unwrap();
    let listening = parse_listening(&String::from_utf8_lossy(&ended.stdout)).unwrap_or_default();
    assert!(
        std::path::Path::new("/proc/net/tcp").exists() || listening.contains(&busy),
        "the machine's listening ports are read: {listening:?}"
    );
    let listening = if listening.contains(&busy) { listening } else { std::collections::BTreeSet::from([busy]) };
    let mut file = PortsFile::default();
    let (repo, worktree) =
        (worktree_ports::remote_key("m1", "/srv/app"), worktree_ports::remote_key("m1", "/home/me/.mightycode/worktrees/p"));
    assert_eq!(worktree, "ssh://m1/home/me/.mightycode/worktrees/p");
    let block = file.assign(&repo, &worktree, 10, busy.saturating_sub(5), |port| listening.contains(&port)).unwrap();
    assert!(!block.contains(busy) && block.base > busy.saturating_sub(5), "{block:?} skips {busy}");
    let released = file.prune(|path| worktree_ports::is_remote_key(path) || std::path::Path::new(path).is_dir());
    assert!(released.is_empty() && file.get(&repo, &worktree) == Some(block), "the start's prune keeps a machine's blocks");

    // The PORT on the machine is reached here through `ssh -N -L 127.0.0.1:<local>:localhost:<port>`.
    let forward = place.launch().forward_request(43_210, 41_234, &std::collections::BTreeMap::new());
    let args = forward.args.clone();
    assert!(args.windows(2).any(|w| w[0] == "-L" && w[1] == "127.0.0.1:43210:localhost:41234"), "{args:?}");
    assert!(args.contains(&"ExitOnForwardFailure=yes".to_string()) && args.ends_with(&["--".to_string(), "devbox".to_string()]));
}

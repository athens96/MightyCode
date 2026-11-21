//! Git as the app runs it (C# `DelegationGit`, DelegationWorktree.cs:198-325 on `fix/csharp-from-tauri-review`): git from the PATH
//! through the platform's `ProcessHost`, the user's own settings but no hooks, fsmonitor or auto gc, no program a repository's config
//! names, no inherited `GIT_*` variable, no prompt, at most 1 MiB of output and a deadline that ends git with everything it started.
//!
//! One runner, two argument lists:
//! - [`Git::run_detailed`] is delegation's (mighty-delegation's worktrees, merges, cleanup and auto-commit): C#'s `RunDetailed` byte
//!   for byte, its settings [`delegation_settings`] (with [`no_signature_programs`]), standard input written while the output is read,
//!   and why a run gave no result ([`GitFailure`]).
//! - [`Git::run`] is the work map's (mighty-workmap re-exports this module as `mighty_workmap::git`; its commands only read): the same
//!   runner with the work map's settings, unchanged ([`no_programs`]: also no transport, `GIT_NO_LAZY_FETCH`).
//!
//! Owner from W7 Step 0: W7-B.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mighty_platform::process::{ChildControl, ChildReader, ChildWriter};
use mighty_platform::{ProcessHost, SpawnRequest};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// `DelegationGit.MaximumBytes`: standard output and error together.
pub const MAXIMUM_BYTES: usize = 1_048_576;

/// `GitResult(ExitCode, Stdout, Stderr)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// `GitFailure`: why a delegation git gave no usable result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFailure {
    None,
    /// Git is missing, the hooks folder is not empty, or git could not be started.
    NotStarted,
    /// Git closed its standard input before it had read all of it.
    BrokenPipe,
    /// Git did not finish in time, and the app killed it.
    TimedOut,
    /// Git wrote more than 1 MiB, and the app killed it.
    TooMuchOutput,
    /// Reading git's output failed.
    Unreadable,
}

/// `GitRun`: one git, its result when it finished (also for [`GitFailure::BrokenPipe`], should git have exited), and why not otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRun {
    pub result: Option<GitResult>,
    pub failure: GitFailure,
}

impl GitRun {
    /// `GitRun.Failed`.
    pub fn failed(failure: GitFailure) -> GitRun {
        GitRun { result: None, failure }
    }

    /// `DelegationGit.Run` over this: the result only when nothing failed.
    pub fn ok(self) -> Option<GitResult> {
        if self.failure == GitFailure::None { self.result } else { None }
    }
}

/// `DelegationGit.Line`.
pub fn line(output: &str) -> &str {
    output.trim()
}

/// `DelegationGit.IsObjectName`: 40 or 64 hex digits.
pub fn is_object_name(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `DelegationGit.Executable`: git on the PATH (a relative PATH entry names no place, so the folder a command runs in never decides).
pub fn executable() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let name = if crate::os::windows() { "git.exe" } else { "git" };
    std::env::split_paths(&path)
        .map(|folder| PathBuf::from(folder.to_string_lossy().trim().trim_matches('"')))
        .filter(|folder| mighty_core::normalize::fully_qualified(&folder.to_string_lossy()))
        .map(|folder| folder.join(name))
        .find(|candidate| candidate.is_file())
}

/// The work map's settings that keep a repository's own config from running a program while git only reads it, after the user's and
/// the repository's settings (a `-c` wins over both): no signature is checked (`log.showSignature`), the signature programs name a path
/// under the git executable itself, which is a file, so nothing can ever be there to run (`gpg.program`, `gpg.ssh.program`,
/// `gpg.x509.program`), and no transport is allowed, so a partial clone never fetches what it lacks through a remote's command
/// (`protocol.allow`, with `GIT_NO_LAZY_FETCH`). The header's `git status` (src-tauri host.rs `read_git`) passes the same.
pub fn no_programs(git: &Path) -> Vec<String> {
    let nothing = git.join("no-program").to_string_lossy().into_owned();
    let mut settings = vec!["-c".to_string(), "log.showSignature=false".to_string()];
    for name in ["gpg.program", "gpg.ssh.program", "gpg.x509.program"] {
        settings.extend(["-c".to_string(), format!("{name}={nothing}")]);
    }
    settings.extend(["-c".to_string(), "protocol.allow=never".to_string()]);
    settings
}

/// `DelegationGit.NoSignaturePrograms`: no log shows signatures, and each signature program is the hooks folder, an absolute folder
/// (`/dev/null` off Windows) that cannot be started, so even a format that asks for a signature check (`%G?`) runs nothing. A bare
/// name such as `false`, or an empty value, would be looked up on the PATH.
pub fn no_signature_programs(hooks: &Path) -> Vec<String> {
    let hooks = hooks.to_string_lossy();
    [
        "log.showSignature=false".to_string(),
        format!("gpg.program={hooks}"),
        format!("gpg.ssh.program={hooks}"),
        format!("gpg.x509.program={hooks}"),
    ]
    .into_iter()
    .flat_map(|setting| ["-c".to_string(), setting])
    .collect()
}

/// `RunDetailed`'s settings before the command: `-c core.fsmonitor=false -c core.hooksPath=<hooks> -c gc.auto=0
/// -c maintenance.auto=false`, the
/// [`no_signature_programs`], `-c core.longpaths=true` on Windows (a child's worktree sits deeper than its parent's checkout may), then
/// `-C <directory>`.
pub fn delegation_settings(hooks: &Path, directory: &Path) -> Vec<String> {
    let mut settings: Vec<String> =
        vec!["-c".into(), "core.fsmonitor=false".into(), "-c".into(), format!("core.hooksPath={}", hooks.display())];
    // No automatic maintenance either: a detached `git maintenance` would outlive the run (W9 review M3).
    settings.extend(["-c".into(), "gc.auto=0".into(), "-c".into(), "maintenance.auto=false".into()]);
    settings.extend(no_signature_programs(hooks));
    if crate::os::windows() {
        settings.extend(["-c".into(), "core.longpaths=true".into()]);
    }
    settings.extend(["-C".into(), directory.to_string_lossy().into_owned()]);
    settings
}

/// Which argument list and environment a run takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Settings {
    WorkMap,
    Delegation,
}

/// Git for one app: the process host every child starts through, and the empty folder git is pointed at for hooks.
#[derive(Clone)]
pub struct Git {
    processes: Arc<dyn ProcessHost>,
    hooks: PathBuf,
    /// `DelegationGit.found`: git on the PATH once it has been found; looked for again at each use until then (C# `found ??=`).
    found: Arc<OnceLock<PathBuf>>,
    /// Checks only: the global config file every git reads ([`Git::with_global_config`]).
    global_config: Option<PathBuf>,
}

impl Git {
    /// `hooks`: an empty folder of the app's own under its profile (`DelegationGit.NoHooks`, `UseProfile`: `<profile>\git-no-hooks`;
    /// on Windows git reads a device name such as `NUL` as a relative path, so a real folder is named; `/dev/null` on the Mac). Checked
    /// empty before each run.
    pub fn new(processes: Arc<dyn ProcessHost>, hooks: PathBuf) -> Git {
        Git { processes, hooks, found: Arc::new(OnceLock::new()), global_config: None }
    }

    /// For checks only: every git this runs reads `file` as its global config and no system config (`GIT_CONFIG_GLOBAL`,
    /// `GIT_CONFIG_NOSYSTEM=1`, laid over after the inherited `GIT_*` variables are taken out), so a check never reads the user's own;
    /// and git never looks for a repository above the folder holding `file`, the check's own (`GIT_CEILING_DIRECTORIES` is that
    /// folder's parent), so a folder there that is no repository never climbs into one around it.
    pub fn with_global_config(mut self, file: PathBuf) -> Git {
        self.global_config = Some(file);
        self
    }

    /// Whether git was found.
    pub fn available(&self) -> bool {
        self.program().is_some()
    }

    /// The git executable on the PATH, looked for until it is found once.
    pub fn program(&self) -> Option<&Path> {
        if self.found.get().is_none()
            && let Some(found) = executable()
        {
            let _ = self.found.set(found);
        }
        self.found.get().map(PathBuf::as_path)
    }

    /// The hooks folder (`DelegationGit.NoHooks`).
    pub fn hooks(&self) -> &Path {
        &self.hooks
    }

    /// `HooksEmpty`: the hooks folder is a plain, empty folder (no link, junction or other reparse point), made when it is missing.
    /// Git does not run while it is not.
    fn hooks_empty(&self) -> bool {
        if !crate::os::windows() && self.hooks == Path::new("/dev/null") {
            return true;
        }
        if std::fs::create_dir_all(&self.hooks).is_err() {
            return false;
        }
        let plain = std::fs::symlink_metadata(&self.hooks).is_ok_and(|m| m.is_dir() && !reparse_point(&m));
        plain && std::fs::read_dir(&self.hooks).is_ok_and(|mut entries| entries.next().is_none())
    }

    /// The work map's run: None when git is missing, could not start, wrote more than 1 MiB, did not finish in time, or the hooks
    /// folder is not empty.
    pub async fn run(&self, arguments: &[&str], directory: &Path, timeout: f64) -> Option<GitResult> {
        Box::pin(self.execute(Settings::WorkMap, arguments, directory, timeout, None)).await.ok()
    }

    /// The work map's run with more variables laid over the cleaned environment (the snapshots' `GIT_INDEX_FILE`,
    /// `GIT_OBJECT_DIRECTORY` and `GIT_ALTERNATE_OBJECT_DIRECTORIES`, checkpoints.rs).
    pub async fn run_with(&self, arguments: &[&str], directory: &Path, timeout: f64, variables: &[(&str, String)]) -> Option<GitResult> {
        Box::pin(self.execute_with(Settings::WorkMap, arguments, directory, timeout, None, variables)).await.ok()
    }

    /// `DelegationGit.RunDetailed`: runs git with delegation's settings and says why it gave no result. `input` is written to git's
    /// standard input while its output is read, within the same deadline, and then closed; it is for a git that reads its standard
    /// input to its end (`--pathspec-from-file=-`, `--stdin`): one that exits first is [`GitFailure::BrokenPipe`]. `timeout` is in
    /// seconds (C#'s default is 10). Dropping the future before it ends kills git with everything it started.
    pub async fn run_detailed(&self, arguments: &[&str], directory: &Path, timeout: f64, input: Option<&str>) -> GitRun {
        // Boxed: the run holds both outputs' reads and its input at once, and every step of a merge or a cleanup awaits one.
        Box::pin(self.execute(Settings::Delegation, arguments, directory, timeout, input)).await
    }

    async fn execute(&self, settings: Settings, arguments: &[&str], directory: &Path, timeout: f64, input: Option<&str>) -> GitRun {
        self.execute_with(settings, arguments, directory, timeout, input, &[]).await
    }

    async fn execute_with(
        &self,
        settings: Settings,
        arguments: &[&str],
        directory: &Path,
        timeout: f64,
        input: Option<&str>,
        variables: &[(&str, String)],
    ) -> GitRun {
        let Some(program) = self.program().map(Path::to_path_buf) else { return GitRun::failed(GitFailure::NotStarted) };
        if !self.hooks_empty() {
            return GitRun::failed(GitFailure::NotStarted);
        }
        let mut args = match settings {
            Settings::Delegation => delegation_settings(&self.hooks, directory),
            Settings::WorkMap => {
                let hooks = format!("core.hooksPath={}", self.hooks.display());
                let mut args: Vec<String> =
                    ["-c", "core.fsmonitor=false", "-c", &hooks, "-c", "gc.auto=0"].iter().map(|s| s.to_string()).collect();
                if crate::os::windows() {
                    args.extend(["-c".to_string(), "core.longpaths=true".to_string()]);
                }
                args.extend(no_programs(&program));
                args.extend(["-C".to_string(), directory.to_string_lossy().into_owned()]);
                args
            }
        };
        args.extend(arguments.iter().map(|a| a.to_string()));
        // ChildProcess.StartInfo: NO_COLOR=1 (SpawnRequest::new sets it too).
        let mut request = SpawnRequest::new(program, args, directory).env("NO_COLOR", "1");
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().into_owned();
            if key.to_ascii_uppercase().starts_with("GIT_") {
                request = request.without(key);
            }
        }
        request = request.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C");
        if settings == Settings::WorkMap {
            request = request.env("GIT_NO_LAZY_FETCH", "1");
        }
        for (key, value) in variables {
            request = request.env(*key, value.as_str());
        }
        if let Some(file) = &self.global_config {
            request = request.env("GIT_CONFIG_GLOBAL", file.to_string_lossy()).env("GIT_CONFIG_NOSYSTEM", "1");
            if let Some(ceiling) = file.parent().and_then(Path::parent) {
                request = request.env("GIT_CEILING_DIRECTORIES", ceiling.to_string_lossy());
            }
        }
        let Ok(mut child) = self.processes.spawn(request).await else { return GitRun::failed(GitFailure::NotStarted) };
        let control = child.control();
        // A caller that drops this future before it ends still ends git and everything it started.
        let guard = CloseOnDrop(Some(control.clone()));
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let total = Arc::new(AtomicUsize::new(0));
        let over = Arc::new(AtomicBool::new(false));
        let broken_pipe = Arc::new(AtomicBool::new(false));
        let input = input.map(|text| text.as_bytes().to_vec());
        let waiting = control.clone();
        let (out_over, err_over, pipe) = (over.clone(), over.clone(), broken_pipe.clone());
        let (out_control, err_control) = (control.clone(), control.clone());
        // A process git started can keep its output open after git exits: the deadline ends the whole job, so the reads end too.
        let outcome = tokio::time::timeout(Duration::from_secs_f64(timeout.max(0.01)), async move {
            // The input goes in while the output is read, so a git that writes before it has read everything never stalls on a full
            // pipe.
            let ((), out, err) = tokio::join!(
                write(stdin, input, pipe),
                read(stdout, total.clone(), out_over, out_control),
                read(stderr, total, err_over, err_control)
            );
            let exit = waiting.wait().await;
            (exit, out, err)
        })
        .await;
        // Ending the job here is the app's own clean-up, not a timeout.
        guard.close().await;
        let Ok((exit, out, err)) = outcome else { return GitRun::failed(GitFailure::TimedOut) };
        if over.load(Ordering::SeqCst) {
            return GitRun::failed(GitFailure::TooMuchOutput);
        }
        let (Ok(exit), Some(out), Some(err)) = (exit, out, err) else { return GitRun::failed(GitFailure::Unreadable) };
        let result = GitResult {
            exit_code: exit,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        };
        let failure = if broken_pipe.load(Ordering::SeqCst) { GitFailure::BrokenPipe } else { GitFailure::None };
        GitRun { result: Some(result), failure }
    }
}

/// Kills the child and closes it in a task of its own when dropped armed (a run's future dropped before it ended); [`CloseOnDrop::close`]
/// does it in place on the way out.
struct CloseOnDrop(Option<Arc<dyn ChildControl>>);

impl CloseOnDrop {
    async fn close(mut self) {
        if let Some(control) = self.0.take() {
            control.kill();
            let _ = control.close().await;
        }
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some(control) = self.0.take() {
            control.kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = control.close().await;
                });
            }
        }
    }
}

/// `FileAttributes.ReparsePoint` of what `symlink_metadata` read (a link elsewhere).
fn reparse_point(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Writes `input` (when there is one) and closes git's standard input; a pipe git closed first marks `broken_pipe`.
async fn write(stdin: Option<ChildWriter>, input: Option<Vec<u8>>, broken_pipe: Arc<AtomicBool>) {
    let Some(mut pipe) = stdin else { return };
    let given = input.is_some();
    // A Windows pipe takes a write whole and says only at the next write that git never read it: the empty write waits for the one
    // before to end and fails as it did; on the Mac an empty write to a pipe git has closed fails too. Git has not seen the end of its
    // input yet (that comes with the shutdown below), so a git that reads to the end is still reading here.
    if let Some(input) = input
        && (pipe.write_all(&input).await.is_err() || pipe.flush().await.is_err() || pipe.write(&[]).await.is_err())
    {
        broken_pipe.store(true, Ordering::SeqCst);
    }
    if pipe.shutdown().await.is_err() && given {
        broken_pipe.store(true, Ordering::SeqCst);
    }
}

/// Reads one output to its end; past [`MAXIMUM_BYTES`] for both together it marks `over`, kills git and stops. None when the read
/// failed.
async fn read(
    reader: Option<ChildReader>,
    total: Arc<AtomicUsize>,
    over: Arc<AtomicBool>,
    control: Arc<dyn ChildControl>,
) -> Option<Vec<u8>> {
    let mut kept = vec![];
    let Some(mut reader) = reader else { return Some(kept) };
    let mut buffer = vec![0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await.ok()?;
        if read == 0 {
            return Some(kept);
        }
        if total.fetch_add(read, Ordering::SeqCst) + read > MAXIMUM_BYTES {
            over.store(true, Ordering::SeqCst);
            control.kill();
            return Some(kept);
        }
        kept.extend_from_slice(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RunDetailed`'s settings, byte for byte as the fix branch's DelegationWorktree.cs:256 lists them, with automatic maintenance off
    /// (W9 review M3).
    #[test]
    fn delegations_settings_are_the_hardened_runners() {
        let hooks = Path::new("H");
        let mut expected: Vec<String> = [
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=H",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "log.showSignature=false",
            "-c",
            "gpg.program=H",
            "-c",
            "gpg.ssh.program=H",
            "-c",
            "gpg.x509.program=H",
        ]
        .map(String::from)
        .to_vec();
        if crate::os::windows() {
            expected.extend(["-c".to_string(), "core.longpaths=true".to_string()]);
        }
        expected.extend(["-C".to_string(), "D".to_string()]);
        assert_eq!(delegation_settings(hooks, Path::new("D")), expected);
        assert!(
            is_object_name(&"a".repeat(40))
                && is_object_name(&"F".repeat(64))
                && !is_object_name(&"a".repeat(39))
                && !is_object_name("HEAD")
        );
        let run = GitRun {
            result: Some(GitResult { exit_code: 0, stdout: String::new(), stderr: String::new() }),
            failure: GitFailure::BrokenPipe,
        };
        assert!(run.ok().is_none() && GitRun::failed(GitFailure::TimedOut).result.is_none());
    }
}

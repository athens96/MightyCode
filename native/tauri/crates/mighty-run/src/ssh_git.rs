//! MC-47: git and the file system of an SSH workspace's folder, over the machine's ssh (its shared connection where there is one):
//! what a pane worktree and the work map ask of a folder on the far side. Every command is one of mighty-core `ssh::worktree`'s
//! scripts under `/bin/sh -c` ([`sh`]), started as a batch-mode `ssh -T` with a connect timeout ([`SshLaunch::exec_request`]); its
//! input goes through ssh's standard input, never the command line.
//!
//! A remote git answers as [`crate::git::Git::run_detailed`] does ([`GitRun`]): its exit code and output, or why there is none (the
//! folder gone, no git there, ssh failing: [`GitFailure::NotStarted`]; past its deadline: [`GitFailure::TimedOut`], and the far side
//! ends it too; more than 1 MiB: [`GitFailure::TooMuchOutput`]).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mighty_core::ssh::command::{SSH_FAILED, sh};
use mighty_core::ssh::worktree::{self as scripts, GitKind, Info};
use mighty_platform::{PlatformError, ProcessHost};

use crate::git::{GitFailure, GitResult, GitRun};
use crate::ssh::SshLaunch;

/// What reaching the far side adds to a git's own deadline: the connection (its connect timeout and a sign-in).
pub const CONNECTION_SLACK: Duration = Duration::from_secs(20);

/// A remote command's end: its exit code and both outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// One SSH workspace's machine, as its folders' git and file system are reached.
#[derive(Clone)]
pub struct SshFolder {
    processes: Arc<dyn ProcessHost>,
    launch: SshLaunch,
}

impl SshFolder {
    pub fn new(processes: Arc<dyn ProcessHost>, launch: SshLaunch) -> SshFolder {
        SshFolder { processes, launch }
    }

    pub fn launch(&self) -> &SshLaunch {
        &self.launch
    }

    /// `script` run on the far side with `input` on its standard input, within `limit`, each output at most `cap` bytes. Err with
    /// ssh's words when the machine could not be reached, `timed out` past the limit.
    pub async fn exec(&self, script: &str, input: Option<Vec<u8>>, limit: Duration, cap: usize) -> Result<Ended, String> {
        let request = self.launch.exec_request(sh(script), &BTreeMap::new());
        match mighty_platform::process::capture(self.processes.as_ref(), request, input, limit, cap).await {
            Ok(captured) if captured.exit_code == SSH_FAILED => Err(String::from_utf8_lossy(&captured.stderr).trim().to_string()),
            Ok(captured) => Ok(Ended { code: captured.exit_code, stdout: captured.stdout, stderr: captured.stderr }),
            Err(PlatformError::TimedOut(_)) => Err("timed out".into()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The far side's home (its real path) and whether it has git.
    pub async fn info(&self) -> Result<Info, String> {
        let ended = self.exec(&scripts::info_script(), None, Duration::from_secs(30), 64 * 1024).await?;
        if ended.code != 0 {
            return Err(format!("the machine answered {}", ended.code));
        }
        scripts::parse_info(&String::from_utf8_lossy(&ended.stdout)).ok_or_else(|| "the machine's answer could not be read".into())
    }

    /// One git in `dir` on the far side with `kind`'s settings (mighty-core `ssh::worktree::git_script`), `input` on its standard
    /// input; `timeout` in seconds is git's own deadline (the far side ends it then), the connection gets [`CONNECTION_SLACK`] more.
    pub async fn git(&self, kind: GitKind, arguments: &[&str], dir: &str, timeout: f64, input: Option<&str>) -> GitRun {
        let seconds = timeout.max(1.0).ceil() as u64;
        let Some(script) = scripts::git_script(dir, kind, arguments, seconds) else { return GitRun::failed(GitFailure::NotStarted) };
        let limit = Duration::from_secs(seconds) + CONNECTION_SLACK;
        let input = input.map(|text| text.as_bytes().to_vec());
        let request = self.launch.exec_request(sh(&script), &BTreeMap::new());
        let captured =
            match mighty_platform::process::capture(self.processes.as_ref(), request, input, limit, scripts::MAXIMUM_GIT_BYTES).await {
                Ok(captured) => captured,
                Err(PlatformError::TimedOut(_)) => return GitRun::failed(GitFailure::TimedOut),
                Err(PlatformError::TooLarge(_)) => return GitRun::failed(GitFailure::TooMuchOutput),
                Err(_) => return GitRun::failed(GitFailure::NotStarted),
            };
        match captured.exit_code {
            SSH_FAILED | scripts::FOLDER_GONE | scripts::NO_GIT => return GitRun::failed(GitFailure::NotStarted),
            scripts::TIMED_OUT => return GitRun::failed(GitFailure::TimedOut),
            _ => {}
        }
        if captured.stdout.len() + captured.stderr.len() > scripts::MAXIMUM_GIT_BYTES {
            return GitRun::failed(GitFailure::TooMuchOutput);
        }
        let result = GitResult {
            exit_code: captured.exit_code,
            stdout: String::from_utf8_lossy(&captured.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&captured.stderr).into_owned(),
        };
        GitRun { result: Some(result), failure: GitFailure::None }
    }
}

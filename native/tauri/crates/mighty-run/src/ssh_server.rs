//! MC-33: a Mighty Code server on an SSH machine, carried out over the machine's ssh (mighty-core's `ssh::server` writes the scripts,
//! this runs them). Every step is one `ssh -o BatchMode=yes -T … -- <host> '/bin/sh -c …'` over the machine's shared connection where
//! there is one, with its own time limit; ssh's own failures (exit 255) are told apart from a step's refusal (the script's exit code).
//! The package goes to the far side on ssh's standard input, read from the local file a piece at a time; the pairing code comes back
//! on standard output and is never logged or kept here.
//!
//! The install's steps in order ([`RemoteServer::install`]): upload (the far side's SHA-256 of what arrived compared with the signed
//! manifest's), unpack and verify (`ditto`, one `.app`, its identifier, its version, `codesign --verify --deep --strict` against the
//! requirement), stop the running server with its own CLI, swap it into `~/Applications/MightyCode.app`, start it without a window.
//! A step that fails removes the install folder; nothing is ever run with `sudo`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mighty_core::ssh::command::SSH_FAILED;
use mighty_core::ssh::failure::{self, Failure};
use mighty_core::ssh::server::{self as scripts, AppPlace, Detection, Pairing, ServerStatus, UnpackRefusal};
use mighty_platform::{PlatformError, ProcessHost};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::ssh::SshLaunch;

/// The detection's limit (the far side's `mighty server status` waits up to 3 s for its app).
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The upload's limit (a package of about 100 MB over a slow link).
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Unpacking and verifying the signature of the whole bundle.
pub const UNPACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// `mighty server stop` waits up to 60 s for the app to end.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(90);
pub const SWAP_TIMEOUT: Duration = Duration::from_secs(60);
/// `mighty server start` waits up to 60 s for the app's control API.
pub const START_TIMEOUT: Duration = Duration::from_secs(90);
pub const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_CAP: usize = 256 * 1024;
const CHUNK: usize = 256 * 1024;

/// A step of the install, for its progress and its failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Upload,
    Unpack,
    Stop,
    Swap,
    Start,
}

/// What the install is doing now: the upload with its fraction, or the step begun.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    Upload(f64),
    Step(Step),
}

/// Why a server step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    /// ssh itself: not reachable, sign-in refused, a host key, no ssh, out of time.
    Ssh(Failure),
    /// The package on the far side is not the one sent.
    Digest,
    /// The unpacked app was refused.
    Unpack(UnpackRefusal),
    /// The running server did not stop (its CLI's last line).
    Stop(String),
    /// The new app could not be put in place (the old one is back where it could be).
    Swap,
    /// The server did not start or never answered (its CLI's last line): often no one is logged in at the Mac's screen.
    Start(String),
    /// The far side's app gave no pairing (its CLI's last line).
    Pair(String),
    /// The removal's stop failed; nothing was deleted.
    Remove(String),
    /// Something here: the package could not be read, a step's output did not read.
    Local(String),
}

/// One machine's server, over its ssh.
pub struct RemoteServer {
    pub processes: Arc<dyn ProcessHost>,
    pub launch: SshLaunch,
}

/// The install's inputs: the verified package here, the version it holds, the code-signature requirement and a fresh folder id.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub package: PathBuf,
    /// The signed manifest's SHA-256 of the package (lower-case hex).
    pub sha256: String,
    pub version: String,
    pub requirement: String,
    /// [`scripts::valid_install_id`].
    pub id: String,
    /// Whether a server runs now (stopped before the swap).
    pub stop_first: bool,
}

/// ssh's stderr's last line, as said after a step's own words (no command line ever reaches it: ssh does not echo it).
fn last_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().chars().take(200).collect()
}

/// A CLI's error message from its JSON answer (`{"ok": false, "error": {"message"}}`), else its stderr's last line.
fn cli_words(stdout: &[u8], stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    let from_json = text
        .find('{')
        .and_then(|at| serde_json::from_str::<serde_json::Value>(text[at..].trim()).ok())
        .and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(|m| m.chars().take(200).collect::<String>()));
    from_json.unwrap_or_else(|| last_line(stderr))
}

/// A finished step: its exit code, its standard output and error.
struct Output {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl RemoteServer {
    pub fn new(processes: Arc<dyn ProcessHost>, launch: SshLaunch) -> RemoteServer {
        RemoteServer { processes, launch }
    }

    async fn run(&self, script: &str, timeout: Duration) -> Result<Output, ServerError> {
        let request = self.launch.exec_request(scripts::sh(script), &BTreeMap::new());
        match mighty_platform::process::capture(self.processes.as_ref(), request, None, timeout, OUTPUT_CAP).await {
            Ok(captured) if captured.exit_code == SSH_FAILED => {
                Err(ServerError::Ssh(failure::classify(&String::from_utf8_lossy(&captured.stderr))))
            }
            Ok(captured) => Ok(Output { code: captured.exit_code, stdout: captured.stdout, stderr: captured.stderr }),
            Err(PlatformError::TimedOut(_)) => Err(ServerError::Ssh(Failure::TimedOut)),
            Err(PlatformError::TooLarge(_)) => Err(ServerError::Local("the far side's answer is too long".into())),
            Err(_) => Err(ServerError::Ssh(Failure::NoSsh)),
        }
    }

    /// What the machine is and has: [`Detection`].
    pub async fn detect(&self) -> Result<Detection, ServerError> {
        let output = self.run(&scripts::detect_script(), DETECT_TIMEOUT).await?;
        scripts::parse_detection(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| ServerError::Ssh(Failure::Other(last_line(&output.stderr))))
    }

    /// The package sent on ssh's standard input into a fresh install folder, `progress` told the fraction sent; the far side's SHA-256
    /// of what arrived.
    async fn upload(&self, plan: &InstallPlan, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<String, ServerError> {
        let size = std::fs::metadata(&plan.package).map_err(|e| ServerError::Local(e.to_string()))?.len();
        let mut file = tokio::fs::File::open(&plan.package).await.map_err(|e| ServerError::Local(e.to_string()))?;
        let request = self.launch.exec_request(scripts::sh(&scripts::upload_script(&plan.id)), &BTreeMap::new());
        let mut child = self.processes.spawn(request).await.map_err(|_| ServerError::Ssh(Failure::NoSsh))?;
        let control = child.control();
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let send = async move {
            let Some(mut pipe) = stdin else { return Ok::<(), std::io::Error>(()) };
            let mut buffer = vec![0u8; CHUNK];
            let mut sent: u64 = 0;
            let mut told = -1i64;
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    break;
                }
                // A far side that stopped reading (its step failed) ends the sending; its exit code says why.
                if pipe.write_all(&buffer[..read]).await.is_err() {
                    return Ok(());
                }
                sent += read as u64;
                let fraction = if size == 0 { 1.0 } else { (sent as f64 / size as f64).min(1.0) };
                let percent = (fraction * 100.0) as i64;
                if percent != told {
                    told = percent;
                    progress(Progress::Upload(fraction));
                }
            }
            let _ = pipe.shutdown().await;
            Ok(())
        };
        let read = |reader: Option<mighty_platform::process::ChildReader>| async move {
            let mut out = Vec::new();
            if let Some(mut reader) = reader {
                let mut buffer = vec![0u8; 8192];
                loop {
                    let n = reader.read(&mut buffer).await?;
                    if n == 0 {
                        break;
                    }
                    if out.len() < OUTPUT_CAP {
                        out.extend_from_slice(&buffer[..n]);
                    }
                }
            }
            Ok::<Vec<u8>, std::io::Error>(out)
        };
        let work = async {
            let (sent, out, err) = tokio::join!(send, read(stdout), read(stderr));
            let code = control.wait().await.map_err(|_| ServerError::Ssh(Failure::NoSsh))?;
            sent.map_err(|e| ServerError::Local(e.to_string()))?;
            Ok::<Output, ServerError>(Output { code, stdout: out.unwrap_or_default(), stderr: err.unwrap_or_default() })
        };
        let outcome = match tokio::time::timeout(UPLOAD_TIMEOUT, work).await {
            Ok(outcome) => outcome,
            Err(_) => Err(ServerError::Ssh(Failure::TimedOut)),
        };
        if outcome.is_err() {
            control.kill();
        }
        let _ = control.close().await;
        let output = outcome?;
        if output.code == SSH_FAILED {
            return Err(ServerError::Ssh(failure::classify(&String::from_utf8_lossy(&output.stderr))));
        }
        if output.code != 0 {
            return Err(ServerError::Local(format!("upload: {} ({})", last_line(&output.stderr), output.code)));
        }
        scripts::parse_sha256(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| ServerError::Local("upload: no digest".into()))
    }

    /// The install folder removed after a failed step (best effort).
    async fn clean(&self, id: &str) {
        let _ = self.run(&scripts::cleanup_script(id), SWAP_TIMEOUT).await;
    }

    /// The whole install or update (see the module's notes), each step told to `progress` as it begins; the started server's status.
    pub async fn install(&self, plan: &InstallPlan, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<ServerStatus, ServerError> {
        if !scripts::valid_install_id(&plan.id) {
            return Err(ServerError::Local("install id".into()));
        }
        progress(Progress::Step(Step::Upload));
        let digest = match self.upload(plan, progress).await {
            Ok(digest) => digest,
            Err(error) => {
                self.clean(&plan.id).await;
                return Err(error);
            }
        };
        if !digest.eq_ignore_ascii_case(&plan.sha256) {
            self.clean(&plan.id).await;
            return Err(ServerError::Digest);
        }
        progress(Progress::Step(Step::Unpack));
        let unpacked = self.run(&scripts::unpack_script(&plan.id, &plan.version, &plan.requirement), UNPACK_TIMEOUT).await;
        match unpacked {
            Ok(output) if output.code == 0 => {}
            Ok(output) => {
                self.clean(&plan.id).await;
                return Err(match UnpackRefusal::from_code(output.code) {
                    Some(refusal) => ServerError::Unpack(refusal),
                    None => ServerError::Local(format!("unpack: {} ({})", last_line(&output.stderr), output.code)),
                });
            }
            Err(error) => {
                self.clean(&plan.id).await;
                return Err(error);
            }
        }
        if plan.stop_first {
            progress(Progress::Step(Step::Stop));
            if let Err(error) = self.stop().await {
                self.clean(&plan.id).await;
                return Err(error);
            }
        }
        progress(Progress::Step(Step::Swap));
        match self.run(&scripts::swap_script(&plan.id), SWAP_TIMEOUT).await {
            Ok(output) if output.code == 0 => {}
            Ok(_) => {
                self.clean(&plan.id).await;
                return Err(ServerError::Swap);
            }
            Err(error) => {
                self.clean(&plan.id).await;
                return Err(error);
            }
        }
        progress(Progress::Step(Step::Start));
        self.start(AppPlace::Home).await
    }

    /// The server started (`mighty server start`); its status once its control API answered.
    pub async fn start(&self, app: AppPlace) -> Result<ServerStatus, ServerError> {
        let output = self.run(&scripts::start_script(app), START_TIMEOUT).await?;
        let status = scripts::parse_status(&String::from_utf8_lossy(&output.stdout));
        if output.code == 0 && status.running { Ok(status) } else { Err(ServerError::Start(cli_words(&output.stdout, &output.stderr))) }
    }

    /// The running server stopped with an installed app's CLI (nothing running is no error).
    pub async fn stop(&self) -> Result<(), ServerError> {
        let output = self.run(&scripts::stop_script(), STOP_TIMEOUT).await?;
        if output.code == 0 { Ok(()) } else { Err(ServerError::Stop(cli_words(&output.stdout, &output.stderr))) }
    }

    /// A one-time pairing code from the far side's running app (its loopback listener started when it is not).
    pub async fn pair(&self, app: AppPlace) -> Result<Pairing, ServerError> {
        let output = self.run(&scripts::pair_script(app), PAIR_TIMEOUT).await?;
        if output.code != 0 {
            return Err(ServerError::Pair(cli_words(&output.stdout, &output.stderr)));
        }
        scripts::parse_pairing(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| ServerError::Pair(String::new()))
    }

    /// The server stopped and `~/Applications/MightyCode.app` deleted.
    pub async fn remove(&self) -> Result<(), ServerError> {
        let output = self.run(&scripts::remove_script(), STOP_TIMEOUT).await?;
        if output.code == 0 { Ok(()) } else { Err(ServerError::Remove(last_line(&output.stderr))) }
    }
}

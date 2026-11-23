//! MC-40: an SSH workspace's agent run that outlives a dropped connection. The CLI runs detached on the far side (mighty-core
//! `ssh::held`: a holder, a FIFO for its input, files for its output); what the run engine gets here is a [`Child`] whose pipes are
//! this module's: its output is read through a chain of **attaches** (batch-mode ssh over the machine's shared connection), each from
//! the byte where the last stopped, and its input goes to the attach of the moment, the bytes an attach did not deliver sent again by
//! the next one. The run engine never sees a connection go: a waiting approval card stays waiting, its answer is written once a
//! connection is back.
//!
//! - **Lost**: an attach that ends without the CLI having ended (ssh's 255, or the far side saying its input or output went) starts
//!   reconnecting after 2, 5, 15, 30, 60 and 120 s, then every 120 s; past [`GIVE_UP`] without a connection the run ends here as an
//!   error (the CLI goes on over there until it ends; its folder is removed by a later start after a week).
//! - **Ended**: the attach ends with 0, a drain reads the last bytes, the exit code and the end of standard error; the folder goes.
//! - **Stopped** (`kill`, `close`): the far side's `TERM`, then `KILL` (the exit code written there), then drained as ended.
//! - **Input closed** by the run engine: the far side marks it and ends the FIFO's keeper, so the CLI reads its end.
//! - **Kept** (MC-38): each attach is a keyed spawn, so with the background daemon on the daemon owns the attach's ssh; when the app
//!   quits keeping its work, letting go is the attach's ([`ChildControl::let_go`], [`ChildControl::leave`] go to it) and a later app
//!   takes the run up through that attach.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::ssh::held;
use mighty_platform::process::ChildWriter;
use mighty_platform::{BoxFuture, Child, ChildControl, PlatformError, ProcessHost, Result};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio::sync::{Notify, watch};

use crate::ssh::SshLaunch;

/// The gaps between reconnects; the last one repeats.
const GAPS: [u64; 6] = [2, 5, 15, 30, 60, 120];
/// How long a run waits for its machine before it ends here as an error.
pub const GIVE_UP: Duration = Duration::from_secs(30 * 60);
/// How long a drain and a stop may take.
const SHORT: Duration = Duration::from_secs(30);
/// How long the start may take (its first snapshot reads the whole folder once).
const START: Duration = Duration::from_secs(120);
/// How long an attach may take to say how much input arrived (a new sign-in included).
const HEADER: Duration = Duration::from_secs(60);
/// The most input kept to send again (an agent's input is prompts and answers; past this the oldest is forgotten).
const MAXIMUM_SENT: usize = 8 * 1024 * 1024;
/// The exit code of a run that ended here because its machine could not be reached (ssh's own).
pub const LOST: i32 = 255;

/// What starting a held run said.
#[derive(Debug)]
pub enum Refused {
    /// The far side would not start it (no folder could be made, the holder did not start): the caller runs it directly instead.
    Unsupported(String),
    /// The machine could not be reached or refused the sign-in.
    Ssh(String),
}

/// The input the attaches carry: everything the run engine wrote (to send again what an attach did not deliver), the writer of the
/// attach of the moment, and whether the engine closed it.
#[derive(Default)]
struct Input {
    sent: Vec<u8>,
    /// How many bytes were forgotten from the start of `sent`.
    base: u64,
    writer: Option<ChildWriter>,
    closed: bool,
    /// The far side was told the input is closed (only once every byte went).
    close_sent: bool,
}

struct Shared {
    processes: Arc<dyn ProcessHost>,
    launch: SshLaunch,
    id: String,
    key: String,
    exit: watch::Sender<Option<i32>>,
    stopping: AtomicBool,
    stop: Notify,
    current: Mutex<Option<Arc<dyn ChildControl>>>,
    input: tokio::sync::Mutex<Input>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    async fn exec(&self, script: Option<String>, limit: Duration) -> Option<mighty_platform::process::Captured> {
        let request = self.launch.exec_request(script?, &BTreeMap::new());
        mighty_platform::process::capture(self.processes.as_ref(), request, None, limit, 4 * 1024 * 1024).await.ok()
    }

    fn current(&self) -> Option<Arc<dyn ChildControl>> {
        lock(&self.current).clone()
    }
}

/// Starts `script` (mighty-core `command::run_script`) as a held run on `launch`'s machine; `key` is the run's keyed-spawn key (its
/// attaches go through [`ProcessHost::spawn_keyed`]).
pub async fn spawn(
    processes: Arc<dyn ProcessHost>,
    launch: SshLaunch,
    script: &str,
    key: &str,
    snapshots: Option<(&str, &str)>,
    attachments: Option<&str>,
) -> std::result::Result<Child, Refused> {
    let id = mighty_core::wire::new_id().replace('-', "").to_ascii_lowercase();
    // MC-40: the request's snapshots (its pane and request), for the review of what it changed.
    let snapshots = snapshots.map(|(pane, run)| held::Snapshots { dir: launch.dir.clone(), pane: pane.to_string(), run: run.to_string() });
    let start = held::start_script_with(&id, script, snapshots.as_ref(), attachments).ok_or_else(|| Refused::Unsupported("id".into()))?;
    let request = launch.exec_request(start, &BTreeMap::new());
    let started = mighty_platform::process::capture(processes.as_ref(), request, None, START, 64 * 1024)
        .await
        .map_err(|e| Refused::Ssh(e.to_string()))?;
    let stderr = String::from_utf8_lossy(&started.stderr).trim().to_string();
    match started.exit_code {
        0 if started.stdout.ends_with(b"started") => {}
        mighty_core::ssh::command::SSH_FAILED => return Err(Refused::Ssh(stderr)),
        code => return Err(Refused::Unsupported(format!("{code}: {stderr}"))),
    }
    let (exit, _) = watch::channel(None);
    let shared = Arc::new(Shared {
        processes,
        launch,
        id,
        key: key.to_string(),
        exit,
        stopping: AtomicBool::new(false),
        stop: Notify::new(),
        current: Mutex::new(None),
        input: tokio::sync::Mutex::new(Input::default()),
    });
    let (engine_in, input) = tokio::io::duplex(64 * 1024);
    let (output, engine_out) = tokio::io::duplex(256 * 1024);
    let (errors, engine_err) = tokio::io::duplex(128 * 1024);
    tokio::spawn(forward_input(shared.clone(), input));
    tokio::spawn(pump(shared.clone(), output, errors));
    Ok(Child::new(0, Box::new(engine_in), Box::new(engine_out), Box::new(engine_err), Arc::new(HeldControl { shared })))
}

/// The run engine's input to the attach of the moment (kept to be sent again); its end closes the run's input over there.
async fn forward_input(shared: Arc<Shared>, mut input: DuplexStream) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let read = input.read(&mut buffer).await.unwrap_or(0);
        let mut state = shared.input.lock().await;
        if read == 0 {
            state.closed = true;
            let writer = state.writer.take();
            drop(state);
            // With an attach to write to: marked over there first, so the attach reads the end of its input as a close, not a lost
            // connection. Without one, the next attach closes it once it sent what did not arrive.
            if let Some(mut writer) = writer {
                let told = shared.exec(held::close_input_script(&shared.id), SHORT).await.is_some_and(|c| c.exit_code == 0);
                shared.input.lock().await.close_sent = told;
                let _ = writer.shutdown().await;
            }
            return;
        }
        state.sent.extend_from_slice(&buffer[..read]);
        if state.sent.len() > MAXIMUM_SENT {
            let over = state.sent.len() - MAXIMUM_SENT;
            state.sent.drain(..over);
            state.base += over as u64;
        }
        if let Some(writer) = state.writer.as_mut()
            && (writer.write_all(&buffer[..read]).await.is_err() || writer.flush().await.is_err())
        {
            state.writer = None;
        }
    }
}

/// How one attach ended.
enum Attach {
    /// The CLI ended (0): drain.
    Ended,
    /// The run's folder is gone (3).
    Gone,
    /// The connection went (anything else).
    Lost,
    /// The app let go of the run (the daemon keeps the attach).
    LetGo(i32),
}

/// The attaches, one after another, until the CLI ended, was stopped, the machine stayed away past [`GIVE_UP`] or the app let go.
async fn pump(shared: Arc<Shared>, mut output: DuplexStream, mut errors: DuplexStream) {
    let mut offset: u64 = 0;
    let mut lost: Option<(Instant, usize)> = None;
    let code = loop {
        let ended = attach(&shared, &mut output, &mut offset, &mut lost).await;
        let ended = match ended {
            Attach::LetGo(code) => break code,
            Attach::Gone => {
                let _ = errors.write_all(format!("{}\n", crate::strings::get("ssh.held.error.gone")).as_bytes()).await;
                break LOST;
            }
            Attach::Ended => true,
            Attach::Lost => false,
        };
        // Ended, stopped, or a quiet end: read what is left and the exit code (a run still going is attached again).
        if ended || shared.stopping.load(Ordering::SeqCst) {
            match shared.exec(held::drain_script(&shared.id, offset), SHORT).await {
                Some(captured) if captured.exit_code == 0 => {
                    if let Some((code, rest)) = held::parse_drain(captured.stdout) {
                        let _ = output.write_all(&rest).await;
                        let _ = errors.write_all(&captured.stderr).await;
                        let _ = shared.exec(held::cleanup_script(&shared.id), SHORT).await;
                        break code;
                    }
                    if ended {
                        continue;
                    }
                }
                Some(captured) if captured.exit_code == 3 => {
                    let _ = errors.write_all(format!("{}\n", crate::strings::get("ssh.held.error.gone")).as_bytes()).await;
                    break LOST;
                }
                _ => {}
            }
        }
        // The connection went: wait the next gap (a stop cuts it short), unless it has been away too long.
        let (since, step) = lost.get_or_insert((Instant::now(), 0));
        if since.elapsed() >= GIVE_UP {
            let _ = errors.write_all(format!("{}\n", crate::strings::get("ssh.held.error.lost")).as_bytes()).await;
            break LOST;
        }
        let gap = Duration::from_secs(GAPS[(*step).min(GAPS.len() - 1)]);
        *step += 1;
        if !shared.stopping.load(Ordering::SeqCst) {
            tokio::select! {
                _ = tokio::time::sleep(gap) => {}
                _ = shared.stop.notified() => {}
            }
        }
    };
    shared.exit.send_replace(Some(code));
    let _ = output.shutdown().await;
    let _ = errors.shutdown().await;
}

/// One attach from `offset`: its header read, the input it did not get sent again, its output copied on.
async fn attach(shared: &Arc<Shared>, output: &mut DuplexStream, offset: &mut u64, lost: &mut Option<(Instant, usize)>) -> Attach {
    let Some(script) = held::attach_script(&shared.id, *offset) else { return Attach::Gone };
    let request = shared.launch.exec_request(script, &BTreeMap::new());
    let Ok(mut child) = shared.processes.spawn_keyed(request, &shared.key).await else { return Attach::Lost };
    let control = child.control();
    *lock(&shared.current) = Some(control.clone());
    let (Some(mut stdout), stdin, stderr) = (child.stdout.take(), child.stdin.take(), child.stderr.take()) else {
        control.kill();
        return Attach::Lost;
    };
    drop(child);
    // ssh's own words are not the run's: read and dropped so ssh never waits on them.
    if let Some(mut stderr) = stderr {
        tokio::spawn(async move {
            let mut sink = vec![0u8; 4096];
            while matches!(stderr.read(&mut sink).await, Ok(n) if n > 0) {}
        });
    }
    // The header: how many input bytes arrived.
    let mut head: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let header = tokio::time::timeout(HEADER, async {
        loop {
            let read = stdout.read(&mut buffer).await.unwrap_or(0);
            if read == 0 {
                return None;
            }
            head.extend_from_slice(&buffer[..read]);
            if let Some(at) = head.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&head[..at]).into_owned();
                let rest = head.split_off(at + 1);
                return held::parse_header(&line).map(|arrived| (arrived, rest));
            }
            if head.len() > 256 {
                return None;
            }
        }
    })
    .await
    .ok()
    .flatten();
    if let Some((arrived, rest)) = header {
        *lost = None;
        let mut input = shared.input.lock().await;
        if let Some(mut writer) = stdin
            && !input.close_sent
        {
            let from = arrived.saturating_sub(input.base) as usize;
            let again = input.sent.get(from..).unwrap_or_default().to_vec();
            if again.is_empty() || (writer.write_all(&again).await.is_ok() && writer.flush().await.is_ok()) {
                if input.closed {
                    input.close_sent = shared.exec(held::close_input_script(&shared.id), SHORT).await.is_some_and(|c| c.exit_code == 0);
                    let _ = writer.shutdown().await;
                } else {
                    input.writer = Some(writer);
                }
            }
        }
        drop(input);
        if !rest.is_empty() && output.write_all(&rest).await.is_ok() {
            *offset += rest.len() as u64;
        }
        loop {
            let read = stdout.read(&mut buffer).await.unwrap_or(0);
            if read == 0 || output.write_all(&buffer[..read]).await.is_err() {
                break;
            }
            *offset += read as u64;
        }
    } else {
        control.kill();
    }
    shared.input.lock().await.writer = None;
    let code = control.wait().await.unwrap_or(LOST);
    if control.let_go() {
        return Attach::LetGo(code);
    }
    *lock(&shared.current) = None;
    match code {
        0 => Attach::Ended,
        3 => Attach::Gone,
        _ => Attach::Lost,
    }
}

/// The held run's control: stops it on the far side, waits for its exit code; letting go is the attach's.
struct HeldControl {
    shared: Arc<Shared>,
}

impl ChildControl for HeldControl {
    fn kill(&self) {
        let shared = &self.shared;
        if shared.exit.borrow().is_some() {
            return;
        }
        if let Some(current) = shared.current()
            && current.let_go()
        {
            current.kill();
            return;
        }
        if shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = shared.clone();
        tokio::spawn(async move {
            let _ = shared.exec(held::kill_script(&shared.id), SHORT).await;
            if let Some(current) = shared.current() {
                current.kill();
            }
            shared.stop.notify_waiters();
        });
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        let mut exit = self.shared.exit.subscribe();
        Box::pin(async move {
            let code = exit.wait_for(Option::is_some).await.map_err(|_| PlatformError::TimedOut("the held run"))?;
            Ok(code.unwrap_or(LOST))
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.kill();
            match tokio::time::timeout(Duration::from_secs(20), self.wait()).await {
                Ok(_) => Ok(()),
                Err(_) => Err(PlatformError::TimedOut("the held run")),
            }
        })
    }

    fn let_go(&self) -> bool {
        self.shared.current().is_some_and(|current| current.let_go())
    }

    fn leave(&self, state: String) {
        if let Some(current) = self.shared.current() {
            current.leave(state);
        }
    }
}

/// MC-40: a run's attachments uploaded to the SSH machine (mighty-core `ssh::held::upload_script`: each file through ssh's standard
/// input into a new 0700 folder, its `cksum` checked there), as the run's staged copies with that machine's paths, and the folder's
/// tag (the held run removes it when its CLI ends; else a later upload after a week). Err with the sentence to show.
pub async fn upload(
    processes: &dyn ProcessHost,
    launch: &SshLaunch,
    attachments: &[mighty_core::attachment::RunAttachment],
) -> std::result::Result<(crate::attachments::StagedAttachments, String), String> {
    let failed = || crate::strings::get("ssh.attachments.error.upload");
    let files = crate::attachments::validate(Some(attachments)).map_err(|e| e.to_string())?.ok_or_else(failed)?;
    let tag = mighty_core::wire::new_id().replace('-', "").to_ascii_lowercase();
    let mut folder: Option<String> = None;
    let mut uploaded = Vec::new();
    for (index, attachment) in files.iter().enumerate() {
        let bytes = crate::attachments::decode(attachment).map_err(|e| e.to_string())?;
        let suffix = mighty_core::wire::new_id().replace('-', "").to_ascii_lowercase();
        let name = format!("{index}-{}{}", &suffix[..16], crate::attachments::extension(attachment));
        let script = held::upload_script(&tag, &name, mighty_core::ssh::files::cksum(&bytes), index == 0).ok_or_else(failed)?;
        let request = launch.exec_request(script, &BTreeMap::new());
        let captured = mighty_platform::process::capture(processes, request, Some(bytes), Duration::from_secs(120), 64 * 1024)
            .await
            .map_err(|_| failed())?;
        if captured.exit_code != 0 {
            return Err(failed());
        }
        let printed = String::from_utf8_lossy(&captured.stdout).lines().last().unwrap_or_default().trim().to_string();
        if !printed.starts_with('/') || printed.chars().any(char::is_control) || folder.as_ref().is_some_and(|f| *f != printed) {
            return Err(failed());
        }
        uploaded.push((attachment.clone(), format!("{printed}/{name}")));
        folder = Some(printed);
    }
    let folder = folder.ok_or_else(failed)?;
    Ok((crate::attachments::StagedAttachments::remote(folder, uploaded), tag))
}

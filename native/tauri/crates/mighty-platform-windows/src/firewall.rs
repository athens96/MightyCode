//! The Tailscale firewall rule on Windows (C# `WindowsFirewallRuleRunner.ForThisApp`, `RunAsync`, `RunReadingAsync`,
//! Core\WindowsFirewallRule.cs; plan §1.4, D4): mighty-platform's shared runner (`firewall::NetshRunner`: the validation, the read
//! before an add, the one command) over [`WindowsNetsh`], which starts netsh — `show rule` through the platform's `ProcessHost` (hidden,
//! its output read, killed past 30 s), and a change through `ShellExecuteExW` with the verb "runas" and `SEE_MASK_NOCLOSEPROCESS`,
//! its window hidden, waited 30 s (`ERROR_CANCELLED`: the user declined; the wait running out: a timeout, the elevated netsh left to
//! finish). The program is this executable, the folder Windows' own system folder.
//!
//! This is the only file in the workspace that names the elevation verb (mighty-platform's `windows_firewall_only_the_rule_file_elevates`
//! check, C#'s L497, and CI's grep). The app runs it only with `--allow-state-write` and on the user's click; the checks and the smoke
//! give the runner a recorder. Owner: W8-C.

use std::sync::Arc;

use mighty_platform::firewall::netsh::{ELEVATION_CANCELLED, NetshLaunch};
use mighty_platform::firewall::{LIMIT, NetshExit, NetshFailure, NetshRun, NetshRunner};
use mighty_platform::{BoxFuture, Firewall, FirewallChange, FirewallRule, PlatformError, ProcessHost, Result, SpawnRequest, capture};

/// What an unelevated netsh may print before the rest is dropped (C#: 64 KiB of it are kept).
const OUTPUT_KEPT: usize = 65_536;

/// netsh started for real (C# `RunReadingAsync`).
pub struct WindowsNetsh {
    processes: Arc<dyn ProcessHost>,
}

impl WindowsNetsh {
    pub fn new(processes: Arc<dyn ProcessHost>) -> WindowsNetsh {
        WindowsNetsh { processes }
    }

    async fn query(&self, launch: NetshLaunch) -> std::result::Result<NetshExit, NetshFailure> {
        let request = SpawnRequest::new(&launch.exe, launch.arguments.iter().cloned(), &launch.working_directory);
        match capture(self.processes.as_ref(), request, None, LIMIT, 4 * OUTPUT_KEPT).await {
            Ok(captured) => {
                let mut output = String::from_utf8_lossy(&captured.stdout).into_owned();
                if output.len() > OUTPUT_KEPT {
                    let mut end = OUTPUT_KEPT;
                    while !output.is_char_boundary(end) {
                        end -= 1;
                    }
                    output.truncate(end);
                }
                Ok(NetshExit { code: captured.exit_code, output })
            }
            Err(PlatformError::TimedOut(_)) => Err(NetshFailure::TimedOut),
            Err(error) => Err(NetshFailure::Failed(error.to_string())),
        }
    }

    /// The elevated start on a thread of its own (the call waits on the prompt), then the wait for its exit code.
    async fn elevated(&self, launch: NetshLaunch) -> std::result::Result<NetshExit, NetshFailure> {
        let started = tokio::task::spawn_blocking(move || elevate(&launch)).await;
        match started {
            Ok(outcome) => outcome,
            Err(_) => Err(NetshFailure::Failed("the elevated start did not finish".into())),
        }
    }
}

/// `ShellExecuteExW` with the elevation verb, hidden, from the system folder; waits up to [`LIMIT`] for the exit code.
fn elevate(launch: &NetshLaunch) -> std::result::Result<NetshExit, NetshFailure> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let (verb, file, parameters, folder) = (
        crate::win::wide("runas"),
        crate::win::wide(&launch.exe),
        crate::win::wide(&launch.command_line),
        crate::win::wide(&launch.working_directory),
    );
    // SAFETY: an all-zero SHELLEXECUTEINFOW is its documented empty value.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.lpDirectory = folder.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY: the structure and the strings it points at live across the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: read right after the failing call on this thread.
        let error = unsafe { GetLastError() };
        if error == ELEVATION_CANCELLED as u32 {
            return Err(NetshFailure::Cancelled);
        }
        return Err(NetshFailure::Failed(std::io::Error::from_raw_os_error(error as i32).to_string()));
    }
    let process = info.hProcess;
    if process.is_null() {
        return Err(NetshFailure::Failed("netsh started without a process handle".into()));
    }
    // SAFETY: the process handle SEE_MASK_NOCLOSEPROCESS handed over; closed once below.
    let waited = unsafe { WaitForSingleObject(process, LIMIT.as_millis() as u32) };
    let outcome = if waited == WAIT_OBJECT_0 {
        let mut code: u32 = 0;
        // SAFETY: the same live handle.
        if unsafe { GetExitCodeProcess(process, &mut code) } != 0 {
            Ok(NetshExit { code: code as i32, output: String::new() })
        } else {
            Err(NetshFailure::Failed(std::io::Error::last_os_error().to_string()))
        }
    } else if waited == WAIT_TIMEOUT {
        Err(NetshFailure::TimedOut)
    } else {
        Err(NetshFailure::Failed(std::io::Error::last_os_error().to_string()))
    };
    // SAFETY: closed once; an elevated netsh still running is left to finish.
    unsafe { CloseHandle(process) };
    outcome
}

impl NetshRun for WindowsNetsh {
    fn run(&self, launch: NetshLaunch) -> BoxFuture<'_, std::result::Result<NetshExit, NetshFailure>> {
        Box::pin(async move { if launch.elevated { self.elevated(launch).await } else { self.query(launch).await } })
    }
}

/// The app's rule: C#'s runner over [`WindowsNetsh`], netsh from the folder Windows reports, the rule naming this executable.
pub struct WindowsFirewall {
    runner: NetshRunner,
}

impl WindowsFirewall {
    pub fn new(processes: Arc<dyn ProcessHost>) -> WindowsFirewall {
        let system = crate::win::system_directory().map(|folder| folder.to_string_lossy().into_owned());
        let system = system.filter(|folder| mighty_platform::firewall::netsh::valid_system_directory(Some(folder)));
        let program = std::env::current_exe().ok().map(|exe| exe.to_string_lossy().into_owned());
        let runner = NetshRunner::new(Arc::new(WindowsNetsh::new(processes)), system.clone(), program, system);
        WindowsFirewall { runner }
    }
}

impl Firewall for WindowsFirewall {
    fn read(&self) -> BoxFuture<'_, Result<FirewallRule>> {
        self.runner.read()
    }
    fn apply(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        self.runner.apply(ports)
    }
    fn add(&self, ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        self.runner.add(ports)
    }
    fn remove(&self) -> BoxFuture<'_, Result<FirewallChange>> {
        self.runner.remove()
    }
}

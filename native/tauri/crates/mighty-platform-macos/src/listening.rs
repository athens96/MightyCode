//! MC-26: the machine's processes and listening TCP sockets for a pane's ports ([`mighty_platform::listening`]): each process's parent
//! from `proc_pidinfo` (as the shell's activity reads it, [`crate::pty::groups`]), the listening sockets from
//! `lsof -nP -iTCP -sTCP:LISTEN -Fpn` (libproc's socket records have no binding in `libc`). Blocking: call it off the async runtime.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mighty_platform::listening::{ProcessNet, parse_lsof};

/// How long `lsof` may take before it is stopped (and the reading has no sockets).
const LSOF_LIMIT: Duration = Duration::from_secs(5);

/// Every process's parent and every listening TCP socket this user can see; None when the process list cannot be read.
pub fn snapshot() -> Option<ProcessNet> {
    let parents = parents()?;
    Some(ProcessNet { parents, listeners: listeners().unwrap_or_default() })
}

fn parents() -> Option<HashMap<u32, u32>> {
    let mut capacity = 1024usize;
    loop {
        let mut pids = vec![0 as libc::pid_t; capacity];
        // SAFETY: a buffer of that many bytes.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (capacity * size_of::<libc::pid_t>()) as libc::c_int) };
        if count < 0 {
            return None;
        }
        let count = count as usize;
        if count >= capacity && capacity < 65_536 {
            capacity *= 4;
            continue;
        }
        pids.truncate(count.min(capacity));
        return Some(
            pids.into_iter()
                .filter(|&pid| pid > 0)
                .filter_map(|pid| crate::pty::groups(pid).map(|groups| (pid as u32, groups.ppid.max(0) as u32)))
                .collect(),
        );
    }
}

fn listeners() -> Option<Vec<(u32, u16)>> {
    #[allow(
        clippy::disallowed_methods,
        reason = "macOS std::process::Command sets FD_CLOEXEC (the handle inheritance the rule guards against is Windows'); a short read-only lsof"
    )]
    let mut child = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + LSOF_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    // lsof exits 1 when it found nothing: what it wrote is still the answer.
    reader.join().ok().map(|text| parse_lsof(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listener this test opens is found under this process.
    #[test]
    fn a_listening_socket_of_this_process_is_found() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let net = snapshot().expect("the process list");
        assert!(net.parents.contains_key(&std::process::id()));
        assert!(net.ports([std::process::id()]).contains(&port), "{port} in {:?}", net.ports([std::process::id()]));
    }
}

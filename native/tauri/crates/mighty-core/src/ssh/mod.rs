//! MC-33: SSH machines with nothing installed on the far side. The pure parts the app (src-tauri `ssh`) and the run engine (mighty-run
//! `ssh`) share; no I/O here except through the caller's [`config::ConfigSource`].
//!
//! - [`target`]: what the user may name — a `Host` alias from `~/.ssh/config` or the plain `user@host[:port]` form, never anything ssh
//!   would read as an option.
//! - [`config`]: the `Host` aliases of an ssh_config (with `Include`; wildcards and `Match` blocks left out).
//! - [`command`]: the remote command lines (quoted for the far side's POSIX shell) and the local `ssh` argument arrays: terminals
//!   (`-tt`), runs (`-T`, batch mode), the connection test, port forwards, read-only file reads, with ControlMaster where the local ssh
//!   has it.
//! - [`failure`]: what ssh's stderr says went wrong (not reachable, sign-in refused, host key unknown or changed).
//! - [`files`]: the remote file listing's parsing and the relative paths a read may name.
//! - [`server`]: a Mighty Code server on a remote Mac: the detection, the install's steps, the pairing and the removal as scripts.
//!
//! Authentication stays with OpenSSH (`~/.ssh/config`, ssh-agent, keys): the app never sees a password or a key.

pub mod command;
pub mod config;
pub mod failure;
pub mod files;
pub mod held;
pub mod server;
pub mod target;

pub use command::{Mux, quote};
pub use failure::Failure;
pub use target::{Target, TargetError};

/// The longest remote folder a workspace may name (as a local path's limit).
pub const MAXIMUM_DIR_UNITS: usize = 4096;

/// A remote folder a workspace may open: an absolute path (`/srv/app`) or one under the remote home (`~`, `~/src/app`), at most 4,096
/// characters, no NUL, line break or other control character.
pub fn valid_remote_dir(dir: &str) -> bool {
    let units = dir.encode_utf16().count();
    (1..=MAXIMUM_DIR_UNITS).contains(&units)
        && !dir.chars().any(char::is_control)
        && (dir.starts_with('/') || dir == "~" || dir.starts_with("~/"))
}

/// A name for a workspace from its remote folder: the last part, or the alias for the remote home.
pub fn dir_name(dir: &str, machine: &str) -> String {
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(last) if !last.is_empty() && last != "~" => last.to_string(),
        _ => machine.to_string(),
    }
}

/// The most dev-server ports offered for one pane.
pub const MAXIMUM_OFFERED_PORTS: usize = 4;

/// The ports a remote dev server says it listens on, from a pane's recent output: `localhost:3000`, `127.0.0.1:5173`, `0.0.0.0:8000`,
/// `[::1]:8080`, `[::]:4000` (1024–65535), the newest last, each once, at most [`MAXIMUM_OFFERED_PORTS`].
pub fn dev_server_ports(text: &str) -> Vec<u16> {
    let mut seen: Vec<(usize, u16)> = Vec::new();
    for host in ["localhost:", "127.0.0.1:", "0.0.0.0:", "[::1]:", "[::]:"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(host) {
            let begin = from + at;
            let start = begin + host.len();
            from = start;
            if text[..begin].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                continue;
            }
            let digits: String = text[start..].chars().take_while(char::is_ascii_digit).take(6).collect();
            if let Ok(port) = digits.parse::<u32>()
                && (1024..=65535).contains(&port)
                && digits.len() <= 5
            {
                seen.push((begin, port as u16));
            }
        }
    }
    seen.sort();
    let mut found: Vec<u16> = Vec::new();
    for (_, port) in seen {
        found.retain(|p| *p != port);
        found.push(port);
    }
    let keep = found.len().saturating_sub(MAXIMUM_OFFERED_PORTS);
    found.drain(..keep);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_server_ports_are_read_from_output() {
        let text = "  VITE v5  ready\n  ➜  Local:   http://localhost:5173/\nListening on 0.0.0.0:8000\nmyhost.localhost:9999 notlocalhost:7000 \
                    http://127.0.0.1:80/ [::1]:8080 localhost:5173";
        assert_eq!(dev_server_ports(text), [8000, 8080, 5173]);
        assert!(dev_server_ports("localhost:999999 localhost:abc").is_empty());
        let many: String = (3000..3010).map(|p| format!("localhost:{p} ")).collect();
        assert_eq!(dev_server_ports(&many), [3006, 3007, 3008, 3009]);
    }

    #[test]
    fn a_remote_dir_is_absolute_or_under_home() {
        for good in ["/srv/app", "/", "~", "~/src/my app", "/home/o'neil/x \"y\""] {
            assert!(valid_remote_dir(good), "{good}");
        }
        for bad in ["", "src/app", "~other/x", "-rf", "/a\nb", "/a\0b", "C:\\x"] {
            assert!(!valid_remote_dir(bad), "{bad:?}");
        }
        assert!(!valid_remote_dir(&format!("/{}", "a".repeat(4096))));
    }

    #[test]
    fn a_dirs_name_is_its_last_part() {
        assert_eq!(dir_name("/srv/app/", "box"), "app");
        assert_eq!(dir_name("~/src/my app", "box"), "my app");
        assert_eq!(dir_name("~", "box"), "box");
        assert_eq!(dir_name("/", "box"), "box");
    }
}

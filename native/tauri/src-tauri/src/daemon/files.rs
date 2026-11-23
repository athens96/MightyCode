//! MC-3: the daemon's files, in the profile's `daemon` folder that only this user can read (0700; 0600 files on the Mac, a protected
//! list of this user alone on Windows; written as the control API's are, [`crate::control::endpoint::write_private`]):
//! - `token`: the 64 hex digits every connection's `hello` carries (its own, never the control API's), kept across daemons;
//! - `endpoint.json`: where the running daemon listens (`unix` and a socket's path, or `pipe` and a pipe's name), its process, version
//!   and protocol; removed when it stops;
//! - `daemon.lock`: held by the running daemon (one per profile);
//! - `daemon.log`: what it did (never a token, an environment value or output), started again past 1 MB;
//! - `child-groups/`: its children's process groups while it runs (the Mac), so what a crashed daemon left is ended later.

use std::path::{Path, PathBuf};

use mighty_core::daemon::protocol::is_token;
use serde_json::{Value, json};

use crate::control::endpoint::{new_token, read_small, write_private};

pub const FOLDER: &str = "daemon";
pub const TOKEN_FILE: &str = "token";
pub const ENDPOINT_FILE: &str = "endpoint.json";
pub const LOCK_FILE: &str = "daemon.lock";
pub const LOG_FILE: &str = "daemon.log";
/// Where the daemon writes its children's process groups down for the next launch should it crash (the Mac).
pub const GROUPS_FOLDER: &str = "child-groups";
/// A log longer than this starts again when a daemon starts.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

pub fn read_token(data_dir: &Path) -> Option<String> {
    read_small(&folder(data_dir).join(TOKEN_FILE)).map(|t| t.trim().to_string()).filter(|t| is_token(t))
}

/// The token kept from before, or a new one written.
pub fn token_or_new(data_dir: &Path) -> std::io::Result<String> {
    if let Some(token) = read_token(data_dir) {
        return Ok(token);
    }
    let token = new_token();
    write_private(&folder(data_dir).join(TOKEN_FILE), format!("{token}\n").as_bytes())?;
    Ok(token)
}

/// Where a daemon listens, its process, its version and its protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub transport: String,
    pub address: String,
    pub pid: u32,
    pub app: String,
    pub protocol: u64,
}

pub fn write_endpoint(data_dir: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    let text = json!({
        "version": 1,
        "transport": endpoint.transport,
        "address": endpoint.address,
        "pid": endpoint.pid,
        "app": endpoint.app,
        "protocol": endpoint.protocol,
    });
    write_private(&folder(data_dir).join(ENDPOINT_FILE), format!("{text}\n").as_bytes())
}

pub fn read_endpoint(data_dir: &Path) -> Option<Endpoint> {
    let value: Value = serde_json::from_str(&read_small(&folder(data_dir).join(ENDPOINT_FILE))?).ok()?;
    Some(Endpoint {
        transport: value.get("transport")?.as_str()?.to_string(),
        address: value.get("address")?.as_str()?.to_string(),
        pid: value.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
        app: value.get("app").and_then(Value::as_str).unwrap_or("").to_string(),
        protocol: value.get("protocol").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// The endpoint file goes when the daemon stops, only while it still names `address`.
pub fn remove_endpoint(data_dir: &Path, address: &str) {
    if read_endpoint(data_dir).is_some_and(|e| e.address == address) {
        let _ = std::fs::remove_file(folder(data_dir).join(ENDPOINT_FILE));
    }
}

/// The daemon's log: lines appended with the time, the file started again when it grew past its limit.
pub struct DaemonLog {
    file: std::sync::Mutex<Option<std::fs::File>>,
}

impl DaemonLog {
    pub fn open(data_dir: &Path) -> DaemonLog {
        let path = folder(data_dir).join(LOG_FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = std::fs::remove_file(&path);
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&path).ok();
        DaemonLog { file: std::sync::Mutex::new(file) }
    }

    pub fn line(&self, text: &str) {
        use std::io::Write as _;
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        if let Some(file) = self.file.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            let _ = writeln!(file, "{stamp} [{}] {text}", std::process::id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_files_are_private_apart_from_the_control_apis_and_read_back() {
        let data = std::env::temp_dir().join(format!("mc-daemon-files-{}", mighty_core::wire::new_id()));
        assert_eq!(read_token(&data), None);
        let token = token_or_new(&data).unwrap();
        assert!(is_token(&token));
        assert_eq!(token_or_new(&data).unwrap(), token, "kept across daemons");
        assert_ne!(crate::control::endpoint::token_or_new(&data).unwrap(), token, "never the control API's token");
        let endpoint = Endpoint { transport: "unix".into(), address: "/tmp/d.sock".into(), pid: 7, app: "0.6.0".into(), protocol: 1 };
        write_endpoint(&data, &endpoint).unwrap();
        assert_eq!(read_endpoint(&data), Some(endpoint));
        let log = DaemonLog::open(&data);
        log.line("daemon: started");
        let log_path = folder(&data).join(LOG_FILE);
        assert!(std::fs::read_to_string(&log_path).unwrap().contains("daemon: started"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&folder(&data)), 0o700);
            assert_eq!(mode(&folder(&data).join(TOKEN_FILE)), 0o600);
            assert_eq!(mode(&folder(&data).join(ENDPOINT_FILE)), 0o600);
            assert_eq!(mode(&log_path), 0o600);
            // A token file that is a link is not read.
            std::fs::remove_file(folder(&data).join(TOKEN_FILE)).unwrap();
            let elsewhere = data.join("elsewhere");
            std::fs::write(&elsewhere, format!("{token}\n")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, folder(&data).join(TOKEN_FILE)).unwrap();
            assert_eq!(read_token(&data), None);
        }
        remove_endpoint(&data, "/tmp/other.sock");
        assert!(read_endpoint(&data).is_some(), "another daemon's endpoint stays");
        remove_endpoint(&data, "/tmp/d.sock");
        assert!(read_endpoint(&data).is_none());
        std::fs::remove_dir_all(&data).unwrap();
    }
}

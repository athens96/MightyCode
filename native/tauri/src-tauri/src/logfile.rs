//! The host's log, `<data dir>\tauri-logs\host.log` (or `--log-dir`): one timestamped line per entry, appended.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const HOST_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub struct LogFile {
    dir: PathBuf,
    file: Mutex<Option<File>>,
}

impl LogFile {
    pub fn new(dir: PathBuf) -> Self {
        LogFile { dir, file: Mutex::new(None) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Rotates `host.log` -> `host.log.1` when its size exceeds `max_bytes` (`10 MiB` by default).
    pub fn rotate_if_oversize(&self, max_bytes: u64) -> bool {
        let Ok(mut file) = self.file.lock() else { return false };
        let log_path = self.dir.join("host.log");
        if fs::metadata(&log_path).is_ok_and(|m| m.len() > max_bytes) {
            *file = None;
            return fs::rename(&log_path, self.dir.join("host.log.1")).is_ok();
        }
        false
    }

    /// Appends one line; a log that cannot be written is skipped (logging never changes what the app does).
    pub fn line(&self, text: &str) {
        let Ok(mut file) = self.file.lock() else { return };
        if file.is_none() {
            let log_path = self.dir.join("host.log");
            *file = fs::create_dir_all(&self.dir)
                .and_then(|_| {
                    if fs::metadata(&log_path).is_ok_and(|m| m.len() > HOST_LOG_MAX_BYTES) {
                        let _ = fs::rename(&log_path, self.dir.join("host.log.1"));
                    }
                    OpenOptions::new().create(true).append(true).open(&log_path)
                })
                .ok();
        }
        if let Some(file) = file.as_mut() {
            let _ = writeln!(
                file,
                "{} {}",
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                text.replace(['\r', '\n'], " ")
            );
        }
    }
}

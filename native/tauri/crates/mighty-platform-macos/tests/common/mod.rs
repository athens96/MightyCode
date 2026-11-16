//! What the Mac checks share (C# `Verification.Temp`, `Until`, `Alive`; Swift's `temporary()`, `waitUntil`).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mighty_platform::process::ChildReader;
use tokio::io::AsyncReadExt;

/// A new empty folder of this test's own, by its canonical path (the Mac's temporary folder is under `/var`, a link to `/private/var`).
pub fn temp(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let count = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("mighty-macos-test-{name}-{}-{nanos}-{count}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(&path).unwrap()
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

/// Waits until `condition` holds, failing after `milliseconds`.
pub async fn until(mut condition: impl FnMut() -> bool, milliseconds: u64, what: &str) {
    let end = Instant::now() + Duration::from_millis(milliseconds);
    while !condition() {
        assert!(Instant::now() < end, "expected condition was not reached: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Whether a process of this id is still running (and not a zombie: signal 0 reaches a zombie too, so its state is read).
pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
        return false;
    }
    // SAFETY: a plain struct proc_pidinfo fills.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: a buffer of `size` bytes.
    let read = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    // SZOMB is 5 (sys/proc.h).
    read != size || info.pbi_status != 5
}

/// The number in a file a fixture wrote, once it is there.
pub async fn pid_from(file: &Path) -> u32 {
    until(|| std::fs::read_to_string(file).is_ok_and(|text| text.trim().parse::<u32>().is_ok()), 20_000, "the fixture's pid file").await;
    std::fs::read_to_string(file).unwrap().trim().parse().unwrap()
}

/// All of a pipe, as bytes.
pub async fn read_all(reader: Option<ChildReader>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut reader) = reader {
        reader.read_to_end(&mut bytes).await.unwrap();
    }
    bytes
}

/// An executable `#!/bin/sh` script named `name` in `folder`.
pub fn script(folder: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

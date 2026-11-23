//! MC-9: where the control API listens and its token, as files in the profile's `control` folder that only this user can read
//! (0600 on the Mac; a protected list of this user alone on Windows). The `mighty` CLI reads both; the app writes them once it
//! listens, keeps the token across restarts and makes a new one when the person asks (Settings › 자동화).

use std::path::{Path, PathBuf};

use mighty_core::control::protocol::is_token;
use serde_json::{Value, json};

pub const FOLDER: &str = "control";
pub const TOKEN_FILE: &str = "token";
pub const ENDPOINT_FILE: &str = "endpoint.json";
/// The longest endpoint file the CLI reads.
const MAX_FILE_BYTES: u64 = 16 * 1024;

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

/// A new token: 64 hex digits of two random v4 UUIDs (244 random bits).
pub fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Writes `bytes` to `path` so only this user can read it: a new file beside it, private from the start, then moved in place.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    create_private_folder(parent)?;
    let staged = parent.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let written = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&staged)?
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&staged)?;
        #[cfg(windows)]
        crate::platform::trust_restrict(&staged, false)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staged, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written
}

/// The control folder, made private (0700) where it is new; never a link.
fn create_private_folder(folder: &Path) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(folder) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(std::io::Error::other("the control folder is not a folder"));
        }
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(folder)?;
        #[cfg(windows)]
        crate::platform::trust_restrict(folder, true)?;
        Ok(())
    }
}

/// Reads a small file that must be a plain file (not a link) of this user's.
fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        // SAFETY: getuid has no preconditions.
        if meta.uid() != unsafe { libc_getuid() } {
            return None;
        }
    }
    std::fs::read_to_string(path).ok()
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// The token in the profile, when one is there and well formed.
pub fn read_token(data_dir: &Path) -> Option<String> {
    read_small(&folder(data_dir).join(TOKEN_FILE)).map(|t| t.trim().to_string()).filter(|t| is_token(t))
}

pub fn write_token(data_dir: &Path, token: &str) -> std::io::Result<()> {
    write_private(&folder(data_dir).join(TOKEN_FILE), format!("{token}\n").as_bytes())
}

/// The token kept from before, or a new one written.
pub fn token_or_new(data_dir: &Path) -> std::io::Result<String> {
    if let Some(token) = read_token(data_dir) {
        return Ok(token);
    }
    let token = new_token();
    write_token(data_dir, &token)?;
    Ok(token)
}

/// Where the app listens: `unix` (a socket's path) or `pipe` (a named pipe's name), with the app's process and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub transport: String,
    pub address: String,
    pub pid: u32,
    pub version: String,
}

pub fn write_endpoint(data_dir: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    let text = json!({
        "version": 1,
        "transport": endpoint.transport,
        "address": endpoint.address,
        "pid": endpoint.pid,
        "app": endpoint.version,
    });
    write_private(&folder(data_dir).join(ENDPOINT_FILE), format!("{text}\n").as_bytes())
}

pub fn read_endpoint(data_dir: &Path) -> Option<Endpoint> {
    let value: Value = serde_json::from_str(&read_small(&folder(data_dir).join(ENDPOINT_FILE))?).ok()?;
    Some(Endpoint {
        transport: value.get("transport")?.as_str()?.to_string(),
        address: value.get("address")?.as_str()?.to_string(),
        pid: value.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
        version: value.get("app").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

/// The endpoint file goes when the app stops listening (the token stays for the next run), only while it still names `address`.
pub fn remove_endpoint(data_dir: &Path, address: &str) {
    if read_endpoint(data_dir).is_some_and(|e| e.address == address) {
        let _ = std::fs::remove_file(folder(data_dir).join(ENDPOINT_FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_and_endpoint_files_are_private_and_read_back() {
        let data = std::env::temp_dir().join(format!("mc-control-files-{}", mighty_core::wire::new_id()));
        assert_eq!(read_token(&data), None);
        let token = token_or_new(&data).unwrap();
        assert!(is_token(&token));
        assert_eq!(token_or_new(&data).unwrap(), token, "kept across restarts");
        let fresh = new_token();
        assert_ne!(fresh, token);
        write_token(&data, &fresh).unwrap();
        assert_eq!(read_token(&data).as_deref(), Some(fresh.as_str()), "a rotation replaces it");
        let endpoint = Endpoint { transport: "unix".into(), address: "/tmp/x.sock".into(), pid: 42, version: "0.6.0".into() };
        write_endpoint(&data, &endpoint).unwrap();
        assert_eq!(read_endpoint(&data), Some(endpoint));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&folder(&data).join(TOKEN_FILE)), 0o600);
            assert_eq!(mode(&folder(&data).join(ENDPOINT_FILE)), 0o600);
            assert_eq!(mode(&folder(&data)), 0o700);
            // A token file that is a link is not read.
            std::fs::remove_file(folder(&data).join(TOKEN_FILE)).unwrap();
            let elsewhere = data.join("elsewhere");
            std::fs::write(&elsewhere, format!("{fresh}\n")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, folder(&data).join(TOKEN_FILE)).unwrap();
            assert_eq!(read_token(&data), None);
        }
        remove_endpoint(&data, "/tmp/other.sock");
        assert!(read_endpoint(&data).is_some(), "another run's endpoint stays");
        remove_endpoint(&data, "/tmp/x.sock");
        assert!(read_endpoint(&data).is_none());
        assert_eq!(
            std::fs::read_dir(folder(&data))
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
        std::fs::remove_dir_all(&data).unwrap();
    }
}

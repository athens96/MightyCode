//! Port blocks for worktrees (`MC-25`): each worktree of a repository (and its main checkout) gets a block of ports no other
//! worktree on this computer holds, remembered in `<data>/ports.json` as `{ repos: { <main checkout>: { <worktree>: { base, count } } } }`
//! (both keys the host's normalized paths). A block skips every block already assigned (in any repository: ports belong to the
//! computer) and every port something listens on now. A worktree's block is released when the worktree is removed; blocks of worktrees
//! that vanished while the app was closed are pruned at the start. The host hands the block to the worktree's scripts, tasks, run
//! actions and agent as `MIGHTY_PORT` (the first port) and `MIGHTY_PORT_COUNT`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The file in the app's data folder.
pub const FILE_NAME: &str = "ports.json";
/// Where the search for a free block starts.
pub const DEFAULT_START: u16 = 41_000;
/// No block reaches past this port (the OS hands out ephemeral ports above it on most systems).
pub const HIGHEST_PORT: u16 = 60_999;
/// The largest file read (a corrupt or hostile file reads as empty).
const MAXIMUM_FILE_BYTES: u64 = 1024 * 1024;

/// A block of `count` ports from `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBlock {
    pub base: u16,
    pub count: u16,
}

impl PortBlock {
    /// The block's last port.
    pub fn last(&self) -> u16 {
        self.base.saturating_add(self.count.saturating_sub(1))
    }

    pub fn contains(&self, port: u16) -> bool {
        self.count > 0 && (self.base..=self.last()).contains(&port)
    }

    pub fn overlaps(&self, other: &PortBlock) -> bool {
        self.count > 0 && other.count > 0 && self.base <= other.last() && other.base <= self.last()
    }

    /// `:41010–41019` (`:41010` for a block of one).
    pub fn label(&self) -> String {
        if self.count <= 1 { format!(":{}", self.base) } else { format!(":{}\u{2013}{}", self.base, self.last()) }
    }

    fn valid(&self) -> bool {
        self.base > 0 && self.count > 0 && u32::from(self.base) + u32::from(self.count) - 1 <= 65_535
    }
}

fn default_version() -> u32 {
    1
}

/// `ports.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Main checkout -> worktree (the main checkout's own key for its block) -> block.
    #[serde(default)]
    pub repos: BTreeMap<String, BTreeMap<String, PortBlock>>,
}

impl Default for PortsFile {
    fn default() -> Self {
        PortsFile { version: 1, repos: BTreeMap::new() }
    }
}

impl PortsFile {
    /// The block `worktree` of `repo` holds.
    pub fn get(&self, repo: &str, worktree: &str) -> Option<PortBlock> {
        self.repos.get(repo).and_then(|blocks| blocks.get(worktree)).copied()
    }

    /// Every assigned block, in every repository.
    pub fn assigned(&self) -> Vec<PortBlock> {
        self.repos.values().flat_map(|blocks| blocks.values().copied()).collect()
    }

    /// The block of `count` ports for `worktree` of `repo`: the one it holds when that is the same size, else a new one ([`find_free_block`]
    /// from `start`, skipping assigned blocks and ports `in_use` says something listens on). `count` 0 releases what it held and
    /// assigns none; None too when no free block is left.
    pub fn assign(&mut self, repo: &str, worktree: &str, count: u16, start: u16, in_use: impl Fn(u16) -> bool) -> Option<PortBlock> {
        if let Some(held) = self.get(repo, worktree) {
            if held.count == count && count > 0 {
                return Some(held);
            }
            self.release(repo, worktree);
        }
        if count == 0 {
            return None;
        }
        let block = find_free_block(&self.assigned(), count, start, in_use)?;
        self.repos.entry(repo.to_string()).or_default().insert(worktree.to_string(), block);
        Some(block)
    }

    /// Records a block a worktree already holds (its saved record, when `ports.json` lost it): only when it is valid and overlaps no
    /// other worktree's block. True when it is recorded (or was already).
    pub fn adopt(&mut self, repo: &str, worktree: &str, block: PortBlock) -> bool {
        if self.get(repo, worktree) == Some(block) {
            return true;
        }
        let others_overlap = self
            .repos
            .iter()
            .flat_map(|(r, blocks)| blocks.iter().filter(move |(w, _)| !(r == repo && w.as_str() == worktree)).map(|(_, b)| b))
            .any(|other| other.overlaps(&block));
        if !block.valid() || others_overlap {
            return false;
        }
        self.repos.entry(repo.to_string()).or_default().insert(worktree.to_string(), block);
        true
    }

    /// Gives back the block `worktree` of `repo` holds.
    pub fn release(&mut self, repo: &str, worktree: &str) -> Option<PortBlock> {
        let blocks = self.repos.get_mut(repo)?;
        let released = blocks.remove(worktree);
        if blocks.is_empty() {
            self.repos.remove(repo);
        }
        released
    }

    /// Gives back the blocks of worktrees `exists` says are gone (a worktree removed while the app was closed), and of every worktree
    /// of a main checkout that is gone; the `(repo, worktree)` pairs released.
    pub fn prune(&mut self, exists: impl Fn(&str) -> bool) -> Vec<(String, String)> {
        let mut released = Vec::new();
        self.repos.retain(|repo, blocks| {
            let repo_exists = exists(repo);
            blocks.retain(|worktree, _| {
                let keep = repo_exists && exists(worktree);
                if !keep {
                    released.push((repo.clone(), worktree.clone()));
                }
                keep
            });
            !blocks.is_empty()
        });
        released
    }

    /// Drops invalid blocks and later blocks that overlap an earlier one (a file edited by hand).
    fn sanitized(mut self) -> PortsFile {
        let mut seen: Vec<PortBlock> = Vec::new();
        for blocks in self.repos.values_mut() {
            blocks.retain(|key, block| {
                let keep = !key.trim().is_empty() && block.valid() && !seen.iter().any(|other| other.overlaps(block));
                if keep {
                    seen.push(*block);
                }
                keep
            });
        }
        self.repos.retain(|key, blocks| !key.trim().is_empty() && !blocks.is_empty());
        self.version = 1;
        self
    }
}

/// MC-47: the key an SSH machine's folder has in `ports.json`: `ssh://<machine id><absolute path>`. Never a local path, so a block on
/// that machine stays apart from this computer's paths, and the start's prune (which looks for local folders) never reads it as gone.
pub fn remote_key(machine: &str, path: &str) -> String {
    format!("ssh://{machine}{}", mighty_core::ssh::worktree::posix::standardized(path))
}

/// Whether `key` names a folder on an SSH machine ([`remote_key`]).
pub fn is_remote_key(key: &str) -> bool {
    key.starts_with("ssh://")
}

/// The lowest block of `count` ports from `start` that overlaps none of `taken` and holds no port `in_use`; None when none fits below
/// [`HIGHEST_PORT`].
pub fn find_free_block(taken: &[PortBlock], count: u16, start: u16, in_use: impl Fn(u16) -> bool) -> Option<PortBlock> {
    if count == 0 {
        return None;
    }
    let mut base = u32::from(start.max(1));
    'search: while base + u32::from(count) - 1 <= u32::from(HIGHEST_PORT) {
        let candidate = PortBlock { base: base as u16, count };
        if let Some(last) = taken.iter().filter(|block| block.overlaps(&candidate)).map(PortBlock::last).max() {
            base = u32::from(last) + 1;
            continue;
        }
        for port in candidate.base..=candidate.last() {
            if in_use(port) {
                base = u32::from(port) + 1;
                continue 'search;
            }
        }
        return Some(candidate);
    }
    None
}

/// Whether something listens on `port` on this computer now: a connection to it on loopback (IPv4 or IPv6) is accepted, or the port
/// cannot be bound on IPv4 loopback.
pub fn port_in_use(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
    let wait = Duration::from_millis(60);
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&v4, wait).is_ok() || TcpStream::connect_timeout(&v6, wait).is_ok() || TcpListener::bind(v4).is_err()
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Reads `<data_dir>/ports.json`; a missing, linked, oversized or malformed file reads as empty.
pub fn load(data_dir: &Path) -> PortsFile {
    let path = data_dir.join(FILE_NAME);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return PortsFile::default() };
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() || meta.len() > MAXIMUM_FILE_BYTES {
        return PortsFile::default();
    }
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PortsFile>(&bytes).ok())
        .map(PortsFile::sanitized)
        .unwrap_or_default()
}

/// Writes `<data_dir>/ports.json` through a temporary file and a rename; refuses a linked target.
pub fn save(data_dir: &Path, file: &PortsFile) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(data_dir)?;
    let target = data_dir.join(FILE_NAME);
    if let Ok(meta) = std::fs::symlink_metadata(&target)
        && (meta.file_type().is_symlink() || is_reparse_point(&meta))
    {
        return Err(std::io::Error::other("ports.json must not be a symbolic link"));
    }
    let mut bytes = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let temp = data_dir.join(format!("{FILE_NAME}.{}.{}.tmp", std::process::id(), uuid::Uuid::new_v4().simple()));
    let written: std::io::Result<()> = (|| {
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

//! Cross-device and remote-host shared agent memory (`<data_dir>/shared-memory.json`), secret sanitization guard,
//! 3-way / LWW side-version conflict tracking, prompt injection rendering, and `<data_dir>/remote-cache/<connection_id>/`
//! offline fallback storage (`MC-13`).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SHARED_MEMORY_FILE: &str = "shared-memory.json";
pub const SHARED_MEMORY_BACKUP_FILE: &str = "shared-memory.json.bak";
pub const REMOTE_CACHE_DIR: &str = "remote-cache";
pub const REMOTE_CACHE_STATE_FILE: &str = "state.json";
pub const DEFAULT_INJECT_MAX_BYTES: usize = 4096;
pub const MAX_MEMORY_ENTRIES: usize = 1000;
pub const MAX_ENTRY_CONTENT_BYTES: usize = 32 * 1024;
pub const MAX_CONFLICTS: usize = 200;
pub const REDACTED_SECRET: &str = "[REDACTED_SECRET]";

fn default_true() -> bool {
    true
}

fn default_version() -> u32 {
    1
}

/// Scope of a shared memory entry: either cross-workspace user conventions or a specific project/workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MemoryScope {
    User,
    #[serde(rename_all = "camelCase")]
    Project {
        workspace_id: String,
        #[serde(default)]
        project_slug: String,
    },
}

impl MemoryScope {
    pub fn is_user(&self) -> bool {
        matches!(self, MemoryScope::User)
    }

    /// Canonical logical scope identifier used to match `(scope, key)` slots across paired devices.
    pub fn scope_key(&self) -> String {
        match self {
            MemoryScope::User => "user".to_string(),
            MemoryScope::Project { workspace_id, project_slug } => {
                let normalized = normalize_slug(project_slug);
                if !normalized.is_empty() { format!("project:slug:{normalized}") } else { format!("project:ws:{workspace_id}") }
            }
        }
    }

    /// True when this scope applies to the project identified by `workspace_id` and optional `project_slug`.
    pub fn matches_project(&self, workspace_id: &str, project_slug: Option<&str>) -> bool {
        let MemoryScope::Project { workspace_id: own_ws, project_slug: own_slug } = self else {
            return false;
        };
        if !workspace_id.is_empty() && own_ws == workspace_id {
            return true;
        }
        let own_norm = normalize_slug(own_slug);
        if own_norm.is_empty() {
            return false;
        }
        if let Some(target_slug) = project_slug {
            let target_norm = normalize_slug(target_slug);
            if !target_norm.is_empty() {
                if own_norm == target_norm {
                    return true;
                }
                // Also match `org/repo` against folder basename `repo` when one side has no git remote URL.
                let own_tail = own_norm.rsplit('/').next().unwrap_or(&own_norm);
                let target_tail = target_norm.rsplit('/').next().unwrap_or(&target_norm);
                if (!own_norm.contains('/') || !target_norm.contains('/')) && own_tail == target_tail {
                    return true;
                }
            }
        }
        false
    }
}

/// One shared memory entry with monotonic sequence and revision tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedMemoryEntry {
    pub id: String,
    pub scope: MemoryScope,
    pub key: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_inject: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub content_hash: String,
    pub updated_at: String,
    #[serde(default)]
    pub updated_by_device_id: String,
    #[serde(default)]
    pub updated_by_device_name: String,
    #[serde(default)]
    pub deleted: bool,
}

impl SharedMemoryEntry {
    pub fn recompute_hash(&mut self) {
        self.content_hash = compute_entry_hash(self);
    }
}

/// Preserved side-versions when two devices edit the same memory entry concurrently without fast-forward ancestry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConflict {
    pub id: String,
    pub entry_id: String,
    pub key: String,
    pub scope: MemoryScope,
    pub local_version: SharedMemoryEntry,
    pub remote_version: SharedMemoryEntry,
    pub detected_at: String,
}

/// User-chosen resolution strategy for a `MemoryConflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConflictResolution {
    KeepLocal,
    KeepRemote,
    Merged { content: String },
}

/// User-configurable sharing & injection policy persisted inside `shared-memory.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedMemorySettings {
    #[serde(default = "default_true")]
    pub share_project_memory: bool,
    #[serde(default)]
    pub share_user_memory: bool,
    #[serde(default = "default_true")]
    pub auto_inject: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_endpoint_url: Option<String>,
}

impl Default for SharedMemorySettings {
    fn default() -> Self {
        SharedMemorySettings { share_project_memory: true, share_user_memory: false, auto_inject: true, custom_endpoint_url: None }
    }
}

/// Incremental delta returned by `changes_since` and `/v2/memory/changes`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDelta {
    pub server_seq: u64,
    pub entries: Vec<SharedMemoryEntry>,
}

/// Summary of applying incoming remote entries via `apply_incoming`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcome {
    pub applied: usize,
    pub skipped: usize,
    pub conflicts: Vec<MemoryConflict>,
    pub redacted_secrets: usize,
    pub server_seq: u64,
}

/// Input parameters for creating or updating a local shared memory entry.
#[derive(Debug, Clone)]
pub struct UpsertMemoryInput {
    pub id: Option<String>,
    pub scope: MemoryScope,
    pub key: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub auto_inject: bool,
    pub pinned: bool,
}

/// Persisted shared memory book at `<data_dir>/shared-memory.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedMemoryBook {
    #[serde(default = "default_version")]
    pub version: u32,
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub server_seq: u64,
    #[serde(default)]
    pub entries: Vec<SharedMemoryEntry>,
    #[serde(default)]
    pub conflicts: Vec<MemoryConflict>,
    #[serde(default)]
    pub host_cursors: BTreeMap<String, u64>,
    #[serde(default)]
    pub host_base_hashes: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub settings: SharedMemorySettings,
}

impl Default for SharedMemoryBook {
    fn default() -> Self {
        Self::new(crate::wire::new_id(), "Mighty Code")
    }
}

impl SharedMemoryBook {
    pub fn new(device_id: impl Into<String>, device_name: impl Into<String>) -> Self {
        let device_id = device_id.into();
        let device_name = device_name.into();
        SharedMemoryBook {
            version: 1,
            device_id: if device_id.trim().is_empty() { crate::wire::new_id() } else { device_id },
            device_name: if device_name.trim().is_empty() { "Mighty Code".into() } else { device_name },
            server_seq: 0,
            entries: Vec::new(),
            conflicts: Vec::new(),
            host_cursors: BTreeMap::new(),
            host_base_hashes: BTreeMap::new(),
            settings: SharedMemorySettings::default(),
        }
    }

    /// Loads `<data_dir>/shared-memory.json` (falling back to `.bak` on corruption), or initializes a fresh book.
    pub fn load_or_new(data_dir: &Path, default_device_name: &str) -> Self {
        let primary = data_dir.join(SHARED_MEMORY_FILE);
        let backup = data_dir.join(SHARED_MEMORY_BACKUP_FILE);
        for candidate in [&primary, &backup] {
            let Ok(bytes) = fs::read(candidate) else { continue };
            if bytes.is_empty() || crate::store::damaged(&bytes) {
                continue;
            }
            if let Ok(mut book) = serde_json::from_slice::<SharedMemoryBook>(&bytes) {
                book.normalize_loaded(default_device_name);
                return book;
            }
        }
        Self::new(crate::wire::new_id(), default_device_name)
    }

    fn normalize_loaded(&mut self, default_device_name: &str) {
        self.version = 1;
        if self.device_id.trim().is_empty() {
            self.device_id = crate::wire::new_id();
        }
        if self.device_name.trim().is_empty() {
            self.device_name =
                if default_device_name.trim().is_empty() { "Mighty Code".into() } else { default_device_name.trim().to_string() };
        }
        let max_seq = self.entries.iter().map(|e| e.seq).max().unwrap_or(0);
        if self.server_seq < max_seq {
            self.server_seq = max_seq;
        }
        for entry in &mut self.entries {
            if entry.content_hash.is_empty() {
                entry.recompute_hash();
            }
        }
    }

    /// Atomically persists the book to `<data_dir>/shared-memory.json` (`0600` on Unix) with `.bak` backup.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let mut json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        json.push(b'\n');
        let target = data_dir.join(SHARED_MEMORY_FILE);
        let backup = data_dir.join(SHARED_MEMORY_BACKUP_FILE);
        atomic_write_private(&target, Some(&backup), &json)
    }

    fn next_seq(&mut self) -> u64 {
        self.server_seq = self.server_seq.saturating_add(1);
        self.server_seq
    }

    /// Creates or updates a local entry, running `sanitize_secrets` on `title` and `content` and returning `(entry, redacted_count)`.
    pub fn upsert(&mut self, input: UpsertMemoryInput) -> Result<(SharedMemoryEntry, usize), String> {
        let Some(key) = normalize_memory_key(&input.key) else {
            return Err("Memory key must contain at least one letter or digit.".into());
        };
        if input.content.trim().is_empty() {
            return Err("Memory content must not be empty.".into());
        }
        if input.content.len() > MAX_ENTRY_CONTENT_BYTES {
            return Err(format!("Memory content exceeds {MAX_ENTRY_CONTENT_BYTES} bytes."));
        }

        let raw_title = if input.title.trim().is_empty() { key.clone() } else { input.title.trim().to_string() };
        let (clean_title, redacted_title) = sanitize_secrets(&truncate_utf8(&raw_title, 160));
        let (clean_content, redacted_body) = sanitize_secrets(input.content.trim());
        let redacted_total = redacted_title + redacted_body;
        let tags = normalize_tags(&input.tags);
        let now = crate::time::now_o();
        let target_id = input.id.as_deref().filter(|s| !s.trim().is_empty());

        let existing_idx = self.find_entry_index(target_id, &input.scope, &key);
        let seq = self.next_seq();
        let device_id = self.device_id.clone();
        let device_name = self.device_name.clone();

        if let Some(idx) = existing_idx {
            let slot = &mut self.entries[idx];
            slot.scope = input.scope;
            slot.key = key;
            slot.title = clean_title;
            slot.content = clean_content;
            slot.tags = tags;
            slot.auto_inject = input.auto_inject;
            slot.pinned = input.pinned;
            slot.deleted = false;
            slot.revision = slot.revision.saturating_add(1);
            slot.seq = seq;
            slot.updated_at = now;
            slot.updated_by_device_id = device_id;
            slot.updated_by_device_name = device_name;
            slot.recompute_hash();
            return Ok((slot.clone(), redacted_total));
        }

        let mut entry = SharedMemoryEntry {
            id: target_id.map(str::to_string).unwrap_or_else(crate::wire::new_id),
            scope: input.scope,
            key,
            title: clean_title,
            content: clean_content,
            tags,
            auto_inject: input.auto_inject,
            pinned: input.pinned,
            revision: 1,
            seq,
            content_hash: String::new(),
            updated_at: now,
            updated_by_device_id: device_id,
            updated_by_device_name: device_name,
            deleted: false,
        };
        entry.recompute_hash();

        if self.entries.len() >= MAX_MEMORY_ENTRIES {
            if let Some(del_pos) = self.entries.iter().position(|e| e.deleted) {
                self.entries.remove(del_pos);
            } else {
                self.entries.remove(0);
            }
        }
        self.entries.push(entry.clone());
        Ok((entry, redacted_total))
    }

    /// Marks an entry deleted as a tombstone so deletions replicate incrementally across hosts.
    pub fn mark_deleted(&mut self, entry_id: &str) -> Option<SharedMemoryEntry> {
        let idx = self.entries.iter().position(|e| e.id == entry_id)?;
        if self.entries[idx].deleted {
            return Some(self.entries[idx].clone());
        }
        let seq = self.next_seq();
        let now = crate::time::now_o();
        let device_id = self.device_id.clone();
        let device_name = self.device_name.clone();
        let slot = &mut self.entries[idx];
        slot.deleted = true;
        slot.pinned = false;
        slot.content.clear();
        slot.revision = slot.revision.saturating_add(1);
        slot.seq = seq;
        slot.updated_at = now;
        slot.updated_by_device_id = device_id;
        slot.updated_by_device_name = device_name;
        slot.recompute_hash();
        let out = slot.clone();
        self.conflicts.retain(|c| c.entry_id != entry_id);
        Some(out)
    }

    fn find_entry_index(&self, id: Option<&str>, scope: &MemoryScope, key: &str) -> Option<usize> {
        if let Some(id) = id
            && let Some(pos) = self.entries.iter().position(|e| e.id == id)
        {
            return Some(pos);
        }
        self.entries.iter().position(|e| scopes_share_slot(&e.scope, scope) && e.key == key)
    }

    /// Returns incremental changes with `entry.seq > since_seq` filtered by allowed project/user scopes.
    pub fn changes_since(
        &self,
        since_seq: u64,
        allowed_workspace_ids: Option<&HashSet<String>>,
        allowed_slugs: Option<&HashSet<String>>,
        include_user: bool,
    ) -> MemoryDelta {
        let normalized_slugs: Option<HashSet<String>> =
            allowed_slugs.map(|set| set.iter().map(|s| normalize_slug(s)).filter(|s| !s.is_empty()).collect());
        let mut matching: Vec<SharedMemoryEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.seq > since_seq)
            .filter(|entry| match &entry.scope {
                MemoryScope::User => include_user,
                MemoryScope::Project { workspace_id, project_slug } => {
                    if allowed_workspace_ids.is_none() && normalized_slugs.is_none() {
                        return true;
                    }
                    let ws_ok = allowed_workspace_ids.is_some_and(|ids| ids.contains(workspace_id));
                    let slug_norm = normalize_slug(project_slug);
                    let slug_ok = !slug_norm.is_empty() && normalized_slugs.as_ref().is_some_and(|slugs| slugs.contains(&slug_norm));
                    ws_ok || slug_ok
                }
            })
            .cloned()
            .collect();
        matching.sort_by_key(|e| e.seq);
        MemoryDelta { server_seq: self.server_seq, entries: matching }
    }

    /// Applies incoming entries from another host/device with secret sanitization, 3-way fast-forward check,
    /// deterministic `(updated_at, updated_by_device_id)` LWW winner selection, and side-version conflict preservation.
    pub fn apply_incoming(&mut self, incoming: Vec<SharedMemoryEntry>, base_hashes: &BTreeMap<String, String>) -> ApplyOutcome {
        self.apply_incoming_scoped(incoming, base_hashes, &|_| true)
    }

    /// [`SharedMemoryBook::apply_incoming`] for a peer that may only touch the scopes `allowed` accepts: an incoming entry whose own
    /// scope is outside them, or that lands on a local entry (by id or by slot) whose scope is outside them, is skipped, so a peer can
    /// neither overwrite nor tombstone what it was never given. A full book never drops a live local entry to make room for a remote
    /// one (only a tombstone); the entry is skipped.
    pub fn apply_incoming_scoped(
        &mut self,
        incoming: Vec<SharedMemoryEntry>,
        base_hashes: &BTreeMap<String, String>,
        allowed: &dyn Fn(&MemoryScope) -> bool,
    ) -> ApplyOutcome {
        let mut outcome = ApplyOutcome::default();

        for mut remote in incoming {
            if !allowed(&remote.scope) {
                outcome.skipped += 1;
                continue;
            }
            let Some(clean_key) = normalize_memory_key(&remote.key) else {
                outcome.skipped += 1;
                continue;
            };
            remote.key = clean_key;
            let (clean_title, r_title) = sanitize_secrets(&truncate_utf8(&remote.title, 160));
            let (clean_content, r_body) = sanitize_secrets(&truncate_utf8(&remote.content, MAX_ENTRY_CONTENT_BYTES));
            outcome.redacted_secrets += r_title + r_body;
            remote.title = if clean_title.trim().is_empty() { remote.key.clone() } else { clean_title };
            remote.content = if remote.deleted { String::new() } else { clean_content };
            remote.tags = normalize_tags(&remote.tags);
            remote.recompute_hash();

            let existing_idx = self.find_entry_index(Some(&remote.id), &remote.scope, &remote.key);
            if existing_idx.is_some_and(|idx| !allowed(&self.entries[idx].scope)) {
                outcome.skipped += 1;
                continue;
            }
            let Some(idx) = existing_idx else {
                if self.entries.len() >= MAX_MEMORY_ENTRIES {
                    let Some(del_pos) = self.entries.iter().position(|e| e.deleted) else {
                        outcome.skipped += 1;
                        continue;
                    };
                    self.entries.remove(del_pos);
                }
                let seq = self.next_seq();
                remote.seq = seq;
                if remote.revision == 0 {
                    remote.revision = 1;
                }
                self.entries.push(remote);
                outcome.applied += 1;
                continue;
            };

            let local = self.entries[idx].clone();
            if local.content_hash == remote.content_hash {
                outcome.skipped += 1;
                continue;
            }

            let slot_lookup = format!("{}::{}", local.scope.scope_key(), local.key);
            let base_hash = base_hashes
                .get(&remote.id)
                .or_else(|| base_hashes.get(&local.id))
                .or_else(|| base_hashes.get(&slot_lookup))
                .map(String::as_str);

            // Fast-forward when local is still at the shared base hash!
            if base_hash == Some(local.content_hash.as_str()) {
                let seq = self.next_seq();
                let mut next = remote;
                next.id = local.id;
                next.revision = local.revision.max(next.revision).max(1);
                next.seq = seq;
                next.recompute_hash();
                self.entries[idx] = next;
                outcome.applied += 1;
                continue;
            }

            // If incoming matches base_hash, local is strictly ahead of incoming — keep local without false conflict.
            if base_hash == Some(remote.content_hash.as_str()) {
                outcome.skipped += 1;
                continue;
            }

            // Concurrent edit on both devices: deterministic LWW by `(updated_at_utc, updated_by_device_id)` + preserve both versions!
            let remote_wins =
                compare_lww_order(&remote.updated_at, &remote.updated_by_device_id, &local.updated_at, &local.updated_by_device_id)
                    != std::cmp::Ordering::Less;
            let next_rev = local.revision.max(remote.revision).saturating_add(1);
            let seq = self.next_seq();
            let mut winner = if remote_wins { remote.clone() } else { local.clone() };
            winner.id = local.id.clone();
            winner.revision = next_rev;
            winner.seq = seq;
            winner.recompute_hash();
            self.entries[idx] = winner;

            let conflict = MemoryConflict {
                id: crate::wire::new_id(),
                entry_id: local.id.clone(),
                key: local.key.clone(),
                scope: local.scope.clone(),
                local_version: local,
                remote_version: remote,
                detected_at: crate::time::now_o(),
            };
            self.conflicts.retain(|existing| existing.entry_id != conflict.entry_id);
            if self.conflicts.len() >= MAX_CONFLICTS {
                self.conflicts.remove(0);
            }
            self.conflicts.push(conflict.clone());
            outcome.conflicts.push(conflict);
            outcome.applied += 1;
        }

        outcome.server_seq = self.server_seq;
        outcome
    }

    /// Resolves a recorded conflict (`KeepLocal`, `KeepRemote`, or `Merged { content }`), increments `revision` & `seq`,
    /// and removes the conflict record.
    pub fn resolve_conflict(&mut self, conflict_id: &str, resolution: ConflictResolution) -> Result<SharedMemoryEntry, String> {
        let Some(pos) = self.conflicts.iter().position(|c| c.id == conflict_id) else {
            return Err("Conflict record not found.".into());
        };
        let conflict = self.conflicts.remove(pos);
        let max_rev = conflict.local_version.revision.max(conflict.remote_version.revision);

        let chosen = match resolution {
            ConflictResolution::KeepLocal => conflict.local_version,
            ConflictResolution::KeepRemote => conflict.remote_version,
            ConflictResolution::Merged { content } => {
                if content.trim().is_empty() {
                    return Err("Merged memory content must not be empty.".into());
                }
                let (clean, _) = sanitize_secrets(&truncate_utf8(content.trim(), MAX_ENTRY_CONTENT_BYTES));
                let mut merged = conflict.local_version;
                merged.content = clean;
                merged.deleted = false;
                merged
            }
        };

        let seq = self.next_seq();
        let now = crate::time::now_o();
        let device_id = self.device_id.clone();
        let device_name = self.device_name.clone();

        if let Some(idx) = self.find_entry_index(Some(&conflict.entry_id), &conflict.scope, &conflict.key) {
            let slot = &mut self.entries[idx];
            slot.title = chosen.title;
            slot.content = chosen.content;
            slot.tags = chosen.tags;
            slot.auto_inject = chosen.auto_inject;
            slot.pinned = chosen.pinned;
            slot.deleted = chosen.deleted;
            slot.revision = slot.revision.max(max_rev).saturating_add(1);
            slot.seq = seq;
            slot.updated_at = now;
            slot.updated_by_device_id = device_id;
            slot.updated_by_device_name = device_name;
            slot.recompute_hash();
            return Ok(slot.clone());
        }

        let mut restored = chosen;
        restored.id = conflict.entry_id;
        restored.revision = max_rev.saturating_add(1);
        restored.seq = seq;
        restored.updated_at = now;
        restored.updated_by_device_id = device_id;
        restored.updated_by_device_name = device_name;
        restored.recompute_hash();
        self.entries.push(restored.clone());
        Ok(restored)
    }

    /// Formats non-deleted `auto_inject` entries for `(workspace_id, project_slug)` + `User` scope (pinned first, then newest),
    /// strictly bounded to `max_bytes` UTF-8 bytes (`4096` default). Returns `None` when `settings.auto_inject` is false or empty.
    pub fn render_for_run(&self, workspace_id: &str, project_slug: Option<&str>, max_bytes: usize) -> Option<String> {
        if !self.settings.auto_inject || max_bytes == 0 {
            return None;
        }
        render_entries_for_run(&self.entries, workspace_id, project_slug, max_bytes)
    }

    /// Records the latest synced `content_hash` map and sequence cursor for `host_key`.
    pub fn record_host_sync(&mut self, host_key: &str, server_seq: u64, synced_entries: &[SharedMemoryEntry]) {
        self.host_cursors.insert(host_key.to_string(), server_seq);
        let hashes = self.host_base_hashes.entry(host_key.to_string()).or_default();
        for entry in synced_entries {
            hashes.insert(entry.id.clone(), entry.content_hash.clone());
            hashes.insert(format!("{}::{}", entry.scope.scope_key(), entry.key), entry.content_hash.clone());
        }
    }
}

/// Standalone formatter used by both `SharedMemoryBook::render_for_run` and offline `remote-cache` fallback contexts.
pub fn render_entries_for_run(
    entries: &[SharedMemoryEntry],
    workspace_id: &str,
    project_slug: Option<&str>,
    max_bytes: usize,
) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    let mut selected: Vec<&SharedMemoryEntry> = entries
        .iter()
        .filter(|e| !e.deleted && e.auto_inject && !e.content.trim().is_empty())
        .filter(|e| e.scope.is_user() || e.scope.matches_project(workspace_id, project_slug))
        .collect();
    if selected.is_empty() {
        return None;
    }
    selected.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| a.scope.is_user().cmp(&b.scope.is_user()))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut out = String::new();
    for entry in selected {
        let scope_tag = if entry.scope.is_user() { "user" } else { "project" };
        let pin_tag = if entry.pinned { " [pinned]" } else { "" };
        let tags_note = if entry.tags.is_empty() { String::new() } else { format!(" ({})", entry.tags.join(", ")) };
        let bullet_head = if entry.title.eq_ignore_ascii_case(&entry.key) {
            format!("- [{scope_tag}:{}{}]{}: ", entry.key, pin_tag, tags_note)
        } else {
            format!("- [{scope_tag}:{}{}] **{}**{}: ", entry.key, pin_tag, entry.title, tags_note)
        };
        let clean_body = entry.content.trim();
        let line = format!("{bullet_head}{clean_body}\n");
        if out.len() + line.len() <= max_bytes {
            out.push_str(&line);
        } else if out.is_empty() {
            // Fit at least a truncated slice of the highest-priority entry within `max_bytes`.
            let room = max_bytes.saturating_sub(bullet_head.len() + 4);
            if room > 0 {
                out.push_str(&bullet_head);
                out.push_str(&truncate_utf8(clean_body, room));
                out.push_str("...\n");
            }
            break;
        } else {
            break;
        }
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

fn compare_lww_order(a_time: &str, a_dev: &str, b_time: &str, b_dev: &str) -> std::cmp::Ordering {
    let time_ord = match (chrono::DateTime::parse_from_rfc3339(a_time), chrono::DateTime::parse_from_rfc3339(b_time)) {
        (Ok(ta), Ok(tb)) => ta.with_timezone(&chrono::Utc).cmp(&tb.with_timezone(&chrono::Utc)),
        _ => a_time.cmp(b_time),
    };
    time_ord.then_with(|| a_dev.cmp(b_dev))
}

fn scopes_share_slot(a: &MemoryScope, b: &MemoryScope) -> bool {
    match (a, b) {
        (MemoryScope::User, MemoryScope::User) => true,
        (MemoryScope::Project { workspace_id: wa, project_slug: sa }, MemoryScope::Project { workspace_id: wb, project_slug: sb }) => {
            if !wa.is_empty() && wa == wb {
                return true;
            }
            let na = normalize_slug(sa);
            let nb = normalize_slug(sb);
            !na.is_empty() && na == nb
        }
        _ => false,
    }
}

/// Normalizes a memory key slug (`[a-z0-9._-]`, up to 64 chars).
pub fn normalize_memory_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len().min(64));
    let mut prev_dash = false;
    for ch in trimmed.chars() {
        if out.len() >= 64 {
            break;
        }
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || matches!(lower, '_' | '.') {
            out.push(lower);
            prev_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '/' || ch == ':') && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let cleaned = out.trim_matches(['-', '.', '_']).to_string();
    if cleaned.bytes().any(|b| b.is_ascii_alphanumeric()) { Some(cleaned) } else { None }
}

pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in tags {
        for part in raw.split(',') {
            let tag = part.trim().trim_start_matches('#').to_ascii_lowercase();
            let clean: String = tag
                .chars()
                .map(|c| if c.is_whitespace() { '-' } else { c })
                .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_' | '.' | '/'))
                .take(32)
                .collect();
            if !clean.is_empty() && seen.insert(clean.clone()) {
                out.push(clean);
                if out.len() >= 16 {
                    return out;
                }
            }
        }
    }
    out
}

/// Normalizes a git remote URL or project slug into a canonical lowercase identifier (e.g. `"org/repo"` or `"repo"`).
pub fn normalize_slug(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(['/', '\\']);
    if trimmed.is_empty() {
        return String::new();
    }
    let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    // Parse SSH `git@host:org/repo` or URL `https://host/org/repo`.
    let path_part = if let Some(after_scheme) = without_git.split_once("://").map(|(_, rest)| rest) {
        after_scheme.split_once('/').map(|(_, p)| p).unwrap_or(after_scheme)
    } else if let Some((left, right)) = without_git.split_once(':')
        && left.contains('@')
        && !right.contains('\\')
    {
        right
    } else {
        without_git
    };
    let normalized = path_part.replace('\\', "/");
    let parts: Vec<String> = normalized
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            seg.chars()
                .map(|c| if c.is_whitespace() { '-' } else { c.to_ascii_lowercase() })
                .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_' | '.'))
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() >= 2 {
        format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        parts.into_iter().next().unwrap_or_default()
    }
}

/// [`infer_project_slug`] for a workspace of another computer (a mirrored or remote workspace): its folder is not on this disk, so
/// nothing is read; the slug is its folder's basename, or `workspace_name`.
pub fn infer_remote_project_slug(workspace_path: &str, workspace_name: &str) -> String {
    let base = workspace_path.trim().trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("");
    let base_slug = normalize_slug(base);
    if base_slug.is_empty() { normalize_slug(workspace_name) } else { base_slug }
}

/// Infers `project_slug` from `<workspace_path>/.git/config` remote origin URL (without spawning any subprocess!),
/// falling back to folder basename or `workspace_name`.
pub fn infer_project_slug(workspace_path: &str, workspace_name: &str) -> String {
    let trimmed_path = workspace_path.trim();
    if !trimmed_path.is_empty() {
        let root = Path::new(trimmed_path);
        if let Some(origin_url) = read_git_origin_url(root) {
            let slug = normalize_slug(&origin_url);
            if !slug.is_empty() {
                return slug;
            }
        }
        let base = trimmed_path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("");
        let base_slug = normalize_slug(base);
        if !base_slug.is_empty() {
            return base_slug;
        }
    }
    normalize_slug(workspace_name)
}

fn read_git_origin_url(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        let pointer = fs::read_to_string(&dot_git).ok()?;
        let raw_dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        let resolved = Path::new(raw_dir);
        let worktree_git = if resolved.is_absolute() { resolved.to_path_buf() } else { root.join(resolved) };
        if let Ok(common) = fs::read_to_string(worktree_git.join("commondir")) {
            let rel = common.trim();
            let common_path = Path::new(rel);
            if common_path.is_absolute() { common_path.to_path_buf() } else { worktree_git.join(common_path) }
        } else {
            worktree_git
        }
    } else {
        return None;
    };

    let config_text = fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    let mut fallback_url: Option<String> = None;
    let mut in_any_remote = false;
    for raw_line in config_text.lines().take(2000) {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let section = line[1..line.len() - 1].trim();
            in_origin = section.eq_ignore_ascii_case("remote \"origin\"");
            in_any_remote = section.to_ascii_lowercase().starts_with("remote ");
            continue;
        }
        if (in_origin || in_any_remote)
            && let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case("url")
        {
            let value = v.trim().trim_matches('"').trim().to_string();
            if !value.is_empty() {
                if in_origin {
                    return Some(value);
                }
                if fallback_url.is_none() {
                    fallback_url = Some(value);
                }
            }
        }
    }
    fallback_url
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

// ── Secret Sanitization Guard ────────────────────────────────────────────────────────────────────────────────────────────

/// Scans `text` for credentials, API tokens, PEM private keys, `Bearer` headers, and `.env` secret assignments,
/// replacing each secret with `[REDACTED_SECRET]` and returning `(sanitized_text, redacted_count)`.
pub fn sanitize_secrets(text: &str) -> (String, usize) {
    if text.is_empty() {
        return (String::new(), 0);
    }
    let mut redacted = 0usize;

    // Pass 1: PEM `-----BEGIN ... PRIVATE KEY-----` blocks (multiline or single-line).
    let after_pem = redact_pem_blocks(text, &mut redacted);

    // Pass 2: Line-by-line `.env` `*_SECRET=`, `*_TOKEN=`, `*_API_KEY=`, `*_PASSWORD=` assignments.
    let after_env = redact_env_assignments(&after_pem, &mut redacted);

    // Pass 3: `Bearer <token>` authorization values.
    let after_bearer = redact_bearer_tokens(&after_env, &mut redacted);

    // Pass 4: Known provider / cloud / SaaS key prefixes (`AKIA`, `ASIA`, `ghp_`, `github_pat_`, `glpat-`, `xox[abprs]-`,
    // `sk-ant-`, `sk_live_`, `sk-`, `AIza`).
    let final_text = redact_prefixed_tokens(&after_bearer, &mut redacted);

    (final_text, redacted)
}

fn redact_pem_blocks(text: &str, count: &mut usize) -> String {
    const BEGIN: &str = "-----BEGIN ";
    const PRIV_SUFFIX: &str = "PRIVATE KEY-----";
    if !text.contains(BEGIN) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(rel_start) = text[cursor..].find(BEGIN) {
        let abs_start = cursor + rel_start;
        let after_begin = abs_start + BEGIN.len();
        let line_end = text[after_begin..].find('\n').map_or(text.len(), |i| after_begin + i);
        let header_slice = &text[after_begin..line_end];
        let Some(priv_pos) = header_slice.find(PRIV_SUFFIX) else {
            out.push_str(&text[cursor..after_begin]);
            cursor = after_begin;
            continue;
        };
        out.push_str(&text[cursor..abs_start]);
        let header_end = after_begin + priv_pos + PRIV_SUFFIX.len();
        // Find matching `-----END ... PRIVATE KEY-----` if present.
        let mut end_cursor = header_end;
        if let Some(rel_end_tag) = text[header_end..].find("-----END ") {
            let end_tag_start = header_end + rel_end_tag;
            let end_line_limit = text[end_tag_start..].find('\n').map_or(text.len(), |i| end_tag_start + i);
            if let Some(end_priv) = text[end_tag_start..end_line_limit].find(PRIV_SUFFIX) {
                end_cursor = end_tag_start + end_priv + PRIV_SUFFIX.len();
            } else {
                end_cursor = end_line_limit;
            }
        }
        out.push_str(REDACTED_SECRET);
        *count += 1;
        cursor = end_cursor;
    }
    out.push_str(&text[cursor..]);
    out
}

fn redact_env_assignments(text: &str, count: &mut usize) -> String {
    let mut out = String::with_capacity(text.len());
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let (clean_line, has_cr) = match line.strip_suffix('\r') {
            Some(stripped) => (stripped, true),
            None => (line, false),
        };
        if let Some(replaced) = redact_single_env_line(clean_line) {
            out.push_str(&replaced);
            *count += 1;
        } else {
            out.push_str(clean_line);
        }
        if has_cr {
            out.push('\r');
        }
    }
    out
}

fn redact_single_env_line(line: &str) -> Option<String> {
    let trimmed_start = line.trim_start();
    if trimmed_start.starts_with('#') || trimmed_start.starts_with("//") {
        return None;
    }
    let lead_len = line.len() - trimmed_start.len();
    let after_export = if let Some(rest) = trimmed_start.strip_prefix("export ") { rest.trim_start() } else { trimmed_start };
    let eq_rel = after_export.find('=')?;
    let key_raw = after_export[..eq_rel].trim();
    if key_raw.is_empty() || !key_raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let upper = key_raw.to_ascii_uppercase();
    let sensitive_key = upper == "SECRET"
        || upper == "TOKEN"
        || upper == "API_KEY"
        || upper == "APIKEY"
        || upper == "PASSWORD"
        || upper == "PASSWD"
        || upper.ends_with("_SECRET")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_API_KEY")
        || upper.ends_with("_APIKEY")
        || upper.ends_with("_PASSWORD")
        || upper.ends_with("_PRIVATE_KEY");
    if !sensitive_key {
        return None;
    }
    let prefix_len = line.len() - after_export.len() + eq_rel + 1;
    let raw_val = line[prefix_len..].trim();
    if raw_val.is_empty() {
        return None;
    }
    // Strip inline comment if unquoted.
    let val_part = if (raw_val.starts_with('"') && raw_val.ends_with('"') && raw_val.len() >= 2)
        || (raw_val.starts_with('\'') && raw_val.ends_with('\'') && raw_val.len() >= 2)
    {
        &raw_val[1..raw_val.len() - 1]
    } else {
        raw_val.split(" #").next().unwrap_or(raw_val).trim()
    };
    if is_placeholder_secret(val_part) {
        return None;
    }
    let _ = lead_len;
    Some(format!("{}{REDACTED_SECRET}", &line[..prefix_len]))
}

fn is_placeholder_secret(val: &str) -> bool {
    let trimmed = val.trim();
    if trimmed.is_empty() || trimmed == REDACTED_SECRET {
        return true;
    }
    if (trimmed.starts_with('<') && trimmed.ends_with('>')) || (trimmed.starts_with("${") && trimmed.ends_with('}')) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    matches!(lower.as_str(), "changeme" | "placeholder" | "none" | "null" | "true" | "false" | "example" | "redacted" | "xxx" | "todo")
        || lower.starts_with("your_")
        || lower.starts_with("your-")
}

fn redact_bearer_tokens(text: &str, count: &mut usize) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut copied = 0usize;

    while i + 7 < bytes.len() {
        let boundary_before = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if boundary_before && text.get(i..i + 6).is_some_and(|s| s.eq_ignore_ascii_case("bearer")) {
            let after_word = i + 6;
            if matches!(bytes[after_word], b' ' | b'\t') {
                let mut ws_end = after_word;
                while ws_end < bytes.len() && matches!(bytes[ws_end], b' ' | b'\t') {
                    ws_end += 1;
                }
                if text[ws_end..].starts_with(REDACTED_SECRET) {
                    i = ws_end + REDACTED_SECRET.len();
                    continue;
                }
                let mut tok_end = ws_end;
                while tok_end < bytes.len() && is_bearer_token_byte(bytes[tok_end]) {
                    tok_end += 1;
                }
                // Trim trailing sentence punctuation (`.`, `,`, `;`) that isn't part of a JWT/token.
                while tok_end > ws_end && matches!(bytes[tok_end - 1], b'.' | b'=') && tok_end - ws_end < 16 {
                    tok_end -= 1;
                }
                let token = &text[ws_end..tok_end];
                if token.len() >= 12 && !is_placeholder_secret(token) {
                    out.push_str(&text[copied..ws_end]);
                    out.push_str(REDACTED_SECRET);
                    *count += 1;
                    i = tok_end;
                    copied = tok_end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

fn is_bearer_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
}

fn redact_prefixed_tokens(text: &str, count: &mut usize) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    let mut copied = 0usize;

    while i < bytes.len() {
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if boundary && let Some(end) = match_secret_token_at(text, i) {
            out.push_str(&text[copied..i]);
            out.push_str(REDACTED_SECRET);
            *count += 1;
            i = end;
            copied = end;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

fn match_secret_token_at(text: &str, start: usize) -> Option<usize> {
    let rest = text.get(start..)?;
    let bytes = rest.as_bytes();

    // 1. AWS Access Key ID: `AKIA` or `ASIA` + 16 uppercase alphanumeric chars (exact total = 20).
    if (rest.starts_with("AKIA") || rest.starts_with("ASIA")) && bytes.len() >= 20 {
        let body_ok = bytes[4..20].iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        let end_boundary = bytes.get(20).is_none_or(|b| !b.is_ascii_alphanumeric());
        if body_ok && end_boundary {
            return Some(start + 20);
        }
    }

    // 2. GitHub fine-grained PAT: `github_pat_` + >= 20 `[A-Za-z0-9_]`.
    if rest.starts_with("github_pat_") {
        let len = take_while_ascii(&bytes[11..], |b| b.is_ascii_alphanumeric() || b == b'_');
        if len >= 20 {
            return Some(start + 11 + len);
        }
    }

    // 3. GitHub classic tokens: `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` + >= 20 `[A-Za-z0-9]`.
    if matches!(rest.get(..4), Some("ghp_" | "gho_" | "ghu_" | "ghs_" | "ghr_")) {
        let len = take_while_ascii(&bytes[4..], |b| b.is_ascii_alphanumeric());
        if len >= 20 {
            return Some(start + 4 + len);
        }
    }

    // 4. GitLab PAT: `glpat-` + >= 16 `[A-Za-z0-9_-]`.
    if rest.starts_with("glpat-") {
        let len = take_while_ascii(&bytes[6..], |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if len >= 16 {
            return Some(start + 6 + len);
        }
    }

    // 5. Slack token: `xox[abprs]-` + >= 10 `[A-Za-z0-9-]`.
    if bytes.len() >= 15 && rest.starts_with("xox") && matches!(bytes[3], b'a' | b'b' | b'p' | b'r' | b's') && bytes[4] == b'-' {
        let len = take_while_ascii(&bytes[5..], |b| b.is_ascii_alphanumeric() || b == b'-');
        if len >= 10 {
            return Some(start + 5 + len);
        }
    }

    // 6. Anthropic API key: `sk-ant-` + >= 16 `[A-Za-z0-9_-]`.
    if rest.starts_with("sk-ant-") {
        let len = take_while_ascii(&bytes[7..], |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if len >= 16 {
            return Some(start + 7 + len);
        }
    }

    // 7. Stripe secret key: `sk_live_` or `rk_live_` + >= 16 `[A-Za-z0-9]`.
    if rest.starts_with("sk_live_") || rest.starts_with("rk_live_") {
        let len = take_while_ascii(&bytes[8..], |b| b.is_ascii_alphanumeric());
        if len >= 16 {
            return Some(start + 8 + len);
        }
    }

    // 8. OpenAI API key: `sk-` + >= 16 `[A-Za-z0-9_-]`.
    if rest.starts_with("sk-") {
        let len = take_while_ascii(&bytes[3..], |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if len >= 16 {
            return Some(start + 3 + len);
        }
    }

    // 9. Google API key: `AIza` + >= 28 `[A-Za-z0-9_-]`.
    if rest.starts_with("AIza") {
        let len = take_while_ascii(&bytes[4..], |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if len >= 28 {
            return Some(start + 4 + len);
        }
    }

    None
}

fn take_while_ascii(bytes: &[u8], pred: impl Fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|b| pred(**b)).count()
}

// ── `remote-cache/<connection_id>/` Offline Fallback Helpers ─────────────────────────────────────────────────────────────

/// Cached workspace summary from a remote host (`<data_dir>/remote-cache/<connection_id>/state.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedRemoteWorkspace {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub project_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Recent transcript line kept inside `CachedRemoteSessionDigest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSessionLine {
    pub kind: String,
    pub text: String,
}

/// Cached session digest for offline `mighty-workspace` (`MC-14`) lookup & activity inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedRemoteSessionDigest {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub body_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub pending: i64,
    #[serde(default)]
    pub queued: i64,
    #[serde(default)]
    pub tail: Vec<CachedSessionLine>,
}

/// Persisted offline snapshot for a remote connection under `<data_dir>/remote-cache/<connection_id>/state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedRemoteState {
    pub connection_id: String,
    pub host_name: String,
    pub cached_at: String,
    #[serde(default)]
    pub workspaces: Vec<CachedRemoteWorkspace>,
    #[serde(default)]
    pub sessions: Vec<CachedRemoteSessionDigest>,
    #[serde(default)]
    pub memory_entries: Vec<SharedMemoryEntry>,
}

pub fn safe_connection_dir_name(connection_id: &str) -> String {
    let trimmed = connection_id.trim();
    if !trimmed.is_empty() && trimmed.len() <= 96 && trimmed.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return trimmed.to_string();
    }
    format!("conn-{}", &sha256_hex(trimmed.as_bytes())[..24])
}

pub fn remote_cache_state_path(data_dir: &Path, connection_id: &str) -> PathBuf {
    data_dir.join(REMOTE_CACHE_DIR).join(safe_connection_dir_name(connection_id)).join(REMOTE_CACHE_STATE_FILE)
}

/// Atomically writes `<data_dir>/remote-cache/<connection_id>/state.json` (`0600` on Unix) after sanitizing any memory secrets.
pub fn write_cached_remote_state(data_dir: &Path, mut state: CachedRemoteState) -> std::io::Result<()> {
    for entry in &mut state.memory_entries {
        let (clean_title, _) = sanitize_secrets(&entry.title);
        let (clean_content, _) = sanitize_secrets(&entry.content);
        entry.title = clean_title;
        entry.content = clean_content;
        if entry.content_hash.is_empty() {
            entry.recompute_hash();
        }
    }
    if state.cached_at.trim().is_empty() {
        state.cached_at = crate::time::now_o();
    }
    let path = remote_cache_state_path(data_dir, &state.connection_id);
    let mut json = serde_json::to_vec_pretty(&state).map_err(std::io::Error::other)?;
    json.push(b'\n');
    atomic_write_private(&path, None, &json)
}

/// Reads `<data_dir>/remote-cache/<connection_id>/state.json`, returning `None` if absent or unreadable.
pub fn read_cached_remote_state(data_dir: &Path, connection_id: &str) -> Option<CachedRemoteState> {
    let path = remote_cache_state_path(data_dir, connection_id);
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || crate::store::damaged(&bytes) {
        return None;
    }
    serde_json::from_slice::<CachedRemoteState>(&bytes).ok()
}

/// Lists all valid `CachedRemoteState` records stored under `<data_dir>/remote-cache/*`.
pub fn list_cached_remote_states(data_dir: &Path) -> Vec<CachedRemoteState> {
    let root = data_dir.join(REMOTE_CACHE_DIR);
    let Ok(read) = fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    for item in read.flatten() {
        let Ok(ft) = item.file_type() else { continue };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let state_file = item.path().join(REMOTE_CACHE_STATE_FILE);
        let Ok(bytes) = fs::read(&state_file) else { continue };
        if bytes.is_empty() || crate::store::damaged(&bytes) {
            continue;
        }
        if let Ok(parsed) = serde_json::from_slice::<CachedRemoteState>(&bytes) {
            out.push(parsed);
        }
    }
    out.sort_by(|a, b| a.connection_id.cmp(&b.connection_id));
    out
}

// ── Private Atomic File Writer & Pure-Rust SHA-256 ──────────────────────────────────────────────────────────────────────

fn atomic_write_private(target: &Path, backup: Option<&Path>, bytes: &[u8]) -> std::io::Result<()> {
    let Some(parent) = target.parent() else {
        return Err(std::io::Error::other("missing parent directory"));
    };
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if parent.file_name().and_then(|n| n.to_str()) == Some(REMOTE_CACHE_DIR)
            || parent.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some(REMOTE_CACHE_DIR)
        {
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let file_name = target.file_name().and_then(|n| n.to_str()).unwrap_or("state.json");
    let nonce = crate::wire::new_id();
    let temp = parent.join(format!("{file_name}.{}.{nonce}.tmp", std::process::id()));

    let write_res = (|| -> std::io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = opts.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }

    if let Some(bak) = backup
        && target.is_file()
    {
        let _ = fs::copy(target, bak);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(bak, fs::Permissions::from_mode(0o600));
        }
    }

    if let Err(err) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fs::set_permissions(target, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Deterministic SHA-256 content hash for a `SharedMemoryEntry`.
pub fn compute_entry_hash(entry: &SharedMemoryEntry) -> String {
    let mut sorted_tags = entry.tags.clone();
    sorted_tags.sort();
    let canonical = serde_json::json!({
        "scope": entry.scope.scope_key(),
        "key": entry.key,
        "title": entry.title,
        "content": entry.content,
        "tags": sorted_tags,
        "autoInject": entry.auto_inject,
        "pinned": entry.pinned,
        "deleted": entry.deleted,
    });
    sha256_hex(canonical.to_string().as_bytes())
}

/// Pure-Rust FIPS 180-4 SHA-256 hex digest (zero external crate dependencies).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
        0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
        0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
        0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
        0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut hex = String::with_capacity(64);
    for word in h {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{word:08x}");
    }
    hex
}

//! MC-42: the Windows sandbox of a workspace's agent runs, as pure logic (the plan, the access-list texts, the approval record, what
//! is enforced). The Windows calls that carry it out are `mighty-platform-windows`'s `sandbox` (labels and access lists) and its
//! process host (the token); this module decides what they do, so every rule is checked on any OS.
//!
//! The decided design (docs/sandbox.md, "Windows"): no administrator rights, so no AppContainer loopback exemption, no firewall rule,
//! no second user. A sandboxed CLI starts with a **low-integrity, write-restricted** copy of the person's own token (all privileges
//! but traverse removed) inside a kill-on-close job with UI limits:
//!
//! - **Writes** need both walls: Windows' integrity check (a low process writes only objects labelled low) and the write-restricted
//!   check (one of the token's restricting SIDs — the workspace's synthetic SID, the agent's synthetic SID, Everyone, the logon
//!   session — must be granted the write too). The run folder and the agent's own folders get a low label and an allow entry for the
//!   synthetic SID; git's `config`, `hooks`, `info` and the instruction files get a deny entry for the workspace SID (and an explicit
//!   medium label as a second wall). The grant is "modify" without `FILE_DELETE_CHILD`, so nothing is deleted through its folder.
//! - **Secret folders** get a medium label with no-read-up: the person's own (medium) programs read them as before, a low process
//!   cannot. That is a label change on the person's folders, so it is listed and approved first; browser profiles are left alone
//!   (their own sandboxes run at low integrity and would break).
//! - **Network**: the allow-list proxy on 127.0.0.1 is reachable (a low process may use loopback; an AppContainer may not without an
//!   administrator's exemption), but a program that ignores `HTTPS_PROXY` connects directly: the allow list is **advisory** here.
//! - Every change to the person's folders is approved first, recorded with what it replaced ([`Record`]) and undone when the sandbox
//!   is turned off for the workspace (shared ones when no workspace uses them any more).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::profile;

/// Everyone: in the restricting list, as Codex's unelevated Windows sandbox has it (named objects, devices any user may write).
pub const EVERYONE: &str = "S-1-1-0";
/// "Modify" without `WRITE_DAC`, `WRITE_OWNER` and `FILE_DELETE_CHILD`: read, write, execute, delete (`0x1301bf`).
pub const MODIFY: u32 = 0x0013_01bf;
/// What a deny entry refuses: write data / add file, append / add folder, write EA, delete child, write attributes, delete,
/// `WRITE_DAC`, `WRITE_OWNER`.
pub const DENY_WRITE: u32 = 0x000d_0156;
/// `DELETE` alone (a git folder cannot be renamed away).
pub const DELETE: u32 = 0x0001_0000;

/// Secret places closed to a sandboxed run (relative to home, `/`-separated): labelled medium with no-read-up when they exist.
pub const SECRET_PLACES: [&str; 15] = [
    ".ssh",
    ".aws",
    ".gnupg",
    "AppData/Roaming/gnupg",
    ".config/gh",
    "AppData/Roaming/GitHub CLI",
    ".config/gcloud",
    "AppData/Roaming/gcloud",
    ".azure",
    ".kube",
    ".docker",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".password-store",
];

/// The folders searched for instruction files are many; these are skipped (their own files, never the project's instructions).
const SKIPPED_FOLDERS: [&str; 2] = [".git", "node_modules"];
/// The most folder entries looked at for instruction files (a larger tree keeps what was found so far).
pub const SCAN_LIMIT: usize = 20_000;

/// The agents the Windows sandbox runs (Hermes is refused: [`cli_refusal`]).
pub const AGENTS: [&str; 5] = ["claude", "codex", "gemini", "opencode", "pi"];

/// A synthetic SID (`S-1-5-21-a-b-c-d`, four u32 of a SHA-256 of the name; the shape OpenAI's Codex gives its sandbox's SIDs): no
/// account has it, only the restricting list of a sandboxed run's token and the access entries made for it. Not a capability SID
/// (`S-1-15-3-…`): those belong to AppContainer (lowbox) tokens, and the run's token could not be made with one in its restricting
/// list (ERROR_INVALID_PARAMETER, 0.6.0's Windows CI); a plain `S-1-5-21` SID is what a write-restricted token is known to take.
pub fn synthetic_sid(kind: &str, value: &str) -> String {
    let digest = Sha256::digest(format!("MightyCode.Sandbox/{kind}/{value}").as_bytes());
    let parts: Vec<String> = digest.as_chunks::<4>().0.iter().take(4).map(|c| u32::from_le_bytes(*c).to_string()).collect();
    format!("S-1-5-21-{}", parts.join("-"))
}

/// The SID that may write a workspace's folders (and its pane worktrees, and its private temporary folder).
pub fn workspace_sid(workspace_id: &str) -> String {
    synthetic_sid("workspace", workspace_id)
}

/// The SID that may write an agent's own folders (one per agent, so one agent's run cannot write another's settings).
pub fn agent_sid(provider: &str) -> String {
    synthetic_sid("agent", provider)
}

/// A short name of the workspace for its private folder.
pub fn workspace_key(workspace_id: &str) -> String {
    Sha256::digest(workspace_id.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The restricting SIDs of a run's token (the platform adds the logon session's).
pub fn restricting_sids(workspace_id: &str, provider: &str) -> Vec<String> {
    vec![workspace_sid(workspace_id), agent_sid(provider), EVERYONE.to_string()]
}

/// Each agent's own folders on Windows (relative to home): low-labelled and writable for that agent's SID only.
pub fn state_folders(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &[".claude"],
        "codex" => &[".codex"],
        "gemini" => &[".gemini"],
        "opencode" => &[".config/opencode", ".local/share/opencode", ".local/state/opencode", ".cache/opencode"],
        "pi" => &[".pi"],
        _ => &[],
    }
}

/// Each agent's own files beside its folders (relative to home).
pub fn state_files(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &[".claude.json"],
        _ => &[],
    }
}

/// Why an agent cannot run sandboxed on Windows (a locale key), or None.
///
/// Hermes's installer keeps its program, its Python and its state in one folder (`%LOCALAPPDATA%\hermes`, `HERMES_HOME`): opening
/// its state for writing would let a sandboxed run rewrite the program its next, unsandboxed run starts.
pub fn cli_refusal(provider: &str) -> Option<&'static str> {
    match provider {
        p if AGENTS.contains(&p) => None,
        "hermes" => Some("sandbox.windows.hermes"),
        _ => Some("sandbox.windows.unknownAgent"),
    }
}

/// Whether `program` lies in a folder the sandbox makes writable (the run could change the program a later unsandboxed run starts).
pub fn program_inside(program: &Path, writable: &[PathBuf]) -> bool {
    let program = fold(program);
    writable.iter().any(|w| inside(&program, &fold(w)))
}

/// What a change does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    /// Integrity label low (a low process may write it).
    LowLabel,
    /// Integrity label medium with no-read-up and no-write-up (a low process may neither read nor write it).
    SecretLabel,
    /// Integrity label medium with no-write-up (the second wall of a read-only file inside a writable folder).
    ReadOnlyLabel,
    /// An allow entry for the SID: modify.
    Allow,
    /// A deny entry for the SID: every write, delete and permission change.
    Deny,
    /// A deny entry for the SID: delete only (the folder itself, not inherited).
    DenyDelete,
}

/// Why a change is made (how the approval list groups it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// The workspace folder (writable).
    Workspace,
    /// A pane worktree of the workspace (writable).
    Worktree,
    /// git's config, hooks and info (read-only).
    Git,
    /// AGENTS.md, CLAUDE.md, .mcp.json… (read-only).
    Instructions,
    /// The person's own read-only paths.
    ReadOnly,
    /// An agent's own folder or file (writable for that agent).
    Agent,
    /// A secret folder (closed).
    Secret,
    /// The app's data folder (closed). The app's own.
    AppData,
    /// The workspace's private temporary and cache folder under `LocalLow`. The app's own.
    Private,
}

/// One change to one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: PathBuf,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    /// Inherited by everything under a folder (and propagated to what is already there).
    pub tree: bool,
    pub kind: Kind,
    /// A folder made first when it is missing (an agent's folder).
    #[serde(default)]
    pub create: bool,
    /// The agent whose folder it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

impl Change {
    fn new(path: PathBuf, op: Op, sid: Option<&str>, tree: bool, kind: Kind) -> Change {
        Change { path, op, sid: sid.map(str::to_string), tree, kind, create: false, agent: None }
    }

    /// The change's key in the record (paths compared ignoring case, as Windows does).
    pub fn id(&self) -> String {
        format!("{:?}|{}|{}", self.op, self.sid.as_deref().unwrap_or(""), fold(&self.path).to_string_lossy())
    }

    /// Used by several workspaces (an agent's folder, a secret place, the app's data): undone when none uses it.
    pub fn shared(&self) -> bool {
        matches!(self.kind, Kind::Agent | Kind::Secret | Kind::AppData)
    }

    /// The app's own folders: made without asking (they are still listed).
    pub fn own(&self) -> bool {
        matches!(self.kind, Kind::AppData | Kind::Private)
    }

    /// Whether it is a label change (else an access-list entry).
    pub fn is_label(&self) -> bool {
        matches!(self.op, Op::LowLabel | Op::SecretLabel | Op::ReadOnlyLabel)
    }

    /// The entry in SDDL: `(ML;OICI;NW;;;LW)`, `(A;OICI;0x1301bf;;;S-…)`.
    pub fn ace(&self) -> String {
        let inherit = if self.tree { "OICI" } else { "" };
        let sid = self.sid.as_deref().unwrap_or("");
        match self.op {
            Op::LowLabel => format!("(ML;{inherit};NW;;;LW)"),
            Op::SecretLabel => format!("(ML;{inherit};NRNW;;;ME)"),
            Op::ReadOnlyLabel => format!("(ML;{inherit};NW;;;ME)"),
            Op::Allow => format!("(A;{inherit};0x{MODIFY:x};;;{sid})"),
            Op::Deny => format!("(D;{inherit};0x{DENY_WRITE:x};;;{sid})"),
            Op::DenyDelete => format!("(D;;0x{DELETE:x};;;{sid})"),
        }
    }
}

/// `path` lowercased (Windows compares paths ignoring case).
fn fold(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// `path` is `root` or under it (both already folded, compared by components).
fn inside(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// A path without Windows' `\\?\` (`\\?\UNC\server` → `\\server`), as people and access-list tools write it.
pub fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// `path` as the file system names it (links followed), plain.
fn real(path: &Path) -> Option<PathBuf> {
    profile::canonical(path).map(|p| plain(&p))
}

/// `base` joined with a `/`-separated relative path, piece by piece (right on every OS).
pub fn under(base: &Path, relative: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for piece in relative.split('/').filter(|p| !p.is_empty()) {
        out.push(piece);
    }
    out
}

/// A network path (`\\server\share`): labels are not kept there, so it cannot be sandboxed.
pub fn network_path(path: &Path) -> bool {
    let text = plain(path).to_string_lossy().replace('/', "\\");
    text.starts_with(r"\\")
}

/// What one plan is made from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanInput {
    pub workspace_id: String,
    /// The run's agent; None for the approval list (every agent of [`AGENTS`]).
    pub provider: Option<String>,
    pub home: PathBuf,
    /// The workspace's folder (what the person approves).
    pub workspace_folder: PathBuf,
    /// The run's folder: the workspace's, or one of its pane worktrees.
    pub run_folder: PathBuf,
    /// The app's data folder (closed).
    pub data_dir: Option<PathBuf>,
    /// The workspace's private folder (`%USERPROFILE%\AppData\LocalLow\MightyCode\sandbox\<key>`): temporary files and caches.
    pub private_dir: PathBuf,
    /// The agent's own folders named by its environment (`CODEX_HOME`).
    pub state_overrides: Vec<PathBuf>,
    pub protect_instructions: bool,
    /// More read-only paths, relative to the run folder.
    pub read_only: Vec<String>,
}

/// What a run (or an approval) needs changed, in the order it is done.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub workspace_folder: PathBuf,
    pub run_folder: PathBuf,
    /// The run folder is a worktree of the workspace's repository (so it is within the approval).
    pub worktree: bool,
}

impl Plan {
    /// The folders the run may write (for [`program_inside`]).
    pub fn writable(&self) -> Vec<PathBuf> {
        self.changes.iter().filter(|c| c.op == Op::Allow).map(|c| c.path.clone()).collect()
    }
}

/// Why a plan cannot be made (the run is refused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The instruction files under `folder` (any case, in any folder; `.git` and `node_modules` skipped; at most [`SCAN_LIMIT`]
/// entries looked at).
pub fn instruction_files(folder: &Path) -> Vec<PathBuf> {
    let names: Vec<String> = profile::INSTRUCTION_NAMES.iter().map(|n| n.to_lowercase()).collect();
    let mut found = Vec::new();
    let mut queue = vec![folder.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > SCAN_LIMIT {
                found.sort();
                return found;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !SKIPPED_FOLDERS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                    queue.push(entry.path());
                }
            } else if kind.is_file() && names.contains(&name.to_lowercase()) {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

/// The plan for `input` (Err: the folder cannot be sandboxed).
pub fn plan(input: &PlanInput) -> Result<Plan, PlanError> {
    let home = real(&input.home).ok_or_else(|| PlanError("no home folder".into()))?;
    let workspace = real(&input.workspace_folder).ok_or_else(|| PlanError("the workspace folder is missing".into()))?;
    let run = real(&input.run_folder).ok_or_else(|| PlanError("the run folder is missing".into()))?;
    let data = input.data_dir.as_deref().and_then(real);
    let (fhome, fdata) = (fold(&home), data.as_deref().map(fold));
    for folder in [&workspace, &run] {
        let f = fold(folder);
        if folder.parent().is_none() || f == fhome || inside(&fhome, &f) {
            return Err(PlanError("the run folder must not be a whole drive, the home folder or a folder holding it".into()));
        }
        if network_path(folder) {
            return Err(PlanError("a network folder cannot be sandboxed (its integrity labels are not kept)".into()));
        }
        if fdata.as_deref().is_some_and(|d| inside(&f, d) || inside(d, &f)) {
            return Err(PlanError("the run folder must not hold or be inside the app's data folder".into()));
        }
        if SECRET_PLACES.iter().any(|s| inside(&f, &fold(&under(&home, s)))) {
            return Err(PlanError("the run folder is inside a secret folder".into()));
        }
    }
    let ws_sid = workspace_sid(&input.workspace_id);
    let mut changes: Vec<Change> = Vec::new();

    // The folders the run writes: the workspace and, for a pane worktree, the worktree.
    let git = profile::git_places(&run);
    let common = git.common.as_deref().map(plain);
    let worktree = run != workspace && common.as_deref().is_some_and(|c| inside(&fold(c), &fold(&workspace)));
    let mut roots = vec![(workspace.clone(), Kind::Workspace)];
    if run != workspace {
        roots.push((run.clone(), Kind::Worktree));
    }
    for (root, kind) in &roots {
        changes.push(Change::new(root.clone(), Op::LowLabel, None, true, *kind));
        changes.push(Change::new(root.clone(), Op::Allow, Some(&ws_sid), true, *kind));
    }

    // git: config, hooks and info stay read-only; the git folders cannot be renamed away.
    let mut git_dirs: Vec<PathBuf> = Vec::new();
    for place in [&profile::git_places(&workspace), &git] {
        for dir in [place.common.as_deref(), place.admin.as_deref()].into_iter().flatten() {
            let dir = plain(dir);
            if !git_dirs.iter().any(|d| fold(d) == fold(&dir)) {
                git_dirs.push(dir);
            }
        }
    }
    let mut read_only: Vec<(PathBuf, Kind)> = Vec::new();
    let mut missing: Vec<PathBuf> = Vec::new();
    for dir in &git_dirs {
        if !inside(&fold(dir), &fold(&workspace)) && !inside(&fold(dir), &fold(&run)) {
            // A repository outside the approved folders (the workspace is a folder inside it) is not changed: its git folder is not
            // writable at all then.
            continue;
        }
        changes.push(Change::new(dir.clone(), Op::DenyDelete, Some(&ws_sid), false, Kind::Git));
        let mut nested = vec![dir.clone()];
        for sub in ["worktrees", "modules"] {
            if let Ok(entries) = std::fs::read_dir(dir.join(sub)) {
                nested.extend(entries.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()));
            }
        }
        for at in nested {
            for name in ["config", "config.worktree", "commondir", "gitdir", "hooks", "info"] {
                let path = at.join(name);
                if path.exists() {
                    read_only.push((path, Kind::Git));
                }
            }
        }
        // A missing hooks or info folder would be made by the run (the git folder is writable): it is made first, read-only.
        for name in ["hooks", "info"] {
            if !dir.join(name).exists() {
                missing.push(dir.join(name));
            }
        }
    }
    let dot_git = run.join(".git");
    if dot_git.is_file() {
        read_only.push((dot_git, Kind::Git));
    }

    // Instructions and the person's own read-only paths (those that exist now; a new file of that name is not covered).
    if input.protect_instructions {
        for file in profile::INSTRUCTION_FILES {
            let path = under(&run, file);
            if path.exists() {
                read_only.push((path, Kind::Instructions));
            }
        }
        for path in instruction_files(&run) {
            read_only.push((plain(&path), Kind::Instructions));
        }
    }
    for relative in input.read_only.iter().filter(|p| profile::relative_ok(p)) {
        let path = under(&run, relative.trim().trim_matches('/'));
        if path.exists() {
            read_only.push((path, Kind::ReadOnly));
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for (path, kind) in read_only {
        if seen.iter().any(|s| fold(s) == fold(&path)) {
            continue;
        }
        let tree = path.is_dir();
        changes.push(Change::new(path.clone(), Op::Deny, Some(&ws_sid), tree, kind));
        changes.push(Change::new(path.clone(), Op::ReadOnlyLabel, None, tree, kind));
        seen.push(path);
    }
    for path in missing {
        for op in [Op::Deny, Op::ReadOnlyLabel] {
            let mut change = Change::new(path.clone(), op, (op == Op::Deny).then_some(ws_sid.as_str()), true, Kind::Git);
            change.create = true;
            changes.push(change);
        }
    }

    // The agents' own folders: writable for their own SID only.
    let agents: Vec<&str> = match input.provider.as_deref() {
        Some(p) => vec![p],
        None => AGENTS.to_vec(),
    };
    for agent in agents {
        let sid = agent_sid(agent);
        let mut places: Vec<(PathBuf, bool)> = state_folders(agent).iter().map(|f| (under(&home, f), true)).collect();
        places.extend(state_files(agent).iter().map(|f| (under(&home, f), false)).filter(|(p, _)| p.is_file()));
        if input.provider.as_deref() == Some(agent) {
            places.extend(input.state_overrides.iter().filter(|p| p.is_absolute()).map(|p| (plain(p), true)));
        }
        for (path, folder) in places {
            let f = fold(&path);
            if f == fhome || inside(&fhome, &f) || SECRET_PLACES.iter().any(|s| inside(&f, &fold(&under(&home, s)))) {
                continue;
            }
            for op in [Op::LowLabel, Op::Allow] {
                let sid = (op == Op::Allow).then_some(sid.as_str());
                let mut change = Change::new(path.clone(), op, sid, folder, Kind::Agent);
                change.create = folder;
                change.agent = Some(agent.to_string());
                changes.push(change);
            }
        }
    }

    // Closed: the secret places that exist, and the app's data folder.
    for place in SECRET_PLACES {
        let path = under(&home, place);
        if path.exists() {
            let tree = path.is_dir();
            changes.push(Change::new(path, Op::SecretLabel, None, tree, Kind::Secret));
        }
    }
    if let Some(data) = &data {
        changes.push(Change::new(data.clone(), Op::SecretLabel, None, true, Kind::AppData));
    }

    // The workspace's private temporary and cache folder (under LocalLow, already low).
    let mut private = Change::new(plain(&input.private_dir), Op::Allow, Some(&ws_sid), true, Kind::Private);
    private.create = true;
    changes.push(private);
    Ok(Plan { changes, workspace_folder: workspace, run_folder: run, worktree })
}

/// The environment a sandboxed run gets on Windows: its own temporary folder and the package caches in the private folder (the
/// person's `%TEMP%` and caches are not writable at low integrity).
pub fn environment(private_dir: &Path, run_name: &str) -> Vec<(String, String)> {
    let at = |relative: &str| under(private_dir, relative).to_string_lossy().into_owned();
    let temp = at(&format!("tmp/{run_name}"));
    vec![
        ("TEMP".into(), temp.clone()),
        ("TMP".into(), temp.clone()),
        ("TMPDIR".into(), temp),
        ("npm_config_cache".into(), at("cache/npm")),
        ("YARN_CACHE_FOLDER".into(), at("cache/yarn")),
        ("BUN_INSTALL_CACHE_DIR".into(), at("cache/bun")),
        ("PIP_CACHE_DIR".into(), at("cache/pip")),
        ("UV_CACHE_DIR".into(), at("cache/uv")),
        ("GOCACHE".into(), at("cache/go-build")),
        ("GOMODCACHE".into(), at("cache/go-mod")),
        ("MIGHTY_SANDBOX".into(), "1".into()),
    ]
}

/// One promise of the sandbox and how far it is kept on this OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Guarantee {
    /// `writes`, `git`, `instructions`, `secrets`, `otherAgents`, `network`, `desktop`, `processes` (`sandbox.enforce.<key>`).
    pub key: &'static str,
    /// `enforced`, `partial` or `advisory` (`sandbox.level.<level>`).
    pub level: &'static str,
}

/// What the sandbox enforces: the Mac's Seatbelt profile all of it; Windows (MC-42) less, said as it is.
pub fn guarantees(windows: bool) -> Vec<Guarantee> {
    let g = |key, level| Guarantee { key, level };
    if !windows {
        return ["writes", "git", "instructions", "secrets", "otherAgents", "network", "desktop", "processes"]
            .into_iter()
            .map(|k| g(k, "enforced"))
            .collect();
    }
    vec![
        g("writes", "enforced"),
        g("git", "enforced"),
        // Existing instruction files only: a new AGENTS.md in a folder without one is not refused.
        g("instructions", "partial"),
        // The listed folders that exist (one made later is closed at the next run's start); browser profiles are not closed.
        g("secrets", "partial"),
        // Writes refused; another agent's sign-in file stays readable.
        g("otherAgents", "partial"),
        // Only programs that honour HTTPS_PROXY go through the allow list.
        g("network", "advisory"),
        g("desktop", "enforced"),
        g("processes", "enforced"),
    ]
}

// ----- Access-list texts (SDDL) -----

/// The entries of an SDDL section (`D:` or `S:`): its flags (`P`, `AI`…) and its entries, each `(…)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    pub flags: String,
    pub aces: Vec<String>,
}

/// The `D:` (or `S:`) section of a security descriptor's SDDL, or None when it has none.
pub fn section(sddl: &str, tag: char) -> Option<Section> {
    let bytes: Vec<char> = sddl.chars().collect();
    let mut depth = 0i32;
    let mut start = None;
    for i in 0..bytes.len() {
        match bytes[i] {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if depth == 0 && c == tag && bytes.get(i + 1) == Some(&':') => {
                start = Some(i + 2);
                break;
            }
            _ => {}
        }
    }
    let start = start?;
    let mut flags = String::new();
    let mut aces = Vec::new();
    let mut i = start;
    while i < bytes.len() && bytes[i] != '(' {
        if bytes[i] == ':' {
            // The next section's tag was taken as flags: back off one character.
            flags.pop();
            return Some(Section { flags, aces });
        }
        flags.push(bytes[i]);
        i += 1;
    }
    while i < bytes.len() && bytes[i] == '(' {
        let mut depth = 0i32;
        let mut ace = String::new();
        while i < bytes.len() {
            let c = bytes[i];
            ace.push(c);
            i += 1;
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
        aces.push(ace);
    }
    Some(Section { flags, aces })
}

/// The fields of one entry `(type;flags;rights;object;inherited object;sid…)`.
fn fields(ace: &str) -> Vec<&str> {
    ace.trim_start_matches('(').trim_end_matches(')').splitn(7, ';').collect()
}

/// Whether the entry was inherited (`ID` among its flags).
pub fn inherited(ace: &str) -> bool {
    let fields = fields(ace);
    let flags = fields.get(1).copied().unwrap_or("");
    flags.as_bytes().chunks(2).any(|c| c == b"ID")
}

fn ace_type(ace: &str) -> String {
    fields(ace).first().copied().unwrap_or("").to_ascii_uppercase()
}

fn ace_sid(ace: &str) -> String {
    fields(ace).get(5).copied().unwrap_or("").to_ascii_uppercase()
}

impl Section {
    /// `D:<flags><entries>`.
    pub fn render(&self, tag: char) -> String {
        format!("{tag}:{}{}", self.flags, self.aces.concat())
    }

    /// Whether the list does not inherit from its parent (`P`).
    pub fn protected(&self) -> bool {
        self.flags.contains('P')
    }

    /// The explicit entries only (Windows puts the inherited ones back from the parent).
    pub fn explicit(&self) -> Section {
        Section { flags: self.flags.clone(), aces: self.aces.iter().filter(|a| !inherited(a)).cloned().collect() }
    }

    /// The explicit entries with `ace` added in canonical order (a deny before the first explicit allow, an allow after the explicit
    /// entries); unchanged when it is already there.
    pub fn with(&self, ace: &str) -> Section {
        let mut out = self.explicit();
        if out.aces.iter().any(|a| a.eq_ignore_ascii_case(ace)) {
            return out;
        }
        let at =
            if ace_type(ace) == "D" { out.aces.iter().position(|a| ace_type(a) != "D").unwrap_or(out.aces.len()) } else { out.aces.len() };
        out.aces.insert(at, ace.to_string());
        out
    }

    /// The explicit entries without any naming `sid` (an undo of what was added for that SID alone).
    pub fn without_sid(&self, sid: &str) -> Section {
        let sid = sid.to_ascii_uppercase();
        let mut out = self.explicit();
        out.aces.retain(|a| ace_sid(a) != sid);
        out
    }

    /// The explicit entries without those of `ace`'s type for `ace`'s SID (the undo of one change: Windows may write the rights of
    /// the entry it keeps in other words than were given).
    pub fn without_entry(&self, ace: &str) -> Section {
        let (kind, sid) = (ace_type(ace), ace_sid(ace));
        let mut out = self.explicit();
        out.aces.retain(|a| !(ace_type(a) == kind && ace_sid(a) == sid));
        out
    }

    /// The explicit integrity label entry (`(ML;…)`), if any.
    pub fn label(&self) -> Option<String> {
        self.aces.iter().find(|a| ace_type(a) == "ML" && !inherited(a)).cloned()
    }
}

// ----- The approval record -----

/// One change made, with what it replaced and the workspaces that need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub change: Change,
    /// The explicit label before a label change (None: there was none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Folders made for it (removed again when empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<PathBuf>,
    pub users: Vec<String>,
    pub at: String,
}

/// What the person approved for one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub folder: PathBuf,
    /// The ids of every change the approval listed (some made later, as a run needs them: an agent's folder).
    pub approved: Vec<String>,
    pub at: String,
}

/// What [`AclWriter::apply`] did beside the change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Done {
    pub before: Option<String>,
    pub created: Vec<PathBuf>,
}

/// The Windows calls (mighty-platform-windows through the app); a fake in the checks.
pub trait AclWriter: Send + Sync {
    fn apply(&self, change: &Change) -> Result<Done, String>;
    fn undo(&self, applied: &Applied) -> Result<(), String>;
}

/// Every change the sandbox made on this computer and every approval (`<data>\sandbox\windows-changes.json`, which a sandboxed run
/// cannot read or write: the data folder is closed to it).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default)]
    pub applied: Vec<Applied>,
    #[serde(default)]
    pub approvals: BTreeMap<String, Approval>,
}

/// What an approval, a run's check or an undo did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub applied: usize,
    pub undone: usize,
    pub failures: Vec<(PathBuf, String)>,
}

/// Why a run's changes were not all made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The workspace has no approval, or the run needs a change outside it (the paths).
    NotApproved(Vec<PathBuf>),
    /// A change failed.
    Failed(PathBuf, String),
}

impl Record {
    pub fn parse(text: &str) -> Record {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.applied.iter().position(|a| a.change.id() == id)
    }

    pub fn approved(&self, workspace: &str) -> bool {
        self.approvals.contains_key(workspace)
    }

    /// The changes of `plan` not yet made for `workspace` (what the approval list shows as new).
    pub fn pending<'a>(&self, workspace: &str, plan: &'a Plan) -> Vec<&'a Change> {
        plan.changes
            .iter()
            .filter(|c| self.position(&c.id()).is_none_or(|i| !self.applied[i].users.iter().any(|u| u == workspace)))
            .collect()
    }

    /// Every change of `plan` is within `workspace`'s approval (listed then, or inside its folders, or the app's own).
    pub fn covers(&self, workspace: &str, plan: &Plan) -> bool {
        plan.changes.iter().all(|c| self.in_scope(workspace, plan, c))
    }

    fn in_scope(&self, workspace: &str, plan: &Plan, change: &Change) -> bool {
        let Some(approval) = self.approvals.get(workspace) else { return false };
        // The app's own folders, what was listed, and a secret place that appeared since (closing it only takes from a low process).
        if change.own() || change.kind == Kind::Secret || approval.approved.contains(&change.id()) {
            return true;
        }
        if change.shared() {
            return false;
        }
        let path = fold(&change.path);
        inside(&path, &fold(&approval.folder)) || (plan.worktree && inside(&path, &fold(&plan.run_folder)))
    }

    fn use_change(&mut self, workspace: &str, change: &Change, writer: &dyn AclWriter, now: &str) -> Result<bool, String> {
        if let Some(i) = self.position(&change.id()) {
            let users = &mut self.applied[i].users;
            if !users.iter().any(|u| u == workspace) {
                users.push(workspace.to_string());
            }
            return Ok(false);
        }
        let done = writer.apply(change)?;
        self.applied.push(Applied {
            change: change.clone(),
            before: done.before,
            created: done.created,
            users: vec![workspace.to_string()],
            at: now.to_string(),
        });
        Ok(true)
    }

    /// The person approved `plan` for `workspace`: every change that can be made now is made (an agent's folder that does not exist
    /// yet waits for that agent's first run) and the approval is kept. A failure takes back what this approval made and keeps none.
    pub fn approve(&mut self, workspace: &str, plan: &Plan, writer: &dyn AclWriter, now: &str) -> Result<Outcome, Refusal> {
        let mut outcome = Outcome::default();
        let mut made: Vec<String> = Vec::new();
        let mut added_user: Vec<String> = Vec::new();
        for change in &plan.changes {
            if change.kind == Kind::Agent && !change.path.exists() {
                continue;
            }
            let id = change.id();
            let had = self.position(&id).is_some_and(|i| self.applied[i].users.iter().any(|u| u == workspace));
            match self.use_change(workspace, change, writer, now) {
                Ok(true) => {
                    made.push(id);
                    outcome.applied += 1;
                }
                Ok(false) if !had => added_user.push(id),
                Ok(false) => {}
                Err(reason) => {
                    self.take_back(workspace, &made, &added_user, writer);
                    return Err(Refusal::Failed(change.path.clone(), reason));
                }
            }
        }
        let approved = plan.changes.iter().map(Change::id).collect();
        self.approvals.insert(workspace.to_string(), Approval { folder: plan.workspace_folder.clone(), approved, at: now.to_string() });
        Ok(outcome)
    }

    fn take_back(&mut self, workspace: &str, made: &[String], added_user: &[String], writer: &dyn AclWriter) {
        for id in added_user {
            if let Some(i) = self.position(id) {
                self.applied[i].users.retain(|u| u != workspace);
            }
        }
        for id in made.iter().rev() {
            if let Some(i) = self.position(id)
                && writer.undo(&self.applied[i]).is_ok()
            {
                self.applied.remove(i);
            }
        }
    }

    /// Before a run: every change of its plan made for `workspace`, those not yet made made now when they are within the approval
    /// (a new pane worktree, a new instruction file, an approved agent's folder). Anything else refuses the run.
    pub fn ensure(&mut self, workspace: &str, plan: &Plan, writer: &dyn AclWriter, now: &str) -> Result<Outcome, Refusal> {
        if !self.approved(workspace) {
            return Err(Refusal::NotApproved(vec![plan.workspace_folder.clone()]));
        }
        let outside: Vec<PathBuf> = plan
            .changes
            .iter()
            .filter(|c| self.position(&c.id()).is_none() && !self.in_scope(workspace, plan, c))
            .map(|c| c.path.clone())
            .collect();
        if !outside.is_empty() {
            return Err(Refusal::NotApproved(outside));
        }
        let mut outcome = Outcome::default();
        for change in &plan.changes {
            match self.use_change(workspace, change, writer, now) {
                Ok(made) => outcome.applied += usize::from(made),
                Err(reason) => return Err(Refusal::Failed(change.path.clone(), reason)),
            }
        }
        Ok(outcome)
    }

    /// The sandbox was turned off for `workspace`: its approval goes, and every change no other workspace still needs is undone,
    /// the last made first. A change that could not be undone stays recorded (with no user) for the next undo.
    pub fn release(&mut self, workspace: &str, writer: &dyn AclWriter) -> Outcome {
        self.approvals.remove(workspace);
        for applied in &mut self.applied {
            applied.users.retain(|u| u != workspace);
        }
        self.undo_unused(writer)
    }

    /// Undoes every recorded change no workspace uses (the last made first).
    pub fn undo_unused(&mut self, writer: &dyn AclWriter) -> Outcome {
        let mut outcome = Outcome::default();
        let mut i = self.applied.len();
        while i > 0 {
            i -= 1;
            if !self.applied[i].users.is_empty() {
                continue;
            }
            match writer.undo(&self.applied[i]) {
                Ok(()) => {
                    self.applied.remove(i);
                    outcome.undone += 1;
                }
                Err(reason) => outcome.failures.push((self.applied[i].change.path.clone(), reason)),
            }
        }
        outcome
    }

    /// Everything undone, every approval gone (the settings' "undo every change", before the app is removed).
    pub fn release_all(&mut self, writer: &dyn AclWriter) -> Outcome {
        self.approvals.clear();
        for applied in &mut self.applied {
            applied.users.clear();
        }
        self.undo_unused(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn root() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mc-win-plan-{}-{n}", std::process::id()));
        for d in ["home/work/app/.git/hooks", "home/work/app/.git/info", "home/work/app/docs", "home/work/app/node_modules/x", "home/.ssh"]
        {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::create_dir_all(dir.join("home/.claude")).unwrap();
        std::fs::write(dir.join("home/.claude.json"), "{}").unwrap();
        std::fs::write(dir.join("home/work/app/.git/config"), "").unwrap();
        std::fs::write(dir.join("home/work/app/AGENTS.md"), "").unwrap();
        std::fs::write(dir.join("home/work/app/docs/claude.md"), "").unwrap();
        std::fs::write(dir.join("home/work/app/node_modules/x/AGENTS.md"), "").unwrap();
        std::fs::write(dir.join("home/work/app/.mcp.json"), "").unwrap();
        plain(&profile::canonical(&dir).unwrap())
    }

    fn input(root: &Path) -> PlanInput {
        PlanInput {
            workspace_id: "ws1".into(),
            provider: Some("claude".into()),
            home: root.join("home"),
            workspace_folder: root.join("home/work/app"),
            run_folder: root.join("home/work/app"),
            data_dir: Some(root.join("home/data")),
            private_dir: root.join("low/MightyCode/sandbox/k"),
            state_overrides: vec![],
            protect_instructions: true,
            read_only: vec!["docs".into(), "../up".into()],
        }
    }

    fn has(plan: &Plan, path: &Path, op: Op) -> bool {
        plan.changes.iter().any(|c| c.path == path && c.op == op)
    }

    #[test]
    fn sids_are_stable_and_distinct() {
        let a = workspace_sid("ws1");
        assert_eq!(a, workspace_sid("ws1"));
        assert_ne!(a, workspace_sid("ws2"));
        assert_ne!(agent_sid("claude"), agent_sid("codex"));
        assert!(a.starts_with("S-1-5-21-") && a.split('-').count() == 8, "{a}");
        assert!(a.split('-').skip(1).all(|part| part.parse::<u32>().is_ok()), "every part a number: {a}");
        assert_eq!(restricting_sids("ws1", "codex"), [a, agent_sid("codex"), EVERYONE.to_string()]);
        assert_eq!(workspace_key("ws1").len(), 16);
    }

    #[test]
    fn the_plan_opens_the_folder_and_closes_git_instructions_and_secrets() {
        let root = root();
        std::fs::create_dir_all(root.join("home/data")).unwrap();
        let plan = super::plan(&input(&root)).unwrap();
        let ws = root.join("home/work/app");
        let sid = workspace_sid("ws1");
        assert!(has(&plan, &ws, Op::LowLabel) && has(&plan, &ws, Op::Allow));
        assert!(plan.changes.iter().any(|c| c.path == ws && c.op == Op::Allow && c.sid.as_deref() == Some(sid.as_str()) && c.tree));
        for p in [".git/config", ".git/hooks", ".git/info", "AGENTS.md", "docs/claude.md", ".mcp.json", "docs"] {
            assert!(has(&plan, &under(&ws, p), Op::Deny), "{p} is read-only");
            assert!(has(&plan, &under(&ws, p), Op::ReadOnlyLabel), "{p} has the second wall");
        }
        assert!(!plan.changes.iter().any(|c| c.path.to_string_lossy().contains("node_modules")), "node_modules is not searched");
        assert!(has(&plan, &ws.join(".git"), Op::DenyDelete));
        assert!(has(&plan, &root.join("home/.ssh"), Op::SecretLabel));
        assert!(!has(&plan, &root.join("home/.aws"), Op::SecretLabel), "only what exists");
        assert!(has(&plan, &root.join("home/data"), Op::SecretLabel));
        let claude = plan.changes.iter().find(|c| c.path == root.join("home/.claude") && c.op == Op::Allow).unwrap();
        assert_eq!(claude.sid.as_deref(), Some(agent_sid("claude").as_str()));
        assert!(has(&plan, &root.join("home/.claude.json"), Op::Allow));
        assert!(!plan.changes.iter().any(|c| c.path.ends_with(".codex")), "only the run's agent");
        assert!(has(&plan, &root.join("low/MightyCode/sandbox/k"), Op::Allow));
        assert!(!plan.changes.iter().any(|c| c.path.to_string_lossy().contains("up")), "a path leaving the folder is ignored");
        // The deny entries come before the labels are lowered nowhere: every label is per path, the folder's first.
        assert_eq!(plan.changes[0].op, Op::LowLabel);
        let all = super::plan(&PlanInput { provider: None, ..input(&root) }).unwrap();
        for agent in AGENTS {
            assert!(all.changes.iter().any(|c| c.agent.as_deref() == Some(agent)), "{agent} listed for the approval");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsafe_folders_are_refused() {
        let root = root();
        let i = input(&root);
        for bad in [root.join("home"), root.clone(), root.join("home/.ssh/keys")] {
            std::fs::create_dir_all(&bad).unwrap();
            assert!(plan(&PlanInput { workspace_folder: bad.clone(), run_folder: bad.clone(), ..i.clone() }).is_err(), "{bad:?}");
        }
        std::fs::create_dir_all(root.join("home/data/inner")).unwrap();
        let inner = root.join("home/data/inner");
        assert!(plan(&PlanInput { workspace_folder: inner.clone(), run_folder: inner, ..i.clone() }).is_err());
        assert!(network_path(Path::new(r"\\server\share\x")) && network_path(Path::new(r"\\?\UNC\server\share")));
        assert!(!network_path(Path::new(r"C:\work")) && !network_path(Path::new(r"\\?\C:\work")));
        assert_eq!(plain(Path::new(r"\\?\C:\work")), PathBuf::from(r"C:\work"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_of_the_workspace_is_within_its_approval() {
        let root = root();
        let main = root.join("home/work/app/.git");
        let admin = main.join("worktrees/w1");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("gitdir"), "x").unwrap();
        let tree = root.join("home/.mightycode/worktrees/w1");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        let p = plan(&PlanInput { run_folder: tree.clone(), ..input(&root) }).unwrap();
        assert!(p.worktree);
        assert!(has(&p, &tree, Op::Allow) && has(&p, &tree.join(".git"), Op::Deny));
        assert!(has(&p, &admin.join("commondir"), Op::Deny) && has(&p, &admin.join("gitdir"), Op::Deny));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hermes_and_programs_in_writable_folders_are_refused() {
        assert_eq!(cli_refusal("hermes"), Some("sandbox.windows.hermes"));
        assert_eq!(cli_refusal("claude"), None);
        assert!(cli_refusal("other").is_some());
        let writable = vec![PathBuf::from("/Users/me/AppData/Local/hermes")];
        assert!(program_inside(Path::new("/users/me/appdata/local/Hermes/bin/hermes.exe"), &writable));
        assert!(!program_inside(Path::new("/Users/me/AppData/Roaming/npm/pi.cmd"), &writable));
    }

    #[test]
    fn the_environment_points_at_the_private_folder() {
        let env: BTreeMap<String, String> = environment(Path::new("/low/k"), "r1").into_iter().collect();
        assert_eq!(env["TEMP"], under(Path::new("/low/k"), "tmp/r1").to_string_lossy());
        assert_eq!(env["TMP"], env["TEMP"]);
        assert!(env["npm_config_cache"].ends_with("npm") && env["PIP_CACHE_DIR"].ends_with("pip"));
        assert_eq!(env["MIGHTY_SANDBOX"], "1");
    }

    #[test]
    fn the_guarantees_say_what_windows_does_not_enforce() {
        let w = guarantees(true);
        assert_eq!(w.iter().find(|g| g.key == "network").unwrap().level, "advisory");
        assert!(w.iter().any(|g| g.level == "partial"));
        assert!(guarantees(false).iter().all(|g| g.level == "enforced"));
        assert_eq!(w.len(), guarantees(false).len());
    }

    #[test]
    fn sddl_sections_are_read_and_entries_added_or_taken_out() {
        let sddl = "O:S-1-5-21-1G:S-1-5-21-1D:AI(A;OICIID;FA;;;SY)(A;OICI;FA;;;S-1-5-21-1)(D;;WD;;;S-1-1-0)(XA;;FX;;;WD;(@User.x == 1))S:(ML;OICI;NW;;;LW)";
        let d = section(sddl, 'D').unwrap();
        assert_eq!(d.flags, "AI");
        assert_eq!(d.aces.len(), 4);
        assert!(d.aces[3].ends_with("== 1))"), "a conditional entry keeps its parentheses");
        assert!(inherited(&d.aces[0]) && !inherited(&d.aces[1]));
        let s = section(sddl, 'S').unwrap();
        assert_eq!(s.label().as_deref(), Some("(ML;OICI;NW;;;LW)"));
        assert_eq!(section("D:P(A;;FA;;;SY)", 'S'), None);
        let p = section("D:PAI(A;;FA;;;SY)", 'D').unwrap();
        assert!(p.protected());

        let sid = workspace_sid("w");
        let allow = Change::new("x".into(), Op::Allow, Some(&sid), true, Kind::Workspace).ace();
        let deny = Change::new("x".into(), Op::Deny, Some(&sid), false, Kind::Git).ace();
        assert_eq!(allow, format!("(A;OICI;0x1301bf;;;{sid})"));
        assert_eq!(deny, format!("(D;;0xd0156;;;{sid})"));
        let added = d.with(&allow).with(&deny);
        assert!(!added.aces.iter().any(|a| inherited(a)), "inherited entries come back from the parent");

        assert_eq!(added.aces[0], deny, "a deny before the first allow");
        assert_eq!(added.aces.last().unwrap(), &allow);
        assert_eq!(added.with(&allow), added, "added once");
        let back = added.without_sid(&sid.to_lowercase());
        assert_eq!(back, d.explicit());
        let rendered = format!("(D;;SDWDWO;;;{sid})");
        let one = added.without_entry(&allow);
        assert!(one.aces.contains(&deny) && !one.aces.contains(&allow), "only the allow entry goes");
        assert!(added.with(&rendered).without_entry(&deny).aces.iter().all(|a| !a.contains(&sid) || a == &allow), "in Windows' words too");
        assert_eq!(back.render('D'), "D:AI(A;OICI;FA;;;S-1-5-21-1)(D;;WD;;;S-1-1-0)(XA;;FX;;;WD;(@User.x == 1))");
        assert_eq!(Change::new("x".into(), Op::SecretLabel, None, true, Kind::Secret).ace(), "(ML;OICI;NRNW;;;ME)");
        assert_eq!(Change::new("x".into(), Op::ReadOnlyLabel, None, false, Kind::Git).ace(), "(ML;;NW;;;ME)");
    }

    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<String>>,
        fail: Option<PathBuf>,
    }

    impl AclWriter for Fake {
        fn apply(&self, change: &Change) -> Result<Done, String> {
            if self.fail.as_ref() == Some(&change.path) {
                return Err("denied".into());
            }
            self.log.lock().unwrap().push(format!("+{}", change.id()));
            Ok(Done { before: change.is_label().then(|| "(ML;;NW;;;ME)".to_string()), created: vec![] })
        }
        fn undo(&self, applied: &Applied) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("-{}", applied.change.id()));
            Ok(())
        }
    }

    fn change(path: &str, op: Op, kind: Kind) -> Change {
        Change::new(PathBuf::from(path), op, Some("S-1-5-21-1-2-3-4"), true, kind)
    }

    fn sample(folder: &str, extra: Vec<Change>) -> Plan {
        let mut changes = vec![change(folder, Op::Allow, Kind::Workspace), change("/h/.ssh", Op::SecretLabel, Kind::Secret)];
        changes.extend(extra);
        Plan { changes, workspace_folder: PathBuf::from(folder), run_folder: PathBuf::from(folder), worktree: false }
    }

    #[test]
    fn an_approval_makes_records_and_undoes_its_changes() {
        let fake = Fake::default();
        let mut record = Record::default();
        let a = sample("/w/a", vec![]);
        let out = record.approve("a", &a, &fake, "t").unwrap();
        assert_eq!(out.applied, 2);
        assert!(record.approved("a") && record.covers("a", &a));
        assert!(record.pending("a", &a).is_empty());
        // A second workspace shares the secret label: made once, two users.
        let b = sample("/w/b", vec![]);
        assert_eq!(record.pending("b", &b).len(), 2);
        assert_eq!(record.approve("b", &b, &fake, "t").unwrap().applied, 1);
        assert_eq!(record.applied.iter().find(|x| x.change.kind == Kind::Secret).unwrap().users, ["a", "b"]);
        // Turning a off undoes only its own folder; the shared label stays for b.
        let out = record.release("a", &fake);
        assert_eq!(out.undone, 1);
        assert!(!record.approved("a"));
        assert!(record.applied.iter().any(|x| x.change.kind == Kind::Secret));
        let out = record.release("b", &fake);
        assert_eq!(out.undone, 2);
        assert!(record.applied.is_empty());
        let log = fake.log.lock().unwrap().clone();
        let undone: Vec<&String> = log.iter().filter(|l| l.starts_with('-')).collect();
        assert_eq!(undone.len(), 3);
        assert_eq!(undone[1], &format!("-{}", change("/w/b", Op::Allow, Kind::Workspace).id()), "the last made is undone first");
        // The record survives its file.
        let again = Record::parse(&Record::default().text());
        assert_eq!(again, Record::default());
    }

    #[test]
    fn a_failed_approval_takes_back_what_it_made() {
        let fake = Fake { fail: Some(PathBuf::from("/h/.ssh")), ..Fake::default() };
        let mut record = Record::default();
        let err = record.approve("a", &sample("/w/a", vec![]), &fake, "t").unwrap_err();
        assert!(matches!(err, Refusal::Failed(ref p, _) if p == Path::new("/h/.ssh")));
        assert!(record.applied.is_empty() && !record.approved("a"));
    }

    #[test]
    fn a_run_makes_what_is_within_the_approval_and_refuses_the_rest() {
        let fake = Fake::default();
        let mut record = Record::default();
        let base = sample("/w/a", vec![]);
        assert!(matches!(record.ensure("a", &base, &fake, "t"), Err(Refusal::NotApproved(_))), "no approval, no run");
        record.approve("a", &base, &fake, "t").unwrap();
        // A new instruction file inside the folder, the app's own folder: made at the run.
        let inner =
            sample("/w/a", vec![change("/w/a/sub/AGENTS.md", Op::Deny, Kind::Instructions), change("/low/k", Op::Allow, Kind::Private)]);
        assert_eq!(record.ensure("a", &inner, &fake, "t").unwrap().applied, 2);
        // A secret place that appeared since the approval: closed at the run.
        let later = sample("/w/a", vec![change("/h/.aws", Op::SecretLabel, Kind::Secret)]);
        assert_eq!(record.ensure("a", &later, &fake, "t").unwrap().applied, 1);
        // An agent's folder nobody approved (a shared place): refused.
        let other = sample("/w/a", vec![change("/h/.pi", Op::Allow, Kind::Agent)]);
        assert_eq!(record.ensure("a", &other, &fake, "t"), Err(Refusal::NotApproved(vec![PathBuf::from("/h/.pi")])));
        // A pane worktree of the repository: within the approval.
        let mut tree = sample("/w/a", vec![change("/wt/1", Op::Allow, Kind::Worktree)]);
        tree.run_folder = PathBuf::from("/wt/1");
        tree.worktree = true;
        assert!(record.ensure("a", &tree, &fake, "t").is_ok());
        tree.worktree = false;
        tree.changes[2].path = PathBuf::from("/wt/2");
        assert!(record.ensure("a", &tree, &fake, "t").is_err(), "a folder that is not the workspace's worktree");
        // Undo all.
        let out = record.release_all(&fake);
        assert!(out.failures.is_empty() && record.applied.is_empty() && record.approvals.is_empty());
    }

    #[test]
    fn an_agents_folder_listed_at_the_approval_is_made_at_its_first_run() {
        let fake = Fake::default();
        let mut record = Record::default();
        let mut pi = change("/nonexistent-home/.pi", Op::Allow, Kind::Agent);
        pi.create = true;
        let plan = sample("/w/a", vec![pi.clone()]);
        record.approve("a", &plan, &fake, "t").unwrap();
        assert!(record.position(&pi.id()).is_none(), "missing then: waits");
        assert_eq!(record.ensure("a", &plan, &fake, "t").unwrap().applied, 1);
        assert!(record.position(&pi.id()).is_some());
    }
}

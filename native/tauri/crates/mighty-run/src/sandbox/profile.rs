//! MC-34: the Seatbelt profile (SBPL) of one sandboxed agent run, made from rules rather than written by hand.
//!
//! The profile starts closed (`deny default`, the base Codex and Chromium use) and opens what an agent CLI needs to start and work:
//!
//! - **Reads** everywhere, except the secret places of [`SECRET_PLACES`], the app's own data folder (but its agent IO folder, where a
//!   run's MCP config and socket are), the login keychains (open only to the CLIs that keep their sign-in there: [`uses_keychain`])
//!   and the other agents' sign-in files ([`credentials`]). Each CLI keeps its own folders ([`state_folders`]).
//! - **Writes** only in the run's folder (the workspace or its pane worktree), the CLI's own folders, the temporary folders and the
//!   package caches of [`CACHES`].
//! - **Git** in the folder: `.git` is closed for writing, then what a commit, a merge, a rebase and a fetch write is opened again
//!   (objects, refs, logs, the index, the `*_HEAD` files, `packed-refs`, `worktrees/`, `lfs/`). `config`, `config.worktree`, `hooks/`
//!   and `info/` stay closed: each can make git run a program outside the sandbox later. A pane worktree's `.git` file stays closed too.
//! - **Instructions** (on by default): `.mighty.yaml`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md` (in any folder) and the agents' project
//!   settings stay read-only; the person's own read-only paths join them.
//! - **Network**: with the allow list on, TCP only to the given loopback ports (the allow-list proxy, the app's own bridges); without
//!   it, any TCP. Unix sockets only to the given ones and inside the CLI's own folders (never the app's control socket, ssh's
//!   ControlMaster or ssh-agent).
//!
//! Paths are matched as the kernel sees them (`/tmp` is `/private/tmp`): every path here is made canonical first.

use std::path::{Path, PathBuf};

/// Everything one profile is made from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileInput {
    /// The run's provider id (`claude`, `codex`, `gemini`, `opencode`, `pi`, `hermes`; a shell pane is `shell`).
    pub provider: String,
    pub home: PathBuf,
    /// The folder the run works in.
    pub folder: PathBuf,
    /// `TMPDIR`.
    pub temp: PathBuf,
    /// The app's data folder (closed for reading but its `agent-io` folder).
    pub data_dir: Option<PathBuf>,
    /// The CLI's own folders named by its environment (`CODEX_HOME`, `HERMES_HOME`), beside the usual ones.
    pub state_overrides: Vec<PathBuf>,
    /// Unix sockets the run may connect to (the agent IO's).
    pub sockets: Vec<PathBuf>,
    /// Loopback TCP ports the run may connect to (the proxy, the app's bridges, the pane's own port).
    pub ports: Vec<u16>,
    pub limit_network: bool,
    pub protect_instructions: bool,
    /// More read-only paths, relative to `folder`.
    pub read_only: Vec<String>,
}

/// Secret places no run reads, relative to the home folder.
pub const SECRET_PLACES: [&str; 18] = [
    ".ssh",
    ".aws",
    ".gnupg",
    ".config/gh",
    ".config/gcloud",
    ".azure",
    ".kube",
    ".docker/config.json",
    ".netrc",
    ".git-credentials",
    ".password-store",
    "Library/Cookies",
    "Library/Safari",
    "Library/Messages",
    "Library/Mail",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    "Library/Application Support/com.apple.TCC",
];

/// The login keychains, open only to the CLIs that keep their sign-in there.
pub const KEYCHAINS: &str = "Library/Keychains";

/// Package caches a run may write (relative to home): installs and builds work, nothing on `PATH` changes.
pub const CACHES: [&str; 9] = [
    "Library/Caches",
    ".cache",
    ".npm",
    ".bun/install/cache",
    ".cargo/registry",
    ".cargo/git",
    "go/pkg/mod",
    ".gradle/caches",
    ".m2/repository",
];

/// The files the instructions switch keeps read-only at the folder's root.
pub const INSTRUCTION_FILES: [&str; 9] = [
    ".mighty.yaml",
    ".mighty.local.yaml",
    ".mcp.json",
    ".envrc",
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".codex/config.toml",
    ".gemini/settings.json",
    "opencode.json",
];

/// The instruction files the switch keeps read-only in every folder (any case: the Mac's disks ignore it).
pub const INSTRUCTION_NAMES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "CLAUDE.local.md", "GEMINI.md"];

/// What git writes in its folder for a commit, merge, rebase, cherry-pick, fetch and worktree (relative to the git folder).
pub const GIT_WRITABLE: [&str; 9] = ["objects", "refs", "logs", "worktrees", "lfs", "rebase-merge", "rebase-apply", "sequencer", "modules"];

/// Each provider's own folders (relative to home): read and written, its sign-in among them.
pub fn state_folders(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &[".claude"],
        "codex" => &[".codex"],
        "gemini" => &[".gemini"],
        "opencode" => &[".config/opencode", ".local/share/opencode", ".local/state/opencode", ".cache/opencode"],
        "pi" => &[".pi"],
        "hermes" => &[".hermes"],
        _ => &[],
    }
}

/// Each provider's sign-in files (relative to home): closed to every other provider's run.
pub fn credentials(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &[".claude/.credentials.json"],
        "codex" => &[".codex/auth.json"],
        "gemini" => &[".gemini/oauth_creds.json", ".gemini/google_accounts.json"],
        "opencode" => &[".local/share/opencode/auth.json", ".local/share/opencode/mcp-auth.json"],
        "pi" => &[".pi/agent/auth.json"],
        "hermes" => &[".hermes/auth.json", ".hermes/.env"],
        _ => &[],
    }
}

/// The providers whose sign-in is in the login keychain on the Mac (Claude Code's `Claude Code-credentials`, Codex's `auto` store,
/// Antigravity's Google sign-in). The others keep it in a file of their own folder.
pub fn uses_keychain(provider: &str) -> bool {
    matches!(provider, "claude" | "codex" | "gemini")
}

const PROVIDERS: [&str; 6] = ["claude", "codex", "gemini", "opencode", "pi", "hermes"];

/// Why a profile could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub String);

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A path as a profile string (`"…"`). A path holding a quote, a backslash or a control character is refused: such a name is never
/// put into a profile.
fn quoted(path: &Path) -> Result<String, ProfileError> {
    let text = path.to_string_lossy();
    if text.is_empty() || !text.starts_with('/') || text.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(ProfileError(format!("unsupported path for the sandbox: {text:?}")));
    }
    Ok(format!("\"{text}\""))
}

/// A path as the start of a profile regex.
fn regex_path(path: &Path) -> Result<String, ProfileError> {
    quoted(path)?;
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        if ".^$|()[]{}*+?".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    Ok(out)
}

/// `AGENTS.md` as `[Aa][Gg]…\.[Mm][Dd]`.
fn any_case(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphabetic() {
            out.push('[');
            out.push(c.to_ascii_uppercase());
            out.push(c.to_ascii_lowercase());
            out.push(']');
        } else if ".^$|()[]{}*+?".contains(c) {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

/// A relative path the person typed, checked: no `..`, not absolute, not empty.
pub fn relative_ok(path: &str) -> bool {
    let trimmed = path.trim().trim_matches('/');
    !trimmed.is_empty()
        && !path.trim().starts_with('/')
        && !trimmed.contains('\\')
        && !trimmed.chars().any(char::is_control)
        && trimmed.split('/').all(|part| !part.is_empty() && part != ".." && part != ".")
}

/// Where git keeps the folder's repository: the `.git` folder of the folder or the nearest folder above it, or for a pane worktree
/// (a `.git` file) its own admin folder and the main repository's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitPlaces {
    /// The repository's shared folder (objects, refs).
    pub common: Option<PathBuf>,
    /// A worktree's own folder under `<common>/worktrees/` (its index and HEAD).
    pub admin: Option<PathBuf>,
}

/// Finds [`GitPlaces`] for `folder` (canonical paths; nothing when no repository holds it).
pub fn git_places(folder: &Path) -> GitPlaces {
    let mut at = Some(folder);
    while let Some(dir) = at {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return GitPlaces { common: canonical(&dot), admin: None };
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).unwrap_or_default();
            let Some(target) = text.lines().find_map(|l| l.strip_prefix("gitdir:")).map(str::trim) else { return GitPlaces::default() };
            let admin = if Path::new(target).is_absolute() { PathBuf::from(target) } else { dir.join(target) };
            let Some(admin) = canonical(&admin) else { return GitPlaces::default() };
            let common = std::fs::read_to_string(admin.join("commondir"))
                .ok()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .map(|c| if Path::new(&c).is_absolute() { PathBuf::from(c) } else { admin.join(c) })
                .and_then(|c| canonical(&c))
                .unwrap_or_else(|| admin.clone());
            return GitPlaces { common: Some(common), admin: Some(admin) };
        }
        at = dir.parent();
    }
    GitPlaces::default()
}

/// `path` as the kernel names it (symbolic links followed); a path that does not exist yet keeps its canonical parent.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    if let Ok(found) = std::fs::canonicalize(path) {
        return Some(found);
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    Some(canonical(parent)?.join(name))
}

struct Rules(Vec<String>);

impl Rules {
    fn line(&mut self, text: impl Into<String>) {
        self.0.push(text.into());
    }

    /// `(<verb> <operations> <filters…>)`, nothing when there is no filter.
    fn rule(&mut self, verb: &str, operations: &str, filters: &[String]) {
        if filters.is_empty() {
            return;
        }
        let mut text = format!("({verb} {operations}");
        for filter in filters {
            text.push_str("\n  ");
            text.push_str(filter);
        }
        text.push(')');
        self.0.push(text);
    }
}

fn subpath(path: &Path) -> Result<String, ProfileError> {
    Ok(format!("(subpath {})", quoted(path)?))
}

fn literal(path: &Path) -> Result<String, ProfileError> {
    Ok(format!("(literal {})", quoted(path)?))
}

fn regex(pattern: &str) -> String {
    format!("(regex #\"{pattern}\")")
}

/// The profile for `input`.
pub fn generate(input: &ProfileInput) -> Result<String, ProfileError> {
    let home = canonical(&input.home).ok_or_else(|| ProfileError("no home folder".into()))?;
    let folder = canonical(&input.folder).ok_or_else(|| ProfileError("no run folder".into()))?;
    if folder == Path::new("/") || folder == home {
        return Err(ProfileError("the run folder must not be the whole disk or the home folder".into()));
    }
    let temp = canonical(&input.temp).ok_or_else(|| ProfileError("no temporary folder".into()))?;
    let provider = input.provider.as_str();
    let mut r = Rules(Vec::new());
    r.line(format!(";; Mighty Code sandbox (MC-34) for a {provider} run. Made by the app; changes are overwritten."));
    r.line("(version 1)");
    r.line("(deny default)");
    // What any process needs to start, run its children and talk to the few system services every CLI uses.
    r.line("(allow process-exec)");
    r.line("(allow process-fork)");
    r.line("(allow signal (target same-sandbox))");
    r.line("(allow process-info* (target same-sandbox))");
    r.line("(allow user-preference-read)");
    r.line("(allow sysctl-read)");
    r.line("(allow ipc-posix-sem)");
    r.line("(allow ipc-posix-shm)");
    r.line("(allow iokit-open (iokit-registry-entry-class \"RootDomainUserClient\"))");
    let mut services = vec![
        "com.apple.system.opendirectoryd.libinfo",
        "com.apple.system.opendirectoryd.membership",
        "com.apple.PowerManagement.control",
        "com.apple.system.logger",
        "com.apple.logd",
        "com.apple.diagnosticd",
        "com.apple.system.notification_center",
        "com.apple.bsd.dirhelper",
        "com.apple.FSEvents",
        "com.apple.SystemConfiguration.configd",
        "com.apple.SystemConfiguration.DNSConfiguration",
        "com.apple.trustd.agent",
        "com.apple.ocspd",
    ];
    if uses_keychain(provider) {
        services.extend(["com.apple.SecurityServer", "com.apple.securityd.xpc", "com.apple.security.agent"]);
    }
    let names: Vec<String> = services.iter().map(|s| format!("(global-name \"{s}\")")).collect();
    r.rule("allow", "mach-lookup", &names);
    r.line("(allow pseudo-tty)");
    r.line("(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\") (regex #\"^/dev/ttys[0-9]+$\"))");
    r.line(
        "(allow file-write-data file-ioctl (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/tty\") (literal \"/dev/stdout\") (literal \"/dev/stderr\") (literal \"/dev/dtracehelper\") (regex #\"^/dev/fd/[0-9]+$\"))",
    );

    // Reads: everywhere, then the secret places closed.
    r.line(";; reads");
    r.line("(allow file-read*)");
    let mut secrets: Vec<String> = Vec::new();
    for place in SECRET_PLACES {
        secrets.push(subpath(&home.join(place))?);
    }
    if !uses_keychain(provider) {
        secrets.push(subpath(&home.join(KEYCHAINS))?);
    }
    for other in PROVIDERS.iter().filter(|p| **p != provider) {
        for file in credentials(other) {
            secrets.push(literal(&home.join(file))?);
        }
    }
    if let Some(data) = input.data_dir.as_deref().and_then(canonical) {
        secrets.push(subpath(&data)?);
    }
    r.rule("deny", "file-read* file-write*", &secrets);
    if let Some(data) = input.data_dir.as_deref().and_then(canonical) {
        r.rule("allow", "file-read*", &[subpath(&data.join("agent-io"))?]);
    }

    // Writes: the folder, the CLI's own folders, the temporary folders and the caches.
    r.line(";; writes");
    let mut writable = vec![subpath(&folder)?, subpath(&temp)?, subpath(Path::new("/private/tmp"))?];
    // `…/T` is TMPDIR; its parent also holds the per-user cache (`…/C`) the system's libraries write.
    if temp.file_name().is_some_and(|n| n == "T")
        && let Some(parent) = temp.parent().filter(|p| p.starts_with("/private/var/folders"))
    {
        writable.push(subpath(parent)?);
    }
    let mut own: Vec<PathBuf> = state_folders(provider).iter().map(|f| home.join(f)).collect();
    own.extend(input.state_overrides.iter().filter_map(|p| canonical(p)).filter(|p| p != &home && p != Path::new("/")));
    for place in &own {
        writable.push(subpath(place)?);
    }
    if provider == "claude" {
        // `~/.claude.json` is replaced through a temporary file beside it.
        writable.push(regex(&format!("^{}/\\.claude\\.json", regex_path(&home)?)));
    }
    for cache in CACHES {
        writable.push(subpath(&home.join(cache))?);
    }
    r.rule("allow", "file-write*", &writable);
    // The secrets stay closed even inside a writable place.
    r.rule("deny", "file-write*", &secrets);

    // Git: `.git` closed, then what git's own work writes opened again; config, hooks and info never.
    r.line(";; git");
    let git = git_places(&folder);
    let mut git_closed = vec![subpath(&folder.join(".git"))?];
    let mut git_open = Vec::new();
    let mut git_never = Vec::new();
    for dir in [git.common.as_ref(), git.admin.as_ref()].into_iter().flatten() {
        git_closed.push(subpath(dir)?);
        let at = regex_path(dir)?;
        for piece in GIT_WRITABLE {
            git_open.push(subpath(&dir.join(piece))?);
        }
        // HEAD, ORIG_HEAD, FETCH_HEAD, MERGE_MSG, COMMIT_EDITMSG, AUTO_MERGE…, the index and packed-refs, and their locks.
        git_open.push(regex(&format!("^{at}/[A-Z_]+(\\.lock)?$")));
        git_open.push(regex(&format!("^{at}/(index|sharedindex\\.[0-9a-f]+|packed-refs|shallow|gc\\.pid|gc\\.log)(\\.lock)?$")));
        git_never.push(regex(&format!("^{at}/((worktrees|modules)/.+/)?(config|config\\.worktree|commondir|gitdir)(\\.lock)?$")));
        git_never.push(regex(&format!("^{at}/((worktrees|modules)/.+/)?(hooks|info)(/|$)")));
    }
    r.rule("deny", "file-write*", &git_closed);
    r.rule("allow", "file-write*", &git_open);
    r.rule("deny", "file-write*", &git_never);

    // Instructions and the person's own read-only paths.
    let mut read_only = Vec::new();
    if input.protect_instructions {
        r.line(";; instructions");
        for file in INSTRUCTION_FILES {
            read_only.push(subpath(&folder.join(file))?);
        }
        let names: Vec<String> = INSTRUCTION_NAMES.iter().map(|n| any_case(n)).collect();
        read_only.push(regex(&format!("^{}/(.+/)?({})$", regex_path(&folder)?, names.join("|"))));
    }
    for path in input.read_only.iter().filter(|p| relative_ok(p)) {
        read_only.push(subpath(&folder.join(path.trim().trim_matches('/')))?);
    }
    r.rule("deny", "file-write*", &read_only);

    // Network.
    r.line(";; network");
    r.line("(allow system-socket)");
    if input.limit_network {
        let mut ports: Vec<u16> = input.ports.iter().copied().filter(|p| *p != 0).collect();
        ports.sort_unstable();
        ports.dedup();
        let remote: Vec<String> = ports.iter().map(|p| format!("(remote ip \"localhost:{p}\")")).collect();
        r.rule("allow", "network-outbound", &remote);
        // A dev server the agent starts may listen on this computer only.
        r.line("(allow network-bind network-inbound (local ip \"localhost:*\"))");
    } else {
        r.line("(allow network-outbound (remote ip \"*:*\"))");
        r.line("(allow network-bind network-inbound (local ip \"*:*\"))");
    }
    let mut sockets = Vec::new();
    for socket in &input.sockets {
        if let Some(found) = canonical(socket) {
            sockets.push(format!("(remote unix-socket (path-literal {}))", quoted(&found)?));
        }
    }
    for place in &own {
        sockets.push(format!("(remote unix-socket (subpath {}))", quoted(place)?));
    }
    r.rule("allow", "network-outbound", &sockets);
    let mut text = r.0.join("\n");
    text.push('\n');
    Ok(text)
}

// Seatbelt paths are the Mac's (absolute `/…`): the checks run where they mean something.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn input(root: &Path) -> ProfileInput {
        ProfileInput {
            provider: "claude".into(),
            home: root.join("home"),
            folder: root.join("home/work/app"),
            temp: root.join("tmp"),
            data_dir: Some(root.join("home/Library/Application Support/MightyCode")),
            state_overrides: vec![],
            sockets: vec![],
            ports: vec![41000, 41000, 0],
            limit_network: true,
            protect_instructions: true,
            read_only: vec!["secrets/".into(), "../escape".into(), "/abs".into()],
        }
    }

    fn root() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mc-profile-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("home/work/app/.git")).unwrap();
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        canonical(&dir).unwrap()
    }

    #[test]
    fn the_profile_closes_by_default_and_names_every_rule() {
        let root = root();
        let text = generate(&input(&root)).unwrap();
        let home = root.join("home");
        assert!(text.starts_with(";; Mighty Code sandbox"));
        assert!(text.contains("(deny default)"));
        assert!(text.contains(&format!("(subpath \"{}\")", home.join(".ssh").display())));
        assert!(text.contains(&format!("(subpath \"{}\")", home.join(".aws").display())));
        assert!(text.contains(&format!("(literal \"{}\")", home.join(".codex/auth.json").display())), "another agent's sign-in");
        assert!(!text.contains(&format!("(literal \"{}\")", home.join(".claude/.credentials.json").display())), "not its own");
        assert!(!text.contains(&format!("(subpath \"{}\")", home.join(KEYCHAINS).display())), "Claude keeps its sign-in there");
        assert!(text.contains("com.apple.SecurityServer"));
        assert!(text.contains(&format!("(subpath \"{}\")", home.join(".claude").display())));
        assert!(text.contains("(remote ip \"localhost:41000\")"));
        assert_eq!(text.matches("localhost:41000").count(), 1, "ports once each");
        assert!(!text.contains("localhost:0"));
        assert!(text.contains(&format!("(subpath \"{}\")", root.join("home/work/app/secrets").display())));
        assert!(!text.contains("escape") && !text.contains("\"/abs\""), "a path that leaves the folder is ignored");
        assert!(text.contains("[Aa][Gg][Ee][Nn][Tt][Ss]\\.[Mm][Dd]"));
        assert!(text.contains(".mighty.yaml"));
        assert!(text.contains(&format!("(subpath \"{}\")", root.join("home/Library/Application Support/MightyCode/agent-io").display())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn providers_without_the_keychain_cannot_read_it() {
        let root = root();
        for provider in ["opencode", "pi", "hermes", "shell"] {
            let text = generate(&ProfileInput { provider: provider.into(), ..input(&root) }).unwrap();
            assert!(text.contains(&format!("(subpath \"{}\")", root.join("home").join(KEYCHAINS).display())), "{provider}");
            assert!(!text.contains("com.apple.SecurityServer"), "{provider}");
        }
        let text = generate(&ProfileInput { provider: "codex".into(), ..input(&root) }).unwrap();
        assert!(text.contains(&format!("(literal \"{}\")", root.join("home/.claude/.credentials.json").display())));
        assert!(!text.contains(&format!("(literal \"{}\")", root.join("home/.codex/auth.json").display())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_network_and_switches_off() {
        let root = root();
        let text =
            generate(&ProfileInput { limit_network: false, protect_instructions: false, read_only: vec![], ..input(&root) }).unwrap();
        assert!(text.contains("(remote ip \"*:*\")"));
        assert!(!text.contains("AGENTS") && !text.contains("[Aa][Gg]"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsafe_places_are_refused() {
        let root = root();
        assert!(generate(&ProfileInput { folder: root.join("home"), ..input(&root) }).is_err(), "the whole home");
        assert!(generate(&ProfileInput { folder: "/".into(), ..input(&root) }).is_err());
        let odd = root.join("home/work/a\"b");
        std::fs::create_dir_all(&odd).unwrap();
        assert!(generate(&ProfileInput { folder: odd, ..input(&root) }).is_err(), "a quote never reaches a profile");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_worktree_opens_its_own_and_the_main_repositorys_git_folders() {
        let root = root();
        let main = root.join("home/work/app/.git");
        let admin = main.join("worktrees/w1");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let tree = root.join("home/.mightycode/worktrees/w1");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        let places = git_places(&tree);
        assert_eq!(places.common.as_deref(), Some(main.as_path()));
        assert_eq!(places.admin.as_deref(), Some(admin.as_path()));
        let text = generate(&ProfileInput { folder: tree.clone(), ..input(&root) }).unwrap();
        assert!(text.contains(&format!("(subpath \"{}\")", main.join("objects").display())));
        assert!(text.contains(&format!("(subpath \"{}\")", tree.join(".git").display())), "the .git file stays closed");
        // A folder inside a repository finds the repository above it.
        let inner = root.join("home/work/app/src");
        std::fs::create_dir_all(&inner).unwrap();
        assert_eq!(git_places(&inner).common.as_deref(), Some(main.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_paths_are_checked() {
        assert!(relative_ok("secrets") && relative_ok("config/prod.yaml") && relative_ok("dir/"));
        assert!(!relative_ok("") && !relative_ok("/etc") && !relative_ok("a/../b") && !relative_ok("..") && !relative_ok("./x"));
        assert_eq!(any_case("A.md"), "[Aa]\\.[Mm][Dd]");
    }
}

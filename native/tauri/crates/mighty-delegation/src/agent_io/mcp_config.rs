//! Where a run's pane token travels to its MCP servers without passing through the agent CLI's own environment or arguments.
//!
//! - Claude: the `--mcp-config` JSON (each server with its own `env`: the token and the transport's name) is written to a file of the
//!   user's alone (`mcp-<random>.json`, 0600 on Unix) in the agent IO's private folder (the Unix socket's 0700 folder; on Windows a
//!   folder of the user's temp folder, which only that user may read), and the argument names its path. The file goes when the run
//!   ends or its binding is revoked; one an earlier app left (it crashed) is swept when the agent IO starts. Every local user can read
//!   any process's arguments (`ps aux`), so the token is never one of them.
//! - Codex: the token stays in the Codex process's environment, forwarded to the servers by name (`mcp_servers.<name>.env_vars`), and
//!   `shell_environment_policy.exclude` keeps `MIGHTY_*` out of the commands its shell tool runs ([`codex_shell_exclude`]).

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The config files' names: `mcp-<32 hex>.json`.
const PREFIX: &str = "mcp-";
const SUFFIX: &str = ".json";
/// A file older than this is no live run's (a CLI reads its config as it starts).
const STALE: Duration = Duration::from_secs(60);
/// The environment names Codex's shell tool never passes on.
pub const CODEX_SHELL_EXCLUDE: &str = "MIGHTY_*";
/// A Codex config larger than this is not read (the user's `exclude` is then not kept).
const MAXIMUM_CODEX_CONFIG: u64 = 1024 * 1024;

/// The folder the config files go in when the agent IO named none (the checks' bindings): under the user's temp folder.
pub fn fallback_folder() -> PathBuf {
    std::env::temp_dir().join("mighty-agent-io-mcp")
}

/// The agent IO's own folder for a transport named `name`: the Unix socket's folder (made 0700 by its listener), else
/// [`fallback_folder`].
pub fn folder_for(name: &str, unix_socket: bool) -> PathBuf {
    match Path::new(name).parent().filter(|parent| unix_socket && !parent.as_os_str().is_empty()) {
        Some(parent) => parent.to_path_buf(),
        None => fallback_folder(),
    }
}

/// The folder made if missing; on Unix 0700 and refused when it is a link or not a folder (setting its mode fails unless it is this
/// user's own).
pub fn private_folder(folder: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)?;
        let meta = std::fs::symlink_metadata(folder)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(std::io::Error::other("the agent IO folder is not a folder"));
        }
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(folder)
    }
}

/// `contents` in a new file of `folder` only this user may read (created, never opened when it exists); its path.
pub fn write(folder: &Path, contents: &str) -> std::io::Result<PathBuf> {
    private_folder(folder)?;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = folder.join(format!("{PREFIX}{name}{SUFFIX}"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    if let Err(error) = file.write_all(contents.as_bytes()).and_then(|()| file.flush()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

/// Whether `name` is one of these files' names.
fn ours(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Every config file in `folder` older than a minute deleted (an app that ended without deleting its runs' files); the count.
pub fn sweep(folder: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    let now = SystemTime::now();
    let mut swept = 0;
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .is_some_and(|modified| now.duration_since(modified).unwrap_or_default() >= STALE);
        if stale && entry.file_name().to_str().is_some_and(ours) && std::fs::remove_file(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// Reads a bounded, non-symlink Codex `config.toml` file.
fn read_codex_config(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() || meta.len() > MAXIMUM_CODEX_CONFIG {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAXIMUM_CODEX_CONFIG + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAXIMUM_CODEX_CONFIG {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// `$CODEX_HOME/config.toml` (`~/.codex/config.toml` without it), as this process's environment names it; None when it cannot be read.
pub fn codex_home_config() -> Option<String> {
    let home = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".codex"))
    })?;
    read_codex_config(&home.join("config.toml"))
}

/// `<workspace>/.codex/config.toml`, when present. Returns `None` only if no config entry exists there; if an entry exists but is a
/// symlink, oversized, or unreadable, returns a non-TOML marker so [`codex_shell_exclude_with_project`] fails closed and enforces
/// `MIGHTY_*` exclusion.
pub fn codex_project_config(workspace: &Path) -> Option<String> {
    let path = workspace.join(".codex").join("config.toml");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some("[".to_string()),
        Ok(_) => Some(read_codex_config(&path).unwrap_or_else(|| "[".to_string())),
    }
}

/// Extracts `shell_environment_policy` from a parsed TOML config and notes whether any profile defines one too.
fn project_touches_shell_policy(table: &toml::Table) -> bool {
    if table.contains_key("shell_environment_policy") {
        return true;
    }
    table
        .get("profiles")
        .and_then(toml::Value::as_table)
        .is_some_and(|profiles| profiles.values().filter_map(toml::Value::as_table).any(|p| p.contains_key("shell_environment_policy")))
}

fn collect_excludes(policy: Option<&toml::Table>, patterns: &mut Vec<String>) {
    if let Some(list) = policy.and_then(|p| p.get("exclude")).and_then(toml::Value::as_array) {
        for item in list.iter().filter_map(toml::Value::as_str) {
            if !patterns.iter().any(|existing| existing == item) {
                patterns.push(item.to_string());
            }
        }
    }
}

/// The value of `-c shell_environment_policy.exclude=…` for a Codex run: the user's own `exclude` (from `config`, the text of their
/// Codex `config.toml`) with [`CODEX_SHELL_EXCLUDE`] added, as a TOML array. A `-c` override replaces only that one key, so the
/// user's `inherit`, `include_only`, `set` and `ignore_default_excludes` stand. None when their policy uses `filters`, which Codex
/// refuses beside `exclude` (that run's shell then keeps what their filters let through).
pub fn codex_shell_exclude(config: Option<&str>) -> Option<String> {
    codex_shell_exclude_with_project(config, None)
}

/// Like [`codex_shell_exclude`], also inspecting a workspace-local `.codex/config.toml` (`project`) so a repo-local Codex config
/// cannot silently override `MIGHTY_*` exclusion via `shell_environment_policy` (`exclude`, `filters`, or a profile override).
pub fn codex_shell_exclude_with_project(home: Option<&str>, project: Option<&str>) -> Option<String> {
    let home_table: Option<toml::Table> = home.and_then(|text| text.parse().ok());
    let home_policy = home_table.as_ref().and_then(|t| t.get("shell_environment_policy")).and_then(toml::Value::as_table);

    let (project_table, project_invalid) = match project {
        None => (None, false),
        Some(text) => match text.parse::<toml::Table>() {
            Ok(table) => (Some(table), false),
            Err(_) => (None, true),
        },
    };
    let project_policy = project_table.as_ref().and_then(|t| t.get("shell_environment_policy")).and_then(toml::Value::as_table);
    let project_overrides_policy = project_invalid || project_table.as_ref().is_some_and(project_touches_shell_policy);

    // Only skip `-c shell_environment_policy.exclude` when the trusted user home config uses `filters` AND the repository itself
    // does not define or tamper with `shell_environment_policy`.
    if home_policy.is_some_and(|p| p.contains_key("filters")) && !project_overrides_policy {
        return None;
    }

    let mut patterns = Vec::new();
    collect_excludes(home_policy, &mut patterns);
    collect_excludes(project_policy, &mut patterns);
    if !patterns.iter().any(|p| p == CODEX_SHELL_EXCLUDE) {
        patterns.push(CODEX_SHELL_EXCLUDE.to_string());
    }
    Some(serde_json::to_string(&patterns).unwrap_or_else(|_| format!("[\"{CODEX_SHELL_EXCLUDE}\"]")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("mighty-mcp-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    /// The file is new, the user's alone in a folder of the user's alone, and holds what was written; a sweep takes only old ones.
    #[test]
    fn a_config_file_is_private_and_a_sweep_takes_only_stale_ones() {
        let place = folder("write");
        let path = write(&place, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        assert!(path.file_name().unwrap().to_str().is_some_and(ours));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "the file is the user's alone");
            assert_eq!(std::fs::metadata(&place).unwrap().permissions().mode() & 0o777, 0o700, "so is its folder");
        }
        let other = place.join("notes.json");
        std::fs::write(&other, "x").unwrap();
        assert_eq!(sweep(&place), 0, "a fresh file is a starting run's");
        let old = SystemTime::now() - Duration::from_secs(120);
        for file in [&path, &other] {
            std::fs::File::options().write(true).open(file).unwrap().set_modified(old).unwrap();
        }
        assert_eq!(sweep(&place), 1, "an old one is swept");
        assert!(!path.exists() && other.exists(), "and only these files");
        let _ = std::fs::remove_dir_all(&place);
    }

    /// `MIGHTY_*` joins the user's own `exclude`; their policy's other keys are not named; with `filters` nothing is added.
    #[test]
    fn the_codex_shell_exclude_keeps_the_users_own() {
        assert_eq!(codex_shell_exclude(None).as_deref(), Some(r#"["MIGHTY_*"]"#), "no config");
        assert_eq!(codex_shell_exclude(Some("not toml [")).as_deref(), Some(r#"["MIGHTY_*"]"#));
        let users =
            "[shell_environment_policy]\ninherit = \"core\"\ninclude_only = [\"PATH\", \"HOME\"]\nexclude = [\"AWS_*\", \"MIGHTY_*\"]\n";
        assert_eq!(codex_shell_exclude(Some(users)).as_deref(), Some(r#"["AWS_*","MIGHTY_*"]"#), "theirs kept, ours once");
        let filters = "[shell_environment_policy]\nfilters = [{ exclude = [\"AWS_*\"] }]\n";
        assert_eq!(codex_shell_exclude(Some(filters)), None, "Codex refuses exclude beside filters");
    }

    /// P1-SEC-1: project-local `.codex/config.toml` is inspected so a repo cannot silently override `MIGHTY_*` exclusion.
    #[test]
    fn project_local_codex_config_cannot_override_mighty_exclusion() {
        let user_exclude = "[shell_environment_policy]\nexclude = [\"AWS_*\"]\n";
        let user_filters = "[shell_environment_policy]\nfilters = [{ exclude = [\"AWS_*\"] }]\n";
        let repo_exclude = "[shell_environment_policy]\nexclude = [\"GCP_*\"]\n";
        let repo_filters = "[shell_environment_policy]\nfilters = [{ include = [\"*\"] }]\n";
        let repo_profile = "[profiles.dev.shell_environment_policy]\nexclude = [\"NONE\"]\n";
        let repo_unrelated = "model = \"o3\"\n";

        assert_eq!(
            codex_shell_exclude_with_project(Some(user_exclude), Some(repo_exclude)).as_deref(),
            Some(r#"["AWS_*","GCP_*","MIGHTY_*"]"#),
            "merges user and project excludes and always keeps MIGHTY_*"
        );
        assert_eq!(
            codex_shell_exclude_with_project(None, Some(repo_filters)).as_deref(),
            Some(r#"["MIGHTY_*"]"#),
            "repo-local filters must not suppress MIGHTY_* exclude override"
        );
        assert_eq!(
            codex_shell_exclude_with_project(Some(user_filters), Some(repo_filters)).as_deref(),
            Some(r#"["MIGHTY_*"]"#),
            "repo-local filters cannot ride on user filters to bypass MIGHTY_* exclusion"
        );
        assert_eq!(
            codex_shell_exclude_with_project(Some(user_filters), Some(repo_exclude)).as_deref(),
            Some(r#"["GCP_*","MIGHTY_*"]"#),
            "repo-local exclude overrides user filters, so MIGHTY_* exclusion must be enforced"
        );
        assert_eq!(
            codex_shell_exclude_with_project(Some(user_filters), Some(repo_profile)).as_deref(),
            Some(r#"["MIGHTY_*"]"#),
            "repo-local profile shell_environment_policy must fail closed"
        );
        assert_eq!(
            codex_shell_exclude_with_project(Some(user_filters), Some("not toml [")).as_deref(),
            Some(r#"["MIGHTY_*"]"#),
            "unparseable repo-local config must fail closed"
        );
        assert_eq!(
            codex_shell_exclude_with_project(Some(user_filters), Some(repo_unrelated)),
            None,
            "unrelated repo config still respects trusted user filters"
        );

        // Disk reading via `codex_project_config`: absent -> None, present -> Some(content), symlink -> fail closed.
        let workspace = folder("project-codex");
        std::fs::create_dir_all(&workspace).unwrap();
        assert_eq!(codex_project_config(&workspace), None);
        std::fs::create_dir_all(workspace.join(".codex")).unwrap();
        let config_path = workspace.join(".codex").join("config.toml");
        std::fs::write(&config_path, repo_exclude).unwrap();
        let loaded = codex_project_config(&workspace);
        assert_eq!(codex_shell_exclude_with_project(None, loaded.as_deref()).as_deref(), Some(r#"["GCP_*","MIGHTY_*"]"#));
        #[cfg(unix)]
        {
            let target = workspace.join("real.toml");
            std::fs::write(&target, repo_unrelated).unwrap();
            std::fs::remove_file(&config_path).unwrap();
            std::os::unix::fs::symlink(&target, &config_path).unwrap();
            let linked = codex_project_config(&workspace);
            assert_eq!(
                codex_shell_exclude_with_project(Some(user_filters), linked.as_deref()).as_deref(),
                Some(r#"["MIGHTY_*"]"#),
                "symlinked repo-local .codex/config.toml must fail closed"
            );
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }
}

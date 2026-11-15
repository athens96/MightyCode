//! What runs before the window (W9; C# `WinUI\Program.cs:158-205`): the user's environment taken back after a bare start, the write
//! policy, and the pre-UI migration of the data an app from before the rename left. Owners: `repair_environment` W9-B; the rest W9-F.
//!
//! The write policy (plan §4, D3) replaces W6's single input to the gates (`--allow-state-write`): a release-stamped build saves the
//! state and may change the machine by default, a development build only with `--allow-state-write`, and `--read-only` turns writing
//! off in any build. A smoke is unchanged: it writes only with `--allow-state-write` in any build (so `smoke.ps1 -Profile` stays
//! read-only on a packaged release exe), and never changes the machine whatever the policy (`Host::may_change_machine`). The gates keep
//! their bodies; only their input moves here: the store gets `policy.writes`, `Host::writes` reads the store, and every gate
//! (`may_change_machine`, `may_listen`, `may_reach_relay`, `may_touch_repo`, `delegation_live`, `AutomaticUpdatePolicy`) reads that.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::rename_migration::{self, RegistryStore};

use crate::args::Options;
use crate::build_info;

/// Whether this run writes its profile (and, outside a smoke, may change the machine: `Host::may_change_machine` and the gates over it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WritePolicy {
    pub writes: bool,
}

/// The policy for this run: this build's stamp ([`build_info::RELEASE`]) over the options ([`policy`]).
pub fn write_policy(options: &Options) -> WritePolicy {
    policy(build_info::RELEASE, options)
}

/// Plan D3's rule: `--read-only` never writes; `--allow-state-write` writes in any build; otherwise a release build writes outside a
/// smoke and a development build does not.
fn policy(release: bool, options: &Options) -> WritePolicy {
    WritePolicy { writes: !options.read_only && (options.allow_state_write || (release && !options.smoke_test)) }
}

/// C# `UserEnvironment.RepairCurrentProcess` (Program.cs:161): an app started from a bare environment (only `SystemRoot`, as an update
/// helper from before the fix started it) takes the user's own variables back before anything runs — every missing or empty one from
/// `CreateEnvironmentBlock`, none present overridden — through `std::env::set_var` while no other thread exists yet (it runs first in
/// `main`, after the exe's other modes). Step 0 stub (W9-B writes it): nothing changes.
pub fn repair_environment() {}

/// Whether RenameMigration runs (C# Program.cs:169: the default profile outside a smoke; plan §4: and only where the policy writes,
/// as it moves folders and writes HKCU).
fn migrates(options: &Options, policy: WritePolicy) -> bool {
    options.profile.is_none() && !options.smoke_test && policy.writes
}

/// What runs after the arguments and the platform, before the store reads the profile (C# Program.cs:169 `MigrateRenamedIdentifiers`):
/// RenameMigration for the default profile outside a smoke where the policy writes — `%APPDATA%\MightyClaudeNative`, `~\.mightyclaude`
/// with the records and Claude's conversations that name it, and the HKCU key (mighty-core's `rename_migration`) — then `git worktree
/// repair` in each moved worktree on a task of its own (C#'s `Task.Run`), started now and run while the app starts. A failure leaves the
/// old data where it was.
pub fn before_ui(options: &Options, platform: &mighty_platform::Platform, policy: WritePolicy) {
    if !migrates(options, policy) {
        return;
    }
    // The default profile's data folder is `<roaming>\MightyCode`: its parent is C#'s `SpecialFolder.ApplicationData`.
    let Some(app_data) = platform.paths.data_dir().parent().map(Path::to_path_buf) else { return };
    let home = crate::composer::dirs_home();
    let environment: HashMap<String, String> = platform.environment.current().values.into_iter().collect();
    let claude = mighty_history::history::claude_config(&home, &environment);
    let report = rename_migration::run(&app_data, &home, &claude, registry().as_ref());
    if report.worktrees.is_empty() {
        return;
    }
    let git = mighty_run::git::Git::new(platform.processes.clone(), hooks_folder(platform.paths.data_dir()));
    tauri::async_runtime::spawn(repair_worktrees(git, report.worktrees));
}

/// C# `RenameMigration.RepairWorktreesAsync`: each moved worktree's parent repository pointed at its new place, one git at a time,
/// with delegation's settings (C# `DelegationGit.Run`, its 10 s default).
async fn repair_worktrees(git: mighty_run::git::Git, worktrees: Vec<PathBuf>) {
    for worktree in worktrees {
        let _ = git.run_detailed(&["worktree", "repair"], &worktree, 10.0, None).await;
    }
}

/// The hooks folder git is pointed at while the migration repairs (C# `DelegationGit.NoHooks` before any profile is chosen:
/// `%APPDATA%\MightyCode\git-no-hooks`, the default profile's; `/dev/null` off Windows).
fn hooks_folder(data_dir: &Path) -> PathBuf {
    if cfg!(windows) { data_dir.join("git-no-hooks") } else { PathBuf::from("/dev/null") }
}

/// C# `CurrentUserRegistry`: HKEY_CURRENT_USER on Windows, a registry without keys elsewhere.
fn registry() -> Arc<dyn RegistryStore> {
    #[cfg(windows)]
    {
        Arc::new(CurrentUser(mighty_platform_windows::registry::CurrentUserRegistry::new()))
    }
    #[cfg(not(windows))]
    {
        Arc::new(rename_migration::NoRegistry)
    }
}

/// The Windows registry as RenameMigration reads and writes it.
#[cfg(windows)]
struct CurrentUser(mighty_platform_windows::registry::CurrentUserRegistry);

#[cfg(windows)]
impl RegistryStore for CurrentUser {
    fn key_exists(&self, path: &str) -> bool {
        self.0.key_exists(path)
    }
    fn sub_key_names(&self, path: &str) -> Vec<String> {
        self.0.sub_key_names(path)
    }
    fn values(&self, path: &str) -> Vec<rename_migration::RegistryValue> {
        let values = self.0.values(path).into_iter();
        values.map(|value| rename_migration::RegistryValue { name: value.name, kind: value.kind, data: value.data }).collect()
    }
    fn create_key(&self, path: &str) -> std::io::Result<()> {
        self.0.create_key(path)
    }
    fn set_value(&self, path: &str, value: &rename_migration::RegistryValue) -> std::io::Result<()> {
        self.0.set_value(path, &value.name, value.kind, &value.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 0's table in a development build: the flag writes, `--read-only` never does, nothing writes by default; and this build's
    /// policy is the rule over its own stamp.
    #[test]
    fn the_flag_writes_and_read_only_never_does() {
        let policy = |allow: bool, read_only: bool| {
            let options = Options { allow_state_write: allow, read_only, ..Options::default() };
            assert_eq!(write_policy(&options), super::policy(build_info::RELEASE, &options));
            super::policy(false, &options).writes
        };
        assert!(!policy(false, false), "a development build writes nothing unless asked");
        assert!(policy(true, false));
        assert!(!policy(true, true), "--read-only wins");
        assert!(!policy(false, true));
    }

    /// Plan §4's table, whole: release or development build × `--allow-state-write` × `--read-only` × smoke → whether the run writes its
    /// profile, and each gate's answer over it (`Host::may_change_machine` = writes ∧ not a smoke, which `may_listen`,
    /// `may_reach_relay` and `AutomaticUpdatePolicy` follow), and whether RenameMigration and the toast registration run (the default
    /// profile only).
    #[test]
    fn the_write_policy_table() {
        // (release, flag, read-only, smoke) → (writes, may change the machine)
        type Row = ((bool, bool, bool, bool), (bool, bool));
        let rows: [Row; 16] = [
            ((false, false, false, false), (false, false)),
            ((false, false, false, true), (false, false)),
            ((false, false, true, false), (false, false)),
            ((false, false, true, true), (false, false)),
            ((false, true, false, false), (true, true)),
            ((false, true, false, true), (true, false)),
            ((false, true, true, false), (false, false)),
            ((false, true, true, true), (false, false)),
            ((true, false, false, false), (true, true)),
            ((true, false, false, true), (false, false)),
            ((true, false, true, false), (false, false)),
            ((true, false, true, true), (false, false)),
            ((true, true, false, false), (true, true)),
            ((true, true, false, true), (true, false)),
            ((true, true, true, false), (false, false)),
            ((true, true, true, true), (false, false)),
        ];
        for ((release, allow, read_only, smoke), (writes, machine)) in rows {
            let options = Options {
                allow_state_write: allow,
                read_only,
                smoke_test: smoke,
                profile: smoke.then(|| PathBuf::from(r"C:\smoke")),
                ..Options::default()
            };
            let policy = policy(release, &options);
            let row = format!("release {release}, flag {allow}, read-only {read_only}, smoke {smoke}");
            assert_eq!(policy.writes, writes, "writes: {row}");
            // `Host::may_change_machine`'s body over the store's `writes`.
            assert_eq!(policy.writes && !smoke, machine, "may change the machine: {row}");
            assert_eq!(migrates(&options, policy), machine, "RenameMigration: {row}");
            let named = Options { profile: Some(PathBuf::from(r"C:\p")), ..options.clone() };
            assert!(!migrates(&named, policy), "never for --profile: {row}");
        }
    }

    /// The migration's Claude folder name is mighty-history's `ClaudeProjectFolder`, the one Claude names a child's conversations by.
    #[test]
    fn the_migration_names_claude_projects_as_history_does() {
        for path in [r"C:\Users\me\.mightyclaude\worktrees", "/Users/김/.mightycode/worktrees", r"D:\a b\é\x"] {
            assert_eq!(rename_migration::claude_project_folder(path), mighty_history::history::claude_project_folder(path));
        }
    }

    /// The repair's hooks folder is the default profile's own empty folder (C#'s), never the install folder; off Windows git's null.
    #[test]
    fn the_repair_runs_git_without_hooks() {
        let data = Path::new(r"C:\Users\me\AppData\Roaming\MightyCode");
        if cfg!(windows) {
            assert_eq!(hooks_folder(data), data.join("git-no-hooks"));
        } else {
            assert_eq!(hooks_folder(data), Path::new("/dev/null"));
        }
    }
}

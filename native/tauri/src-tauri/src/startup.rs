//! What runs before the window (W9; C# `WinUI\Program.cs:158-205`): the user's environment taken back after a bare start, the write
//! policy, and the pre-UI migration of the data an app from before the rename left. Owners: `repair_environment` W9-B; the rest W9-F.
//!
//! The write policy (plan §4, D3) replaces W6's single input to the gates (`--allow-state-write`): a release-stamped build saves the
//! state and may change the machine by default, a development build only with `--allow-state-write`, and `--read-only` turns writing
//! off in any build. A smoke never changes the machine whatever the policy (`Host::may_change_machine`). The gates keep their bodies;
//! only their input moves here.

use crate::args::Options;

/// Whether this run writes its profile (and, outside a smoke, may change the machine: `Host::may_change_machine` and the gates over it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WritePolicy {
    pub writes: bool,
}

/// The policy for this run. Step 0: `--allow-state-write` and not `--read-only`, as before W9 (W9-F adds the release build's default,
/// `build_info::RELEASE`).
pub fn write_policy(options: &Options) -> WritePolicy {
    WritePolicy { writes: options.allow_state_write && !options.read_only }
}

/// C# `UserEnvironment.RepairCurrentProcess` (Program.cs:161): an app started from a bare environment (only `SystemRoot`, as an update
/// helper from before the fix started it) takes the user's own variables back before anything runs — every missing or empty one from
/// `CreateEnvironmentBlock`, none present overridden — through `std::env::set_var` while no other thread exists yet (it runs first in
/// `main`, after the exe's other modes). Step 0 stub (W9-B writes it): nothing changes.
pub fn repair_environment() {}

/// What runs after the arguments and the platform, before the store reads the profile (C# Program.cs:169 `MigrateRenamedIdentifiers`):
/// RenameMigration for the default profile outside a smoke where the policy writes, the worktree repair handed to a task after start.
/// Step 0 stub (W9-F writes it): nothing moves.
pub fn before_ui(_options: &Options, _platform: &mighty_platform::Platform, _policy: WritePolicy) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 0's table (W9-F extends it with the release build's rows): the flag writes, `--read-only` never does, nothing writes by
    /// default in a development build.
    #[test]
    fn the_flag_writes_and_read_only_never_does() {
        let policy =
            |allow: bool, read_only: bool| write_policy(&Options { allow_state_write: allow, read_only, ..Options::default() }).writes;
        assert!(!policy(false, false), "a development build writes nothing unless asked");
        assert!(policy(true, false));
        assert!(!policy(true, true), "--read-only wins");
        assert!(!policy(false, true));
    }
}

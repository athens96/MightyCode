//! M-A: the process host. Swift `MightyCore/ProcessRunner.swift` (1,223 lines: `NativeChildProcess`, `ProcessCapture`,
//! `LiveRunRegistry`), `ToolkitProcess*`; contract `mighty_platform::process` (`ProcessHost`, `ChildControl`, `start_detached*`).
//!
//! To port (plan §1.3): `posix_spawn` with `POSIX_SPAWN_SETPGROUP` (group 0), an empty `SETSIGMASK`, `SETSIGDEF` for SIGPIPE,
//! `POSIX_SPAWN_CLOEXEC_DEFAULT`, `posix_spawn_file_actions_addchdir_np` and the pipes moved to fd ≥ 3 (`ProcessRunner.swift:62-84`);
//! stop = SIGTERM to the group then SIGKILL after 0.2 s, kill = SIGKILL to the group, signalled only while unreaped (`:231-245`);
//! children never outlive the app (`LiveRunRegistry`'s `signalAll` at quit, `killAll` at the 6 s deadline). `start_detached` and
//! `start_detached_waitable`: `POSIX_SPAWN_SETSID`, no pipes, the environment exactly as given (the source guard's callers only).
//! Un-gating the ~150 tests gated only for the process host (plan fact 8) is M-A's too.

use std::sync::Arc;

use mighty_platform::ProcessHost;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `ProcessHost` (Unsupported until M-A).
pub fn process_host() -> Arc<dyn ProcessHost> {
    Arc::new(Unsupported)
}

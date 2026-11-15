//! M-H: the exe mode `--verify-resources` (Swift `ResourceVerifier.swift:10-37`, run by the Mac build, notarize and install scripts
//! and excluded by 0.4.x's install-on-quit check, `AppUpdate.swift:344-351`): the locale catalogues, the default pet, the help files,
//! the setup catalogue, the Ghostty resource bundle and, when bundled, the CEF framework, checked from the running bundle; prints
//! `VERIFY_RESOURCES_OK` or `VERIFY_RESOURCES_FAILED: missing …`, exits 0 or 1, opens no window.

/// The argument (Swift's).
#[allow(dead_code, reason = "M-H serves the mode under this argument")]
pub const ARGUMENT: &str = "--verify-resources";

/// The mode, served before the arguments are parsed (main.rs `exe_mode`). Step 0: not this mode yet (None), so the argument still
/// reaches the parser and is refused, as before.
pub fn try_run(_args: &[String]) -> Option<i32> {
    None
}

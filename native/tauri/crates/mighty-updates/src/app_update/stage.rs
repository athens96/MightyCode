//! Unpacking a downloaded package (C# `AppUpdateService.Stage`, AppUpdateService.cs:185-259; plan §1.3, fact 3). Owner: W9-A.
//!
//! The same refusals the 0.4.x stager makes, which also decide whether a 0.4.x app accepts the Rust package: a zip of at most 512 MiB,
//! no absolute, `..`, link (`unix_mode()` with `S_IFLNK`) or escaping entry, exactly one `MightyCode.exe` in the whole tree, at the root
//! or one folder down, whose PE machine is x64 (`0x8664`) or arm64 (`0xAA64`) as `architecture` asks. The answer is the folder holding
//! the exe. The `zip` crate reads deflate only.

use std::path::{Path, PathBuf};

use super::UpdateError;

/// The PE machine of an x64 executable.
pub const MACHINE_X64: u16 = 0x8664;

/// The PE machine of an arm64 executable.
pub const MACHINE_ARM64: u16 = 0xAA64;

/// C# `AppUpdateService.Stage(package, destination, architecture)`. Step 0 stub (W9-A writes it): no package is staged.
pub fn stage(_package: &Path, _destination: &Path, _architecture: &str) -> Result<PathBuf, UpdateError> {
    Err(UpdateError::Plain("staging is not written yet (W9-A)".into()))
}

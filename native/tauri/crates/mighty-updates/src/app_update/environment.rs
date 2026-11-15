//! A bare environment and the user's own (C# `UserEnvironment.cs:14-41`, the pure half; plan §1.1 step 4). Owner: W9-A; the Win32 half
//! (`CreateEnvironmentBlock`) is W9-B's, today in mighty-platform-windows' `user_environment` (whose pure functions move here).
//!
//! The helper starts with only `SystemRoot`; an app started from such an environment takes the user's variables back before anything
//! runs (`startup::repair_environment`): bare when `USERPROFILE`, `APPDATA` or `LOCALAPPDATA` is missing or empty; every fresh variable
//! missing or empty now is added, none present is overridden. A block parses as `NAME=value` entries, the drive-letter `=C:` ones left out,
//! a later duplicate winning, names compared without case.

use std::collections::BTreeMap;

/// The variables whose absence marks a bare environment (C# `UserEnvironment.cs:15`).
pub const MARKERS: [&str; 3] = ["USERPROFILE", "APPDATA", "LOCALAPPDATA"];

/// C# `IsBare`. Step 0 stub (W9-A writes it): never bare.
pub fn is_bare(_current: &dyn Fn(&str) -> Option<String>) -> bool {
    false
}

/// C# `Missing(current, fresh)`: what to add from `fresh`. Step 0 stub (W9-A writes it): nothing.
pub fn missing(_current: &dyn Fn(&str) -> Option<String>, _fresh: &BTreeMap<String, String>) -> Vec<(String, String)> {
    Vec::new()
}

/// C# `Parse(block)`: a `CreateEnvironmentBlock` block (`NAME=value\0…\0\0`) as names and values (names kept as written; a later
/// duplicate, compared without case, replaces the earlier). Step 0 stub (W9-A writes it): empty.
pub fn parse(_block: &str) -> BTreeMap<String, String> {
    BTreeMap::new()
}

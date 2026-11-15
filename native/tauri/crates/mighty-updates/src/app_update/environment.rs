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

/// C# `IsBare`: any marker missing or empty in `current`.
pub fn is_bare(current: &dyn Fn(&str) -> Option<String>) -> bool {
    MARKERS.iter().any(|name| current(name).is_none_or(|value| value.is_empty()))
}

/// C# `Missing(current, fresh)`: each fresh variable with a name and a value that is missing or empty in `current` now; a variable the
/// process already has keeps its value, so a launch with its own settings is never overridden.
pub fn missing(current: &dyn Fn(&str) -> Option<String>, fresh: &BTreeMap<String, String>) -> Vec<(String, String)> {
    fresh
        .iter()
        .filter(|(name, value)| !name.is_empty() && !value.is_empty() && current(name).is_none_or(|now| now.is_empty()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// C# `Parse(block)`: a `CreateEnvironmentBlock` block (`NAME=value\0…\0\0`) as names and values. An entry without a name (the
/// drive-letter `=C:=C:\` ones begin with `=`) is left out; a later duplicate, compared without case, replaces the earlier's value and
/// keeps its name as first written (C#'s case-blind dictionary). Read it case-blind with [`get`].
pub fn parse(block: &str) -> BTreeMap<String, String> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for entry in block.split('\0') {
        let Some(equals) = entry.find('=').filter(|at| *at > 0) else { continue };
        let (name, value) = (&entry[..equals], &entry[equals + 1..]);
        match values.iter_mut().find(|(known, _)| super::dotnet::eq_ignore_case(known, name)) {
            Some((_, kept)) => *kept = value.to_string(),
            None => {
                values.insert(name.to_string(), value.to_string());
            }
        }
    }
    values
}

/// A variable of a parsed block, its name compared without case as C#'s dictionary does.
pub fn get<'a>(values: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    values.iter().find(|(known, _)| super::dotnet::eq_ignore_case(known, name)).map(|(_, value)| value.as_str())
}

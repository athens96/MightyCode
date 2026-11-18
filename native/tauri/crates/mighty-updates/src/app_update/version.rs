//! Versions (C# `AppVersion`, AppUpdateManifest.cs:128-169; plan §1.2). Owner: W9-A.
//!
//! `normalized`: trimmed, a leading `v`/`V` dropped, at most 64 characters, matching
//! `^[0-9]+(\.[0-9]+){0,3}(-[0-9A-Za-z.\-]+)?(\+[0-9A-Za-z.\-]+)?$`. `compare`: numeric parts (a missing one is 0, one past `int` is
//! 0 as `int.TryParse` leaves it), a release above its pre-release, pre-releases ordinal ignoring case, `+build` ignored, an
//! unparsable side equal. Checked against C# by `native/contracts/fixtures/update-versions.json`.

use std::cmp::Ordering;
use std::sync::OnceLock;

use regex::Regex;

/// The longest version text read.
pub const MAX_LENGTH: usize = 64;

fn pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^[0-9]+(\.[0-9]+){0,3}(-[0-9A-Za-z.\-]+)?(\+[0-9A-Za-z.\-]+)?$").expect("the version pattern"))
}

/// C# `AppVersion.Normalized`.
pub fn normalized(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let bare = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
    // .NET's Length counts UTF-16 units; a text that passes the pattern is ASCII, so the two counts only differ for texts refused
    // either way.
    (bare.encode_utf16().count() <= MAX_LENGTH && pattern().is_match(bare)).then(|| bare.to_string())
}

/// C#'s `Split`: the numeric parts (`int.TryParse` or 0) and the pre-release after the first `-` of the part before `+`.
fn split(version: &str) -> (Vec<i32>, Option<&str>) {
    let core = version.split('+').next().unwrap_or_default();
    let mut parts = core.splitn(2, '-');
    let numbers = parts.next().unwrap_or_default().split('.').map(|n| super::dotnet::parse_i32(n).unwrap_or(0)).collect();
    (numbers, parts.next())
}

/// C# `AppVersion.Compare`.
pub fn compare(left: &str, right: &str) -> Ordering {
    let (Some(left), Some(right)) = (normalized(left), normalized(right)) else { return Ordering::Equal };
    let (a, a_pre) = split(&left);
    let (b, b_pre) = split(&right);
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(ap), Some(bp)) => super::dotnet::compare_ignore_case(ap, bp),
    }
}

/// C# `AppVersion.IsNewer`: `candidate` above `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare(candidate, current) == Ordering::Greater
}

//! Versions (C# `AppVersion`, AppUpdateManifest.cs:128-169; plan §1.2). Owner: W9-A.
//!
//! `normalized`: trimmed, a leading `v`/`V` dropped, at most 64 characters, matching
//! `^[0-9]+(\.[0-9]+){0,3}(-[0-9A-Za-z.\-]+)?(\+[0-9A-Za-z.\-]+)?$`. `compare`: numeric parts (a missing one is 0), a release above its
//! pre-release, pre-releases ordinal ignoring case, `+build` ignored, an unparsable side equal.

use std::cmp::Ordering;

/// The longest version text read.
pub const MAX_LENGTH: usize = 64;

/// C# `AppVersion.Normalized`. Step 0 stub (W9-A writes it): no text reads as a version.
pub fn normalized(_text: &str) -> Option<String> {
    None
}

/// C# `AppVersion.Compare`. Step 0 stub (W9-A writes it): every pair compares equal, as two unparsable sides do.
pub fn compare(_left: &str, _right: &str) -> Ordering {
    Ordering::Equal
}

/// C# `AppVersion.IsNewer`: `candidate` above `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare(candidate, current) == Ordering::Greater
}

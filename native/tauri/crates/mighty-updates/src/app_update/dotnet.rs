//! The few .NET behaviours the update code leans on, written once so every module reads C#'s rule the same way: `Convert`'s base64,
//! `ToUpperInvariant`/`OrdinalIgnoreCase`, `long.TryParse`/`int.TryParse` (`NumberStyles.Integer`), `Path.GetFullPath`,
//! `string.TrimEnd(chars)` on a path, and `DateTimeOffset.ToString("O")`. Each is checked against C# by the parity fixtures
//! (`native/contracts/fixtures/update-*`, written by `tools/parity/Update.cs`).

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use chrono::{DateTime, Utc};

/// `Convert.FromBase64String` / `TryFromBase64String`: the four white-space characters .NET skips (space, tab, CR, LF; not form
/// feed or vertical tab) removed, then the standard alphabet with its padding required, non-zero bits in the last character
/// taken as .NET takes them (ignored). None where .NET refuses.
pub fn base64(text: &str) -> Option<Vec<u8>> {
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_encode_padding(true)
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    );
    let compact: String = text.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect();
    LENIENT.decode(compact.as_bytes()).ok()
}

/// .NET's invariant upper case of one character: the simple (one to one) Unicode mapping, so `ß` stays `ß`, with .NET's one
/// exception: the dotless `ı` stays itself (ICU would make it `I`; .NET keeps Windows' invariant rule).
pub fn upper_char(c: char) -> char {
    match c {
        'ı' => 'ı',
        // The Greek letters with ypogegrammeni whose full mapping has two characters: their simple mapping is the capital with
        // prosgegrammeni.
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => char::from_u32(c as u32 + 8).unwrap_or(c),
        '\u{1FB3}' => '\u{1FBC}',
        '\u{1FC3}' => '\u{1FCC}',
        '\u{1FF3}' => '\u{1FFC}',
        _ => {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(one), None) => one,
                // A character whose upper case is two or more (ß, ŉ, ǰ, the ligatures) has no simple mapping: it stays.
                _ => c,
            }
        }
    }
}

/// `string.ToUpperInvariant()`.
pub fn to_upper_invariant(text: &str) -> String {
    text.chars().map(upper_char).collect()
}

/// `string.Equals(a, b, StringComparison.OrdinalIgnoreCase)`.
pub fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.chars().map(upper_char).eq(b.chars().map(upper_char))
}

/// `string.Compare(a, b, StringComparison.OrdinalIgnoreCase)`'s sign: the invariant upper cases compared as UTF-16 code units.
pub fn compare_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let units = |s: &str| to_upper_invariant(s).encode_utf16().collect::<Vec<u16>>();
    units(a).cmp(&units(b))
}

/// `string.CompareOrdinal`'s sign: UTF-16 code units.
pub fn compare_ordinal(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `long.TryParse(text)` / `int.TryParse(text)` with `NumberStyles.Integer` and the invariant signs: white space (U+0009–U+000D,
/// U+0020) allowed around, one leading `+` or `-`, then ASCII digits only; an overflow refuses.
pub fn parse_integer(text: &str) -> Option<i128> {
    let trimmed = text.trim_matches(|c: char| matches!(c, '\u{9}'..='\u{d}' | ' '));
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value: i128 = 0;
    for b in digits.bytes() {
        value = value.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
        // Past any 64-bit value: stop before i128 itself could overflow on an absurdly long run of digits.
        if value > i128::from(u64::MAX) {
            return None;
        }
    }
    Some(if negative { -value } else { value })
}

/// `long.TryParse`.
pub fn parse_i64(text: &str) -> Option<i64> {
    parse_integer(text).and_then(|value| i64::try_from(value).ok())
}

/// `int.TryParse`.
pub fn parse_i32(text: &str) -> Option<i32> {
    parse_integer(text).and_then(|value| i32::try_from(value).ok())
}

/// `path.TrimEnd(separators)`: the trailing run of the given ASCII characters removed (a path of nothing but them becomes empty).
pub fn trim_end(path: &Path, separators: &[char]) -> PathBuf {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut end = bytes.len();
    while end > 0 && separators.iter().any(|c| c.is_ascii() && bytes[end - 1] == *c as u8) {
        end -= 1;
    }
    // SAFETY: only ASCII bytes were cut from the end, so the split falls right after a non-empty UTF-8 substring or at the very
    // start, as `from_encoded_bytes_unchecked` requires.
    PathBuf::from(unsafe { OsStr::from_encoded_bytes_unchecked(&bytes[..end]) })
}

/// `Path.DirectorySeparatorChar`: `\` on Windows, `/` elsewhere.
pub const SEPARATOR: char = std::path::MAIN_SEPARATOR;

/// `path.TrimEnd(Path.DirectorySeparatorChar)`.
pub fn trim_separator(path: &Path) -> PathBuf {
    trim_end(path, &[SEPARATOR])
}

/// `Path.GetFullPath(path)`: Windows' `GetFullPathNameW` (through `std::path::absolute`: `.`, `..`, `/` and repeated separators
/// resolved, a trailing separator kept); elsewhere the path joined to the working folder with `.` and `..` resolved by name, as
/// .NET does on macOS. An empty path is an error, as .NET's.
pub fn full_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if !absolute.components().any(|c| matches!(c, Component::CurDir | Component::ParentDir)) {
        return Ok(absolute);
    }
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    Ok(resolved)
}

/// `full + suffix` as a path (`Path.GetFullPath(x) + ".update-new"`).
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// `Path.GetFileName(path)`: the last component, or empty.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A UTC time as .NET's `DateTimeOffset.ToString("O")` writes it: `2026-11-15T03:04:05.1234567+00:00` (seven fraction digits,
/// .NET's 100 ns ticks).
pub fn round_trip(time: DateTime<Utc>) -> String {
    let ticks = time.timestamp_subsec_nanos().min(999_999_999) / 100;
    format!("{}.{ticks:07}+00:00", time.format("%Y-%m-%dT%H:%M:%S"))
}

/// .NET ticks (100 ns since 0001-01-01) as a UTC time; None outside chrono's range.
pub fn from_ticks(ticks: i64) -> Option<DateTime<Utc>> {
    const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
    let since = ticks.checked_sub(UNIX_EPOCH_TICKS)?;
    let seconds = since.div_euclid(10_000_000);
    let nanos = u32::try_from(since.rem_euclid(10_000_000) * 100).ok()?;
    DateTime::from_timestamp(seconds, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_parse_as_dotnet_reads_them() {
        assert_eq!(parse_i64(" 10 "), Some(10));
        assert_eq!(parse_i64("+10"), Some(10));
        assert_eq!(parse_i64("-0"), Some(0));
        assert_eq!(parse_i64("010"), Some(10));
        assert_eq!(parse_i64("1e3"), None);
        assert_eq!(parse_i64("1 0"), None);
        assert_eq!(parse_i64(""), None);
        assert_eq!(parse_i64("9223372036854775808"), None);
        assert_eq!(parse_i32("2147483648"), None);
        assert_eq!(parse_i32("\u{661}"), None, "only ASCII digits");
    }

    #[test]
    fn times_are_written_as_dotnet_round_trips_them() {
        let time = DateTime::from_timestamp(1_763_175_845, 123_456_789).unwrap();
        assert_eq!(round_trip(time), "2025-11-15T03:04:05.1234567+00:00");
        assert_eq!(round_trip(DateTime::from_timestamp(0, 0).unwrap()), "1970-01-01T00:00:00.0000000+00:00");
        assert_eq!(from_ticks(621_355_968_000_000_001), DateTime::from_timestamp(0, 100));
    }

    #[test]
    fn trimming_cuts_only_the_trailing_run() {
        assert_eq!(trim_end(Path::new("a//"), &['/']), PathBuf::from("a"));
        assert_eq!(trim_end(Path::new("//"), &['/']), PathBuf::from(""));
        assert_eq!(trim_end(Path::new("a/b"), &['/']), PathBuf::from("a/b"));
    }
}

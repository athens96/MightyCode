//! C# `UserEnvironmentVerification`'s Win32 half (MightyCode.Core.Tests/UserEnvironmentVerification.cs; plan L754): the block's parse
//! and the user's real block. The pure half (`IsBare`, `Missing`) is mighty-updates' `app_update::environment` and its tests (W9-A).
#![cfg(windows)]

use mighty_platform_windows::user_environment::{fresh, parse};

/// C# "an app started from a bare environment takes the user's own variables back" (UserEnvironmentVerification.cs:7-29), the parts
/// that read Windows' block.
#[test]
fn repair() {
    let fresh_block = parse(
        "=C:=C:\\\0USERPROFILE=C:\\Users\\u\0APPDATA=C:\\Users\\u\\AppData\\Roaming\0PATHEXT=.COM;.EXE;.BAT;.CMD\0SystemRoot=D:\\Other\0Path=C:\\Windows\\System32\0EMPTY=\0\0",
    );
    assert!(!fresh_block.contains("") && !fresh_block.names().any(|name| name.starts_with('=')), "drive-letter entries are left out");
    assert!(
        fresh_block.get("userprofile") == Some(r"C:\Users\u") && fresh_block.get("PATH") == Some(r"C:\Windows\System32"),
        "the parsed block is case-blind"
    );

    assert_eq!(fresh_block.get("EMPTY"), Some(""), "an empty value is kept as written");

    // On Windows the real block names the user's home, so a relaunched app has one.
    let real = fresh().expect("Windows builds the user's environment");
    assert!(
        real.get("USERPROFILE").is_some_and(|home| !home.is_empty()) && real.contains("PATHEXT"),
        "the user's fresh environment has USERPROFILE and PATHEXT"
    );
}

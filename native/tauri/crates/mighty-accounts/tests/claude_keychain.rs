//! The Mac's read of the Claude CLI's sign-in for the account usage (Mac stage M-B, Swift `ClaudeQuotaCredentials.read`), over the
//! `ClaudeCredentials` port's fake: the Keychain first, the CLI's file after it, and a Keychain that would ask staying silent until the
//! user asks. The Mac's own Keychain reader is mighty-platform-macos `claude_credentials.rs` (tested in a throwaway keychain there).

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use mighty_accounts::usage::{ClaudeSignIn, claude_credential};
use mighty_platform::accounts::FakeClaudeCredentials;

const SIGN_IN: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-keychain","subscriptionType":"max","scopes":["user:profile"]}}"#;

fn home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("mighty-claude-keychain-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// `AccountUsageTests.swift:52 "keychainPermissionStaysSilentUntilAnInteractiveRead"` (the read's half; the service's hour-long
/// back-off before asking again is the app's, with the status line): a silent read where the Keychain would ask is `Permission`
/// without asking, and only an interactive one reads the item.
#[test]
fn keychain_permission_stays_silent_until_an_interactive_read() {
    let home = home("permission");
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).unwrap();
    let keychain = FakeClaudeCredentials::with(SIGN_IN, true);
    let environment = BTreeMap::new();
    let first = claude_credential(home.to_str().unwrap(), &environment, now, Some(&keychain), false);
    assert_eq!(first, ClaudeSignIn::Permission);
    let polled = claude_credential(home.to_str().unwrap(), &environment, now, Some(&keychain), false);
    assert_eq!(polled, ClaudeSignIn::Permission, "automatic reads never ask");
    let granted = claude_credential(home.to_str().unwrap(), &environment, now, Some(&keychain), true);
    let ClaudeSignIn::Found(credential) = granted else { panic!("an interactive read gets the item") };
    assert_eq!((credential.token.as_str(), credential.plan.as_deref()), ("sk-ant-keychain", Some("max")));
    assert_eq!(keychain.reads().iter().map(|(_, interactive)| *interactive).collect::<Vec<_>>(), [false, false, true]);
    assert!(!format!("{credential:?}").contains("sk-ant"), "the token never shows in Debug");
    let _ = std::fs::remove_dir_all(&home);
}

/// The CLI's file is read after the Keychain, so a Keychain that would ask costs nothing when the file has the sign-in; a custom
/// config folder asks the Keychain for its own item, and one that is not an absolute path is no sign-in at all.
#[test]
fn the_file_follows_the_keychain_and_a_custom_profile_keeps_its_own() {
    let home = home("file");
    let now = Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), SIGN_IN.replace("keychain", "file")).unwrap();
    let keychain = FakeClaudeCredentials::with(SIGN_IN, true);
    let ClaudeSignIn::Found(credential) = claude_credential(home.to_str().unwrap(), &BTreeMap::new(), now, Some(&keychain), false) else {
        panic!("the file's sign-in")
    };
    assert_eq!(credential.token, "sk-ant-file");
    let config: BTreeMap<String, String> = [("CLAUDE_CONFIG_DIR".to_string(), "/Users/me/.claude-work".to_string())].into();
    let open = FakeClaudeCredentials::with(SIGN_IN, false);
    assert!(matches!(claude_credential(home.to_str().unwrap(), &config, now, Some(&open), false), ClaudeSignIn::Found(_)));
    assert_eq!(open.reads(), [(Some(PathBuf::from("/Users/me/.claude-work")), false)]);
    let relative: BTreeMap<String, String> = [("CLAUDE_CONFIG_DIR".to_string(), "relative/dir".to_string())].into();
    assert_eq!(claude_credential(home.to_str().unwrap(), &relative, now, Some(&open), false), ClaudeSignIn::Missing);
    assert_eq!(open.reads().len(), 1, "a relative config folder asks the Keychain nothing");
    let _ = std::fs::remove_dir_all(&home);
}

/// `SessionUsageTests.swift:178 "claudeRateLimitEventFillsQuotaWindowsWithoutKeychain"`: the quota windows come from the CLI's own
/// `rate_limit_event`, no Keychain read involved (mighty-run's stream parser test covers the event end to end).
#[test]
fn claude_rate_limit_event_fills_quota_windows_without_keychain() {
    let info: serde_json::Value = serde_json::from_str(
        r#"{"status":"allowed_warning","rateLimitType":"seven_day","utilization":0.86,"resetsAt":1789668000,"unifiedWindows":{"five_hour":{"utilization":0.22,"resetsAt":1789641600},"seven_day":{"utilization":0.86,"resetsAt":1789668000}}}"#,
    )
    .unwrap();
    let limits = mighty_run::usage::rate_limit_windows(Some(&info));
    assert_eq!(limits.iter().map(|l| l.kind.as_str()).collect::<Vec<_>>(), ["five_hour", "seven_day"]);
    assert_eq!(limits.iter().map(|l| l.percent_used.map(|p| p.round() as i64)).collect::<Vec<_>>(), [Some(22), Some(86)]);
    assert!(limits[0].resets_at.as_deref().is_some_and(|r| r.starts_with("2026-09-")));
    let fallback: serde_json::Value =
        serde_json::from_str(r#"{"rateLimitType":"five_hour","utilization":0.5,"unifiedWindows":{"x":{"utilization":7}}}"#).unwrap();
    let fallback = mighty_run::usage::rate_limit_windows(Some(&fallback));
    assert_eq!((fallback.len(), fallback[0].kind.as_str(), fallback[0].percent_used), (1, "five_hour", Some(50.0)));
    let none: serde_json::Value = serde_json::from_str(r#"{"unifiedWindows":{"five_hour":{"utilization":true}}}"#).unwrap();
    assert!(mighty_run::usage::rate_limit_windows(Some(&none)).is_empty());
}

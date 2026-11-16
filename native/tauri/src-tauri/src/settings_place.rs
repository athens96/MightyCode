//! Mac stage M-J, `PlatformProfile::settings_store`: where the settings the state file and the Swift app's UserDefaults both name
//! are kept. Windows (`RegistryAndSnapshot`) keeps them in the state file, as C# does, and nothing here runs. The Mac (`UserDefaults`)
//! keeps them where the Swift app did, in the domain `dev.mightycode.native` under Swift's keys and types: at start they are read from
//! there into the snapshot (a missing key is Swift's default), and each save writes the ones that changed back there, so 0.4.x and
//! this app read the same choice. The rest of the host keeps reading the snapshot.
//!
//! The text size, the font families, `delegation.enabled` and `setup.neverShow` need nothing here: they go through the
//! `PrefsStore` on both OSes already (frame.rs, settings/general.rs, setup.rs).

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use mighty_core::AppSnapshot;
use mighty_platform::{PlatformProfile, PrefValue, PrefsStore, SettingsStore};
use tauri::Manager;

/// One setting both places name: its UserDefaults key, its value as Swift stores it (None: no value, Swift's default), and how a
/// stored value (as `PrefsStore::get` gives it, `prefs::from_swift`'s forms) is set on the snapshot.
struct Entry {
    key: &'static str,
    stored: fn(&AppSnapshot) -> Option<PrefValue>,
    restore: fn(&mut AppSnapshot, Option<PrefValue>),
}

/// Swift's `object(forKey:) as? Bool ?? default` (and `bool(forKey:)`, whose default is false).
fn switch(value: Option<PrefValue>, default: bool) -> bool {
    match value {
        Some(PrefValue::Bool(on)) => on,
        Some(PrefValue::Int(number)) => number != 0,
        Some(PrefValue::Float(number)) => number != 0.0,
        _ => default,
    }
}

fn text(value: Option<PrefValue>) -> Option<String> {
    match value {
        Some(PrefValue::Text(text)) => Some(text),
        _ => None,
    }
}

/// A Double Swift stores (`@AppStorage` of a size), read back as the shared forms give it (text) or as stored.
fn number(value: Option<PrefValue>) -> Option<f64> {
    match value {
        Some(PrefValue::Float(number)) => Some(number),
        Some(PrefValue::Int(number)) => Some(number as f64),
        Some(PrefValue::Text(text)) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite() && *number > 0.0)
}

/// A string dictionary (`[workspaceId: value]`), each value mapped (`None` drops the entry).
fn map(value: Option<PrefValue>, each: impl Fn(&str) -> Option<&'static str>) -> Option<IndexMap<String, Option<String>>> {
    let Some(PrefValue::Map(entries)) = value else { return None };
    let kept: IndexMap<String, Option<String>> = entries
        .into_iter()
        .filter_map(|(key, value)| match value {
            PrefValue::Text(text) => each(&text).map(|mapped| (key, Some(mapped.to_string()))),
            _ => None,
        })
        .collect();
    (!kept.is_empty()).then_some(kept)
}

fn stored_map(values: Option<&IndexMap<String, Option<String>>>, each: impl Fn(&str) -> Option<String>) -> Option<PrefValue> {
    let entries: Vec<(String, PrefValue)> = values?
        .iter()
        .filter_map(|(key, value)| value.as_deref().and_then(&each).map(|mapped| (key.clone(), PrefValue::Text(mapped))))
        .collect();
    (!entries.is_empty()).then_some(PrefValue::Map(entries))
}

/// .NET ticks at 1970.
const UNIX_TICKS: i64 = 621_355_968_000_000_000;

/// `appUpdateLastCheckedAt` (C#'s round-trip text) as Swift's `appUpdate.lastCheck` Date, and back.
fn check_date(text: Option<&str>) -> Option<PrefValue> {
    let ticks = mighty_core::time::parse(text)?.utc_ticks();
    Some(PrefValue::Date((ticks - UNIX_TICKS) as f64 / 10_000_000.0))
}

fn check_text(seconds: f64) -> Option<String> {
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.floor();
    let at = chrono::DateTime::<chrono::Utc>::from_timestamp(whole as i64, ((seconds - whole) * 1e9) as u32)?;
    Some(mighty_updates::app_update::dotnet::round_trip(at))
}

/// Swift's `WebOpenDestination` raw values beside the state file's (`in_app` is `inApp`).
fn web_open_from_swift(value: &str) -> Option<&'static str> {
    match value {
        "in_app" => Some("inApp"),
        "external" => Some("external"),
        _ => None,
    }
}

fn web_open_to_swift(value: &str) -> Option<String> {
    match value {
        "inApp" => Some("in_app".into()),
        "external" => Some("external".into()),
        _ => None,
    }
}

/// Swift's `SettingsPane` raw values (SettingsViews.swift:155-156), the state file's too.
const SETTINGS_PANES: [&str; 9] = ["general", "models", "styles", "tools", "cli", "mobile", "remote", "companion", "about"];

/// A trusted status line the state file keeps (normalize's rule): a workspace id and a fingerprint of 1 to 64 UTF-16 units.
fn trusted(workspace: &str, fingerprint: &str) -> bool {
    mighty_core::wire::identifier(workspace) && (1..=64).contains(&mighty_core::wire::utf16_len(fingerprint))
}

/// Every setting the Swift app keeps in UserDefaults that the state file also names (Swift's sites: LocaleBundle.swift:80,
/// AppStore+AppUpdate.swift:31-33, WebOpenChoice.swift:108, SettingsViews.swift:205, AppStore+StatusLine.swift:20-21,
/// CefBrowserEngine.swift:49, StatusBarUsage.swift:24, MightyGraphView.swift:118-119).
const ENTRIES: [Entry; 12] = [
    Entry {
        key: "language",
        stored: |s| Some(PrefValue::Text(s.language_preference().to_string())),
        restore: |s, v| {
            let chosen = text(v).filter(|l| mighty_core::locale::LANGUAGES.contains(&l.as_str()));
            s.language_preference = Some(chosen.unwrap_or_else(|| "system".into()));
        },
    },
    Entry {
        key: "appUpdate.automatic",
        stored: |s| Some(PrefValue::Bool(s.app_update_auto_check)),
        restore: |s, v| s.app_update_auto_check = switch(v, true),
    },
    Entry {
        key: "appUpdate.lastCheck",
        stored: |s| check_date(s.app_update_last_checked_at.as_deref()),
        restore: |s, v| {
            s.app_update_last_checked_at = match v {
                Some(PrefValue::Date(seconds)) => check_text(seconds),
                _ => None,
            }
        },
    },
    Entry {
        key: "appUpdate.manifestURL",
        stored: |s| s.app_update_manifest_url_override.clone().filter(|u| !u.is_empty()).map(PrefValue::Text),
        restore: |s, v| s.app_update_manifest_url_override = text(v).filter(|u| !u.is_empty()),
    },
    Entry {
        key: "agentTerminal.webOpenChoices",
        stored: |s| stored_map(s.agent_web_open_choices.as_ref(), web_open_to_swift),
        restore: |s, v| s.agent_web_open_choices = map(v, web_open_from_swift),
    },
    Entry {
        key: "settingsPane",
        stored: |s| s.settings_pane.clone().map(PrefValue::Text),
        restore: |s, v| {
            s.settings_pane = Some(text(v).filter(|p| SETTINGS_PANES.contains(&p.as_str())).unwrap_or_else(|| "general".into()))
        },
    },
    Entry {
        key: "statusLine.enabled",
        stored: |s| Some(PrefValue::Bool(s.status_line_enabled)),
        restore: |s, v| s.status_line_enabled = switch(v, true),
    },
    Entry {
        key: "statusLine.trusted",
        stored: |s| stored_map(s.trusted_status_lines.as_ref(), |fingerprint| Some(fingerprint.to_string())),
        restore: |s, v| {
            s.trusted_status_lines = match v {
                Some(PrefValue::Map(entries)) => {
                    let kept: IndexMap<String, Option<String>> = entries
                        .into_iter()
                        .filter_map(|(k, v)| v.text().filter(|t| trusted(&k, t)).map(|t| (k, Some(t.to_string()))))
                        .collect();
                    (!kept.is_empty()).then_some(kept)
                }
                _ => None,
            }
        },
    },
    Entry {
        key: "browser.engineEnabled",
        stored: |s| Some(PrefValue::Bool(s.browser_engine_enabled)),
        restore: |s, v| s.browser_engine_enabled = switch(v, false),
    },
    Entry {
        key: "usage.claudeKeychain",
        stored: |s| Some(PrefValue::Bool(s.claude_direct_usage_lookup_enabled)),
        restore: |s, v| s.claude_direct_usage_lookup_enabled = switch(v, false),
    },
    Entry {
        key: "mighty.referenceBubble.width",
        stored: |s| s.reference_bubble_width.map(PrefValue::Float),
        restore: |s, v| s.reference_bubble_width = number(v),
    },
    Entry {
        key: "mighty.referenceBubble.height",
        stored: |s| s.reference_bubble_height.map(PrefValue::Float),
        restore: |s, v| s.reference_bubble_height = number(v),
    },
];

/// The keys this file moves between the two places.
#[cfg(test)]
pub fn keys() -> Vec<&'static str> {
    ENTRIES.iter().map(|entry| entry.key).collect()
}

/// Where the profile keeps them: UserDefaults (the Mac) or the state file (Windows: nothing to do).
fn in_prefs(profile: &PlatformProfile) -> bool {
    profile.settings_store == SettingsStore::UserDefaults
}

/// At start, before anything reads the snapshot: on the Mac, each setting from UserDefaults (a key that is missing or does not read
/// is Swift's default). Windows: the snapshot as loaded.
pub fn read_into(profile: &PlatformProfile, prefs: &dyn PrefsStore, snapshot: &mut AppSnapshot) {
    if !in_prefs(profile) {
        return;
    }
    for entry in &ENTRIES {
        (entry.restore)(snapshot, prefs.get(entry.key).ok().flatten());
    }
}

/// The values as Swift stores them, key by key.
fn values(snapshot: &AppSnapshot) -> Vec<Option<PrefValue>> {
    ENTRIES.iter().map(|entry| (entry.stored)(snapshot)).collect()
}

/// On the Mac, each setting that differs between `before` and `after` written to UserDefaults (removed when it went back to no
/// value), and the system interface's language following a new choice (Swift `AppLanguage.applyToSystemInterface`). The keys that
/// did not take, with why. Windows: nothing.
pub fn write_changes(profile: &PlatformProfile, prefs: &dyn PrefsStore, before: &AppSnapshot, after: &AppSnapshot) -> Vec<String> {
    if !in_prefs(profile) {
        return Vec::new();
    }
    let mut failures = Vec::new();
    for ((entry, old), new) in ENTRIES.iter().zip(values(before)).zip(values(after)) {
        if old == new {
            continue;
        }
        let written = match new {
            Some(value) => prefs.set(entry.key, value),
            None => prefs.remove(entry.key),
        };
        let written =
            written.and_then(|()| if entry.key == "language" { mighty_platform::prefs::apply_to_system_interface(prefs) } else { Ok(()) });
        if let Err(error) = written {
            failures.push(format!("{}: {error}", entry.key));
        }
    }
    failures
}

/// The settings as UserDefaults last had them from this app (what [`mirror`] compares a save with).
static WRITTEN: Mutex<Option<AppSnapshot>> = Mutex::new(None);

fn written() -> std::sync::MutexGuard<'static, Option<AppSnapshot>> {
    WRITTEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// main.rs, at start: the settings read into the loaded state (not in a smoke, which keeps its own profile's), and remembered as
/// what UserDefaults holds.
pub fn load(profile: &PlatformProfile, prefs: &dyn PrefsStore, snapshot: &mut AppSnapshot, smoke: bool) {
    if !smoke {
        read_into(profile, prefs, snapshot);
    }
    *written() = Some(snapshot.clone());
}

/// The host's saver, with each state it saves: on the Mac, the settings that changed since the last write go to UserDefaults, only
/// where the app may change the machine (never a smoke, never a read-only or development run without `--allow-state-write`).
pub fn mirror(app: &tauri::AppHandle, snapshot: &AppSnapshot) {
    let profile = crate::platform::profile();
    if !in_prefs(&profile) {
        return;
    }
    let Some(host) = app.try_state::<Arc<crate::host::Host>>() else { return };
    let Some(prefs) = app.try_state::<Arc<dyn PrefsStore>>() else { return };
    if !host.may_change_machine() {
        return;
    }
    let mut last = written();
    let Some(before) = last.as_ref() else { return };
    for failure in write_changes(&profile, prefs.as_ref(), before, snapshot) {
        host.log.line(&format!("setting not saved: {failure}"));
    }
    *last = Some(snapshot.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::fake::MemoryPrefs;

    fn chosen() -> AppSnapshot {
        AppSnapshot {
            language_preference: Some("ja".into()),
            app_update_auto_check: false,
            app_update_last_checked_at: Some("2026-11-16T03:04:05.5000000+00:00".into()),
            app_update_manifest_url_override: Some("https://example.invalid/latest.json".into()),
            agent_web_open_choices: Some(IndexMap::from([("w1".into(), Some("inApp".into())), ("w2".into(), Some("external".into()))])),
            settings_pane: Some("remote".into()),
            status_line_enabled: false,
            trusted_status_lines: Some(IndexMap::from([("w1".into(), Some("0123456789abcdef".into()))])),
            browser_engine_enabled: true,
            claude_direct_usage_lookup_enabled: true,
            reference_bubble_width: Some(420.0),
            reference_bubble_height: Some(300.0),
            ..AppSnapshot::default()
        }
    }

    /// Swift's own values in its domain read into the snapshot, in the state file's forms, on the Mac; Windows reads none of them.
    #[test]
    fn the_mac_reads_the_settings_swift_kept_in_user_defaults() {
        let prefs = MemoryPrefs::default();
        prefs.set("language", PrefValue::Text("ko".into())).unwrap();
        prefs.set("appUpdate.automatic", PrefValue::Bool(false)).unwrap();
        prefs.set("appUpdate.lastCheck", PrefValue::Date(1_700_000_000.25)).unwrap();
        prefs.set("appUpdate.manifestURL", PrefValue::Text("https://example.invalid/m.json".into())).unwrap();
        prefs
            .set(
                "agentTerminal.webOpenChoices",
                PrefValue::Map(vec![("w1".into(), PrefValue::Text("in_app".into())), ("w2".into(), PrefValue::Text("sideways".into()))]),
            )
            .unwrap();
        prefs.set("settingsPane", PrefValue::Text("cli".into())).unwrap();
        prefs.set("statusLine.enabled", PrefValue::Bool(false)).unwrap();
        prefs.set("statusLine.trusted", PrefValue::Map(vec![("w1".into(), PrefValue::Text("feed".into()))])).unwrap();
        prefs.set("browser.engineEnabled", PrefValue::Bool(true)).unwrap();
        prefs.set("usage.claudeKeychain", PrefValue::Bool(true)).unwrap();
        prefs.set("mighty.referenceBubble.width", PrefValue::Text("420".into())).unwrap();

        let loaded = AppSnapshot { language_preference: Some("en".into()), ..AppSnapshot::default() };
        let mut windows = loaded.clone();
        read_into(&PlatformProfile::windows(), &prefs, &mut windows);
        assert_eq!(windows, loaded, "Windows keeps them in the state file");

        let mut mac = loaded;
        read_into(&PlatformProfile::macos(), &prefs, &mut mac);
        assert_eq!(mac.language_preference.as_deref(), Some("ko"));
        assert!(!mac.app_update_auto_check);
        assert_eq!(mac.app_update_last_checked_at.as_deref(), Some("2023-11-14T22:13:20.2500000+00:00"));
        assert_eq!(mac.app_update_manifest_url_override.as_deref(), Some("https://example.invalid/m.json"));
        assert_eq!(mac.agent_web_open_choices, Some(IndexMap::from([("w1".into(), Some("inApp".into()))])), "Swift's in_app; no stranger");
        assert_eq!(mac.settings_pane.as_deref(), Some("cli"));
        assert!(!mac.status_line_enabled && mac.browser_engine_enabled && mac.claude_direct_usage_lookup_enabled);
        assert_eq!(mac.trusted_status_lines, Some(IndexMap::from([("w1".into(), Some("feed".into()))])));
        assert_eq!((mac.reference_bubble_width, mac.reference_bubble_height), (Some(420.0), None));
    }

    /// A domain Swift never wrote reads as Swift's defaults, whatever the state file said.
    #[test]
    fn a_missing_key_is_swifts_default() {
        let mut mac = chosen();
        read_into(&PlatformProfile::macos(), &MemoryPrefs::default(), &mut mac);
        assert_eq!(mac.language_preference.as_deref(), Some("system"));
        assert!(mac.app_update_auto_check && mac.status_line_enabled, "on until switched off");
        assert!(!mac.browser_engine_enabled && !mac.claude_direct_usage_lookup_enabled, "off until switched on");
        assert_eq!(mac.settings_pane.as_deref(), Some("general"));
        assert!(mac.app_update_last_checked_at.is_none() && mac.app_update_manifest_url_override.is_none());
        assert!(mac.agent_web_open_choices.is_none() && mac.trusted_status_lines.is_none() && mac.reference_bubble_width.is_none());
    }

    /// What changed is written in Swift's keys and types, read back the same, and the system interface follows the language; what
    /// did not change is not written; Windows writes nothing.
    #[test]
    fn a_change_is_written_where_swift_reads_it() {
        let before = AppSnapshot::default();
        let after = chosen();
        let windows = MemoryPrefs::default();
        assert!(write_changes(&PlatformProfile::windows(), &windows, &before, &after).is_empty());
        assert!(keys().iter().all(|key| windows.get(key).unwrap().is_none()), "Windows keeps them in the state file");

        let prefs = MemoryPrefs::default();
        assert!(write_changes(&PlatformProfile::macos(), &prefs, &before, &after).is_empty());
        assert_eq!(prefs.get("language").unwrap(), Some(PrefValue::Text("ja".into())));
        assert_eq!(prefs.get("AppleLanguages").unwrap(), Some(PrefValue::List(vec![PrefValue::Text("ja".into())])));
        assert_eq!(prefs.get("appUpdate.automatic").unwrap(), Some(PrefValue::Bool(false)));
        assert_eq!(prefs.get("appUpdate.lastCheck").unwrap(), Some(PrefValue::Date(1_794_798_245.5)));
        assert_eq!(
            prefs.get("agentTerminal.webOpenChoices").unwrap(),
            Some(PrefValue::Map(vec![("w1".into(), PrefValue::Text("in_app".into())), ("w2".into(), PrefValue::Text("external".into()))]))
        );
        assert_eq!(prefs.get("mighty.referenceBubble.width").unwrap(), Some(PrefValue::Float(420.0)));
        let mut read = AppSnapshot::default();
        read_into(&PlatformProfile::macos(), &prefs, &mut read);
        for (entry, (left, right)) in ENTRIES.iter().zip(values(&read).into_iter().zip(values(&after))) {
            assert_eq!(left, right, "{} reads back as written", entry.key);
        }

        // Back to no value: the key goes; an unchanged setting is not touched.
        prefs.set("statusLine.enabled", PrefValue::Text("left alone".into())).unwrap();
        let cleared = AppSnapshot { app_update_manifest_url_override: None, ..after.clone() };
        assert!(write_changes(&PlatformProfile::macos(), &prefs, &after, &cleared).is_empty());
        assert_eq!(prefs.get("appUpdate.manifestURL").unwrap(), None);
        assert_eq!(prefs.get("statusLine.enabled").unwrap(), Some(PrefValue::Text("left alone".into())));
    }
}

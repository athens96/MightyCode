//! The settings' words where the Windows and Mac apps differ (Mac integration review 2): the Windows app's keys (C#'s, which the page
//! and this host use) and, on the Mac, the Swift app's keys for the same rows (SettingsView.swift, AgentCompanionViews.swift,
//! MobileRemoteSettingsView.swift). A row Swift has no words for that would be wrong on the Mac is not shown there.
//!
//! The words a crate reads through `mighty_run::strings` (the setup list's subtitle, the CLI tab's explanations, remote sharing's
//! notes) swap there too, once the host said it is the Mac (`strings::use_words`): a Mac never reads "이 PC", PowerShell or a Windows
//! user's vault.

use crate::app_update_view::Words;
use crate::locale::Locale;

/// Each Windows key whose Mac row says something else: Swift's key, or None when the Mac shows no such row.
pub const MAC_KEYS: [(&str, Option<&str>); 14] = [
    // ⌘Enter, not Ctrl+Enter (SettingsView.swift:375).
    ("settings.delegation.descriptionWindows", Some("settings.delegation.description")),
    // ⌘=, ⌘− and ⌘0 in the View menu (SettingsView.swift:313).
    ("settings.text.noteWindows", Some("settings.text.note")),
    // Windows' tips name PowerShell; the Mac's shell pane runs the login shell (Swift's `runtimeNote` names SwiftUI, which this app is
    // not, so the Mac reads its own tips).
    ("settings.appInfo.windowsNote", Some("settings.agents.tipsMac")),
    // "Mac notification when a task finishes" (AgentCompanionViews.swift:96).
    ("windows.notifications.toggleLabel", Some("companion.settings.macNotifications")),
    // "Allow phone to connect to this Mac" (MobileRemoteSettingsView.swift:22).
    ("windows.mobile.allow", Some("settings.mobileRemote.allowToggle")),
    // "Mac and phone each connect to the relay server independently…" (MobileRemoteSettingsView.swift:22).
    ("windows.mobile.relayDescription", Some("settings.mobileRemote.allowDescription")),
    // 구성 요소: the install runs in a terminal of the app, not a separate PowerShell window.
    ("setup.windows.subtitle", Some("setup.subtitle")),
    // CLI 계정: the Mac signs in in a terminal pane (sign_in_pane.rs), not an outside console window.
    ("settings.cliAccounts.sectionDescriptionWindows", Some("settings.cliAccounts.sectionDescriptionMac")),
    // CLI 업데이트: "this Mac", and its own update schedule's words.
    ("settings.cliUpdate.sectionDescriptionWindows", Some("settings.cliUpdate.sectionDescriptionMac")),
    // 원격 연결: "this Mac", the Keychain, and no PowerShell or Windows confinement rule.
    ("remote.windows.settings.tailscaleOff", Some("settings.remote.tailscaleOff")),
    ("remote.windows.settings.shareNote", Some("settings.remote.shareNote")),
    ("remote.windows.settings.scopeKey", Some("settings.remote.scope.key")),
    ("remote.windows.settings.allowFullAccessDescription", Some("settings.remote.allowFullAccessDescription")),
    // 앱 정보: the state folder shows in Finder.
    ("menu.showInExplorer", Some("menu.showInFinder")),
];

/// The Mac's own key for a Windows key that has one (`strings::get`'s swap on the Mac); None when the key reads the same on both or
/// the Mac shows no such row.
pub fn mac_swap(windows_key: &str) -> Option<&'static str> {
    MAC_KEYS.iter().find(|(key, _)| *key == windows_key).and_then(|(_, mac)| *mac)
}

/// The key a row reads in `words`' app, or None when that app shows no such row.
pub fn key(words: Words, windows_key: &'static str) -> Option<&'static str> {
    match words {
        Words::Windows => Some(windows_key),
        Words::Mac => MAC_KEYS.iter().find(|(key, _)| *key == windows_key).map_or(Some(windows_key), |(_, mac)| *mac),
    }
}

/// The row's words in `words`' app, or None when it shows no such row.
pub fn text(locale: &Locale, words: Words, windows_key: &'static str) -> Option<String> {
    key(words, windows_key).map(|key| locale.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both values: Windows reads its own keys; the Mac reads its own (never "PC", PowerShell or Explorer); a key with no Mac row of its
    /// own is the same on both. Every key named exists in every language.
    #[test]
    fn each_app_says_its_own_words() {
        let en = Locale::new("en");
        for (windows, mac) in MAC_KEYS {
            assert_eq!(text(&en, Words::Windows, windows), Some(en.get(windows)));
            assert_eq!(key(Words::Mac, windows), mac);
            for language in ["ko", "en", "zh", "ja"] {
                let locale = Locale::new(language);
                for name in [Some(windows), mac].into_iter().flatten() {
                    assert_ne!(locale.get(name), name, "{language} has {name}");
                }
            }
        }
        assert_eq!(text(&en, Words::Mac, "settings.text.noteWindows"), Some(en.get("settings.text.note")));
        assert!(text(&en, Words::Mac, "windows.notifications.toggleLabel").is_some_and(|t| t.contains("Mac")));
        assert!(text(&en, Words::Mac, "settings.appInfo.windowsNote").is_some_and(|t| !t.contains("PowerShell")));
        assert_eq!(key(Words::Mac, "windows.mobile.relayTitle"), Some("windows.mobile.relayTitle"));
        let ko = Locale::new("ko");
        for (windows, _) in MAC_KEYS {
            let mac = text(&ko, Words::Mac, windows).unwrap_or_default();
            for word in ["이 PC", "PowerShell", "파일 탐색기", "Windows 사용자"] {
                assert!(!mac.contains(word), "the Mac's {windows} says {word}: {mac}");
            }
        }
        assert_eq!(mac_swap("menu.showInExplorer"), Some("menu.showInFinder"));
        assert_eq!(mac_swap("settings.remote.navTitle"), None);
    }
}

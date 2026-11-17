//! The settings' words where the Windows and Mac apps differ (Mac integration review 2): the Windows app's keys (C#'s, which the page
//! and this host use) and, on the Mac, the Swift app's keys for the same rows (SettingsView.swift, AgentCompanionViews.swift,
//! MobileRemoteSettingsView.swift). A row Swift has no words for that would be wrong on the Mac is not shown there.

use crate::app_update_view::Words;
use crate::locale::Locale;

/// Each Windows key whose Mac row says something else: Swift's key, or None when the Mac shows no such row.
pub const MAC_KEYS: [(&str, Option<&str>); 7] = [
    // ⌘Enter, not Ctrl+Enter (SettingsView.swift:375).
    ("settings.delegation.descriptionWindows", Some("settings.delegation.description")),
    // The engine the switch starts on the Mac is Chromium (CEF), not WebView2 (SettingsView.swift:382).
    ("windows.settings.browserDescription", Some("settings.display.browserDescription")),
    // ⌘=, ⌘− and ⌘0 in the View menu (SettingsView.swift:313).
    ("settings.text.noteWindows", Some("settings.text.note")),
    // Windows' note names PowerShell and Ctrl keys; Swift's (`runtimeNote`, "SwiftUI · AppKit") names a runtime this app is not.
    ("settings.appInfo.windowsNote", None),
    // "Mac notification when a task finishes" (AgentCompanionViews.swift:96).
    ("windows.notifications.toggleLabel", Some("companion.settings.macNotifications")),
    // "Allow phone to connect to this Mac" (MobileRemoteSettingsView.swift:22).
    ("windows.mobile.allow", Some("settings.mobileRemote.allowToggle")),
    // "Mac and phone each connect to the relay server independently…" (MobileRemoteSettingsView.swift:22).
    ("windows.mobile.relayDescription", Some("settings.mobileRemote.allowDescription")),
];

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

    /// Both values: Windows reads its own keys; the Mac reads Swift's, and hides the runtime note; a key with no Mac row of its own is
    /// the same on both. Every key named exists in every language.
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
        assert_eq!(text(&en, Words::Mac, "settings.appInfo.windowsNote"), None);
        assert_eq!(key(Words::Mac, "windows.mobile.relayTitle"), Some("windows.mobile.relayTitle"));
    }
}

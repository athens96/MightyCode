//! The app's settings outside the state file: Windows values under `HKCU\Software\MightyCode`, the Mac's UserDefaults domain
//! `dev.mightycode.native`. Both use the same names (`delegation.enabled`, `setup.neverShow`, `textScale`, `textFontFamily`, ...).
//!
//! The Mac's values are what the Swift app stored, in Swift's types (a Bool, a Double, a Date, an array, a dictionary): [`SWIFT_KEYS`]
//! lists every key the Swift app reads with the type it stores, and [`from_swift`] / [`to_swift`] turn them into the forms the shared
//! readers take (C#'s registry forms: text for a number the user picks) and back, so a setting written here still reads in the Swift
//! app. Mac stage M-B; the Swift rules over these keys that the Swift platform checks name are here too ([`switch_on`],
//! [`apply_to_system_interface`], [`WebOpenChoices`]), pure over a [`PrefsStore`] and tested on both OSes.

use std::sync::{Arc, Mutex};

use crate::Result;

/// One stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum PrefValue {
    /// A REG_DWORD / REG_QWORD, or a UserDefaults integer.
    Int(i64),
    /// A REG_SZ / REG_EXPAND_SZ (unexpanded), or a UserDefaults string.
    Text(String),
    /// A UserDefaults boolean (Windows stores switches as DWORD 0/1).
    Bool(bool),
    /// A UserDefaults floating-point number (`defaults.set(1.3, …)`).
    Float(f64),
    /// A UserDefaults date, as seconds since 1970 (`appUpdate.lastCheck`).
    Date(f64),
    /// A UserDefaults array (`AppleLanguages`).
    List(Vec<PrefValue>),
    /// A UserDefaults dictionary with text keys, in the order read (`agentTerminal.webOpenChoices`, `statusLine.trusted`).
    Map(Vec<(String, PrefValue)>),
    /// A value of a kind the app never writes (binary, multi-string, data): present, but not readable as a setting.
    Other,
}

impl PrefValue {
    /// C# `DelegationSwitch.IsOn` for a present value: zero, or the text `0` or `false` (any case, trimmed), is off; anything else
    /// is on. (A missing value's meaning is the caller's: the delegation switch reads it as on.)
    pub fn is_on(&self) -> bool {
        match self {
            PrefValue::Int(number) => *number != 0,
            PrefValue::Bool(on) => *on,
            PrefValue::Float(number) => *number != 0.0,
            PrefValue::Text(text) => {
                let word = text.trim();
                word != "0" && !word.eq_ignore_ascii_case("false")
            }
            PrefValue::Date(_) | PrefValue::List(_) | PrefValue::Map(_) | PrefValue::Other => true,
        }
    }

    /// The text, when it is one.
    pub fn text(&self) -> Option<&str> {
        match self {
            PrefValue::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Contract: `get` of a missing key or a missing store is `Ok(None)`; `Err` only when the OS refused the read. Writes take effect
/// for the next `get` at once.
pub trait PrefsStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<PrefValue>>;
    fn set(&self, key: &str, value: PrefValue) -> Result<()>;
    fn remove(&self, key: &str) -> Result<()>;
}

/// The type the Swift app stores a key as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwiftKind {
    Bool,
    Double,
    String,
    Date,
    Array,
    Dictionary,
}

/// Every UserDefaults key the Swift app reads (mac-plan.md §2 B row, each from its Swift site), with the type it stores. `dialog.size.
/// <id>.*` and `mighty.referenceBubble.*` are prefixes ([`swift_kind`]).
pub const SWIFT_KEYS: [(&str, SwiftKind); 24] = [
    ("appUpdate.manifestURL", SwiftKind::String),
    ("appUpdate.lastCheck", SwiftKind::Date),
    ("appUpdate.automatic", SwiftKind::Bool),
    ("language", SwiftKind::String),
    ("AppleLanguages", SwiftKind::Array),
    ("languageAppliedToSystemInterface", SwiftKind::Bool),
    ("languagePreviousSystemInterface", SwiftKind::Array),
    ("textScale", SwiftKind::Double),
    ("textFontFamily", SwiftKind::String),
    ("textTranscriptFontFamily", SwiftKind::String),
    ("textCodeFontFamily", SwiftKind::String),
    ("browser.engineEnabled", SwiftKind::Bool),
    ("agentTerminal.webOpenChoices", SwiftKind::Dictionary),
    ("delegation.enabled", SwiftKind::Bool),
    ("notifications.webhookURL", SwiftKind::String),
    ("usage.dailyBudgetUSD", SwiftKind::Double),
    ("usage.claudeKeychain", SwiftKind::Bool),
    ("statusLine.enabled", SwiftKind::Bool),
    ("statusLine.trusted", SwiftKind::Dictionary),
    ("setup.neverShow", SwiftKind::Bool),
    ("settingsPane", SwiftKind::String),
    ("screenShare.permissionOnboardingShown", SwiftKind::Bool),
    ("rename.settingsCopiedFromMightyClaude", SwiftKind::Bool),
    ("usage.reset.link", SwiftKind::String),
];

/// The type the Swift app stores `key` as (the prefixed families: a dialog's size as numbers, the reference bubble's as numbers).
pub fn swift_kind(key: &str) -> Option<SwiftKind> {
    if let Some((_, kind)) = SWIFT_KEYS.iter().find(|(name, _)| *name == key) {
        return Some(*kind);
    }
    (key.starts_with("dialog.size.") || key.starts_with("mighty.referenceBubble.")).then_some(SwiftKind::Double)
}

/// .NET's and Swift's shortest text of a double (`1.3`, `2`).
fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{value}") }
}

/// A Swift value as the shared readers take it (C#'s registry forms): a Double the user picks (`textScale`, a budget) as its text,
/// everything else as it is.
pub fn from_swift(key: &str, value: PrefValue) -> PrefValue {
    match (swift_kind(key), value) {
        (Some(SwiftKind::Double), PrefValue::Float(number)) => PrefValue::Text(number_text(number)),
        (Some(SwiftKind::Double), PrefValue::Int(number)) => PrefValue::Text(number.to_string()),
        (_, value) => value,
    }
}

/// A value the shared code writes in the type the Swift app reads for `key`: text or a number as a Double, a switch as a Bool. A
/// value that does not convert is written as it is (Swift reads it as no value, as it reads a damaged one).
pub fn to_swift(key: &str, value: PrefValue) -> PrefValue {
    match (swift_kind(key), value) {
        (Some(SwiftKind::Double), PrefValue::Text(text)) => match text.trim().parse::<f64>() {
            Ok(number) if number.is_finite() => PrefValue::Float(number),
            _ => PrefValue::Text(text),
        },
        (Some(SwiftKind::Double), PrefValue::Int(number)) => PrefValue::Float(number as f64),
        (Some(SwiftKind::Bool), PrefValue::Int(number)) => PrefValue::Bool(number != 0),
        (Some(SwiftKind::Bool), PrefValue::Text(text)) => PrefValue::Bool(PrefValue::Text(text).is_on()),
        (_, value) => value,
    }
}

/// A switch: on while its key is missing (Swift `DelegationSwitch.isOn`: `object(forKey:) == nil || bool(forKey:)`), else as stored.
pub fn switch_on(prefs: &dyn PrefsStore, key: &str) -> bool {
    match prefs.get(key) {
        Ok(Some(value)) => value.is_on(),
        _ => true,
    }
}

/// Swift `AppLanguage.systemInterfaceKey`: the languages AppKit draws its own menus in.
pub const SYSTEM_INTERFACE_KEY: &str = "AppleLanguages";
const APPLIED_MARKER: &str = "languageAppliedToSystemInterface";
const PREVIOUS_VALUE: &str = "languagePreviousSystemInterface";

/// Swift `AppLanguage.applyToSystemInterface` (LocaleBundle.swift:202-217) over the app's own domain: a chosen language goes into
/// `AppleLanguages` (Chinese as `zh-Hans`), keeping what was there before (a language set for the app in System Settings); System
/// takes it back.
pub fn apply_to_system_interface(prefs: &dyn PrefsStore) -> Result<()> {
    let own = prefs.get(SYSTEM_INTERFACE_KEY)?;
    let choice = match prefs.get("language")? {
        Some(PrefValue::Text(text)) if ["ko", "en", "zh", "ja"].contains(&text.as_str()) => Some(text),
        _ => None,
    };
    let applied = prefs.get(APPLIED_MARKER)?.is_some_and(|v| v.is_on());
    match choice {
        None => {
            if !applied {
                return Ok(());
            }
            match prefs.get(PREVIOUS_VALUE)? {
                Some(previous @ PrefValue::List(_)) => prefs.set(SYSTEM_INTERFACE_KEY, previous)?,
                _ => prefs.remove(SYSTEM_INTERFACE_KEY)?,
            }
            prefs.remove(PREVIOUS_VALUE)?;
            prefs.remove(APPLIED_MARKER)
        }
        Some(language) => {
            if !applied && let Some(own @ PrefValue::List(_)) = own {
                prefs.set(PREVIOUS_VALUE, own)?;
            }
            let tag = if language == "zh" { "zh-Hans".to_string() } else { language };
            prefs.set(SYSTEM_INTERFACE_KEY, PrefValue::List(vec![PrefValue::Text(tag)]))?;
            prefs.set(APPLIED_MARKER, PrefValue::Bool(true))
        }
    }
}

/// Swift `WebOpenChoiceStore.defaultsKey`: `[workspaceId: destination]`.
pub const WEB_OPEN_CHOICES_KEY: &str = "agentTerminal.webOpenChoices";
/// `WebOpenDestination`'s raw values.
pub const WEB_OPEN_DESTINATIONS: [&str; 2] = ["in_app", "external"];

/// Swift `WebOpenChoiceStore`: where a URL an agent opens goes, per workspace, kept in the settings (one dictionary) when the store has
/// them, in memory otherwise; a stored value that is not a destination is ignored.
pub struct WebOpenChoices {
    prefs: Option<Arc<dyn PrefsStore>>,
    remembered: Mutex<Vec<(String, String)>>,
}

impl WebOpenChoices {
    pub fn new(prefs: Option<Arc<dyn PrefsStore>>) -> WebOpenChoices {
        let saved = match prefs.as_ref().and_then(|p| p.get(WEB_OPEN_CHOICES_KEY).ok().flatten()) {
            Some(PrefValue::Map(entries)) => entries
                .into_iter()
                .filter_map(|(workspace, value)| match value {
                    PrefValue::Text(text) if WEB_OPEN_DESTINATIONS.contains(&text.as_str()) => Some((workspace, text)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        WebOpenChoices { prefs, remembered: Mutex::new(saved) }
    }

    /// The remembered destination, or None (ask).
    pub fn choice(&self, workspace: &str) -> Option<String> {
        self.remembered.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(w, _)| w == workspace).map(|(_, d)| d.clone())
    }

    /// Records a destination (`setChoice`), or forgets it (`clearChoice`, the setting back to ask); the whole map is written back.
    pub fn set(&self, workspace: &str, destination: Option<&str>) -> Result<()> {
        let mut remembered = self.remembered.lock().unwrap_or_else(|p| p.into_inner());
        remembered.retain(|(w, _)| w != workspace);
        if let Some(destination) = destination.filter(|d| WEB_OPEN_DESTINATIONS.contains(d)) {
            remembered.push((workspace.to_string(), destination.to_string()));
        }
        match &self.prefs {
            Some(prefs) => prefs.set(
                WEB_OPEN_CHOICES_KEY,
                PrefValue::Map(remembered.iter().map(|(w, d)| (w.clone(), PrefValue::Text(d.clone()))).collect()),
            ),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::MemoryPrefs;

    #[test]
    fn switches_read_as_csharp_reads_them() {
        assert!(PrefValue::Int(1).is_on());
        assert!(!PrefValue::Int(0).is_on());
        assert!(!PrefValue::Text(" FALSE ".into()).is_on());
        assert!(!PrefValue::Text("0".into()).is_on());
        assert!(PrefValue::Text("yes".into()).is_on());
        assert!(!PrefValue::Bool(false).is_on());
        assert!(PrefValue::Other.is_on());
    }

    /// Swift's types go to the shared readers in C#'s forms and come back in Swift's.
    #[test]
    fn swift_values_turn_into_the_shared_forms_and_back() {
        assert_eq!(from_swift("textScale", PrefValue::Float(1.3)), PrefValue::Text("1.3".into()));
        assert_eq!(from_swift("textScale", PrefValue::Float(2.0)), PrefValue::Text("2".into()));
        assert_eq!(to_swift("textScale", PrefValue::Text("1.3".into())), PrefValue::Float(1.3));
        assert_eq!(to_swift("textScale", PrefValue::Text("big".into())), PrefValue::Text("big".into()));
        assert_eq!(to_swift("delegation.enabled", PrefValue::Int(0)), PrefValue::Bool(false));
        assert_eq!(to_swift("setup.neverShow", PrefValue::Int(1)), PrefValue::Bool(true));
        assert_eq!(to_swift("dialog.size.settings.width", PrefValue::Int(640)), PrefValue::Float(640.0));
        assert_eq!(to_swift("textFontFamily", PrefValue::Text("Avenir".into())), PrefValue::Text("Avenir".into()));
        assert_eq!(swift_kind("nothing"), None);
    }

    /// `apply_to_system_interface` over a store with no OS behind it: the same steps as the Mac's suite test
    /// (mighty-platform-macos `prefs.rs`).
    #[test]
    fn the_system_interface_language_follows_the_choice() {
        let prefs = MemoryPrefs::default();
        let own = |p: &MemoryPrefs| p.get(SYSTEM_INTERFACE_KEY).unwrap();
        let list = |tags: &[&str]| Some(PrefValue::List(tags.iter().map(|t| PrefValue::Text(t.to_string())).collect()));
        prefs.set(SYSTEM_INTERFACE_KEY, list(&["ja"]).unwrap()).unwrap();
        prefs.set("language", PrefValue::Text("zh".into())).unwrap();
        apply_to_system_interface(&prefs).unwrap();
        assert_eq!(own(&prefs), list(&["zh-Hans"]));
        prefs.set("language", PrefValue::Text("system".into())).unwrap();
        apply_to_system_interface(&prefs).unwrap();
        assert_eq!(own(&prefs), list(&["ja"]));
    }

    #[test]
    fn web_open_choices_keep_only_destinations() {
        let prefs: Arc<dyn PrefsStore> = Arc::new(MemoryPrefs::default());
        prefs
            .set(
                WEB_OPEN_CHOICES_KEY,
                PrefValue::Map(vec![("ws1".into(), PrefValue::Text("sideways".into())), ("ws2".into(), PrefValue::Text("in_app".into()))]),
            )
            .unwrap();
        let store = WebOpenChoices::new(Some(prefs.clone()));
        assert_eq!((store.choice("ws1"), store.choice("ws2")), (None, Some("in_app".to_string())));
        store.set("ws1", Some("external")).unwrap();
        assert_eq!(WebOpenChoices::new(Some(prefs)).choice("ws1").as_deref(), Some("external"));
    }
}

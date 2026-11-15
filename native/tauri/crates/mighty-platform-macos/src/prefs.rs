//! M-B: the user's settings in place, the UserDefaults domain `dev.mightycode.native` the Swift app keeps them in (the same domain
//! whether this build is the bundled app, whose standard domain it is, or a development build). Swift: every UserDefaults site (the
//! keys and their types are `mighty_platform::prefs::SWIFT_KEYS`); contract `mighty_platform::prefs` (`PrefsStore`).
//!
//! A read is the domain's own value (`persistentDomainForName:`), never the global domain's (`AppleLanguages` there is the system's,
//! not the app's), handed to the shared readers in their forms (`from_swift`); a write puts the value in the type Swift reads
//! (`to_swift`) and replaces the domain (`setPersistentDomain:forName:`), so it works from an unbundled build too. Writes only where
//! the write policy writes (the host's `may_change_machine`; a smoke's are in memory).
//!
//! Tests use a `dev.mightycode.tests.<uuid>` domain, removed at the end: never the runner user's own.

use std::sync::{Arc, Mutex};

use mighty_platform::prefs::{from_swift, to_swift};
use mighty_platform::{PlatformError, PrefValue, PrefsStore, Result};
use objc2::encode::Encoding;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, Message, msg_send};
use objc2_foundation::{NSArray, NSDate, NSDictionary, NSNumber, NSString, NSUserDefaults};

/// The Swift app's domain.
pub const DOMAIN: &str = "dev.mightycode.native";

/// One UserDefaults domain.
pub struct DefaultsPrefs {
    domain: String,
    /// A read-modify-write of the domain at a time.
    lock: Mutex<()>,
}

// SAFETY: NSUserDefaults is documented thread-safe; the store holds only the domain's name and takes `standardUserDefaults` per call.
unsafe impl Send for DefaultsPrefs {}
unsafe impl Sync for DefaultsPrefs {}

fn any<T: Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

fn is_kind(object: &AnyObject, class: &objc2::runtime::AnyClass) -> bool {
    // SAFETY: `isKindOfClass:` takes a class and returns BOOL; every object in a defaults domain is an NSObject.
    unsafe { msg_send![object, isKindOfClass: class] }
}

/// A property-list object as a value.
fn value_of(object: &AnyObject) -> PrefValue {
    if let Some(text) = object.downcast_ref::<NSString>() {
        return PrefValue::Text(text.to_string());
    }
    if let Some(number) = object.downcast_ref::<NSNumber>() {
        return match number.encoding() {
            // A CFBoolean (`defaults.set(true, …)`) is a `c`, as Objective-C's BOOL.
            Encoding::Bool | Encoding::Char | Encoding::UChar => PrefValue::Bool(number.as_bool()),
            Encoding::Float | Encoding::Double => PrefValue::Float(number.as_f64()),
            _ => PrefValue::Int(number.as_i64()),
        };
    }
    if is_kind(object, NSDate::class()) {
        // SAFETY: just checked to be an NSDate.
        let date = unsafe { &*(object as *const AnyObject as *const NSDate) };
        return PrefValue::Date(date.timeIntervalSince1970());
    }
    if is_kind(object, NSArray::<AnyObject>::class()) {
        // SAFETY: just checked to be an NSArray (of property-list objects).
        let array = unsafe { &*(object as *const AnyObject as *const NSArray<AnyObject>) };
        return PrefValue::List(array.to_vec().iter().map(|item| value_of(item)).collect());
    }
    if is_kind(object, NSDictionary::<AnyObject, AnyObject>::class()) {
        // SAFETY: just checked to be an NSDictionary; a property list's keys are strings.
        let dictionary = unsafe { &*(object as *const AnyObject as *const NSDictionary<NSString, AnyObject>) };
        let (keys, objects) = dictionary.to_vecs();
        return PrefValue::Map(keys.iter().zip(objects.iter()).map(|(k, v)| (k.to_string(), value_of(v))).collect());
    }
    PrefValue::Other
}

/// A value as the property-list object Swift reads; None for a value of no settable kind.
fn object_of(value: &PrefValue) -> Option<Retained<AnyObject>> {
    Some(match value {
        PrefValue::Text(text) => any(NSString::from_str(text)),
        PrefValue::Int(number) => any(NSNumber::new_i64(*number)),
        PrefValue::Bool(on) => any(NSNumber::new_bool(*on)),
        PrefValue::Float(number) => any(NSNumber::new_f64(*number)),
        PrefValue::Date(seconds) => any(NSDate::dateWithTimeIntervalSince1970(*seconds)),
        PrefValue::List(items) => {
            let objects = items.iter().map(object_of).collect::<Option<Vec<_>>>()?;
            any(NSArray::from_retained_slice(&objects))
        }
        PrefValue::Map(entries) => {
            let keys: Vec<Retained<NSString>> = entries.iter().map(|(k, _)| NSString::from_str(k)).collect();
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            let objects = entries.iter().map(|(_, v)| object_of(v)).collect::<Option<Vec<_>>>()?;
            any(NSDictionary::from_retained_objects(&key_refs, &objects))
        }
        PrefValue::Other => return None,
    })
}

impl DefaultsPrefs {
    /// The Swift app's domain.
    pub fn app() -> DefaultsPrefs {
        DefaultsPrefs::domain(DOMAIN)
    }

    /// Another domain (tests: `dev.mightycode.tests.<uuid>`).
    pub fn domain(name: &str) -> DefaultsPrefs {
        DefaultsPrefs { domain: name.to_string(), lock: Mutex::new(()) }
    }

    pub fn name(&self) -> &str {
        &self.domain
    }

    /// The domain's own values, in the order UserDefaults gives them.
    fn entries(&self) -> (Vec<Retained<NSString>>, Vec<Retained<AnyObject>>) {
        let defaults = NSUserDefaults::standardUserDefaults();
        match defaults.persistentDomainForName(&NSString::from_str(&self.domain)) {
            Some(dictionary) => dictionary.to_vecs(),
            None => (vec![], vec![]),
        }
    }

    /// The value stored under `key`, in Swift's type (no conversion): what the Swift app reads.
    pub fn stored(&self, key: &str) -> Option<PrefValue> {
        let (keys, objects) = self.entries();
        keys.iter().position(|k| k.to_string() == key).map(|at| value_of(&objects[at]))
    }

    /// The domain with `key` replaced (or removed).
    fn replace(&self, key: &str, value: Option<Retained<AnyObject>>) {
        let _held = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let (mut keys, mut objects) = self.entries();
        if let Some(at) = keys.iter().position(|k| k.to_string() == key) {
            keys.remove(at);
            objects.remove(at);
        }
        if let Some(object) = value {
            keys.push(NSString::from_str(key));
            objects.push(object);
        }
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let dictionary = NSDictionary::from_retained_objects(&key_refs, &objects);
        // SAFETY: every value is a property-list object (`object_of`, or what the domain already held).
        unsafe { NSUserDefaults::standardUserDefaults().setPersistentDomain_forName(&dictionary, &NSString::from_str(&self.domain)) };
    }

    /// The domain's keys.
    pub fn keys(&self) -> Vec<String> {
        self.entries().0.iter().map(|k| k.to_string()).collect()
    }

    /// `key`'s value in `other` set here as it is stored there (RenameMigration's copy of the old domain); an error when `other` has
    /// none or it is of no settable kind.
    pub fn copy_from(&self, other: &DefaultsPrefs, key: &str) -> Result<()> {
        let object = other.stored(key).as_ref().and_then(object_of).ok_or_else(|| PlatformError::Invalid("nothing to copy".into()))?;
        self.replace(key, Some(object));
        Ok(())
    }

    /// Removes the whole domain (a test's, at its end).
    pub fn remove_domain(&self) {
        NSUserDefaults::standardUserDefaults().removePersistentDomainForName(&NSString::from_str(&self.domain));
    }
}

impl PrefsStore for DefaultsPrefs {
    fn get(&self, key: &str) -> Result<Option<PrefValue>> {
        Ok(self.stored(key).map(|value| from_swift(key, value)))
    }

    fn set(&self, key: &str, value: PrefValue) -> Result<()> {
        let object = object_of(&to_swift(key, value)).ok_or_else(|| PlatformError::Invalid("a value of no settable kind".into()))?;
        self.replace(key, Some(object));
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<()> {
        self.replace(key, None);
        Ok(())
    }
}

/// The Mac's `PrefsStore`: the Swift app's domain.
pub fn prefs() -> Arc<dyn PrefsStore> {
    Arc::new(DefaultsPrefs::app())
}

/// The OS's preferred languages, most preferred first (`NSLocale.preferredLanguages`: what Swift's "system" language reads).
pub fn preferred_languages() -> Vec<String> {
    let class = objc2::runtime::AnyClass::get(c"NSLocale").expect("Foundation's NSLocale");
    // SAFETY: `+[NSLocale preferredLanguages]` returns an NSArray of NSString.
    let languages: Retained<NSArray<NSString>> = unsafe { msg_send![class, preferredLanguages] };
    languages.to_vec().iter().map(|tag| tag.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::text_appearance::{self, Stored, TextAppearance};
    use mighty_platform::prefs::{SWIFT_KEYS, WebOpenChoices, apply_to_system_interface, swift_kind, switch_on};

    /// A test's own domain, removed when it ends.
    struct Suite(Arc<DefaultsPrefs>);
    impl Suite {
        fn new() -> Suite {
            let id = format!("{:032x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
            Suite(Arc::new(DefaultsPrefs::domain(&format!("dev.mightycode.tests.{}-{id}", std::process::id()))))
        }
    }
    impl Drop for Suite {
        fn drop(&mut self) {
            self.0.remove_domain();
        }
    }

    fn list(tags: &[&str]) -> Option<PrefValue> {
        Some(PrefValue::List(tags.iter().map(|t| PrefValue::Text(t.to_string())).collect()))
    }

    /// Every type a defaults domain holds reads back as written.
    #[test]
    fn every_property_list_type_round_trips() {
        let suite = Suite::new();
        let prefs = &suite.0;
        for (key, value) in [
            ("text", PrefValue::Text("한글 / text".into())),
            ("int", PrefValue::Int(-5)),
            ("bool", PrefValue::Bool(true)),
            ("float", PrefValue::Float(1.25)),
            ("date", PrefValue::Date(1_700_000_000.5)),
            ("list", list(&["en", "ko"]).unwrap()),
            ("map", PrefValue::Map(vec![("ws".into(), PrefValue::Text("external".into()))])),
        ] {
            prefs.set(key, value.clone()).unwrap();
            assert_eq!(prefs.get(key).unwrap(), Some(value), "{key}");
        }
        prefs.remove("text").unwrap();
        prefs.remove("text").unwrap();
        assert_eq!(prefs.get("text").unwrap(), None);
        assert!(prefs.set("x", PrefValue::Other).is_err());
    }

    /// `DelegationSwitchTests.swift:37 "theSwitchIsOnUntilItsKeyTurnsItOff"`.
    #[test]
    fn the_switch_is_on_until_its_key_turns_it_off() {
        let suite = Suite::new();
        let prefs = suite.0.as_ref();
        assert!(switch_on(prefs, "delegation.enabled"));
        prefs.set("delegation.enabled", PrefValue::Bool(false)).unwrap();
        assert!(!switch_on(prefs, "delegation.enabled"));
        prefs.set("delegation.enabled", PrefValue::Bool(true)).unwrap();
        assert!(switch_on(prefs, "delegation.enabled"));
        // What the shared settings write (an Int) is stored as Swift's Bool.
        prefs.set("delegation.enabled", PrefValue::Int(0)).unwrap();
        assert_eq!(suite.0.stored("delegation.enabled"), Some(PrefValue::Bool(false)));
        prefs.remove("delegation.enabled").unwrap();
        assert!(switch_on(prefs, "delegation.enabled"));
    }

    /// `LocalizationTests.swift:86 "systemPreferenceReadsTheOSPreferredLanguages"`: "system" is the first of the OS's preferred
    /// languages the app has, else English; and the OS answers with a list.
    #[test]
    fn system_preference_reads_the_os_preferred_languages() {
        assert_eq!(mighty_core::locale::chosen_language("system", &["fr-FR", "ko-KR"]), "ko");
        assert_eq!(mighty_core::locale::chosen_language("system", &["fr-FR"]), "en");
        assert!(!preferred_languages().is_empty(), "NSLocale.preferredLanguages");
    }

    /// `SystemInterfaceLanguageTests.swift:16 "aChosenLanguageIsWrittenAndSystemTakesItBack"` (AppleLanguages in a test domain only).
    #[test]
    fn a_chosen_language_is_written_and_system_takes_it_back() {
        let suite = Suite::new();
        let prefs = suite.0.as_ref();
        apply_to_system_interface(prefs).unwrap();
        assert_eq!(suite.0.stored("AppleLanguages"), None);
        for (choice, tags) in [("en", "en"), ("ko", "ko"), ("zh", "zh-Hans"), ("ja", "ja")] {
            prefs.set("language", PrefValue::Text(choice.into())).unwrap();
            apply_to_system_interface(prefs).unwrap();
            assert_eq!(suite.0.stored("AppleLanguages"), list(&[tags]), "{choice}");
        }
        prefs.set("language", PrefValue::Text("system".into())).unwrap();
        apply_to_system_interface(prefs).unwrap();
        assert_eq!(suite.0.stored("AppleLanguages"), None);
    }

    /// `SystemInterfaceLanguageTests.swift:38 "aLanguageSetForTheAppInSystemSettingsSurvives"`.
    #[test]
    fn a_language_set_for_the_app_in_system_settings_survives() {
        let suite = Suite::new();
        let prefs = suite.0.as_ref();
        prefs.set("AppleLanguages", list(&["ja"]).unwrap()).unwrap();
        apply_to_system_interface(prefs).unwrap();
        assert_eq!(suite.0.stored("AppleLanguages"), list(&["ja"]));
        prefs.set("language", PrefValue::Text("en".into())).unwrap();
        apply_to_system_interface(prefs).unwrap();
        apply_to_system_interface(prefs).unwrap();
        assert_eq!(suite.0.stored("AppleLanguages"), list(&["en"]));
        prefs.set("language", PrefValue::Text("system".into())).unwrap();
        apply_to_system_interface(prefs).unwrap();
        assert_eq!(suite.0.stored("AppleLanguages"), list(&["ja"]));
    }

    fn saved(prefs: &dyn PrefsStore) -> TextAppearance {
        let text = |key: &str| prefs.get(key).unwrap().and_then(|v| v.text().map(str::to_string));
        TextAppearance::from_values(
            text(text_appearance::SCALE_KEY).map(Stored::Text),
            text(text_appearance::FAMILY_KEY).as_deref(),
            text(text_appearance::TRANSCRIPT_FAMILY_KEY).as_deref(),
            text(text_appearance::CODE_FAMILY_KEY).as_deref(),
        )
    }

    fn save(prefs: &dyn PrefsStore, text: &TextAppearance) {
        for (key, value) in text.to_values() {
            match value {
                Some(value) => prefs.set(key, PrefValue::Text(value)).unwrap(),
                None => prefs.remove(key).unwrap(),
            }
        }
    }

    /// `TextAppearanceTests.swift:96 "theSettingIsSavedAndTheStandardOneLeavesNothingBehind"`: the scale stored as Swift's Double.
    #[test]
    fn the_setting_is_saved_and_the_standard_one_leaves_nothing_behind() {
        let suite = Suite::new();
        let prefs = suite.0.as_ref();
        assert_eq!(saved(prefs), TextAppearance::default());
        let chosen = TextAppearance::new(1.3, Some("Avenir Next"), None, None);
        save(prefs, &chosen);
        assert_eq!(saved(prefs), chosen);
        assert_eq!(suite.0.stored("textScale"), Some(PrefValue::Float(1.3)), "Swift reads `object(forKey:) as? Double`");
        save(prefs, &TextAppearance::default());
        for key in ["textScale", "textFontFamily", "textTranscriptFontFamily", "textCodeFontFamily"] {
            assert_eq!(suite.0.stored(key), None, "{key}");
        }
        prefs.set("textScale", PrefValue::Text("big".into())).unwrap();
        assert_eq!(saved(prefs).scale, 1.0);
        prefs.set("textScale", PrefValue::Float(1.17)).unwrap();
        assert_eq!(saved(prefs).scale, 1.2);
    }

    /// `TextAppearanceTests.swift:112 "theThreeFontsAreSavedUnderTheirOwnKeys"`.
    #[test]
    fn the_three_fonts_are_saved_under_their_own_keys() {
        let suite = Suite::new();
        let prefs = suite.0.as_ref();
        let text = TextAppearance::new(1.1, Some("Avenir Next"), Some("Georgia"), Some("JetBrains Mono"));
        save(prefs, &text);
        assert_eq!(suite.0.stored("textFontFamily"), Some(PrefValue::Text("Avenir Next".into())));
        assert_eq!(suite.0.stored("textTranscriptFontFamily"), Some(PrefValue::Text("Georgia".into())));
        assert_eq!(suite.0.stored("textCodeFontFamily"), Some(PrefValue::Text("JetBrains Mono".into())));
        assert_eq!(saved(prefs), text);
        save(prefs, &text.with_transcript_family(None));
        assert_eq!(suite.0.stored("textTranscriptFontFamily"), None);
        assert_eq!(saved(prefs).resolved_transcript_family(), Some("Avenir Next"));
    }

    /// `OpenURLChoiceWiringTests.swift:120 "rememberedChoicesSurviveANewStoreInstance"`.
    #[test]
    fn remembered_choices_survive_a_new_store_instance() {
        let suite = Suite::new();
        let prefs: Arc<dyn PrefsStore> = suite.0.clone();
        let before = WebOpenChoices::new(Some(prefs.clone()));
        before.set("ws1", Some("external")).unwrap();
        before.set("ws2", Some("in_app")).unwrap();
        let after = WebOpenChoices::new(Some(prefs.clone()));
        assert_eq!(
            (after.choice("ws1").as_deref(), after.choice("ws2").as_deref(), after.choice("ws3")),
            (Some("external"), Some("in_app"), None)
        );
        after.set("ws1", None).unwrap();
        let again = WebOpenChoices::new(Some(prefs));
        assert_eq!((again.choice("ws1"), again.choice("ws2").as_deref()), (None, Some("in_app")));
    }

    /// `OpenURLChoiceWiringTests.swift:139 "dialogRememberIsPersisted"`: a remembered dialog answer is in the domain at once.
    #[test]
    fn dialog_remember_is_persisted() {
        let suite = Suite::new();
        WebOpenChoices::new(Some(suite.0.clone())).set("ws", Some("external")).unwrap();
        assert_eq!(
            suite.0.stored("agentTerminal.webOpenChoices"),
            Some(PrefValue::Map(vec![("ws".into(), PrefValue::Text("external".into()))]))
        );
        assert_eq!(WebOpenChoices::new(Some(suite.0.clone())).choice("ws").as_deref(), Some("external"));
    }

    /// `OpenURLChoiceWiringTests.swift:148 "storedValuesThatAreNotAChoiceAreIgnored"`.
    #[test]
    fn stored_values_that_are_not_a_choice_are_ignored() {
        let suite = Suite::new();
        suite
            .0
            .set(
                "agentTerminal.webOpenChoices",
                PrefValue::Map(vec![("ws1".into(), PrefValue::Text("sideways".into())), ("ws2".into(), PrefValue::Text("in_app".into()))]),
            )
            .unwrap();
        let store = WebOpenChoices::new(Some(suite.0.clone()));
        assert_eq!((store.choice("ws1"), store.choice("ws2").as_deref()), (None, Some("in_app")));
    }

    /// `OpenURLChoiceWiringTests.swift:156 "aStoreWithoutDefaultsWritesNothing"`.
    #[test]
    fn a_store_without_defaults_writes_nothing() {
        let suite = Suite::new();
        WebOpenChoices::new(None).set("ws", Some("external")).unwrap();
        assert_eq!(suite.0.stored("agentTerminal.webOpenChoices"), None);
        assert_eq!(mighty_platform::prefs::WEB_OPEN_CHOICES_KEY, "agentTerminal.webOpenChoices");
    }

    /// `HelpSiteTests.swift:31 "theCurrentGuideFollowsTheDisplayLanguage"`: each language's guide, and "system" read from the domain
    /// opens the folder of the language it resolves to.
    #[test]
    fn the_current_guide_follows_the_display_language() {
        for language in ["ko", "en", "zh", "ja"] {
            assert_eq!(mighty_core::help_site::current(&mighty_core::locale::Locale::new(language)), mighty_core::help_site::url(language));
        }
        let suite = Suite::new();
        suite.0.set("language", PrefValue::Text("system".into())).unwrap();
        let preference = suite.0.get("language").unwrap().and_then(|v| v.text().map(str::to_string)).unwrap();
        let chosen = mighty_core::locale::chosen_language(&preference, &["ja-JP", "en-US"]);
        assert!(mighty_core::help_site::url(chosen).ends_with("/help/ja/"));
    }

    /// `RenameMigrationTests.swift:8 "theOldDataFolderWorktreesConversationsAndSettingsMoveOnce"`, its UserDefaults half (the folders
    /// are mighty-core's `tests/rename_migration_mac.rs`): the old domain's values copied into the new one as stored, once, a value the
    /// new one has never overwritten (src-tauri `platform::rename_settings` does this over the app's two domains).
    #[test]
    fn the_old_domain_is_copied_once_without_overwriting() {
        let (current, legacy) = (Suite::new(), Suite::new());
        current.0.set("already", PrefValue::Text("kept".into())).unwrap();
        legacy.0.set("theme", PrefValue::Text("dark".into())).unwrap();
        legacy.0.set("already", PrefValue::Text("old".into())).unwrap();
        legacy.0.set("textScale", PrefValue::Float(1.3)).unwrap();
        let copy = || {
            if current.0.stored(mighty_core::rename_migration::SETTINGS_MARKER).is_some() {
                return 0;
            }
            let copied =
                legacy.0.keys().iter().filter(|key| current.0.stored(key).is_none() && current.0.copy_from(&legacy.0, key).is_ok()).count();
            current.0.set(mighty_core::rename_migration::SETTINGS_MARKER, PrefValue::Bool(true)).unwrap();
            copied
        };
        assert_eq!(copy(), 2);
        assert_eq!(current.0.stored("theme"), Some(PrefValue::Text("dark".into())));
        assert_eq!(current.0.stored("already"), Some(PrefValue::Text("kept".into())));
        assert_eq!(current.0.stored("textScale"), Some(PrefValue::Float(1.3)), "as stored: a Double stays a Double");
        current.0.remove("theme").unwrap();
        assert_eq!(copy(), 0);
        assert_eq!(current.0.stored("theme"), None, "a setting cleared later stays cleared");
    }

    /// The keys MightyCore names publicly (`parity-mac defaults-keys`, the Mac CI job) are in Rust's table with Swift's type.
    #[test]
    fn the_defaults_keys_are_swifts() {
        let Some(dir) = std::env::var_os("MIGHTY_PARITY_MAC_DIR") else {
            assert_ne!(std::env::var("MIGHTY_PARITY_MAC").as_deref(), Ok("require"));
            return;
        };
        let text = std::fs::read_to_string(std::path::Path::new(&dir).join("defaults-keys.json")).unwrap();
        let root: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        let keys = root["keys"].as_object().expect("keys");
        assert!(!keys.is_empty());
        for (key, kind) in keys {
            let want = match kind.as_str().unwrap() {
                "bool" => mighty_platform::prefs::SwiftKind::Bool,
                "double" => mighty_platform::prefs::SwiftKind::Double,
                "string" => mighty_platform::prefs::SwiftKind::String,
                "array" => mighty_platform::prefs::SwiftKind::Array,
                "dictionary" => mighty_platform::prefs::SwiftKind::Dictionary,
                other => panic!("{other}"),
            };
            assert_eq!(swift_kind(key), Some(want), "{key}");
        }
        assert!(SWIFT_KEYS.len() >= keys.len());
    }
}

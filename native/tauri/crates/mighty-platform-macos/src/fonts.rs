//! M-A: the installed font families for Settings › 글자. Swift `TextAppearance.swift` (`codeFamilies`, `knownCodeFamilies`) over
//! `TextSettings.fontLists` (`NSFontManager.availableFontFamilies`); contract `mighty_platform::fonts`.
//!
//! CoreText's families (which, unlike `NSFontManager`, may be asked from any thread), each by its regular face at 13 pt: hidden (`.`)
//! names and families that do not cover the Latin letters (symbol and dingbat fonts) left out, as Swift's list does. A family counts as
//! monospaced when its face is fixed-pitch or it is one of the well-known code families (Swift's code picker offers D2Coding and
//! SF Mono, whose faces CoreText does not call fixed-pitch).

use std::sync::Arc;

use mighty_platform::{FontCatalog, FontFamily, Result};
use objc2_core_foundation::{CFArray, CFCharacterSet, CFDictionary, CFRetained, CFString};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontManagerCopyAvailableFontFamilyNames, CTFontSymbolicTraits, kCTFontFamilyNameAttribute,
};

/// Swift `TextAppearance.knownCodeFamilies`.
pub const KNOWN_CODE_FAMILIES: [&str; 21] = [
    "Menlo",
    "Monaco",
    "SF Mono",
    "JetBrains Mono",
    "D2Coding",
    "D2Coding ligature",
    "Courier",
    "Courier New",
    "Andale Mono",
    "PT Mono",
    "Fira Code",
    "Fira Mono",
    "Source Code Pro",
    "Cascadia Code",
    "Cascadia Mono",
    "IBM Plex Mono",
    "Hack",
    "Inconsolata",
    "Roboto Mono",
    "Ubuntu Mono",
    "Consolas",
];

/// Swift `TextAppearance.codeFamilies`: the families the code picker offers, in the order given — those whose fonts are fixed-pitch
/// and the well-known monospaced ones, without hidden (`.`) or repeated names.
pub fn code_families(fonts: &[(&str, bool)]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for (family, fixed_pitch) in fonts {
        if family.starts_with('.') || !(*fixed_pitch || KNOWN_CODE_FAMILIES.contains(family)) || seen.iter().any(|s| s == family) {
            continue;
        }
        seen.push(family.to_string());
    }
    seen
}

/// The Mac's `FontCatalog`.
pub fn font_catalog() -> Arc<dyn FontCatalog> {
    Arc::new(CoreTextFonts)
}

#[derive(Debug, Default)]
pub struct CoreTextFonts;

/// The regular face of `family` at 13 pt: whether it covers `letters` and whether it is fixed-pitch.
fn face(family: &CFString, letters: &CFCharacterSet) -> Option<bool> {
    let attributes = CFDictionary::<CFString, CFString>::from_slices(
        &[
            // SAFETY: a CoreText constant, valid for the process's life.
            unsafe { kCTFontFamilyNameAttribute },
        ],
        &[family],
    );
    // SAFETY: a dictionary of CoreText font attributes; plain CoreText calls on the objects they returned.
    unsafe {
        let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
        let font = CTFont::with_font_descriptor(&descriptor, 13.0, std::ptr::null());
        if font.family_name().to_string() != family.to_string() || !font.character_set().is_superset_of_set(Some(letters)) {
            return None;
        }
        Some(font.symbolic_traits().contains(CTFontSymbolicTraits::TraitMonoSpace))
    }
}

impl FontCatalog for CoreTextFonts {
    fn families(&self) -> Result<Vec<FontFamily>> {
        let latin = CFString::from_str("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");
        // SAFETY: plain CoreFoundation and CoreText calls; the array holds CFStrings (CTFontManager's documented result).
        let (letters, names) = unsafe {
            let letters = CFCharacterSet::with_characters_in_string(None, Some(&latin));
            let names: CFRetained<CFArray<CFString>> = CFRetained::cast_unchecked(CTFontManagerCopyAvailableFontFamilyNames());
            (letters, names)
        };
        let Some(letters) = letters else { return Ok(vec![]) };
        let mut families: Vec<FontFamily> = Vec::new();
        for name in names.iter() {
            let text = name.to_string();
            if text.is_empty() || text.starts_with(['.', '@']) {
                continue;
            }
            let Some(fixed_pitch) = face(&name, &letters) else { continue };
            let monospaced = fixed_pitch || KNOWN_CODE_FAMILIES.contains(&text.as_str());
            let folded = text.to_uppercase();
            match families.iter_mut().find(|f| f.name.to_uppercase() == folded) {
                Some(known) => known.monospaced &= monospaced,
                None => families.push(FontFamily { name: text, monospaced }),
            }
        }
        families.sort_by_key(|f| f.name.to_uppercase());
        Ok(families)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `TextAppearanceTests.swift:63 "theCodePickerOffersFixedPitchAndKnownMonospacedFamilies"`.
    #[test]
    fn the_code_picker_offers_fixed_pitch_and_known_monospaced_families() {
        let fonts = [
            ("Helvetica", false),
            ("Menlo", true),
            ("Iosevka", true),
            ("D2Coding", false),
            ("SF Mono", false),
            ("JetBrains Mono", false),
            (".SF NS Mono", true),
            ("Menlo", true),
            ("Georgia", false),
        ];
        assert_eq!(code_families(&fonts), ["Menlo", "Iosevka", "D2Coding", "SF Mono", "JetBrains Mono"]);
        assert!(code_families(&[]).is_empty());
    }

    /// Every Mac has Menlo (fixed-pitch) and Helvetica (not); no hidden name or symbol font (Zapf Dingbats covers no Latin letters)
    /// is listed, and the list is sorted with each name once.
    #[test]
    fn the_installed_families_are_listed_once_without_hidden_or_symbol_fonts() {
        let families = CoreTextFonts.families().unwrap();
        let find = |name: &str| families.iter().find(|f| f.name.eq_ignore_ascii_case(name));
        assert!(find("Menlo").is_some_and(|f| f.monospaced), "Menlo is fixed-pitch");
        assert!(find("Helvetica").is_some_and(|f| !f.monospaced), "Helvetica is proportional");
        assert!(find("Zapf Dingbats").is_none(), "a symbol font is left out");
        assert!(families.iter().all(|f| !f.name.starts_with(['@', '.'])));
        let names: Vec<String> = families.iter().map(|f| f.name.to_uppercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted, "sorted, once each");
    }
}

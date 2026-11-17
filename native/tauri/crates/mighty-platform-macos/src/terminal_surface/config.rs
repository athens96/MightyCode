//! Ghostty's config for Mighty's terminals, as the Swift app wrote it (`AppStore+Terminals.swift:64-105` `sharedTerminalController`,
//! the vendored package's `TerminalTheme+Defaults.swift` and `GhosttyConfigRenderer.swift`): one file of `key = value` lines, loaded
//! instead of the user's own Ghostty config (which is never read). The theme follows the app's (light or dark), the font the text
//! setting (`TextAppearance.terminalFontFamilies`, `terminalSize`).

/// The terminals' family when no code family is chosen, and the fallback behind one (`TextAppearance.terminalStandardFamily`).
pub const STANDARD_FAMILY: &str = "Menlo";

/// The keys the host's menu owns: Ghostty's own new tab/window, font size and settings bindings would take ⌘T, ⌘N, ⌘O, ⌘K, ⌘Q, ⌘,
/// and the app's text size (⌘= / ⌘- / ⌘0) from it; both the physical names and the unicode triggers are unbound.
const MENU_KEYS: [&str; 14] = ["t", "n", "o", "k", "q", "comma", ",", "equal", "minus", "digit_0", "=", "+", "-", "0"];

/// Alabaster (light): background, foreground, cursor, selection and the 16 palette colours; the app's background over it.
const ALABASTER: [(&str, &str); 4] =
    [("background", "F7F7F7"), ("foreground", "000000"), ("cursor-color", "007ACC"), ("selection-background", "C9D0D9")];
const ALABASTER_PALETTE: [&str; 16] = [
    "#000000", "#AA3731", "#448C27", "#CB8800", "#325CC0", "#7A3E9D", "#0083B2", "#F7F7F7", "#777777", "#F03E31", "#60CB00", "#FFBC5D",
    "#007ACC", "#E64CE6", "#00AACB", "#F7F7F7",
];
/// Afterglow (dark), with the app's background, foreground and cursor over it.
const AFTERGLOW: [(&str, &str); 4] =
    [("background", "212121"), ("foreground", "D0D0D0"), ("cursor-color", "D0D0D0"), ("selection-background", "303030")];
const AFTERGLOW_PALETTE: [&str; 16] = [
    "#151515", "#AC4142", "#7E8E50", "#E4B567", "#6C99BB", "#9F4E86", "#7DD5CF", "#D0D0D0", "#505050", "#AC4142", "#7E8E50", "#E4B567",
    "#6C99BB", "#9F4E86", "#7DD5CF", "#F5F5F5",
];
const LIGHT_OVER: [(&str, &str); 1] = [("background", "FAF9F6")];
const DARK_OVER: [(&str, &str); 3] = [("background", "202020"), ("foreground", "DEDCD8"), ("cursor-color", "E1AB91")];

/// `TextAppearance.normalizedFamily` (mighty-core's `text_appearance::normalized_family`, the same rule): trimmed; empty, or holding a
/// control character or a line or paragraph separator, is no family, so a name can never break the config's one-value lines.
pub fn normalized_family(family: Option<&str>) -> Option<String> {
    let trimmed = family?.trim();
    (!trimmed.is_empty() && !trimmed.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))).then(|| trimmed.to_string())
}

/// `TextAppearance.terminalFontFamilies`: the `font-family` values over the config, in order; empty keeps the standard family. With
/// a code family: `""` clears the list, then the family, then Menlo, so a family that is not installed falls back to Menlo rather
/// than to Ghostty's own default. Ghostty takes the outer quotes off and keeps the rest as written.
pub fn terminal_font_families(code_family: Option<&str>) -> Vec<String> {
    let Some(family) = normalized_family(code_family) else { return Vec::new() };
    vec!["\"\"".to_string(), format!("\"{family}\""), format!("\"{STANDARD_FAMILY}\"")]
}

/// A size as the config writes it: at most two decimals, no trailing zeros (`configLiteral`).
fn literal(size: f64) -> String {
    let size = if size.is_finite() && size > 0.0 { size } else { 12.0 };
    let text = format!("{size:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The whole config: the base (the standard family, the size, the padding, the clipboard and scrollback rules, the menu's keys
/// unbound), the code family over it, then the theme of `light`.
pub fn config_text(light: bool, code_family: Option<&str>, size: f64) -> String {
    let mut lines = vec![
        format!("font-family = {STANDARD_FAMILY}"),
        format!("font-size = {}", literal(size)),
        "window-padding-x = 10".to_string(),
        "window-padding-y = 10".to_string(),
        "clipboard-read = deny".to_string(),
        "clipboard-write = ask".to_string(),
        "scrollback-limit = 10000000".to_string(),
    ];
    lines.extend(MENU_KEYS.iter().map(|key| format!("keybind = super+{key}=unbind")));
    lines.extend(terminal_font_families(code_family).into_iter().map(|family| format!("font-family = {family}")));
    let (colours, palette): (&[(&str, &str)], &[&str]) =
        if light { (&ALABASTER, &ALABASTER_PALETTE) } else { (&AFTERGLOW, &AFTERGLOW_PALETTE) };
    let over: &[(&str, &str)] = if light { &LIGHT_OVER } else { &DARK_OVER };
    lines.extend(colours.iter().map(|(key, value)| format!("{key} = {value}")));
    lines.extend(palette.iter().enumerate().map(|(index, colour)| format!("palette = {index}={colour}")));
    lines.extend(over.iter().map(|(key, value)| format!("{key} = {value}")));
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `TextAppearanceTests.swift:72` "aNameThatCouldBreakTheTerminalConfigIsNoFamily": a name holding a line break, a
    /// separator, a NUL, a tab or DEL is no family, so the terminals keep the standard one; blanks around a name go.
    #[test]
    fn a_name_that_could_break_the_terminal_config_is_no_family() {
        for name in ["A\nb = c", "A\rB", "A\u{2028}B", "A\u{2029}B", "A\u{85}B", "A\u{0}B", "A\tB", "A\u{7F}B"] {
            assert_eq!(normalized_family(Some(name)), None, "{name:?}");
            assert!(terminal_font_families(Some(name)).is_empty(), "{name:?}");
        }
        assert!(terminal_font_families(Some("A\nb = c")).is_empty());
        assert_eq!(normalized_family(Some(" Nanum Gothic\n")).as_deref(), Some("Nanum Gothic"));
        let config = config_text(false, Some("A\nb = c"), 12.0);
        assert!(!config.contains("b = c") && config.lines().all(|line| line.contains(" = ")), "every line one key and one value");
    }

    /// Swift `TextAppearanceTests.swift:81` "theTerminalsGetTheCodeFamilyQuotedWithMenloBehindIt": no code family keeps the standard
    /// one; a code family comes quoted, the list cleared before it and Menlo behind it; a quote in a name stays part of the value.
    #[test]
    fn the_terminals_get_the_code_family_quoted_with_menlo_behind_it() {
        assert!(terminal_font_families(None).is_empty() && terminal_font_families(Some(" ")).is_empty());
        assert_eq!(terminal_font_families(Some("JetBrains Mono")), [r#""""#, r#""JetBrains Mono""#, r#""Menlo""#]);
        assert_eq!(terminal_font_families(Some("A\"B")), [r#""""#, r#""A"B""#, r#""Menlo""#]);
        let config = config_text(true, Some("JetBrains Mono"), 13.5);
        let families: Vec<&str> = config.lines().filter(|line| line.starts_with("font-family = ")).collect();
        assert_eq!(
            families,
            ["font-family = Menlo", r#"font-family = """#, r#"font-family = "JetBrains Mono""#, r#"font-family = "Menlo""#]
        );
        assert!(config.contains("font-size = 13.5\n") && config_text(true, None, 12.0).contains("font-size = 12\n"));
    }

    /// Swift's controller settings (`AppStore+Terminals.swift:64-105`): the padding, the clipboard read denied and the writes asked,
    /// the long scrollback, the menu's keys unbound, and the theme: light `FAF9F6`, dark `202020` with `DEDCD8` and cursor `E1AB91`,
    /// each over its palette (the later line wins).
    #[test]
    fn the_config_is_the_swift_controllers() {
        let dark = config_text(false, None, 12.0);
        for line in [
            "window-padding-x = 10",
            "window-padding-y = 10",
            "clipboard-read = deny",
            "clipboard-write = ask",
            "scrollback-limit = 10000000",
            "keybind = super+t=unbind",
            "keybind = super+comma=unbind",
            "keybind = super+digit_0=unbind",
            "keybind = super++=unbind",
            "palette = 15=#F5F5F5",
        ] {
            assert!(dark.lines().any(|l| l == line), "{line}");
        }
        let last = |config: &str, key: &str| config.lines().rfind(|l| l.starts_with(&format!("{key} = "))).map(str::to_string);
        assert_eq!(last(&dark, "background").as_deref(), Some("background = 202020"));
        assert_eq!(last(&dark, "foreground").as_deref(), Some("foreground = DEDCD8"));
        assert_eq!(last(&dark, "cursor-color").as_deref(), Some("cursor-color = E1AB91"));
        let light = config_text(true, None, 12.0);
        assert_eq!(last(&light, "background").as_deref(), Some("background = FAF9F6"));
        assert_eq!(last(&light, "foreground").as_deref(), Some("foreground = 000000"));
        assert!(light.lines().any(|l| l == "palette = 1=#AA3731"));
    }
}

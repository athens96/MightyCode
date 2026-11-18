//! M-F: the menu bar as AppKit holds it, for the smoke (src-tauri smoke_companion.rs, through platform.rs). The menu itself is made
//! with Tauri's Menu API (src-tauri menu.rs); this reads `NSApp.mainMenu` back (every item's menu, title, key equivalent and
//! modifiers) and presses a key equivalent through it as AppKit would for a ⌘ key no view took (`NSMenu performKeyEquivalent:`). Main
//! thread only: each function answers None elsewhere.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu};
use objc2_foundation::{NSPoint, NSString};

/// One item of the menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The title of the menu it is in.
    pub menu: String,
    pub title: String,
    /// Its key equivalent as AppKit keeps it (`k`, `=`, `E` for ⇧⌘E, …), empty for none.
    pub key: String,
    /// `NSEventModifierFlags` bits: ⇧ 1<<17, ⌃ 1<<18, ⌥ 1<<19, ⌘ 1<<20.
    pub modifiers: u64,
}

pub const SHIFT: u64 = 1 << 17;
pub const CONTROL: u64 = 1 << 18;
pub const OPTION: u64 = 1 << 19;
pub const COMMAND: u64 = 1 << 20;

/// Every item of every menu of the menu bar, depth first.
pub fn entries() -> Option<Vec<MenuEntry>> {
    let mtm = MainThreadMarker::new()?;
    let main = NSApplication::sharedApplication(mtm).mainMenu()?;
    let mut out = Vec::new();
    walk(&main, &main.title().to_string(), &mut out);
    Some(out)
}

fn walk(menu: &NSMenu, title: &str, out: &mut Vec<MenuEntry>) {
    for item in menu.itemArray().iter() {
        if item.isSeparatorItem() {
            continue;
        }
        match item.submenu() {
            Some(submenu) => walk(&submenu, &item.title().to_string(), out),
            None => out.push(MenuEntry {
                menu: title.to_string(),
                title: item.title().to_string(),
                key: item.keyEquivalent().to_string(),
                modifiers: item.keyEquivalentModifierMask().0 as u64,
            }),
        }
    }
}

/// A key equivalent pressed through the menu bar (`key`: its characters, `code`: the virtual key, `modifiers`: [`MenuEntry`]'s
/// bits): whether an item took it.
pub fn press(key: &str, code: u16, modifiers: u64) -> Option<bool> {
    let mtm = MainThreadMarker::new()?;
    let main = NSApplication::sharedApplication(mtm).mainMenu()?;
    let characters = NSString::from_str(key);
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags(modifiers as usize),
        0.0,
        0,
        None,
        &characters,
        &characters,
        false,
        code,
    )?;
    Some(main.performKeyEquivalent(&event))
}

/// The item titled `title` given the key equivalent ⌘`key` (a character, as Swift's `keyboardShortcut("?")`); whether it was found.
pub fn set_command_key(title: &str, key: &str) -> Option<bool> {
    let mtm = MainThreadMarker::new()?;
    let main = NSApplication::sharedApplication(mtm).mainMenu()?;
    fn find(menu: &NSMenu, title: &str) -> Option<objc2::rc::Retained<objc2_app_kit::NSMenuItem>> {
        menu.itemArray().iter().find_map(|item| match item.submenu() {
            Some(submenu) => find(&submenu, title),
            None => (item.title().to_string() == title).then_some(item),
        })
    }
    let item = find(&main, title)?;
    item.setKeyEquivalent(&NSString::from_str(key));
    item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    Some(true)
}

//! M-F: the Mac's menu bar through Tauri's Menu API (Swift `MightyCodeApp.swift:62-134`; the items, shortcuts and locale keys are the
//! plan's §2 F row): ⌘O, New Folder…, Create from Git…, ⌘N, ⌃⌘S, ⌘= ⌘- ⌘0, the Workspace menu (⌘K ⌘T ⇧⌘E ⇧⌘F, Work map, ⇧⌘R, agent
//! context, usage, Install shell command 'mighty', Reconnect Input Method, Save Input Method Diagnostics, Export diagnostics…, ⌘,),
//! Help ⌘?, and Tauri's standard App, Edit and Window menus (the Edit menu is what makes ⌘C/⌘V/⌘X/⌘A/⌘Z work in WKWebView text
//! fields). Labels come from the locale catalogues in the chosen language and are rebuilt when it changes; an item runs the host's own
//! handler, or reaches the page as a `{kind:'menu', action}` push (behind tauri+macos only, plan D9). Windows has no menu bar.

/// Installs the menu bar (the Mac only). Step 0: Tauri's default macOS menu stays (App, Edit, Window); M-F builds Swift's.
pub fn install(_app: &tauri::AppHandle) -> tauri::Result<()> {
    Ok(())
}

// The window's shortcuts (spec section 8), owned by the page since the host clears its accelerators in web mode. Pure: a key
// in, the action it asks for out. The caller ignores them while a dialog is open.

export type ShortcutAction =
  | 'openFolder' | 'newPane' | 'search' | 'toggleSidebar' | 'newShell' | 'settings' | 'files' | 'help'
  | 'textLarger' | 'textSmaller' | 'textReset' | 'conversationSearch';

export interface ShortcutKey {
  key: string;
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

export function shortcutAction(event: ShortcutKey): ShortcutAction | null {
  if (event.altKey || event.metaKey) return null;
  if (!event.ctrlKey) return event.key === 'F1' && !event.shiftKey ? 'help' : null;
  if (event.shiftKey) {
    if (event.code === 'KeyE') return 'files';
    if (event.code === 'KeyF') return 'conversationSearch';
    // Ctrl++ on the main keyboard is Ctrl+Shift+=.
    return event.code === 'Equal' ? 'textLarger' : null;
  }
  switch (event.code) {
    case 'KeyO': return 'openFolder';
    case 'KeyN': return 'newPane';
    case 'KeyK': return 'search';
    case 'KeyB': return 'toggleSidebar';
    case 'KeyT': return 'newShell';
    case 'Comma': return 'settings';
    case 'Equal': case 'NumpadAdd': return 'textLarger';
    case 'Minus': case 'NumpadSubtract': return 'textSmaller';
    case 'Digit0': case 'Numpad0': return 'textReset';
    default: return null;
  }
}

/**
 * The Mac's shortcuts in the Rust host's build (plan D9; `import.meta.env.MODE === 'tauri'` and the host's `macos` only): ⌘ is the
 * modifier where Windows has Ctrl (M/MightyCodeApp.swift's menu keys: ⌘O ⌘N ⌘K ⌘T ⇧⌘E ⇧⌘F ⌘, ⌘= ⌘- ⌘0, ⌃⌘S for the sidebar, ⌘? for help by its character),
 * and a Ctrl key is left to the text fields (⌃K, ⌃T edit text on a Mac). The menu bar has the same keys: what the page takes here
 * the menu does not see, and what it leaves the menu runs.
 */
export function macShortcutAction(event: ShortcutKey): ShortcutAction | null {
  if (!event.metaKey || event.altKey) return null;
  if (event.ctrlKey) return !event.shiftKey && event.code === 'KeyS' ? 'toggleSidebar' : null;
  // ⌘? is the `?` character wherever the layout has it (Swift's `keyboardShortcut("?")`).
  if (event.key === '?') return 'help';
  if (event.shiftKey) {
    if (event.code === 'KeyE') return 'files';
    if (event.code === 'KeyF') return 'conversationSearch';
    return event.code === 'Equal' ? 'textLarger' : null;
  }
  switch (event.code) {
    case 'KeyO': return 'openFolder';
    case 'KeyN': return 'newPane';
    case 'KeyK': return 'search';
    case 'KeyT': return 'newShell';
    case 'Comma': return 'settings';
    case 'Equal': case 'NumpadAdd': return 'textLarger';
    case 'Minus': case 'NumpadSubtract': return 'textSmaller';
    case 'Digit0': case 'Numpad0': return 'textReset';
    default: return null;
  }
}

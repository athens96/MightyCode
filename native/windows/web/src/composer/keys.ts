// The editor's Enter (MainWindow.cs PreviewKeyDown, SubmitKeyAllowed): no send during an IME composition, with Shift (a new
// line), or with Alt or the Windows key; a held, repeating Enter does not send again; Ctrl+Enter is the second send. On the
// Mac ⌘Enter is the second send too (NativeComposerEditor.swift's ⌘Return, the help's 단축키 table); Ctrl+Enter stays one
// there as well, and ⌘ with Ctrl together is left to the editor.

export interface EnterKey {
  key: string;
  isComposing: boolean;
  keyCode: number;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  repeat: boolean;
}

/** `send` / `second`: send (the second send with Ctrl, or ⌘ on the Mac); `swallow`: a repeat, kept from the editor; `pass`: the editor's own. */
export type EnterAction = 'send' | 'second' | 'swallow' | 'pass';

/** `mac`: the page runs in the host on the Mac (`macHost()`), where ⌘ is the second send's key; elsewhere ⌘ is the Windows key. */
export function enterAction(event: EnterKey, mac = false): EnterAction {
  if (event.key !== 'Enter') return 'pass';
  // keyCode 229 is the IME's own Enter, which some IMEs send after composition ends.
  if (event.isComposing || event.keyCode === 229) return 'pass';
  if (event.shiftKey || event.altKey) return 'pass';
  if (event.metaKey && (!mac || event.ctrlKey)) return 'pass';
  if (event.repeat) return 'swallow';
  return event.ctrlKey || event.metaKey ? 'second' : 'send';
}

/** The host's words that name the second send's key (mighty-composer composer.rs), as the Mac says them: ⌘Enter, not Ctrl+Enter. */
const macWords: Readonly<Record<string, string>> = {
  'composer.placeholder.busyWorktree': 'composer.placeholder.busyWorktreeMac',
  'queue.waitingOnBackground.windowsWorktree': 'queue.waitingOnBackground.macWorktree',
  'queue.waitingOnBackground.windows': 'queue.waitingOnBackground.mac',
};

export function secondSendWords(key: string, mac: boolean): string {
  return (mac && macWords[key]) || key;
}

/**
 * The run settings popover's Enter in a single-line field: a plain Enter applies, as the Mac's Apply button
 * (`.keyboardShortcut(.defaultAction)`, Return without modifiers); a repeat is kept from the field; any modifier and an
 * IME composition's Enter are left alone.
 */
export function runSettingsEnter(event: EnterKey): 'apply' | 'swallow' | 'pass' {
  const action = enterAction(event);
  return action === 'send' ? 'apply' : action === 'swallow' ? 'swallow' : 'pass';
}

/** MC-119: ⌥⌘Enter on Mac, Ctrl+Alt+Enter on Windows opens the loop create sheet from the composer. */
export function isLoopShortcut(event: EnterKey, mac = false): boolean {
  if (event.key !== 'Enter') return false;
  if (event.isComposing || event.keyCode === 229) return false;
  if (event.shiftKey || event.repeat || !event.altKey) return false;
  return mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

/** The editor's text limit (the WinUI TextBox's MaxLength). */
export const maximumDraft = 100_000;

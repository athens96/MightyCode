// MC-50: a terminal pane takes the keyboard when it becomes the pane in use, so the person types into it without a second click.
// On the Mac the terminal is a native Ghostty view over the page's slot (the view gets it through `surfaceAction focus`); on the
// page it is xterm (`terminal.focus()`). The rules are pure here, so they are tested without a DOM; TerminalView wires them.
import type { StoreState } from '../../store/store';

/** What a request to give a terminal the keyboard does now: give it, keep it for later (something covers the page), or forget it. */
export type FocusVerdict = 'take' | 'wait' | 'drop';

/** The little of an element the rules read (an `Element` in the page, a plain object in the tests). */
export interface FocusedElement {
  readonly tagName: string;
  readonly type?: string;
  readonly isContentEditable?: boolean;
  readonly isConnected?: boolean;
  getAttribute(name: string): string | null;
  closest(selector: string): unknown;
}

/** An input of these types is a control, not a place to type. */
const controls = new Set(['button', 'checkbox', 'color', 'file', 'hidden', 'image', 'radio', 'range', 'reset', 'submit']);
const textRoles = new Set(['textbox', 'searchbox', 'combobox']);
/** A terminal's own keyboard element (xterm's helper textarea) or a native terminal's slot: never "typing elsewhere". */
const terminalKeyboard = '.terminal-surface, [data-native-slot]';
/** Where focus sits while a dialog, menu or popover of the page has it. */
const dialogs = '[role="dialog"], [role="alertdialog"], [aria-modal="true"], dialog[open], [role="menu"], [data-page-overlay]';

/** Whether the person may be typing in `element`: a text field, a text area, a select or editable text, outside every terminal. */
export function typingIn(element: FocusedElement | null): boolean {
  if (!element || element.isConnected === false) return false;
  if (element.closest(terminalKeyboard) || element.closest('[aria-hidden="true"]')) return false;
  const tag = element.tagName.toUpperCase();
  if (tag === 'INPUT') return !controls.has((element.type ?? 'text').toLowerCase());
  if (tag === 'TEXTAREA' || tag === 'SELECT' || element.isContentEditable) return true;
  return textRoles.has(element.getAttribute('role') ?? '');
}

/** Whether focus sits in a dialog, menu or popover of the page. */
export function inDialog(element: FocusedElement | null): boolean {
  return !!element && element.isConnected !== false && !!element.closest(dialogs);
}

/** Whether something of the page covers every pane: a host flow's dialog, a host view the page draws, the frame's dialog or one
 * of the page's menus, popovers and in-page dialogs (slot/bounds.ts `pageCovers` without the overlap: the person is using it). */
export function pageCovered(state: Pick<StoreState, 'dialogs' | 'natives' | 'shell' | 'overlays'>): boolean {
  return state.dialogs.length > 0 || state.natives.length > 0 || (state.shell?.dialogOpen ?? false) || state.overlays > 0;
}

export interface FocusInputs {
  /** The pane is the active one and shown. */
  live: boolean;
  /** The element with the page's focus now (`document.activeElement`). */
  focused: FocusedElement | null;
  covered: boolean;
  /** The person's last input moved through a list with the arrows (a tab strip): moving is not choosing. */
  navigating: boolean;
  /** The person chose this pane just now (its sidebar row or its tab): a field they were typing in is left behind. */
  chosen: boolean;
}

/** Whether a terminal takes the keyboard now (see [`FocusVerdict`]). */
export function focusVerdict({ live, focused, covered, navigating, chosen }: FocusInputs): FocusVerdict {
  if (!live || navigating) return 'drop';
  // A dialog, menu or popover keeps the keyboard while it is open; the terminal takes it once it goes.
  if (covered || inDialog(focused)) return 'wait';
  if (!chosen && typingIn(focused)) return 'drop';
  return 'take';
}

const navigationKeys = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown']);
/** How long a navigation key counts as "moving through a list" (ms). */
export const navigationWindow = 1000;
/** How long a press in the page explains the page getting the window's focus (ms): a click, not the app coming back. */
export const pressWindow = 400;

export interface KeyLike {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  /** The key went to a terminal (xterm's own keyboard): typing there, not moving through the page. */
  inTerminal?: boolean;
}

/** The person's last input to the page: a navigation key (an arrow, Home/End, Page Up/Down without ⌘, Ctrl or Alt) or a press. */
export class InputTracker {
  private last: { navigation: boolean; press: boolean; at: number } | null = null;

  key(event: KeyLike, now: number): void {
    const navigation = !event.inTerminal && navigationKeys.has(event.key) && !event.metaKey && !event.ctrlKey && !event.altKey;
    this.last = { navigation, press: false, at: now };
  }

  press(now: number): void {
    this.last = { navigation: false, press: true, at: now };
  }

  navigating(now: number): boolean {
    return !!this.last?.navigation && now - this.last.at < navigationWindow;
  }

  pressedLately(now: number): boolean {
    return !!this.last?.press && now - this.last.at < pressWindow;
  }
}

/** The page's tracker, listening from the first terminal pane on. */
export const inputs = new InputTracker();
let tracking = false;

export function trackInputs(target: Pick<Window, 'addEventListener'> = window): void {
  if (tracking) return;
  tracking = true;
  target.addEventListener('keydown', (event) => {
    const element = event.target as Element | null;
    inputs.key({ key: event.key, metaKey: event.metaKey, ctrlKey: event.ctrlKey, altKey: event.altKey, inTerminal: !!element?.closest?.(terminalKeyboard) }, Date.now());
  }, true);
  target.addEventListener('pointerdown', () => inputs.press(Date.now()), true);
}

/** How long a choice (a sidebar row, a tab) lets the pane take the keyboard from a text field (ms). */
export const choiceWindow = 1500;

// The panes the person chose by their sidebar row or their tab, for the terminal panes listening.
const choiceListeners = new Set<(paneId: string) => void>();

/** The person chose `paneId` (its sidebar row or its tab, by a click or Enter): a terminal pane takes the keyboard, even when it was
 * already the active one. */
export function chooseKeyboard(paneId: string): void {
  for (const listener of choiceListeners) listener(paneId);
}

export function subscribeChoices(listener: (paneId: string) => void): () => void {
  choiceListeners.add(listener);
  return () => choiceListeners.delete(listener);
}

export interface KeyboardWorld {
  /** Everything but `live` and `chosen`, read when the request is tried. */
  read(): Omit<FocusInputs, 'live' | 'chosen'>;
  /** Gives the terminal the keyboard; false while it cannot take it yet (xterm not mounted, the native view's slot not shown). */
  take(): boolean;
  now(): number;
}

/**
 * One terminal pane's keyboard requests. A request is made when the pane becomes the active one and shown (a click, a sidebar row,
 * a tab, a shortcut, the attention jump, a new pane, a layout change), when the person chooses it again, when the app's window comes
 * back while it is active, and when a cover comes over a native terminal that held the keyboard. A request waits while the page is
 * covered or the terminal is not ready, and is forgotten when the pane stops being active, the person moves with the arrows, or
 * they are typing somewhere else.
 */
export class TerminalKeyboard {
  private pending = false;
  private live = false;
  private covered = false;
  private chosenAt: number | null = null;

  constructor(private readonly world: KeyboardWorld) {}

  /** The pane's state after a render: active and shown (`live`), the page covered, and whether the page held the keyboard. */
  observe(live: boolean, covered: boolean, pageHasKeyboard: boolean): void {
    const becameLive = live && !this.live;
    const coverCame = covered && !this.covered;
    const coverWent = !covered && this.covered;
    this.live = live;
    this.covered = covered;
    if (!live) {
      this.pending = false;
      return;
    }
    if (becameLive) this.pending = true;
    // A cover hides a native terminal and the keyboard goes to the page; it comes back to the terminal when the cover goes.
    if (coverCame && !pageHasKeyboard) this.pending = true;
    if (becameLive || coverWent) this.attempt();
  }

  /** The person chose this pane (its sidebar row or its tab). */
  choose(): void {
    this.chosenAt = this.world.now();
    this.pending = true;
    if (this.live) this.attempt();
  }

  /** The app's window got the focus back; `pressed`: a press in the page did it (a click is the person's own choice). */
  windowFocused(pressed: boolean): void {
    if (!this.live || pressed) return;
    this.pending = true;
    this.attempt();
  }

  /** Tries a waiting request again (the terminal became ready, or the page changed). True when the terminal took the keyboard. */
  attempt(): boolean {
    if (!this.pending) return false;
    const now = this.world.now();
    const chosen = this.chosenAt !== null && now - this.chosenAt < choiceWindow;
    const verdict = focusVerdict({ ...this.world.read(), live: this.live, chosen });
    if (verdict === 'wait') return false;
    if (verdict === 'drop') {
      this.pending = false;
      this.chosenAt = null;
      return false;
    }
    if (!this.world.take()) return false;
    this.pending = false;
    this.chosenAt = null;
    return true;
  }

  /** Whether a request waits (the tests' check). */
  get waiting(): boolean {
    return this.pending;
  }
}

// Centralized modal focus-trap, Tab cycling and topmost-modal stack for DialogFrame (DialogShell.tsx).

export const FOCUSABLE_SELECTOR = [
  'a[href]:not([tabindex^="-"])',
  'button:not(:disabled):not([tabindex^="-"])',
  'input:not(:disabled):not([type="hidden"]):not([tabindex^="-"])',
  'select:not(:disabled):not([tabindex^="-"])',
  'textarea:not(:disabled):not([tabindex^="-"])',
  '[tabindex]:not([tabindex^="-"])',
].join(', ');

export function isFocusableElement(element: HTMLElement): boolean {
  if ((element as HTMLButtonElement).disabled) return false;
  const tabAttr = element.getAttribute?.('tabindex');
  if (tabAttr !== null && tabAttr !== undefined && tabAttr !== '' && Number(tabAttr) < 0) return false;
  if (typeof element.tabIndex === 'number' && element.tabIndex < 0) return false;
  if (element.closest?.('[hidden], [aria-hidden="true"], [inert]')) return false;
  if (
    typeof window !== 'undefined' &&
    typeof window.getComputedStyle === 'function' &&
    typeof Element !== 'undefined' &&
    element instanceof Element
  ) {
    const style = window.getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden') return false;
  }
  return true;
}

/** Focusable controls inside `root`, in DOM order, skipping disabled, negative-tabindex and hidden subtrees. */
export function focusableElements(
  root: { querySelectorAll<T extends Element = HTMLElement>(selector: string): Iterable<T> } | null | undefined,
): HTMLElement[] {
  if (!root) return [];
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)].filter(isFocusableElement);
}

/**
 * Computes the next focus target for `Tab` / `Shift+Tab` inside a modal.
 * If `active` is outside `elements` (e.g. `document.body` or the dialog frame itself), pulls focus onto the first
 * (`Tab`) or last (`Shift+Tab`) element; otherwise wraps strictly around `elements`.
 */
export function nextTabStop<T>(elements: readonly T[], active: unknown, shiftKey: boolean): T | null {
  if (elements.length === 0) return null;
  const index = elements.indexOf(active as T);
  if (index < 0) {
    return shiftKey ? elements[elements.length - 1]! : elements[0]!;
  }
  const step = shiftKey ? -1 : 1;
  return elements[(index + step + elements.length) % elements.length]!;
}

const modalStack: symbol[] = [];

/** Registers an active modal token on top of the stack; returns its cleanup. */
export function registerModal(token: symbol): () => void {
  const existing = modalStack.indexOf(token);
  if (existing >= 0) modalStack.splice(existing, 1);
  modalStack.push(token);
  return () => {
    const index = modalStack.indexOf(token);
    if (index >= 0) modalStack.splice(index, 1);
  };
}

/** True only when `token` is the topmost active modal. */
export function isTopModal(token: symbol): boolean {
  return modalStack.length > 0 && modalStack[modalStack.length - 1] === token;
}

/** Tests only: clear any registered modal tokens. */
export function resetModalStack(): void {
  modalStack.length = 0;
}

export interface ModalKeyEvent {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  isComposing?: boolean;
  keyCode?: number;
  defaultPrevented: boolean;
  preventDefault(): void;
  stopPropagation(): void;
}

export interface ModalKeyOptions {
  root: (HTMLElement & { querySelectorAll<T extends Element = HTMLElement>(selector: string): Iterable<T> }) | null;
  activeElement: unknown;
  busy?: boolean;
  kind: 'alert' | 'sheet' | 'panel';
  onCancel: () => void;
}

/** Handles modal `Escape` and `Tab` / `Shift+Tab` cycling; returns true when handled. */
export function handleModalKeyDown(event: ModalKeyEvent, options: ModalKeyOptions): boolean {
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return false;
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopPropagation();
    if (!(options.kind === 'panel' && options.busy)) options.onCancel();
    return true;
  }
  if (event.key === 'Tab' && !event.ctrlKey && !event.altKey && !event.metaKey) {
    const root = options.root;
    if (!root) return false;
    const items = focusableElements(root);
    event.preventDefault();
    const target = nextTabStop(items, options.activeElement, event.shiftKey) ?? root;
    target.focus({ preventScroll: true });
    return true;
  }
  return false;
}

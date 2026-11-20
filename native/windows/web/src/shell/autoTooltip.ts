// Every button gets a tooltip. A control that names itself with `title` keeps its own; one without gets, on hover, the words it
// already shows or speaks: its aria-label, else its visible text (a truncated pane name or a long label shows whole). The title is
// read again on every hover, so a label that changed (a state word, a count) is never stale, and a title the page set later wins.
// Installed once on the document (main.tsx); it covers the page's own buttons and the host's native forms (native/NativeNode.tsx) alike.

/** What the page treats as a control that can be pressed. */
export const PRESSABLE = 'button, [role="button"], [role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"], [role="tab"], [role="option"], [role="switch"], summary, a[href]';

/** Where a tooltip this module wrote keeps its words, so a title the page sets itself is told apart and left alone. */
export const AUTO_TITLE = 'data-auto-title';

const LONGEST = 160;

/** The tooltip `control` would show: its aria-label, else its text with the whitespace folded, cut to a sane length; empty for none. */
export function tooltipFor(control: Element): string {
  const spoken = control.getAttribute('aria-label')?.trim();
  const words = spoken || (control.textContent ?? '').replace(/\s+/g, ' ').trim();
  return words.length > LONGEST ? words.slice(0, LONGEST - 1) + '…' : words;
}

/** Gives `control` its tooltip unless it has its own: writes, refreshes or (when it has no words) removes the one this module wrote. */
export function applyTooltip(control: Element): void {
  const own = control.getAttribute('title');
  const auto = control.getAttribute(AUTO_TITLE);
  if (own !== null && own !== auto) return;
  const words = tooltipFor(control);
  if (words) {
    if (own !== words) control.setAttribute('title', words);
    control.setAttribute(AUTO_TITLE, words);
  } else if (auto !== null) {
    control.removeAttribute('title');
    control.removeAttribute(AUTO_TITLE);
  }
}

/** Listens on `doc` for the pointer coming over a control. Returns the uninstall. */
export function installAutoTooltips(doc: Document): () => void {
  const over = (event: Event) => {
    const target = event.target as Element | null;
    if (!target || typeof target.closest !== 'function') return;
    const control = target.closest(PRESSABLE);
    if (control) applyTooltip(control);
  };
  doc.addEventListener('pointerover', over, true);
  doc.addEventListener('focusin', over, true);
  return () => {
    doc.removeEventListener('pointerover', over, true);
    doc.removeEventListener('focusin', over, true);
  };
}

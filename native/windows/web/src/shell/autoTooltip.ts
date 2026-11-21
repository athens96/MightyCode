// Every interactive or labelled UI element gets a crisp tooltip on hover. A control that names itself with `title` keeps its own;
// one without gets, on hover, the words it already shows or speaks: its aria-label (plus aria-description when present), else its
// placeholder or visible text (a truncated pane name or a long label shows whole). In a real browser document, a lightweight
// theme-token-styled floating tooltip bubble shows after a short hover delay while temporarily holding the native `title` so the
// OS never draws a duplicate tooltip over it, restoring `title` as soon as the pointer leaves.

/** What the page treats as an interactive control that gets an automatic fallback tooltip when it carries no `title` of its own. */
export const PRESSABLE = 'button, [role="button"], [role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"], [role="radio"], [role="checkbox"], [role="tab"], [role="option"], [role="switch"], [role="link"], [role="treeitem"], [role="separator"], input:not([type="hidden"]), select, textarea, summary, a[href]';

/** Where a tooltip this module wrote keeps its words, so a title the page sets itself is told apart and left alone. */
export const AUTO_TITLE = 'data-auto-title';

/** Where the floating bubble temporarily holds `title` while hovering so the browser does not draw a second native OS tooltip. */
const HELD_TITLE = 'data-held-title';

const LONGEST = 160;
const SHOW_DELAY_MS = 260;
const WARM_DELAY_MS = 70;
const WARM_WINDOW_MS = 450;
const VIEWPORT_MARGIN = 8;
const BUBBLE_GAP = 6;

/** The tooltip `control` would show: its aria-label, else its placeholder or text with whitespace folded, plus aria-description. */
export function tooltipFor(control: Element): string {
  const spoken = control.getAttribute('aria-label')?.trim();
  const placeholder = control.getAttribute('placeholder')?.trim();
  const text = (control.textContent ?? '').replace(/\s+/g, ' ').trim();
  const base = spoken || text || placeholder || '';
  const description = control.getAttribute('aria-description')?.trim();
  const combined = description && !base.includes(description)
    ? (base ? `${base} — ${description}` : description)
    : base;
  return combined.length > LONGEST ? combined.slice(0, LONGEST - 1) + '…' : combined;
}

/** Gives `control` its tooltip unless it has its own: writes, refreshes or (when it has no words) removes the one this module wrote. */
export function applyTooltip(control: Element): void {
  const own = control.getAttribute('title') ?? control.getAttribute(HELD_TITLE);
  const auto = control.getAttribute(AUTO_TITLE);
  if (own !== null && own !== auto) return;
  const words = tooltipFor(control);
  if (words) {
    if (control.hasAttribute(HELD_TITLE)) {
      control.setAttribute(HELD_TITLE, words);
    } else if (own !== words) {
      control.setAttribute('title', words);
    }
    control.setAttribute(AUTO_TITLE, words);
  } else if (auto !== null) {
    control.removeAttribute('title');
    control.removeAttribute(HELD_TITLE);
    control.removeAttribute(AUTO_TITLE);
  }
}

/** Positions `bubble` centered below `anchor` (or above when closer to the bottom edge), clamped inside the window. */
function placeBubble(bubble: HTMLElement, anchor: Element): void {
  if (typeof anchor.getBoundingClientRect !== 'function') return;
  const rect = anchor.getBoundingClientRect();
  if (rect.width <= 0 && rect.height <= 0) return;
  const vw = typeof window !== 'undefined' ? window.innerWidth : 1280;
  const vh = typeof window !== 'undefined' ? window.innerHeight : 800;
  const bw = bubble.offsetWidth || 180;
  const bh = bubble.offsetHeight || 26;
  const preferAbove = rect.bottom + BUBBLE_GAP + bh + VIEWPORT_MARGIN > vh && rect.top - BUBBLE_GAP - bh >= VIEWPORT_MARGIN;
  const top = preferAbove ? rect.top - bh - BUBBLE_GAP : Math.min(vh - bh - VIEWPORT_MARGIN, rect.bottom + BUBBLE_GAP);
  const centeredLeft = rect.left + rect.width / 2 - bw / 2;
  const left = Math.max(VIEWPORT_MARGIN, Math.min(vw - bw - VIEWPORT_MARGIN, centeredLeft));
  bubble.style.left = `${Math.round(left)}px`;
  bubble.style.top = `${Math.round(Math.max(VIEWPORT_MARGIN, top))}px`;
}

/** Listens on `doc` for the pointer coming over a control or titled element. Returns the uninstall. */
export function installAutoTooltips(doc: Document): () => void {
  // Node / unit-test environments pass a minimal fake document without `body` or `createElement`.
  if (!doc.body || typeof doc.createElement !== 'function') {
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

  let active: Element | null = null;
  let bubble: HTMLDivElement | null = null;
  let showTimer: ReturnType<typeof setTimeout> | null = null;
  let visible = false;
  let lastHiddenAt = 0;

  const clearTimer = () => {
    if (showTimer !== null) {
      clearTimeout(showTimer);
      showTimer = null;
    }
  };

  const restoreHeld = (element: Element | null) => {
    if (!element) return;
    const held = element.getAttribute(HELD_TITLE);
    if (held !== null) {
      if (!element.hasAttribute('title')) element.setAttribute('title', held);
      element.removeAttribute(HELD_TITLE);
    }
  };

  const hide = () => {
    clearTimer();
    if (visible) lastHiddenAt = Date.now();
    visible = false;
    if (bubble) bubble.classList.remove('visible');
    restoreHeld(active);
    active = null;
  };

  const ensureBubble = (): HTMLDivElement => {
    if (bubble && bubble.isConnected) return bubble;
    bubble = doc.createElement('div');
    bubble.className = 'ui-tooltip';
    bubble.setAttribute('role', 'tooltip');
    bubble.setAttribute('aria-hidden', 'true');
    doc.body.appendChild(bubble);
    return bubble;
  };

  const show = (element: Element) => {
    if (!element.isConnected) { hide(); return; }
    const freshTitle = element.getAttribute('title');
    if (freshTitle !== null) {
      element.setAttribute(HELD_TITLE, freshTitle);
      element.removeAttribute('title');
    }
    const text = element.getAttribute(HELD_TITLE)?.trim() ?? '';
    if (!text) { hide(); return; }
    const node = ensureBubble();
    node.textContent = text;
    node.classList.add('visible');
    visible = true;
    placeBubble(node, element);
  };

  const enter = (target: Element | null, immediate = false) => {
    if (!target || typeof target.closest !== 'function') return;
    const control = target.closest(PRESSABLE);
    if (control) applyTooltip(control);
    const titled = target.closest(`[title], [${HELD_TITLE}]`);
    if (!titled) {
      hide();
      return;
    }
    if (titled === active) {
      const fresh = titled.getAttribute('title');
      if (fresh !== null) {
        titled.setAttribute(HELD_TITLE, fresh);
        titled.removeAttribute('title');
        if (visible && bubble) {
          bubble.textContent = fresh;
          placeBubble(bubble, titled);
        }
      }
      return;
    }
    restoreHeld(active);
    clearTimer();
    active = titled;
    const words = (titled.getAttribute('title') ?? titled.getAttribute(HELD_TITLE) ?? '').trim();
    if (!words) {
      hide();
      return;
    }
    titled.setAttribute(HELD_TITLE, words);
    titled.removeAttribute('title');
    const warm = visible || Date.now() - lastHiddenAt < WARM_WINDOW_MS;
    if (visible && bubble) bubble.classList.remove('visible');
    visible = false;
    const delay = immediate ? SHOW_DELAY_MS : warm ? WARM_DELAY_MS : SHOW_DELAY_MS;
    showTimer = setTimeout(() => {
      showTimer = null;
      if (active === titled) show(titled);
    }, delay);
  };

  const onPointerOver = (event: Event) => enter(event.target as Element | null);
  const onFocusIn = (event: Event) => {
    const target = event.target as Element | null;
    if (!target || typeof target.closest !== 'function') return;
    const control = target.closest(PRESSABLE);
    if (control) applyTooltip(control);
  };
  const onPointerOut = (event: Event) => {
    if (!active) return;
    const related = (event as PointerEvent).relatedTarget as Node | null;
    if (related && active.contains(related)) return;
    hide();
  };
  const dismiss = () => hide();

  doc.addEventListener('pointerover', onPointerOver, true);
  doc.addEventListener('pointerout', onPointerOut, true);
  doc.addEventListener('pointerdown', dismiss, true);
  doc.addEventListener('focusin', onFocusIn, true);
  doc.addEventListener('keydown', dismiss, true);
  doc.addEventListener('scroll', dismiss, true);
  if (typeof window !== 'undefined') window.addEventListener('blur', dismiss);

  return () => {
    hide();
    if (bubble) {
      bubble.remove();
      bubble = null;
    }
    doc.removeEventListener('pointerover', onPointerOver, true);
    doc.removeEventListener('pointerout', onPointerOut, true);
    doc.removeEventListener('pointerdown', dismiss, true);
    doc.removeEventListener('focusin', onFocusIn, true);
    doc.removeEventListener('keydown', dismiss, true);
    doc.removeEventListener('scroll', dismiss, true);
    if (typeof window !== 'undefined') window.removeEventListener('blur', dismiss);
  };
}

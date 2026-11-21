import { describe, expect, it, vi } from 'vitest';
import { AUTO_TITLE, applyTooltip, installAutoTooltips, PRESSABLE, tooltipFor } from './autoTooltip';

/** The few Element calls the module makes, without a DOM (the page's tests run in node). */
class FakeControl {
  attributes = new Map<string, string>();
  textContent: string | null;
  pressable: boolean;
  constructor(text: string, attributes: Record<string, string> = {}, pressable = true) {
    this.textContent = text;
    this.pressable = pressable;
    for (const [name, value] of Object.entries(attributes)) this.attributes.set(name, value);
  }
  getAttribute(name: string): string | null { return this.attributes.get(name) ?? null; }
  setAttribute(name: string, value: string): void { this.attributes.set(name, value); }
  removeAttribute(name: string): void { this.attributes.delete(name); }
  hasAttribute(name: string): boolean { return this.attributes.has(name); }
  closest(selector: string): FakeControl | null { return selector === PRESSABLE && this.pressable ? this : null; }
}

const el = (control: FakeControl) => control as unknown as Element;

/** A document that only keeps its listeners, to hover a control through them. */
function fakeDocument() {
  const listeners = new Map<string, (event: Event) => void>();
  const doc = {
    addEventListener: (type: string, listener: (event: Event) => void) => listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
  } as unknown as Document;
  const hover = (target: FakeControl) => listeners.get('pointerover')?.({ target } as unknown as Event);
  return { doc, hover, listeners };
}

describe('auto tooltips', () => {
  it('reads a control’s aria-label first, then its visible text or placeholder, and appends aria-description', () => {
    expect(tooltipFor(el(new FakeControl('×', { 'aria-label': 'Close' })))).toBe('Close');
    expect(tooltipFor(el(new FakeControl('  Approve\n  and run ')))).toBe('Approve and run');
    expect(tooltipFor(el(new FakeControl('', { placeholder: 'Search workspaces' })))).toBe('Search workspaces');
    expect(tooltipFor(el(new FakeControl('Next step', { 'aria-description': 'Fills the composer' })))).toBe('Next step — Fills the composer');
    expect(tooltipFor(el(new FakeControl('')))).toBe('');
    expect(tooltipFor(el(new FakeControl('a'.repeat(400)))).length).toBe(160);
  });

  it('gives a control without a title its words on hover and reads them again when they change', () => {
    const { doc, hover, listeners } = fakeDocument();
    const uninstall = installAutoTooltips(doc);
    const button = new FakeControl('Merge');
    hover(button);
    expect(button.getAttribute('title')).toBe('Merge');
    expect(button.getAttribute(AUTO_TITLE)).toBe('Merge');
    button.textContent = 'Merged';
    hover(button);
    expect(button.getAttribute('title')).toBe('Merged');
    uninstall();
    expect(listeners.size).toBe(0);
  });

  it('leaves a title the page set alone, even one set after an automatic one', () => {
    const own = new FakeControl('Open', { title: 'Open the pane in a new window' });
    applyTooltip(el(own));
    expect(own.getAttribute('title')).toBe('Open the pane in a new window');
    expect(own.hasAttribute(AUTO_TITLE)).toBe(false);
    const later = new FakeControl('Run');
    applyTooltip(el(later));
    later.setAttribute('title', 'Run it now in a new worktree');
    applyTooltip(el(later));
    expect(later.getAttribute('title')).toBe('Run it now in a new worktree');
  });

  it('drops an automatic title once the control has no words, and passes over what is not a control', () => {
    const { doc, hover } = fakeDocument();
    const uninstall = installAutoTooltips(doc);
    const option = new FakeControl('Request 3');
    hover(option);
    expect(option.getAttribute('title')).toBe('Request 3');
    option.textContent = '';
    hover(option);
    expect(option.hasAttribute('title')).toBe(false);
    const plain = new FakeControl('Not a control', {}, false);
    hover(plain);
    expect(plain.hasAttribute('title')).toBe(false);
    uninstall();
  });

  it('shows a floating tooltip bubble in a browser document, suppressing duplicate native title only while hovered', () => {
    vi.useFakeTimers();
    try {
      const listeners = new Map<string, (event: Event) => void>();
      const appended: unknown[] = [];
      class DomBubble {
        className = '';
        textContent: string | null = '';
        isConnected = true;
        style: Record<string, string> = {};
        offsetWidth = 140;
        offsetHeight = 24;
        attrs = new Map<string, string>();
        classList = {
          set: new Set<string>(),
          add: (c: string) => { this.classList.set.add(c); },
          remove: (c: string) => { this.classList.set.delete(c); },
          contains: (c: string) => this.classList.set.has(c),
        };
        setAttribute(k: string, v: string) { this.attrs.set(k, v); }
        remove() { this.isConnected = false; }
      }
      let createdBubble: DomBubble | null = null;
      const doc = {
        body: { appendChild: (node: unknown) => appended.push(node) },
        createElement: () => { createdBubble = new DomBubble(); return createdBubble; },
        addEventListener: (type: string, fn: (event: Event) => void) => listeners.set(type, fn),
        removeEventListener: (type: string) => listeners.delete(type),
      } as unknown as Document;

      const target = new FakeControl('Flow', { title: 'Flow view — Inspect requests on a 2D diagram' });
      Object.assign(target, {
        isConnected: true,
        contains: (other: unknown) => other === target,
        getBoundingClientRect: () => ({ left: 100, top: 40, right: 160, bottom: 68, width: 60, height: 28 }),
        closest: (selector: string) => (selector === PRESSABLE || selector.includes('[title]') ? target : null),
      });

      const uninstall = installAutoTooltips(doc);
      listeners.get('pointerover')?.({ target } as unknown as Event);
      // Native title is stashed while hovered so the OS does not show a duplicate tooltip.
      expect(target.hasAttribute('title')).toBe(false);
      vi.advanceTimersByTime(300);
      expect(createdBubble).not.toBeNull();
      expect(createdBubble!.textContent).toBe('Flow view — Inspect requests on a 2D diagram');
      expect(createdBubble!.classList.contains('visible')).toBe(true);

      // Pointer leaving restores native title immediately and hides the bubble.
      listeners.get('pointerout')?.({ target, relatedTarget: null } as unknown as Event);
      expect(target.getAttribute('title')).toBe('Flow view — Inspect requests on a 2D diagram');
      expect(createdBubble!.classList.contains('visible')).toBe(false);

      uninstall();
      expect(listeners.size).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});

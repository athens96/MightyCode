import { describe, expect, it } from 'vitest';
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
  it('reads a control’s aria-label first, then its visible text with the whitespace folded', () => {
    expect(tooltipFor(el(new FakeControl('×', { 'aria-label': 'Close' })))).toBe('Close');
    expect(tooltipFor(el(new FakeControl('  Approve\n  and run ')))).toBe('Approve and run');
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
});

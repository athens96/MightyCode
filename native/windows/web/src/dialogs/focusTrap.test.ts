import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { Store } from '../store/store';
import { Dialogs } from './Dialogs';
import {
  focusableElements,
  handleModalKeyDown,
  isTopModal,
  nextTabStop,
  registerModal,
  resetModalStack,
  type ModalKeyEvent,
} from './focusTrap';

function fakeElement(opts: {
  id: string;
  disabled?: boolean;
  tabIndex?: number;
  tabAttr?: string | null;
  hiddenAncestor?: boolean;
}) {
  const focus = vi.fn();
  const el = {
    id: opts.id,
    disabled: opts.disabled ?? false,
    tabIndex: opts.tabIndex ?? 0,
    getAttribute: (name: string) => (name === 'tabindex' ? (opts.tabAttr ?? null) : null),
    closest: (selector: string) => (opts.hiddenAncestor && selector.includes('[hidden]') ? ({} as Element) : null),
    focus,
  } as unknown as HTMLElement & { focus: ReturnType<typeof vi.fn> };
  return el;
}

function fakeKeyEvent(key: string, extra: Partial<ModalKeyEvent> = {}): ModalKeyEvent & { prevented: boolean; stopped: boolean } {
  const event = {
    key,
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 0,
    defaultPrevented: false,
    prevented: false,
    stopped: false,
    preventDefault() {
      event.prevented = true;
      event.defaultPrevented = true;
    },
    stopPropagation() {
      event.stopped = true;
    },
    ...extra,
  };
  return event;
}

describe('focusTrap nextTabStop', () => {
  it('cycles forward and backward strictly within focusable elements', () => {
    const a = { id: 'a' };
    const b = { id: 'b' };
    const c = { id: 'c' };
    const items = [a, b, c];

    expect(nextTabStop(items, a, false)).toBe(b);
    expect(nextTabStop(items, b, false)).toBe(c);
    expect(nextTabStop(items, c, false)).toBe(a);

    expect(nextTabStop(items, c, true)).toBe(b);
    expect(nextTabStop(items, b, true)).toBe(a);
    expect(nextTabStop(items, a, true)).toBe(c);
  });

  it('pulls escaped focus onto first element on Tab and last element on Shift+Tab', () => {
    const a = { id: 'a' };
    const b = { id: 'b' };
    const c = { id: 'c' };
    const outside = { id: 'body' };

    expect(nextTabStop([a, b, c], outside, false)).toBe(a);
    expect(nextTabStop([a, b, c], outside, true)).toBe(c);
    expect(nextTabStop([a, b, c], null, false)).toBe(a);
    expect(nextTabStop([a, b, c], null, true)).toBe(c);
  });

  it('handles single-element and empty lists cleanly', () => {
    const only = { id: 'only' };
    expect(nextTabStop([only], only, false)).toBe(only);
    expect(nextTabStop([only], only, true)).toBe(only);
    expect(nextTabStop([], only, false)).toBeNull();
  });
});

describe('focusTrap focusableElements', () => {
  it('excludes disabled, negative-tabindex and hidden-subtree controls', () => {
    const first = fakeElement({ id: 'first' });
    const disabled = fakeElement({ id: 'disabled', disabled: true });
    const skipped = fakeElement({ id: 'skipped', tabIndex: -1, tabAttr: '-1' });
    const hidden = fakeElement({ id: 'hidden', hiddenAncestor: true });
    const last = fakeElement({ id: 'last' });
    const root = {
      querySelectorAll: <T extends Element = HTMLElement>() => [first, disabled, skipped, hidden, last] as unknown as T[],
    };

    expect(focusableElements(root).map((el) => el.id)).toEqual(['first', 'last']);
    expect(focusableElements(null)).toEqual([]);
  });
});

describe('focusTrap modal stack & key handling', () => {
  afterEach(resetModalStack);

  it('tracks only the topmost active modal', () => {
    const lower = Symbol('lower');
    const upper = Symbol('upper');
    const releaseLower = registerModal(lower);
    expect(isTopModal(lower)).toBe(true);

    const releaseUpper = registerModal(upper);
    expect(isTopModal(lower)).toBe(false);
    expect(isTopModal(upper)).toBe(true);

    releaseUpper();
    expect(isTopModal(lower)).toBe(true);
    releaseLower();
    expect(isTopModal(lower)).toBe(false);
  });

  it('invokes cancel on Escape even when activeElement is outside, unless panel is busy or event prevented/composing', () => {
    const onCancel = vi.fn();
    const rootFocus = vi.fn();
    const first = fakeElement({ id: 'first' });
    const second = fakeElement({ id: 'second' });
    const root = {
      focus: rootFocus,
      querySelectorAll: <T extends Element = HTMLElement>() => [first, second] as unknown as T[],
    } as unknown as HTMLElement & { querySelectorAll<T extends Element = HTMLElement>(selector: string): Iterable<T> };

    const esc = fakeKeyEvent('Escape');
    expect(handleModalKeyDown(esc, { root, activeElement: null, kind: 'sheet', onCancel })).toBe(true);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(esc.prevented).toBe(true);
    expect(esc.stopped).toBe(true);

    // Busy panel swallows Escape without cancelling.
    const busyEsc = fakeKeyEvent('Escape');
    expect(handleModalKeyDown(busyEsc, { root, activeElement: null, kind: 'panel', busy: true, onCancel })).toBe(true);
    expect(onCancel).toHaveBeenCalledTimes(1);

    // Prevented or IME-composing Escape is ignored.
    expect(handleModalKeyDown(fakeKeyEvent('Escape', { defaultPrevented: true }), { root, activeElement: null, kind: 'alert', onCancel })).toBe(false);
    expect(handleModalKeyDown(fakeKeyEvent('Escape', { isComposing: true }), { root, activeElement: null, kind: 'alert', onCancel })).toBe(false);
    expect(handleModalKeyDown(fakeKeyEvent('Escape', { keyCode: 229 }), { root, activeElement: null, kind: 'alert', onCancel })).toBe(false);

    // Tab from escaped focus pulls onto first; Shift+Tab from first wraps onto second.
    const tab = fakeKeyEvent('Tab');
    expect(handleModalKeyDown(tab, { root, activeElement: { id: 'body' }, kind: 'sheet', onCancel })).toBe(true);
    expect(first.focus).toHaveBeenCalledWith({ preventScroll: true });

    const shiftTab = fakeKeyEvent('Tab', { shiftKey: true });
    expect(handleModalKeyDown(shiftTab, { root, activeElement: first, kind: 'sheet', onCancel })).toBe(true);
    expect(second.focus).toHaveBeenCalledWith({ preventScroll: true });

    // Empty modal focuses the dialog frame itself on Tab.
    const emptyRoot = {
      focus: rootFocus,
      querySelectorAll: <T extends Element = HTMLElement>() => [] as unknown as T[],
    } as unknown as HTMLElement & { querySelectorAll<T extends Element = HTMLElement>(selector: string): Iterable<T> };
    expect(handleModalKeyDown(fakeKeyEvent('Tab'), { root: emptyRoot, activeElement: null, kind: 'alert', onCancel })).toBe(true);
    expect(rootFocus).toHaveBeenCalledWith({ preventScroll: true });
  });
});

describe('PaneWorktreeChanges localized close button', () => {
  it('uses dialog.close locale key instead of hardcoded OK', () => {
    const store = new Store();
    store.dispatch({
      kind: 'dialog',
      id: 1,
      dialog: {
        type: 'paneWorktreeChanges',
        title: 'Changes',
        paneId: 'p1',
        files: [],
        none: 'No changes',
        pick: 'Pick a file',
      },
    });
    const services = {
      bridge: { call: () => Promise.resolve(null) },
      store,
      t: translator([{ 'dialog.close': '닫기', 'editor.open': '편집기에서 열기' }]),
    } as unknown as AppServices;
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(Dialogs)));
    expect(html).toContain('>닫기</button>');
    expect(html).not.toContain('>OK</button>');
  });
});

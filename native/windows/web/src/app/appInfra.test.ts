import { createElement, type ErrorInfo, type ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { devSnapshot } from '../bridge/devHost';
import { translator } from '../i18n/locale';
import { Store } from '../store/store';
import {
  ServicesContext,
  createSelectorSnapshot,
  shallowEqual,
  useStoreSelector,
  type AppServices,
} from './context';
import { escapeCssAttr } from './cssEscape';
import { ErrorBoundary } from './ErrorBoundary';

describe('escapeCssAttr', () => {
  it('escapes quotes, backslashes, brackets, control chars and NUL in fallback mode', () => {
    const originalCss = globalThis.CSS;
    Object.defineProperty(globalThis, 'CSS', { value: undefined, configurable: true, writable: true });
    try {
      expect(escapeCssAttr('plain-id_123')).toBe('plain-id_123');
      expect(escapeCssAttr('a"b\\c[1]')).toBe('a\\"b\\\\c\\[1\\]');
      expect(escapeCssAttr("it's")).toBe("it\\'s");
      expect(escapeCssAttr('line\nbreak\x00end')).toBe('line\\a break\ufffdend');
    } finally {
      Object.defineProperty(globalThis, 'CSS', { value: originalCss, configurable: true, writable: true });
    }
  });

  it('delegates to CSS.escape when available', () => {
    const originalCss = globalThis.CSS;
    const escape = vi.fn((value: string) => `escaped:${value}`);
    Object.defineProperty(globalThis, 'CSS', { value: { escape }, configurable: true, writable: true });
    try {
      expect(escapeCssAttr('pane"1')).toBe('escaped:pane"1');
      expect(escape).toHaveBeenCalledWith('pane"1');
    } finally {
      Object.defineProperty(globalThis, 'CSS', { value: originalCss, configurable: true, writable: true });
    }
  });
});

describe('ErrorBoundary', () => {
  it('renders healthy children directly without wrapping markup', () => {
    const html = renderToStaticMarkup(
      createElement(ErrorBoundary, { scope: 'pane', resetKey: 'p1' }, createElement('span', { className: 'ok' }, 'alive')),
    );
    expect(html).toBe('<span class="ok">alive</span>');
  });

  it('derives error state, renders recoverable fallback UI, logs to bridge, and resets on retry or resetKey change', () => {
    const logCall = vi.fn(() => Promise.resolve(null));
    const boundary = new ErrorBoundary({
      scope: 'pane',
      resetKey: 'pane-a',
      children: createElement('div', null, 'child'),
    });
    boundary.context = {
      bridge: { call: logCall },
      store: new Store(),
      t: translator([{}]),
    } as unknown as AppServices;

    const failure = new Error('boom inside pane');
    const derived = ErrorBoundary.getDerivedStateFromError(failure);
    expect(derived).toEqual({ error: failure });

    // Simulate React applying derived state and invoking componentDidCatch.
    boundary.state = { ...boundary.state, ...derived };
    boundary.componentDidCatch(failure, { componentStack: '\n    at BrokenPane\n    at PaneSlot' } as ErrorInfo);
    expect(logCall).toHaveBeenCalledWith('log', {
      level: 'error',
      message: '[ErrorBoundary:pane] boom inside pane\nat BrokenPane\n    at PaneSlot',
    });

    const fallbackHtml = renderToStaticMarkup(boundary.render() as ReactElement);
    expect(fallbackHtml).toContain('role="alert"');
    expect(fallbackHtml).toContain('data-testid="error-boundary-pane"');
    expect(fallbackHtml).toContain('This pane encountered an error.');
    expect(fallbackHtml).toContain('boom inside pane');
    expect(fallbackHtml).toContain('data-testid="error-boundary-retry"');
    expect(fallbackHtml).toContain('>Retry</button>');

    // Same resetKey keeps the error state; changing resetKey clears it.
    expect(ErrorBoundary.getDerivedStateFromProps({ scope: 'pane', resetKey: 'pane-a', children: null }, boundary.state)).toBeNull();
    expect(ErrorBoundary.getDerivedStateFromProps({ scope: 'pane', resetKey: 'pane-b', children: null }, boundary.state)).toEqual({
      error: null,
      resetKey: 'pane-b',
    });

    // Retry button handler clears the error via setState.
    const setState = vi.fn();
    boundary.setState = setState;
    const tree = boundary.render() as ReactElement<{ children: ReactElement<{ children: ReactElement[] }> }>;
    const retryButton = tree.props.children.props.children.find(
      (child) => child && typeof child === 'object' && (child.props as { 'data-testid'?: string })['data-testid'] === 'error-boundary-retry',
    ) as ReactElement<{ onClick: () => void }>;
    retryButton.props.onClick();
    expect(setState).toHaveBeenCalledWith({ error: null });
  });

  it('does not throw in componentDidCatch when ServicesContext is absent', () => {
    const boundary = new ErrorBoundary({ scope: 'app', children: null });
    boundary.context = null;
    expect(() => boundary.componentDidCatch(new Error('root crash'), { componentStack: '' } as ErrorInfo)).not.toThrow();
  });
});

describe('shallowEqual', () => {
  it('handles primitives, plain objects, arrays and non-plain objects accurately', () => {
    expect(shallowEqual(1, 1)).toBe(true);
    expect(shallowEqual(NaN, NaN)).toBe(true);
    expect(shallowEqual(+0, -0)).toBe(false);
    expect(shallowEqual(null, {})).toBe(false);

    const shared = { nested: true };
    expect(shallowEqual({ a: 1, b: shared }, { a: 1, b: shared })).toBe(true);
    expect(shallowEqual({ a: 1, b: undefined }, { a: 1, c: undefined })).toBe(false);
    expect(shallowEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);

    expect(shallowEqual([1, shared], [1, shared])).toBe(true);
    expect(shallowEqual([1, 2], [1, 3])).toBe(false);
    expect(shallowEqual([1], { 0: 1 })).toBe(false);

    const sameMap = new Map([['a', 1]]);
    expect(shallowEqual(sameMap, sameMap)).toBe(true);
    expect(shallowEqual(new Map([['a', 1]]), new Map([['b', 2]]))).toBe(false);
    expect(shallowEqual(new Set([1]), new Set([2]))).toBe(false);
    expect(shallowEqual(new Date(100), new Date(200))).toBe(false);
  });
});

describe('useStoreSelector / createSelectorSnapshot', () => {
  it('keeps pane B snapshot referentially identical when only pane A receives a streaming delta', () => {
    const store = new Store();
    store.dispatch({ kind: 'snapshot', snapshot: devSnapshot() });
    const [paneA, paneB] = store.getState().snapshot!.sessions;
    expect(paneA).toBeDefined();
    expect(paneB).toBeDefined();

    let runsB = 0;
    const selectorB = (state: ReturnType<Store['getState']>) => {
      runsB += 1;
      return {
        session: state.snapshot?.sessions.find((s) => s.id === paneB!.id),
        activity: state.activities[paneB!.id],
        own: state.ownSubmits[paneB!.id] ?? 0,
      };
    };
    const getB = createSelectorSnapshot(store.getState, () => selectorB, () => shallowEqual);
    const getA = createSelectorSnapshot(
      store.getState,
      () => (state) => state.snapshot?.sessions.find((s) => s.id === paneA!.id),
      () => Object.is,
    );

    const firstA = getA();
    const firstB = getB();
    // Repeated reads on unchanged state hit cache and keep exact identity.
    expect(getB()).toBe(firstB);
    expect(runsB).toBe(1);

    // Streaming delta updates only pane A.
    store.dispatch({
      kind: 'delta',
      sessions: [{ id: paneA!.id, logs: { added: [{ id: 'stream-1', kind: 'assistant', text: 'hello', timestamp: '2026-01-01T00:00:00Z' }] } }],
      removed: [],
    });

    const afterA = getA();
    const afterB = getB();
    expect(afterA).not.toBe(firstA);
    expect(afterB).toBe(firstB);

    // Updating pane B's activity triggers a new reference for pane B.
    store.dispatch({
      kind: 'event',
      event: { sessionId: paneB!.id, type: 'activity', activity: { id: 'act-b', provider: 'claude', kind: 'tool', state: 'running', summary: 'read' } },
    });
    const updatedB = getB();
    expect(updatedB).not.toBe(firstB);
    expect(updatedB.activity?.id).toBe('act-b');
  });

  it('re-evaluates when the selector closure changes even if the store state is unchanged', () => {
    const store = new Store();
    store.dispatch({ kind: 'snapshot', snapshot: devSnapshot() });
    const [paneA, paneB] = store.getState().snapshot!.sessions;

    let selector = (state: ReturnType<Store['getState']>) => state.snapshot?.sessions.find((s) => s.id === paneA!.id)?.id;
    const get = createSelectorSnapshot(store.getState, () => selector, () => Object.is);
    expect(get()).toBe(paneA!.id);

    selector = (state: ReturnType<Store['getState']>) => state.snapshot?.sessions.find((s) => s.id === paneB!.id)?.id;
    expect(get()).toBe(paneB!.id);
  });

  it('renders selected state cleanly via React useStoreSelector', () => {
    const store = new Store();
    store.dispatch({ kind: 'snapshot', snapshot: devSnapshot() });
    const services = {
      bridge: { call: () => Promise.resolve(null) },
      store,
      t: translator([{}]),
    } as unknown as AppServices;

    function ThemeBadge() {
      const { theme, count } = useStoreSelector(
        (state) => ({ theme: state.snapshot?.theme ?? 'none', count: state.snapshot?.sessions.length ?? 0 }),
        shallowEqual,
      );
      return createElement('span', { 'data-theme': theme }, String(count));
    }

    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(ThemeBadge)));
    expect(html).toBe('<span data-theme="dark">2</span>');
  });
});

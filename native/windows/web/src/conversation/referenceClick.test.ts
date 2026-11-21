import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { Markdown } from './MarkdownView';
import { linkKeyTarget } from './referenceClick';

function fakeLink(attrs: { url?: string; path?: string; line?: string; roleLink?: boolean }) {
  const linkEl = {
    dataset: {
      ...(attrs.url !== undefined ? { url: attrs.url } : {}),
      ...(attrs.path !== undefined ? { path: attrs.path } : {}),
      ...(attrs.line !== undefined ? { line: attrs.line } : {}),
    },
    closest(selector: string) {
      if (selector === '[role="link"]') return attrs.roleLink === false ? null : linkEl;
      if (selector === '[data-url]') return attrs.url !== undefined ? linkEl : null;
      if (selector === '[data-path]') return attrs.path !== undefined ? linkEl : null;
      return null;
    },
  } as unknown as Element;
  return linkEl;
}

describe('linkKeyTarget keyboard activation on role="link"', () => {
  it('activates web and local links on plain Enter without requiring Ctrl', () => {
    const webLink = fakeLink({ url: 'https://example.com/docs' });
    expect(linkKeyTarget({ key: 'Enter', target: webLink })).toEqual({ url: 'https://example.com/docs' });

    const fileLink = fakeLink({ path: 'src/dialogs/DialogShell.tsx', line: '42' });
    expect(linkKeyTarget({ key: 'Enter', target: fileLink })).toEqual({
      path: 'src/dialogs/DialogShell.tsx',
      line: 42,
    });

    // Child span inside a [role="link"] delegates via closest('[role="link"]').
    const childSpan = { closest: (selector: string) => fileLink.closest(selector) };
    expect(linkKeyTarget({ key: 'Enter', target: childSpan })).toEqual({
      path: 'src/dialogs/DialogShell.tsx',
      line: 42,
    });
  });

  it('keeps Ctrl+Enter and Meta+Enter working on role="link"', () => {
    const fileLink = fakeLink({ path: 'src/main.tsx' });
    expect(linkKeyTarget({ key: 'Enter', ctrlKey: true, target: fileLink })).toEqual({ path: 'src/main.tsx', line: null });
    expect(linkKeyTarget({ key: 'Enter', metaKey: true, target: fileLink })).toEqual({ path: 'src/main.tsx', line: null });
  });

  it('ignores non-Enter keys, Shift/Alt modifiers, IME composition, prevented events and non-links', () => {
    const webLink = fakeLink({ url: 'https://example.com' });
    expect(linkKeyTarget({ key: ' ', target: webLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', shiftKey: true, target: webLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', altKey: true, target: webLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', isComposing: true, target: webLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', nativeEvent: { isComposing: true }, target: webLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', defaultPrevented: true, target: webLink })).toBeNull();

    const nonLink = fakeLink({ path: 'src/a.ts', roleLink: false });
    expect(linkKeyTarget({ key: 'Enter', target: nonLink })).toBeNull();
    expect(linkKeyTarget({ key: 'Enter', target: null })).toBeNull();
  });

  it('renders focusable role="link" elements with data-url and data-path in Markdown', () => {
    const services = {
      bridge: { call: () => Promise.resolve(null) },
      store: {},
      t: translator([{}]),
    } as unknown as AppServices;
    const html = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(Markdown, { text: 'See [Docs](https://example.com) and `src/app/App.tsx:10`.' }),
      ),
    );
    expect(html).toContain('role="link" tabindex="0"');
    expect(html).toContain('data-url="https://example.com"');
    expect(html).toContain('data-path="src/app/App.tsx"');
    expect(html).toContain('data-line="10"');
  });
});

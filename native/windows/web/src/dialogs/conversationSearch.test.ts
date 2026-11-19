import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { isMenuPush } from '../shell/menu';
import { macShortcutAction, shortcutAction } from '../shell/shortcuts';
import { ConversationSearch, Hit } from './ConversationSearch';
import { highlight, moveSelection, searchTerms } from './conversationSearchRules';

const services = {
  bridge: { call: () => new Promise(() => {}) },
  store: { dispatch: () => {} },
  t: translator([{
    'dialog.close': 'Close', 'dialog.loading': 'Loading…', 'conversationSearch.placeholder': 'Words', 'conversationSearch.note': 'Only requests and answers',
    'conversationSearch.export': 'Save as Markdown…', 'conversationSearch.resume': 'Continue',
  }]),
} as unknown as AppServices;
const render = (element: ReturnType<typeof createElement>) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, element));

describe('대화 검색 (ConversationSearch.tsx)', () => {
  it('splits a query as the host does', () => {
    expect(searchTerms('  Migration   USERS ')).toEqual(['migration', 'users']);
    expect(searchTerms('   ')).toEqual([]);
  });

  it('marks every place a term stands in the snippet, ignoring case, and joins overlaps', () => {
    expect(highlight('Add a Migration for users', 'migration USERS')).toEqual([
      { text: 'Add a ', match: false }, { text: 'Migration', match: true }, { text: ' for ', match: false }, { text: 'users', match: true },
    ]);
    expect(highlight('banana', 'ana')).toEqual([{ text: 'b', match: false }, { text: 'anana', match: true }]);
    expect(highlight('마이그레이션 해 줘', '이션')).toEqual([{ text: '마이그레', match: false }, { text: '이션', match: true }, { text: ' 해 줘', match: false }]);
    // Lowering that changes the length ("İ" → "i̇") keeps the marks on the right letters.
    expect(highlight('İİ needle', 'needle')).toEqual([{ text: 'İİ ', match: false }, { text: 'needle', match: true }]);
    expect(highlight('plain', '  ')).toEqual([{ text: 'plain', match: false }]);
    expect(highlight('', 'x')).toEqual([]);
  });

  it('moves the chosen hit with the arrow keys, held at the ends', () => {
    expect(moveSelection(0, 'ArrowDown', 3)).toBe(1);
    expect(moveSelection(2, 'ArrowDown', 3)).toBe(2);
    expect(moveSelection(0, 'ArrowUp', 3)).toBe(0);
    expect(moveSelection(1, 'PageDown', 3)).toBe(2);
    expect(moveSelection(4, 'PageUp', 9)).toBe(0);
    expect(moveSelection(0, 'ArrowDown', 0)).toBeNull();
    expect(moveSelection(0, 'a', 3)).toBeNull();
  });

  it('opens with ⇧⌘F on the Mac and Ctrl+Shift+F on Windows, and from the menu bar', () => {
    const key = (code: string, extra: Partial<Parameters<typeof shortcutAction>[0]> = {}) => ({ key: '', code, ctrlKey: false, shiftKey: true, altKey: false, metaKey: false, ...extra });
    expect(shortcutAction(key('KeyF', { ctrlKey: true }))).toBe('conversationSearch');
    expect(shortcutAction(key('KeyF', { ctrlKey: true, shiftKey: false }))).toBeNull();
    expect(macShortcutAction(key('KeyF', { metaKey: true }))).toBe('conversationSearch');
    expect(macShortcutAction(key('KeyF', { ctrlKey: true }))).toBeNull();
    expect(isMenuPush({ kind: 'menu', action: 'conversationSearch' } as { kind: string })).toBe(true);
  });

  it('shows the wait while the sessions are read, with the field focused first', () => {
    const html = render(createElement(ConversationSearch, { id: 7, dialog: { type: 'conversationSearch', title: 'Search conversations', path: '/work/app' }, answer: () => {} }));
    expect(html).toContain('data-testid="conversation-search-field"');
    expect(html).toContain('data-autofocus');
    expect(html).toContain('data-testid="dialog-loading"');
    expect(html).toContain('Only requests and answers');
  });

  it('draws a hit with its agent, date, marked snippet and both actions', () => {
    const hit = { id: 'claude:abc', provider: 'claude', agent: 'Claude', title: 'Fix the relay', date: 'Nov 3, 10:00', snippet: '…the relay deploy fails…', matches: '2 matching messages' };
    const html = render(createElement(Hit, { hit, query: 'deploy', selected: true, onSelect: () => {}, onResume: () => {}, onExport: () => {} }));
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('Claude · Nov 3, 10:00');
    expect(html).toContain('<mark>deploy</mark>');
    expect(html).toContain('data-testid="conversation-resume-claude:abc"');
    expect(html).toContain('Save as Markdown…');
    expect(html).toContain('2 matching messages');
  });
});

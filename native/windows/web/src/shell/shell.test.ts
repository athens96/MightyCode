import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { Store } from '../store/store';
import { StatusBar } from './Chrome';
import { filterWorkspaces } from './filter';
import { isMenuPush } from './menu';
import { macShortcutAction, shortcutAction } from './shortcuts';
import type { ShellModel } from './types';

const key = (code: string, extra: Partial<{ key: string; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) =>
  ({ key: extra.key ?? '', code, ctrlKey: extra.ctrlKey ?? true, shiftKey: extra.shiftKey ?? false, altKey: extra.altKey ?? false, metaKey: false });

describe('sidebar search (AddPaneMenu.Filtered)', () => {
  const list = [{ name: 'Alpha', path: 'C:\\work\\alpha' }, { name: 'Beta', path: 'D:\\Projects\\b' }];
  it('matches name or path, case-insensitively, trimmed', () => {
    expect(filterWorkspaces(list, '  ALP ').map((w) => w.name)).toEqual(['Alpha']);
    expect(filterWorkspaces(list, 'projects').map((w) => w.name)).toEqual(['Beta']);
    expect(filterWorkspaces(list, '   ')).toHaveLength(2);
    expect(filterWorkspaces(list, 'zzz')).toHaveLength(0);
  });
});

describe('shortcuts (spec section 8)', () => {
  it('maps the window keys', () => {
    expect(shortcutAction(key('KeyO'))).toBe('openFolder');
    expect(shortcutAction(key('KeyN'))).toBe('newPane');
    expect(shortcutAction(key('KeyK'))).toBe('search');
    expect(shortcutAction(key('KeyB'))).toBe('toggleSidebar');
    expect(shortcutAction(key('KeyT'))).toBe('newShell');
    expect(shortcutAction(key('Comma'))).toBe('settings');
    expect(shortcutAction(key('KeyE', { shiftKey: true }))).toBe('files');
    expect(shortcutAction(key('F1', { key: 'F1', ctrlKey: false }))).toBe('help');
    expect(shortcutAction(key('Equal'))).toBe('textLarger');
    expect(shortcutAction(key('Minus'))).toBe('textSmaller');
    expect(shortcutAction(key('Digit0'))).toBe('textReset');
  });
  it('leaves other keys alone', () => {
    expect(shortcutAction(key('KeyA'))).toBeNull();
    expect(shortcutAction(key('KeyO', { ctrlKey: false }))).toBeNull();
    expect(shortcutAction(key('KeyO', { altKey: true }))).toBeNull();
  });
});

describe("the Mac's shortcuts and menu (D9, the Rust host on macOS only)", () => {
  const mac = (code: string, extra: Partial<{ key: string; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean }> = {}) =>
    ({ key: extra.key ?? '', code, ctrlKey: extra.ctrlKey ?? false, shiftKey: extra.shiftKey ?? false, altKey: extra.altKey ?? false, metaKey: extra.metaKey ?? true });
  it('maps the menu bar keys with ⌘', () => {
    expect(macShortcutAction(mac('KeyO'))).toBe('openFolder');
    expect(macShortcutAction(mac('KeyN'))).toBe('newPane');
    expect(macShortcutAction(mac('KeyK'))).toBe('search');
    expect(macShortcutAction(mac('KeyT'))).toBe('newShell');
    expect(macShortcutAction(mac('Comma'))).toBe('settings');
    expect(macShortcutAction(mac('KeyE', { shiftKey: true }))).toBe('files');
    expect(macShortcutAction(mac('Slash', { key: '?', shiftKey: true }))).toBe('help');
    // A layout whose ? is elsewhere (French: ⇧, on the M key's place) still reaches help.
    expect(macShortcutAction(mac('KeyM', { key: '?', shiftKey: true }))).toBe('help');
    expect(macShortcutAction(mac('Slash', { key: '/', shiftKey: false }))).toBeNull();
    expect(macShortcutAction(mac('KeyS', { ctrlKey: true }))).toBe('toggleSidebar');
    expect(macShortcutAction(mac('Equal'))).toBe('textLarger');
    expect(macShortcutAction(mac('Minus'))).toBe('textSmaller');
    expect(macShortcutAction(mac('Digit0'))).toBe('textReset');
  });
  it('leaves Ctrl keys to the text fields and ⌘ keys of its own to the system', () => {
    expect(macShortcutAction(mac('KeyK', { metaKey: false, ctrlKey: true }))).toBeNull();
    expect(macShortcutAction(mac('KeyB'))).toBeNull();
    expect(macShortcutAction(mac('KeyC'))).toBeNull();
    expect(macShortcutAction(mac('KeyO', { altKey: true }))).toBeNull();
  });
  it("takes only the menu's actions from the host", () => {
    expect(isMenuPush({ kind: 'menu', action: 'workMap' } as { kind: string })).toBe(true);
    expect(isMenuPush({ kind: 'menu', action: 'quit' } as { kind: string })).toBe(false);
    expect(isMenuPush({ kind: 'host' })).toBe(false);
  });
});

describe('usage reset link validation (Chrome.tsx)', () => {
  const services = {
    bridge: { call: () => Promise.resolve(null) },
    store: new Store(),
    t: translator([{}]),
  } as unknown as AppServices;

  const renderStatus = (url: string) => {
    const shell = {
      status: { machine: 'local', panes: '1', running: '0' },
      usage: {
        title: 'Usage',
        refreshLabel: 'Refresh',
        refreshTooltip: 'Refresh',
        refreshing: false,
        shared: '',
        cards: [{
          provider: 'claude',
          name: 'Claude',
          title: 'Claude',
          beta: false,
          windows: [],
          reset: { title: 'Reset', rows: [], link: 'Open guide', url },
        }],
      },
    } as unknown as ShellModel;
    return renderToStaticMarkup(
      createElement(ServicesContext.Provider, { value: services },
        createElement(StatusBar, { shell, usageOpen: true, setUsageOpen: () => {} })),
    );
  };

  it('enables valid http/https reset URLs and disables unsafe schemes', () => {
    const valid = renderStatus('https://example.com/reset');
    expect(valid).toContain('Open guide');
    expect(valid).not.toMatch(/<button[^>]*disabled=""[^>]*>Open guide<\/button>/);

    const invalid = renderStatus('javascript:alert(1)');
    expect(invalid).toMatch(/<button[^>]*disabled=""[^>]*>Open guide<\/button>/);
  });
});

// The Mac's menu bar in the Rust host's build (plan D9; every use is behind `import.meta.env.MODE === 'tauri'`, so the C# build
// holds none of it): the host pushes `{kind:'menu', action}` for an item that is page UI, and the shell runs it as it runs the
// same ⌘ shortcut, never while a sheet is up (Swift disables those items under a modal).
import { useEffect, useRef } from 'react';
import { useServices } from '../app/context';
import type { AppSnapshot } from '../store/models';
import { hostCapabilities } from '../slot/host';
import type { ShortcutAction } from './shortcuts';
import { sidebarSearchId } from './Sidebar';
import { toggleReview } from '../review/queue';

/** The page's actions the menu reaches: its shortcuts' and the menu-only items. */
export type MenuAction =
  | ShortcutAction
  | 'newFolder' | 'fromGit' | 'newCodexPane' | 'newAntigravityPane'
  | 'dashboard' | 'layoutGrid' | 'layoutColumns' | 'layoutFocus' | 'layoutTabs'
  | 'workMap' | 'usage';

/** host → page. */
export interface MenuPush {
  kind: 'menu';
  action: MenuAction;
}

const actions: readonly string[] = [
  'openFolder', 'newPane', 'newCodexPane', 'newAntigravityPane', 'search', 'toggleSidebar', 'newShell', 'settings', 'files', 'help',
  'textLarger', 'textSmaller', 'textReset', 'conversationSearch', 'reviewQueue', 'nextAttention', 'newFolder', 'fromGit',
  'dashboard', 'layoutGrid', 'layoutColumns', 'layoutFocus', 'layoutTabs', 'workMap', 'usage',
];

export function isMenuPush(message: { kind: string }): message is MenuPush {
  return message.kind === 'menu' && actions.includes(String((message as { action?: unknown }).action));
}

const listeners = new Set<(action: MenuAction) => void>();

export function dispatchMenu(message: MenuPush): void {
  for (const listener of listeners) listener(message.action);
}

/** Whether the page runs in the Rust host on the Mac (D9's second gate). */
export function macHost(): boolean {
  return hostCapabilities()?.platform === 'macos';
}

type Call = (method: string, args?: unknown) => Promise<unknown>;

/** One action, as the window's shortcut runs it (Shell.tsx's `useShortcuts`), and the menu's items without a key. */
export function runAction(action: MenuAction, call: Call, snapshot: AppSnapshot, openUsage: () => void, showsDashboard = false): void {
  const run = (method: string, args?: unknown) => void call(method, args);
  switch (action) {
    case 'openFolder': return run('pickFolder');
    case 'newFolder': return run('workspaceCreate', { kind: 'newFolder' });
    case 'fromGit': return run('workspaceCreate', { kind: 'fromGit' });
    case 'newPane': return run('newPane', { kind: 'claude' });
    case 'newCodexPane': return run('newPane', { kind: 'claude', provider: 'codex' });
    case 'newAntigravityPane': return run('newPane', { kind: 'claude', provider: 'gemini' });
    case 'newShell': return run('newPane', { kind: 'shell' });
    case 'toggleSidebar': return run('setSidebarCollapsed', { collapsed: !snapshot.sidebarCollapsed });
    case 'dashboard': return run(showsDashboard ? 'hideDashboard' : 'showDashboard');
    case 'layoutGrid': return run('applyLayoutPreset', { preset: 'grid' });
    case 'layoutColumns': return run('applyLayoutPreset', { preset: 'columns' });
    case 'layoutFocus': return run('applyLayoutPreset', { preset: 'focus' });
    case 'layoutTabs': return run('applyLayoutPreset', { preset: 'tabs' });
    case 'search': {
      const focus = () => { const field = document.getElementById(sidebarSearchId) as HTMLInputElement | null; field?.focus(); field?.select(); };
      if (snapshot.sidebarCollapsed) void call('setSidebarCollapsed', { collapsed: false }).then(() => requestAnimationFrame(focus));
      else focus();
      return;
    }
    case 'settings': return run('openSettings');
    case 'files': return run('openFiles', {});
    case 'conversationSearch': return run('openConversationSearch', {});
    case 'reviewQueue': return toggleReview();
    case 'nextAttention': return run('attentionNext');
    case 'workMap': return snapshot.activeWorkspaceId ? run('openWorkMap', { workspaceId: snapshot.activeWorkspaceId }) : undefined;
    case 'usage': return openUsage();
    case 'help': return run('openHelp');
    case 'textLarger': return run('textSize', { step: 1 });
    case 'textSmaller': return run('textSize', { step: -1 });
    case 'textReset': return run('textSize', { step: 0 });
  }
}

/** The menu's pushes run while the shell is up; none while a sheet, a native view or the picture viewer is open (except Help). */
export function useMenu(blocked: boolean, setUsageOpen: (open: boolean) => void): void {
  const { bridge, store } = useServices();
  const state = useRef({ blocked, setUsageOpen });
  state.current = { blocked, setUsageOpen };
  useEffect(() => {
    if (import.meta.env.MODE !== 'tauri' && import.meta.env.MODE !== 'test') return;
    const call: Call = (method, args) => bridge.call(method, args).catch(() => undefined);
    const listener = (action: MenuAction) => {
      const storeState = store.getState();
      const current = storeState.snapshot;
      const { blocked: held, setUsageOpen: usage } = state.current;
      if (!macHost() || (held && action !== 'help') || !current) return;
      runAction(action, call, current, () => usage(true), Boolean(storeState.shell?.dashboard));
    };
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  }, [bridge, store]);
}

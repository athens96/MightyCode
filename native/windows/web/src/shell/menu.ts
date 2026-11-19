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

/** The page's actions the menu reaches: its shortcuts' and the four items that have no key. */
export type MenuAction = ShortcutAction | 'newFolder' | 'fromGit' | 'workMap' | 'usage';

/** host → page. */
export interface MenuPush {
  kind: 'menu';
  action: MenuAction;
}

const actions: readonly string[] = [
  'openFolder', 'newPane', 'search', 'toggleSidebar', 'newShell', 'settings', 'files', 'help', 'textLarger', 'textSmaller', 'textReset',
  'conversationSearch', 'reviewQueue', 'newFolder', 'fromGit', 'workMap', 'usage',
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

/** One action, as the window's shortcut runs it (Shell.tsx's `useShortcuts`), and the menu's four more. */
export function runAction(action: MenuAction, call: Call, snapshot: AppSnapshot, openUsage: () => void): void {
  const run = (method: string, args?: unknown) => void call(method, args);
  switch (action) {
    case 'openFolder': return run('pickFolder');
    case 'newFolder': return run('workspaceCreate', { kind: 'newFolder' });
    case 'fromGit': return run('workspaceCreate', { kind: 'fromGit' });
    case 'newPane': return run('newPane', { kind: 'claude' });
    case 'newShell': return run('newPane', { kind: 'shell' });
    case 'toggleSidebar': return run('setSidebarCollapsed', { collapsed: !snapshot.sidebarCollapsed });
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
    case 'workMap': return snapshot.activeWorkspaceId ? run('openWorkMap', { workspaceId: snapshot.activeWorkspaceId }) : undefined;
    case 'usage': return openUsage();
    case 'help': return run('openHelp');
    case 'textLarger': return run('textSize', { step: 1 });
    case 'textSmaller': return run('textSize', { step: -1 });
    case 'textReset': return run('textSize', { step: 0 });
  }
}

/** The menu's pushes run while the shell is up; none while a sheet, a native view or the picture viewer is open, or before the state. */
export function useMenu(snapshot: AppSnapshot | null, blocked: boolean, setUsageOpen: (open: boolean) => void): void {
  const { bridge } = useServices();
  const state = useRef({ snapshot, blocked, setUsageOpen });
  state.current = { snapshot, blocked, setUsageOpen };
  useEffect(() => {
    const call: Call = (method, args) => bridge.call(method, args).catch(() => undefined);
    const listener = (action: MenuAction) => {
      const { snapshot: current, blocked: held, setUsageOpen: usage } = state.current;
      if (!macHost() || held || !current) return;
      runAction(action, call, current, () => usage(true));
    };
    listeners.add(listener);
    return () => { listeners.delete(listener); };
  }, [bridge]);
}

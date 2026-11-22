// The window (spec section 1): the sidebar column with its grip, and the detail column of banners, workspace header, the
// panes (layout, dashboard, welcome or the empty-panes invitation) and the status bar; the menus, the dialogs and the
// window's shortcuts (section 8) over them.
import { lazy, Suspense, useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { shallowEqual, useServices, useStoreSelector } from '../app/context';
import { escapeCssAttr } from '../app/cssEscape';
import { Dashboard, EmptyPanes, Welcome } from '../dashboard/Dashboard';
import { Dialogs } from '../dialogs/Dialogs';
import { Layout } from '../layout/Layout';
import { dragSlack, layoutMetrics } from '../layout/geometry';
import { Banners, StatusBar, WorkspaceHeader } from './Chrome';
import { MenuHost } from './parts';
import { macShortcutAction, shortcutAction } from './shortcuts';
import { macHost, useMenu } from './menu';
import { Sidebar, sidebarSearchId } from './Sidebar';
import { toggleReview, useReviewOpen } from '../review/queue';

const LazySurfaces = lazy(() => import('../native/Surfaces').then((m) => ({ default: m.Surfaces })));
const LazyReviewPanel = lazy(() => import('../review/ReviewPanel').then((m) => ({ default: m.ReviewPanel })));
const LazyPictureViewer = lazy(() => import('../pictures/PictureViewer').then((m) => ({ default: m.PictureViewer })));
const LazyUsageReportPanel = lazy(() => import('./UsageReportPanel').then((m) => ({ default: m.UsageReportPanel })));
const LazyIssuePickerPanel = lazy(() => import('../integrations/IssuePickerPanel').then((m) => ({ default: m.IssuePickerPanel })));
const LazyGitWorkbenchPanel = lazy(() => import('../git/GitWorkbenchPanel').then((m) => ({ default: m.GitWorkbenchPanel })));
const LazyTaskRunnerPanel = lazy(() => import('../tasks/TaskRunnerPanel').then((m) => ({ default: m.TaskRunnerPanel })));
const LazyAgentContextPanel = lazy(() => import('../context/AgentContextPanel').then((m) => ({ default: m.AgentContextPanel })));
const LazySharedMemoryPanel = lazy(() => import('../memory/SharedMemoryPanel').then((m) => ({ default: m.SharedMemoryPanel })));

function Grip({ width, setLive }: { width: number; setLive: (width: number | null) => void }) {
  const { t, bridge } = useServices();
  const press = useRef<{ start: number; width: number; moved: boolean; now: number } | null>(null);
  const down = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    press.current = { start: event.clientX, width, moved: false, now: width };
  };
  const move = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = press.current;
    if (!drag) return;
    const travel = event.clientX - drag.start;
    if (!drag.moved && Math.abs(travel) < dragSlack) return;
    drag.moved = true;
    const proposed = drag.width + travel;
    // Below the fold threshold the sidebar folds; the saved width stays the one from before the drag.
    if (proposed < layoutMetrics.sidebarFoldThreshold) {
      press.current = null; setLive(null);
      void bridge.call('setSidebarCollapsed', { collapsed: true }).catch(() => {});
      return;
    }
    drag.now = Math.min(Math.max(proposed, layoutMetrics.sidebarMin), layoutMetrics.sidebarMax);
    setLive(drag.now);
  };
  const up = () => {
    const drag = press.current; press.current = null;
    if (!drag?.moved) return;
    void bridge.call('setSidebarWidth', { width: drag.now }).catch(() => {}).finally(() => setLive(null));
  };
  const key = (event: ReactKeyboardEvent) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const next = Math.min(Math.max(width + (event.key === 'ArrowRight' ? 10 : -10), layoutMetrics.sidebarMin), layoutMetrics.sidebarMax);
    void bridge.call('setSidebarWidth', { width: next }).catch(() => {});
  };
  return (
    <div className="sidebar-grip" role="separator" aria-orientation="vertical" tabIndex={0} aria-label={t('sidebar.resize')} title={t('sidebar.resizeTooltip')}
      aria-valuenow={width} aria-valuemin={layoutMetrics.sidebarMin} aria-valuemax={layoutMetrics.sidebarMax} data-testid="sidebar-grip"
      onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={() => { press.current = null; setLive(null); }} onKeyDown={key}
      onDoubleClick={() => void bridge.call('setSidebarWidth', { width: layoutMetrics.sidebarDefault }).catch(() => {})} />
  );
}

/** The window's shortcuts: the host's WinUI accelerators are cleared in web mode, so the page answers them. */
function useShortcuts(blocked: boolean) {
  const { bridge, store } = useServices();
  const blockedRef = useRef(blocked);
  blockedRef.current = blocked;
  useEffect(() => {
    const call = (method: string, args?: unknown) => void bridge.call(method, args).catch(() => {});
    const key = (event: KeyboardEvent) => {
      // The Mac in the Rust host (D9): ⌘ is the modifier, and a Ctrl key is the text field's.
      const action = import.meta.env.MODE === 'tauri' && macHost() ? macShortcutAction(event) : shortcutAction(event);
      const snapshot = store.getState().snapshot;
      if (!action || (blockedRef.current && action !== 'help') || !snapshot) return;
      event.preventDefault();
      switch (action) {
        case 'openFolder': return call('pickFolder');
        case 'newPane': return call('newPane', { kind: 'claude' });
        case 'newShell': return call('newPane', { kind: 'shell' });
        case 'toggleSidebar': return call('setSidebarCollapsed', { collapsed: !snapshot.sidebarCollapsed });
        case 'search': {
          const focus = () => { const field = document.getElementById(sidebarSearchId) as HTMLInputElement | null; field?.focus(); field?.select(); };
          if (snapshot.sidebarCollapsed) void bridge.call('setSidebarCollapsed', { collapsed: false }).then(() => requestAnimationFrame(focus), () => {});
          else focus();
          return;
        }
        case 'settings': return call('openSettings');
        case 'files': return call('openFiles', {});
        case 'conversationSearch': return call('openConversationSearch', {});
        case 'reviewQueue': return toggleReview();
        case 'help': return call('openHelp');
        case 'textLarger': return call('textSize', { step: 1 });
        case 'textSmaller': return call('textSize', { step: -1 });
        case 'textReset': return call('textSize', { step: 0 });
      }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [bridge, store]);
}

export function Shell() {
  const { t, bridge, store } = useServices();
  const { hasSnapshot, collapsed, savedWidth, hasWorkspaces, snapshotLayout, shell, selected, hasNatives, hasPicture, blocked } = useStoreSelector(
    (state) => ({
      hasSnapshot: state.snapshot !== null,
      collapsed: state.snapshot?.sidebarCollapsed ?? false,
      savedWidth: state.snapshot?.sidebarWidth ?? layoutMetrics.sidebarDefault,
      hasWorkspaces: (state.snapshot?.workspaces.length ?? 0) > 0,
      snapshotLayout: state.snapshot?.layout ?? 'grid',
      shell: state.shell,
      selected: state.selected,
      hasNatives: state.natives.length > 0,
      hasPicture: state.picture !== undefined,
      blocked: state.dialogs.length > 0 || state.natives.length > 0 || state.picture !== undefined || (state.shell?.dialogOpen ?? false),
    }),
    shallowEqual,
  );
  const reviewOpen = useReviewOpen();
  const [live, setLive] = useState<number | null>(null);
  const [usageOpen, setUsageOpen] = useState(false);
  const [usageReportOpen, setUsageReportOpen] = useState(false);
  const [issuePickerState, setIssuePickerState] = useState<{ open: boolean; workspaceId: string | null; paneId: string | null; settingsOpen: boolean }>({
    open: false,
    workspaceId: null,
    paneId: null,
    settingsOpen: false,
  });
  const [gitWorkbenchState, setGitWorkbenchState] = useState<{ open: boolean; workspaceId: string | null; paneId: string | null }>({
    open: false,
    workspaceId: null,
    paneId: null,
  });
  const [taskRunnerState, setTaskRunnerState] = useState<{ open: boolean; workspaceId: string | null; paneId: string | null }>({
    open: false,
    workspaceId: null,
    paneId: null,
  });
  const [agentContextState, setAgentContextState] = useState<{ open: boolean; workspaceId: string | null }>({
    open: false,
    workspaceId: null,
  });
  const [sharedMemoryState, setSharedMemoryState] = useState<{ open: boolean; workspaceId: string | null }>({
    open: false,
    workspaceId: null,
  });
  const activeWorkspaceId = useStoreSelector((state) => state.snapshot?.activeWorkspaceId ?? null);
  const activePaneId = useStoreSelector((state) => {
    const id = state.snapshot?.activeSessionId;
    const pane = id ? state.snapshot?.sessions.find((s) => s.id === id) : undefined;
    return pane && pane.kind !== 'shell' ? pane.id : null;
  });
  const closeUsage = useCallback((open: boolean) => setUsageOpen(open), []);
  const openUsageReportFromMenu = useCallback((open: boolean) => setUsageReportOpen(open), []);
  useEffect(() => {
    const onOpenIssues = (event: Event) => {
      const detail = (event as CustomEvent<{ workspaceId?: string | null; paneId?: string | null; settingsOpen?: boolean }>).detail;
      setIssuePickerState({
        open: true,
        workspaceId: detail?.workspaceId ?? activeWorkspaceId,
        paneId: detail?.paneId ?? activePaneId,
        settingsOpen: Boolean(detail?.settingsOpen),
      });
    };
    const onOpenGit = (event: Event) => {
      const detail = (event as CustomEvent<{ workspaceId?: string | null; paneId?: string | null }>).detail;
      setGitWorkbenchState({
        open: true,
        workspaceId: detail?.workspaceId ?? activeWorkspaceId,
        paneId: detail?.paneId ?? activePaneId,
      });
    };
    const onOpenTasks = (event: Event) => {
      const detail = (event as CustomEvent<{ workspaceId?: string | null; paneId?: string | null }>).detail;
      setTaskRunnerState({
        open: true,
        workspaceId: detail?.workspaceId ?? activeWorkspaceId,
        paneId: detail?.paneId ?? activePaneId,
      });
    };
    const onOpenContext = (event: Event) => {
      const detail = (event as CustomEvent<{ workspaceId?: string | null }>).detail;
      setAgentContextState({
        open: true,
        workspaceId: detail?.workspaceId ?? activeWorkspaceId,
      });
    };
    const onOpenMemory = (event: Event) => {
      const detail = (event as CustomEvent<{ workspaceId?: string | null }>).detail;
      setSharedMemoryState({
        open: true,
        workspaceId: detail?.workspaceId ?? activeWorkspaceId,
      });
    };
    const onOpenReport = () => setUsageReportOpen(true);
    window.addEventListener('mighty:open-issues', onOpenIssues);
    window.addEventListener('mighty:open-git-workbench', onOpenGit);
    window.addEventListener('mighty:open-tasks', onOpenTasks);
    window.addEventListener('mighty:open-agent-context', onOpenContext);
    window.addEventListener('mighty:open-shared-memory', onOpenMemory);
    window.addEventListener('mighty:open-usage-report', onOpenReport);
    return () => {
      window.removeEventListener('mighty:open-issues', onOpenIssues);
      window.removeEventListener('mighty:open-git-workbench', onOpenGit);
      window.removeEventListener('mighty:open-tasks', onOpenTasks);
      window.removeEventListener('mighty:open-agent-context', onOpenContext);
      window.removeEventListener('mighty:open-shared-memory', onOpenMemory);
      window.removeEventListener('mighty:open-usage-report', onOpenReport);
    };
  }, [activeWorkspaceId, activePaneId]);
  useEffect(() => {
    void bridge.call('shellState').then((value) => { if (value) store.dispatch({ kind: 'shell', shell: value as NonNullable<typeof shell> }); }, () => {});
    // Views open before the page loaded (or loaded again) are sent again without eagerly loading Surfaces.
    void bridge.call('nativeState').catch(() => {});
  }, [bridge, store]);
  // A notification's click selected a pane: its row comes into view.
  useEffect(() => {
    if (!selected) return;
    document.querySelector('.sidebar [data-session-id="' + escapeCssAttr(selected.paneId) + '"]')?.scrollIntoView({ block: 'nearest' });
  }, [selected]);
  const modalPanelOpen = usageReportOpen || issuePickerState.open || gitWorkbenchState.open || taskRunnerState.open || agentContextState.open || sharedMemoryState.open;
  useShortcuts(blocked || modalPanelOpen);
  // The Mac's menu bar (D9): Workspace › Show usage opens the cumulative Usage Report dashboard.
  useMenu(blocked || modalPanelOpen, openUsageReportFromMenu);
  if (!hasSnapshot || !shell) return <div className="shell-loading" data-testid="loading">{t('launch.preparing')}</div>;
  const width = live ?? savedWidth;
  const header = shell.dashboard ? undefined : shell.header;
  const counts = header ? shell.workspaces.find((w) => w.id === header.id)?.counts : undefined;
  const main = shell.dashboard ? <Dashboard model={shell.dashboard} onUsage={() => setUsageOpen(true)} />
    : !hasWorkspaces ? <Welcome setupNeeded={shell.setupNeeded} />
    : shell.layout ? <Layout layout={shell.layout} />
    : <EmptyPanes />;
  return (
    <MenuHost>
      <div className={'shell' + (collapsed ? ' collapsed' : '')} data-testid="shell" style={{ gridTemplateColumns: collapsed ? '0 1fr' : `${width}px 1fr` }}>
        <div className="sidebar-column">
          {!collapsed && <Sidebar />}
          {!collapsed && <Grip width={width} setLive={setLive} />}
        </div>
        <div className="detail">
          <Banners shell={shell} />
          {header && <WorkspaceHeader header={header} counts={counts} layoutMode={shell.layout?.mode ?? snapshotLayout} />}
          <main className="shell-main">{main}</main>
          <StatusBar shell={shell} usageOpen={usageOpen} setUsageOpen={closeUsage} />
        </div>
      </div>
      {hasNatives && <Suspense fallback={null}><LazySurfaces skipInitialSync /></Suspense>}
      {reviewOpen && <Suspense fallback={null}><LazyReviewPanel /></Suspense>}
      {usageReportOpen && <Suspense fallback={null}><LazyUsageReportPanel open={usageReportOpen} onClose={() => setUsageReportOpen(false)} /></Suspense>}
      {issuePickerState.open && (
        <Suspense fallback={null}>
          <LazyIssuePickerPanel
            open={issuePickerState.open}
            workspaceId={issuePickerState.workspaceId ?? activeWorkspaceId}
            paneId={issuePickerState.paneId ?? activePaneId}
            initialSettingsOpen={issuePickerState.settingsOpen}
            onClose={() => setIssuePickerState((prev) => ({ ...prev, open: false }))}
            onAttachToComposer={(text) => {
              const targetPane = issuePickerState.paneId ?? activePaneId;
              if (targetPane) store.dispatch({ kind: 'fill', paneId: targetPane, text, append: true });
            }}
          />
        </Suspense>
      )}
      {gitWorkbenchState.open && (
        <Suspense fallback={null}>
          <LazyGitWorkbenchPanel
            open={gitWorkbenchState.open}
            workspaceId={gitWorkbenchState.workspaceId ?? activeWorkspaceId}
            paneId={gitWorkbenchState.paneId ?? activePaneId}
            onClose={() => setGitWorkbenchState((prev) => ({ ...prev, open: false }))}
          />
        </Suspense>
      )}
      {taskRunnerState.open && (
        <Suspense fallback={null}>
          <LazyTaskRunnerPanel
            open={taskRunnerState.open}
            workspaceId={taskRunnerState.workspaceId ?? activeWorkspaceId}
            paneId={taskRunnerState.paneId ?? activePaneId}
            onClose={() => setTaskRunnerState((prev) => ({ ...prev, open: false }))}
          />
        </Suspense>
      )}
      {agentContextState.open && (
        <Suspense fallback={null}>
          <LazyAgentContextPanel
            open={agentContextState.open}
            workspaceId={agentContextState.workspaceId ?? activeWorkspaceId}
            onClose={() => setAgentContextState((prev) => ({ ...prev, open: false }))}
          />
        </Suspense>
      )}
      {sharedMemoryState.open && (
        <Suspense fallback={null}>
          <LazySharedMemoryPanel
            open={sharedMemoryState.open}
            workspaceId={sharedMemoryState.workspaceId ?? activeWorkspaceId}
            onClose={() => setSharedMemoryState((prev) => ({ ...prev, open: false }))}
          />
        </Suspense>
      )}
      <Dialogs />
      {hasPicture && <Suspense fallback={null}><LazyPictureViewer /></Suspense>}
    </MenuHost>
  );
}
